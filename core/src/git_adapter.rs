use std::path::{Path, PathBuf};
use crate::errors::DraftError;
use crate::models::GitOid;

#[derive(Debug, Clone)]
pub struct DiffOptions {
    pub binary: bool,
    pub paths: Vec<PathBuf>,
}

pub trait GitAdapter: Send + Sync {
    fn current_head(&self) -> Result<GitOid, DraftError>;
    fn branch_name(&self) -> Result<Option<String>, DraftError>;
    fn git_dir(&self) -> Result<PathBuf, DraftError>;
    fn status_porcelain(&self) -> Result<String, DraftError>;
    fn diff(&self, opts: DiffOptions) -> Result<String, DraftError>;
    fn stage_paths(&self, paths: &[PathBuf]) -> Result<(), DraftError>;
    fn unstage_all(&self) -> Result<(), DraftError>;
    fn commit(&self, message: &str) -> Result<GitOid, DraftError>;
    fn config_get(&self, key: &str) -> Result<Option<String>, DraftError>;
    fn show_file(&self, revision: &str, path: &Path) -> Result<Vec<u8>, DraftError>;
}

#[derive(Debug, Clone)]
pub struct GitCliAdapter {
    pub repo_root: PathBuf,
}

impl GitCliAdapter {
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }

    fn run_git_args(&self, args: &[String]) -> Result<String, DraftError> {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&self.repo_root)
            .output()?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Trim only the trailing newline — do NOT strip leading whitespace
            // since git status --porcelain uses leading spaces as status codes
            Ok(stdout.trim_end().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Err(DraftError::GitCommandFailed {
                command: format!("git {}", args.join(" ")),
                exit_code: output.status.code(),
                stderr,
            })
        }
    }

    fn run_git(&self, args: &[&str]) -> Result<String, DraftError> {
        let string_args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        self.run_git_args(&string_args)
    }
}

impl GitAdapter for GitCliAdapter {
    fn current_head(&self) -> Result<GitOid, DraftError> {
        match self.run_git(&["rev-parse", "HEAD"]) {
            Ok(oid) => Ok(oid),
            Err(DraftError::GitCommandFailed { exit_code: _, stderr, .. }) if stderr.contains("fatal: Needed a single revision") || stderr.contains("fatal: ambiguous argument 'HEAD'") => {
                // Newly initialized repository (unborn branch)
                Ok("0000000000000000000000000000000000000000".to_string())
            }
            Err(e) => Err(e),
        }
    }

    fn branch_name(&self) -> Result<Option<String>, DraftError> {
        let branch = self.run_git(&["branch", "--show-current"])?;
        if branch.is_empty() {
            Ok(None)
        } else {
            Ok(Some(branch))
        }
    }

    fn git_dir(&self) -> Result<PathBuf, DraftError> {
        let git_dir_str = self.run_git(&["rev-parse", "--git-dir"])?;
        let git_dir_path = Path::new(&git_dir_str);
        if git_dir_path.is_absolute() {
            Ok(git_dir_path.to_path_buf())
        } else {
            Ok(self.repo_root.join(git_dir_path))
        }
    }

    fn status_porcelain(&self) -> Result<String, DraftError> {
        self.run_git(&["status", "--porcelain=v1"])
    }

    fn diff(&self, opts: DiffOptions) -> Result<String, DraftError> {
        let mut args = vec!["diff".to_string()];
        if opts.binary {
            args.push("--binary".to_string());
        }
        if !opts.paths.is_empty() {
            args.push("--".to_string());
            for path in opts.paths {
                args.push(path.to_string_lossy().into_owned());
            }
        }
        self.run_git_args(&args)
    }

    fn stage_paths(&self, paths: &[PathBuf]) -> Result<(), DraftError> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut args = vec!["add".to_string(), "--".to_string()];
        for path in paths {
            args.push(path.to_string_lossy().into_owned());
        }
        self.run_git_args(&args)?;
        Ok(())
    }

    fn unstage_all(&self) -> Result<(), DraftError> {
        self.run_git(&["reset"])?;
        Ok(())
    }

    fn commit(&self, message: &str) -> Result<GitOid, DraftError> {
        self.run_git(&["commit", "-m", message])?;
        self.current_head()
    }

    fn config_get(&self, key: &str) -> Result<Option<String>, DraftError> {
        match self.run_git(&["config", "--get", key]) {
            Ok(val) => Ok(Some(val)),
            Err(DraftError::GitCommandFailed { exit_code: Some(1), .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn show_file(&self, revision: &str, path: &Path) -> Result<Vec<u8>, DraftError> {
        let path_str = path.to_string_lossy().into_owned();
        let spec = format!("{}:{}", revision, path_str);
        
        let output = std::process::Command::new("git")
            .args(&["show", &spec])
            .current_dir(&self.repo_root)
            .output()?;

        if output.status.success() {
            Ok(output.stdout)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Err(DraftError::GitCommandFailed {
                command: format!("git show {}", spec),
                exit_code: output.status.code(),
                stderr,
            })
        }
    }
}
