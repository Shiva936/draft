//! `terminal-cleanup` — the READY record committing the semantic→structural
//! handoff at the end of an `Uninstall`.
//!
//! ```text
//! draft-terminal-cleanup 1
//! installation <ins_…>
//! operation <ilo_…>
//! state ready
//! mode unix | mode windows
//! ```
//!
//! READY means only that every semantic and journal-recoverable obligation is
//! discharged. It authorizes no root or lock removal, schedules nothing, and
//! carries no path: every terminal path is re-derived from the selected root,
//! the fixed layout and the operation id. It sits inside the same
//! installation-private boundary as `bootstrap.recovery` and is not signed.

use super::bootstrap::{field, logical_lines};
use super::{fail, InstallPlatform, InstallationFailure, InstallationId, InstallationOperationId};
use crate::support::error::DraftResult;

pub const MAX_TERMINAL_CLEANUP_RECORD_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalRecord {
    pub installation_id: InstallationId,
    pub operation_id: InstallationOperationId,
    pub mode: InstallPlatform,
}

fn malformed(why: &str) -> crate::support::error::DraftError {
    fail(
        InstallationFailure::TerminalCleanupRecordInvalid,
        format!("terminal-cleanup is malformed: {why}"),
    )
}

impl TerminalRecord {
    pub fn render(&self) -> String {
        let mode = match self.mode {
            InstallPlatform::Unix => "unix",
            InstallPlatform::Windows => "windows",
        };
        format!(
            "draft-terminal-cleanup 1\ninstallation {}\noperation {}\nstate ready\nmode {mode}\n",
            self.installation_id, self.operation_id
        )
    }

    pub fn parse(bytes: &[u8]) -> DraftResult<Self> {
        let lines = logical_lines(bytes, MAX_TERMINAL_CLEANUP_RECORD_BYTES, 5, malformed)?;
        if lines[0] != "draft-terminal-cleanup 1" {
            return Err(malformed("unknown discriminator"));
        }
        let installation = field(&lines[1], "installation")
            .filter(|value| InstallationId::is_well_formed(value))
            .ok_or_else(|| malformed("installation"))?;
        let operation = field(&lines[2], "operation")
            .filter(|value| InstallationOperationId::is_well_formed(value))
            .ok_or_else(|| malformed("operation"))?;
        if lines[3] != "state ready" {
            return Err(malformed("state"));
        }
        let mode = match lines[4].as_str() {
            "mode unix" => InstallPlatform::Unix,
            "mode windows" => InstallPlatform::Windows,
            _ => return Err(malformed("mode")),
        };
        Ok(Self {
            installation_id: InstallationId::new(installation),
            operation_id: InstallationOperationId::new(operation),
            mode,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(mode: InstallPlatform) -> TerminalRecord {
        TerminalRecord {
            installation_id: InstallationId::new("ins_0123456789ab"),
            operation_id: InstallationOperationId::new("ilo_0123456789ab"),
            mode,
        }
    }

    #[test]
    fn ready_round_trips_and_names_no_path() {
        for mode in [InstallPlatform::Unix, InstallPlatform::Windows] {
            let text = record(mode).render();
            assert_eq!(
                TerminalRecord::parse(text.as_bytes()).unwrap(),
                record(mode)
            );
            assert_eq!(
                TerminalRecord::parse(text.replace('\n', "\r\n").as_bytes()).unwrap(),
                record(mode)
            );
            assert!(!text.contains('/') && !text.contains('\\'));
        }
    }

    #[test]
    fn malformed_ready_fails_closed() {
        let good = record(InstallPlatform::Unix).render();
        for bad in [
            good.replace("state ready", "state preparing"),
            good.replace("mode unix", "mode linux"),
            good.replace("installation ins_0123456789ab", "installation ins_x"),
            good.replace("operation ilo_0123456789ab", "operation ins_0123456789ab"),
            good.replace("state ready\n", ""),
            format!("{good}mode unix\n"),
            format!("{good}x"),
            good.replace("state ready", "state\u{7}ready"),
            good.replace("draft-terminal-cleanup 1", "draft-terminal-cleanup  1"),
            format!("{}{}", good, "x".repeat(300)),
        ] {
            assert!(TerminalRecord::parse(bad.as_bytes()).is_err(), "{bad:?}");
        }
    }
}
