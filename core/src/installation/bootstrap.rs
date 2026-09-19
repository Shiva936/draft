//! `bootstrap.recovery` — the bounded, installer-facing identity record an
//! `Uninstall` writes at `HelperStaged`.
//!
//! Six logical lines in a closed, fixed order, one ASCII space as the only
//! separator, so `install.sh` can validate it with shell `case` globs alone —
//! no JSON, no `grep`/`sed`/`awk` over security-sensitive state:
//!
//! ```text
//! draft-lifecycle-bootstrap 1
//! installation <ins_…>
//! operation <ilo_…>
//! kind uninstall
//! helper-sha256 <64 lowercase hex>
//! helper-size <unsigned decimal>
//! ```
//!
//! It contains no path, no phase, no purge target and no command, and confers
//! no destructive authority. Its digest is an identity/integrity consistency
//! check inside the installation's own private lifecycle directory — not a
//! signature: a same-user process able to rewrite both the record and the
//! helper is not defeated by it. The authoritative checks are the Rust
//! helper's, against `operation.json`.

use super::{fail, Identity, InstallationFailure, InstallationId, InstallationOperationId};
use crate::support::error::DraftResult;

pub const MAX_BOOTSTRAP_RECORD_BYTES: usize = 512;
const MAX_LINE_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapRecord {
    pub installation_id: InstallationId,
    pub operation_id: InstallationOperationId,
    pub helper: Identity,
}

fn malformed(why: &str) -> crate::support::error::DraftError {
    fail(
        InstallationFailure::UninstallRecoveryBootstrapFailed,
        format!("bootstrap.recovery is malformed: {why}"),
    )
}

/// Split a bounded record into exactly `expected` logical lines, accepting LF
/// or CRLF (exactly one trailing CR stripped) and nothing else.
pub(crate) fn logical_lines(
    bytes: &[u8],
    cap: usize,
    expected: usize,
    malformed: impl Fn(&str) -> crate::support::error::DraftError,
) -> DraftResult<Vec<String>> {
    if bytes.len() > cap {
        return Err(malformed("the record exceeds its size cap"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| malformed("the record is not UTF-8"))?;
    let body = text
        .strip_suffix('\n')
        .ok_or_else(|| malformed("the last line is not terminated"))?;
    let mut lines = Vec::new();
    for raw in body.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.len() > MAX_LINE_BYTES {
            return Err(malformed("a line exceeds 128 bytes"));
        }
        if line.bytes().any(|b| b.is_ascii_control() || !b.is_ascii()) {
            return Err(malformed("a line carries a control or non-ASCII character"));
        }
        lines.push(line.to_string());
    }
    if lines.len() != expected {
        return Err(malformed(
            "the record does not have exactly its fixed lines",
        ));
    }
    Ok(lines)
}

/// `<key> <value>` with exactly one separating space and a non-empty value.
pub(crate) fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let value = line.strip_prefix(key)?.strip_prefix(' ')?;
    (!value.is_empty() && !value.contains(' ')).then_some(value)
}

impl BootstrapRecord {
    pub fn render(&self) -> String {
        format!(
            "draft-lifecycle-bootstrap 1\ninstallation {}\noperation {}\nkind uninstall\nhelper-sha256 {}\nhelper-size {}\n",
            self.installation_id, self.operation_id, self.helper.sha256, self.helper.size
        )
    }

    pub fn parse(bytes: &[u8]) -> DraftResult<Self> {
        let lines = logical_lines(bytes, MAX_BOOTSTRAP_RECORD_BYTES, 6, malformed)?;
        if lines[0] != "draft-lifecycle-bootstrap 1" {
            return Err(malformed("unknown discriminator"));
        }
        let installation = field(&lines[1], "installation")
            .filter(|value| InstallationId::is_well_formed(value))
            .ok_or_else(|| malformed("installation"))?;
        let operation = field(&lines[2], "operation")
            .filter(|value| InstallationOperationId::is_well_formed(value))
            .ok_or_else(|| malformed("operation"))?;
        if lines[3] != "kind uninstall" {
            return Err(malformed("kind"));
        }
        let sha256 = field(&lines[4], "helper-sha256").ok_or_else(|| malformed("helper-sha256"))?;
        let size = field(&lines[5], "helper-size")
            .filter(|value| value.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| malformed("helper-size"))?;
        let helper = Identity {
            sha256: sha256.to_string(),
            size,
        };
        if !helper.is_well_formed() {
            return Err(malformed("helper-sha256"));
        }
        Ok(Self {
            installation_id: InstallationId::new(installation),
            operation_id: InstallationOperationId::new(operation),
            helper,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> BootstrapRecord {
        BootstrapRecord {
            installation_id: InstallationId::new("ins_0123456789ab"),
            operation_id: InstallationOperationId::new("ilo_0123456789ab"),
            helper: Identity {
                sha256: "a".repeat(64),
                size: 1234,
            },
        }
    }

    #[test]
    fn the_record_round_trips_under_lf_and_crlf() {
        let text = record().render();
        assert_eq!(BootstrapRecord::parse(text.as_bytes()).unwrap(), record());
        let crlf = text.replace('\n', "\r\n");
        assert_eq!(BootstrapRecord::parse(crlf.as_bytes()).unwrap(), record());
        // It carries no path and no phase.
        assert!(!text.contains('/') && !text.contains('\\') && !text.contains("phase"));
    }

    #[test]
    fn every_grammar_violation_fails_closed() {
        let good = record().render();
        let lines: Vec<&str> = good.lines().collect();
        let join = |lines: &[&str]| format!("{}\n", lines.join("\n"));
        let mut cases = vec![
            join(&[lines[0], lines[2], lines[1], lines[3], lines[4], lines[5]]),
            join(&lines[..5]),
            join(&[lines[0], lines[1], lines[1], lines[3], lines[4], lines[5]]),
            join(&[
                lines[0],
                "installation ins_ffffffffffff extra",
                lines[2],
                lines[3],
                lines[4],
                lines[5],
            ]),
            join(&[
                lines[0],
                "installation  ins_0123456789ab",
                lines[2],
                lines[3],
                lines[4],
                lines[5],
            ]),
            join(&[
                lines[0],
                "installation ins_0123456789AB",
                lines[2],
                lines[3],
                lines[4],
                lines[5],
            ]),
            join(&[
                lines[0],
                lines[1],
                "operation op_0123456789ab",
                lines[3],
                lines[4],
                lines[5],
            ]),
            join(&[
                lines[0],
                lines[1],
                lines[2],
                "kind update",
                lines[4],
                lines[5],
            ]),
            join(&[
                lines[0],
                lines[1],
                lines[2],
                lines[3],
                "helper-sha256 xyz",
                lines[5],
            ]),
            join(&[
                lines[0],
                lines[1],
                lines[2],
                lines[3],
                lines[4],
                "helper-size -1",
            ]),
            join(&[
                lines[0],
                lines[1],
                lines[2],
                lines[3],
                lines[4],
                lines[5],
                "phase Resolved",
            ]),
            join(&[
                "draft-lifecycle-bootstrap 2",
                lines[1],
                lines[2],
                lines[3],
                lines[4],
                lines[5],
            ]),
            format!("{good}trailing"),
            good.replace("kind uninstall", "kind unin\tstall"),
            good.trim_end().to_string(),
            good.replace("\n", "\r\r\n"),
        ];
        cases.push(format!("{}{}", good, " ".repeat(600)));
        for case in cases {
            assert!(BootstrapRecord::parse(case.as_bytes()).is_err(), "{case:?}");
        }
    }
}
