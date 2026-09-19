//! Running verification and recording what it produced.
//!
//! Split out of `app/mod.rs`; these are `App` methods and behave
//! identically to when they lived there.

use super::*;

/// What verifying the Activity chain established.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityChainStatus {
    pub ok: bool,
    pub events: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl App {
    /// Verify the Activity chain, reporting how many records held.
    pub fn verify_events(&self, cwd: &Path) -> DraftResult<ActivityChainStatus> {
        let ws = self.open(cwd)?;
        match ws.events()?.verify_chain() {
            Ok(events) => Ok(ActivityChainStatus {
                ok: true,
                events,
                error: None,
            }),
            Err(error) => Ok(ActivityChainStatus {
                ok: false,
                events: ws.events()?.read_all().map(|all| all.len()).unwrap_or(0),
                error: Some(error.message),
            }),
        }
    }
}
