//! Basic LSIF-style symbol index (PRD §9.17, TDD §34).
//!
//! v0.3.3 ships a lightweight, offline symbol index — the "basic" backend the
//! spec permits when a full LSIF toolchain is unavailable. It persists the model
//! `file -> symbol -> reference -> pack` in a SQLite database under
//! `.draft/lsif/index.db` and answers the impact questions Draft needs: which
//! symbols a pack touches, which public APIs it changes, which tests reference
//! changed symbols, which packs touch the same symbols, and whether two packs
//! could semantically conflict.
//!
//! Symbol extraction is heuristic and language-aware (Rust, JS/TS, Python, Go);
//! it is intentionally conservative and marked `backend = "basic"`.

use crate::error::{DraftError, DraftResult};
use crate::layout::ProjectPaths;
use rusqlite::Connection;
use serde::Serialize;
use std::collections::BTreeSet;

/// The backend identifier recorded in evidence so consumers know the fidelity.
pub const LSIF_BACKEND: &str = "basic";

/// A defined symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Symbol {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub public: bool,
}

/// The LSIF index bound to a project.
pub struct LsifIndex {
    conn: Connection,
}

impl LsifIndex {
    /// Open (creating if needed) the project's LSIF database.
    pub fn open(paths: &ProjectPaths) -> DraftResult<Self> {
        crate::fsutil::ensure_dir(&paths.lsif_dir())?;
        let conn = Connection::open(paths.lsif_index_db())
            .map_err(|e| DraftError::storage(format!("open lsif db: {e}")))?;
        let idx = LsifIndex { conn };
        idx.ensure_schema()?;
        Ok(idx)
    }

    /// Open an in-memory index (tests).
    pub fn open_memory() -> DraftResult<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|e| DraftError::storage(format!("open lsif memory db: {e}")))?;
        let idx = LsifIndex { conn };
        idx.ensure_schema()?;
        Ok(idx)
    }

    fn ensure_schema(&self) -> DraftResult<()> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS symbols(
                    file TEXT, name TEXT, kind TEXT, line INTEGER, public INTEGER);
                 CREATE TABLE IF NOT EXISTS refs(
                    file TEXT, name TEXT, line INTEGER);
                 CREATE TABLE IF NOT EXISTS pack_symbols(
                    pack_id TEXT, name TEXT, file TEXT, public INTEGER);
                 CREATE INDEX IF NOT EXISTS idx_ps_pack ON pack_symbols(pack_id);
                 CREATE INDEX IF NOT EXISTS idx_ps_name ON pack_symbols(name);
                 CREATE INDEX IF NOT EXISTS idx_refs_name ON refs(name);",
            )
            .map_err(|e| DraftError::storage(format!("lsif schema: {e}")))?;
        Ok(())
    }

    /// Index a pack: extract defined symbols from its changed files and record
    /// the pack↔symbol association. `files` is (relative path, content).
    pub fn index_pack(&self, pack_id: &str, files: &[(String, String)]) -> DraftResult<usize> {
        // Replace any prior rows for this pack (idempotent re-index).
        self.conn
            .execute("DELETE FROM pack_symbols WHERE pack_id = ?1", [pack_id])
            .map_err(|e| DraftError::storage(e.to_string()))?;
        let mut count = 0;
        for (path, content) in files {
            for sym in extract_symbols(path, content) {
                self.conn
                    .execute(
                        "INSERT INTO symbols(file,name,kind,line,public) VALUES(?1,?2,?3,?4,?5)",
                        rusqlite::params![
                            sym.file,
                            sym.name,
                            sym.kind,
                            sym.line,
                            sym.public as i64
                        ],
                    )
                    .map_err(|e| DraftError::storage(e.to_string()))?;
                self.conn
                    .execute(
                        "INSERT INTO pack_symbols(pack_id,name,file,public) VALUES(?1,?2,?3,?4)",
                        rusqlite::params![pack_id, sym.name, sym.file, sym.public as i64],
                    )
                    .map_err(|e| DraftError::storage(e.to_string()))?;
                count += 1;
            }
        }
        Ok(count)
    }

    /// Record references found in `file` for the given known symbol names.
    pub fn record_refs(
        &self,
        file: &str,
        content: &str,
        known: &BTreeSet<String>,
    ) -> DraftResult<()> {
        for (i, line) in content.lines().enumerate() {
            for tok in tokenize(line) {
                if known.contains(tok) {
                    self.conn
                        .execute(
                            "INSERT INTO refs(file,name,line) VALUES(?1,?2,?3)",
                            rusqlite::params![file, tok, (i + 1) as i64],
                        )
                        .map_err(|e| DraftError::storage(e.to_string()))?;
                }
            }
        }
        Ok(())
    }

    /// Symbols a pack touches.
    pub fn symbols_touched_by_pack(&self, pack_id: &str) -> DraftResult<Vec<String>> {
        self.query_names(
            "SELECT DISTINCT name FROM pack_symbols WHERE pack_id = ?1 ORDER BY name",
            [pack_id],
        )
    }

    /// Public API symbols a pack changed.
    pub fn public_api_symbols_changed(&self, pack_id: &str) -> DraftResult<Vec<String>> {
        self.query_names(
            "SELECT DISTINCT name FROM pack_symbols WHERE pack_id = ?1 AND public = 1 ORDER BY name",
            [pack_id],
        )
    }

    /// Files (typically tests) whose recorded refs include any of `symbols`.
    pub fn files_referencing_symbols(&self, symbols: &[String]) -> DraftResult<Vec<String>> {
        if symbols.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = symbols.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql =
            format!("SELECT DISTINCT file FROM refs WHERE name IN ({placeholders}) ORDER BY file");
        let params: Vec<&dyn rusqlite::ToSql> =
            symbols.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| DraftError::storage(e.to_string()))?;
        let rows = stmt
            .query_map(params.as_slice(), |r| r.get::<_, String>(0))
            .map_err(|e| DraftError::storage(e.to_string()))?;
        Ok(rows.flatten().collect())
    }

    /// Other packs that touch any of `symbols`.
    pub fn packs_touching_symbols(&self, symbols: &[String]) -> DraftResult<Vec<String>> {
        if symbols.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = symbols.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT DISTINCT pack_id FROM pack_symbols WHERE name IN ({placeholders}) ORDER BY pack_id"
        );
        let params: Vec<&dyn rusqlite::ToSql> =
            symbols.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| DraftError::storage(e.to_string()))?;
        let rows = stmt
            .query_map(params.as_slice(), |r| r.get::<_, String>(0))
            .map_err(|e| DraftError::storage(e.to_string()))?;
        Ok(rows.flatten().collect())
    }

    /// Symbols two packs both touch (a possible semantic conflict).
    pub fn possible_semantic_conflicts(
        &self,
        pack_a: &str,
        pack_b: &str,
    ) -> DraftResult<Vec<String>> {
        let a: BTreeSet<String> = self.symbols_touched_by_pack(pack_a)?.into_iter().collect();
        let b: BTreeSet<String> = self.symbols_touched_by_pack(pack_b)?.into_iter().collect();
        Ok(a.intersection(&b).cloned().collect())
    }

    fn query_names<P: rusqlite::Params>(&self, sql: &str, params: P) -> DraftResult<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|e| DraftError::storage(e.to_string()))?;
        let rows = stmt
            .query_map(params, |r| r.get::<_, String>(0))
            .map_err(|e| DraftError::storage(e.to_string()))?;
        Ok(rows.flatten().collect())
    }
}

/// Extract defined symbols from a file using language-aware heuristics.
pub fn extract_symbols(path: &str, content: &str) -> Vec<Symbol> {
    let ext = path.rsplit('.').next().unwrap_or("");
    let mut out = Vec::new();
    for (i, raw) in content.lines().enumerate() {
        let line = raw.trim_start();
        let ln = (i + 1) as u32;
        match ext {
            "rs" => {
                let public = line.starts_with("pub ");
                for kw in [
                    "fn ", "struct ", "enum ", "trait ", "const ", "static ", "type ",
                ] {
                    if let Some(name) = after_keyword(line, kw) {
                        out.push(Symbol {
                            file: path.into(),
                            name,
                            kind: kw.trim().into(),
                            line: ln,
                            public,
                        });
                    }
                }
            }
            "js" | "jsx" | "ts" | "tsx" | "mjs" => {
                let public = line.starts_with("export ");
                for kw in [
                    "function ",
                    "class ",
                    "const ",
                    "let ",
                    "var ",
                    "interface ",
                    "type ",
                ] {
                    if let Some(name) = after_keyword(line, kw) {
                        out.push(Symbol {
                            file: path.into(),
                            name,
                            kind: kw.trim().into(),
                            line: ln,
                            public,
                        });
                    }
                }
            }
            "py" => {
                for kw in ["def ", "class "] {
                    if let Some(name) = after_keyword(line, kw) {
                        let public = !name.starts_with('_');
                        out.push(Symbol {
                            file: path.into(),
                            name,
                            kind: kw.trim().into(),
                            line: ln,
                            public,
                        });
                    }
                }
            }
            "go" => {
                for kw in ["func ", "type ", "const ", "var "] {
                    if let Some(name) = after_keyword(line, kw) {
                        let public = name
                            .chars()
                            .next()
                            .map(|c| c.is_uppercase())
                            .unwrap_or(false);
                        out.push(Symbol {
                            file: path.into(),
                            name,
                            kind: kw.trim().into(),
                            line: ln,
                            public,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Return the identifier immediately following `keyword` in `line`, if any.
fn after_keyword(line: &str, keyword: &str) -> Option<String> {
    let idx = line.find(keyword)?;
    // Only treat it as a definition if the keyword starts a token.
    let before = line[..idx].chars().last();
    if let Some(c) = before {
        if c.is_alphanumeric() || c == '_' {
            return None;
        }
    }
    let rest = &line[idx + keyword.len()..];
    let rest = rest.trim_start_matches(['*', '&', '(', ' ']);
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() || name.chars().next().unwrap().is_numeric() {
        None
    } else {
        Some(name)
    }
}

/// Split a line into identifier tokens.
fn tokenize(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_symbols_per_language() {
        let rs = extract_symbols("src/a.rs", "pub fn login() {}\nstruct Session;\n");
        assert!(rs
            .iter()
            .any(|s| s.name == "login" && s.public && s.kind == "fn"));
        assert!(rs.iter().any(|s| s.name == "Session" && s.kind == "struct"));

        let py = extract_symbols(
            "m.py",
            "def public():\n    pass\ndef _private():\n    pass\n",
        );
        assert!(py.iter().any(|s| s.name == "public" && s.public));
        assert!(py.iter().any(|s| s.name == "_private" && !s.public));

        let go = extract_symbols("m.go", "func Exported() {}\nfunc unexported() {}\n");
        assert!(go.iter().any(|s| s.name == "Exported" && s.public));
        assert!(go.iter().any(|s| s.name == "unexported" && !s.public));
    }

    #[test]
    fn index_and_query_impact() {
        let idx = LsifIndex::open_memory().unwrap();
        idx.index_pack(
            "pck_a",
            &[(
                "src/auth.rs".into(),
                "pub fn validate() {}\nfn helper() {}\n".into(),
            )],
        )
        .unwrap();
        idx.index_pack(
            "pck_b",
            &[("src/session.rs".into(), "pub fn validate() {}\n".into())],
        )
        .unwrap();

        let touched = idx.symbols_touched_by_pack("pck_a").unwrap();
        assert!(touched.contains(&"validate".to_string()));
        let public = idx.public_api_symbols_changed("pck_a").unwrap();
        assert_eq!(public, vec!["validate".to_string()]);

        // Both packs touch `validate` → semantic conflict candidate.
        let conflicts = idx.possible_semantic_conflicts("pck_a", "pck_b").unwrap();
        assert_eq!(conflicts, vec!["validate".to_string()]);

        // A test file referencing `validate` is discoverable.
        let mut known = BTreeSet::new();
        known.insert("validate".to_string());
        idx.record_refs("tests/auth_test.rs", "fn t() { validate(); }", &known)
            .unwrap();
        let tests = idx
            .files_referencing_symbols(&["validate".to_string()])
            .unwrap();
        assert_eq!(tests, vec!["tests/auth_test.rs".to_string()]);
    }
}
