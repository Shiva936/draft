//! Session manager for clients connected to `draftd`.
//!
//! Sessions are lightweight in-memory handles used for accounting and request
//! ownership in the local control plane.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// The source of "now" for capability expiry.
///
/// Capability lifetime is a real security property, so it is measured against a
/// clock rather than a step counter. That clock is injectable for exactly one
/// reason: a test that has to prove *what a capability means* should not also be
/// racing the wall clock. A loaded machine that takes longer than the timeout
/// between issuing and consuming makes such a test fail for a reason unrelated
/// to what it asserts, and a green run stops being evidence.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Milliseconds since the Unix epoch.
    fn now_unix_ms(&self) -> i64;
}

/// The real clock. What production always uses.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64
    }
}

/// A clock that only moves when a test moves it.
///
/// This makes expiry a decision the test states outright — "advance past the
/// timeout" — instead of a side effect of how busy the machine was.
#[derive(Debug)]
pub struct ManualClock {
    now_ms: AtomicI64,
}

impl ManualClock {
    pub fn new(start_unix_ms: i64) -> Self {
        Self {
            now_ms: AtomicI64::new(start_unix_ms),
        }
    }

    /// Move time forward. Returns the new value.
    pub fn advance_ms(&self, delta_ms: i64) -> i64 {
        self.now_ms.fetch_add(delta_ms, Ordering::SeqCst) + delta_ms
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        // A fixed, plausible epoch-millis value. The absolute point does not
        // matter; that it never moves on its own is the whole point.
        Self::new(1_700_000_000_000)
    }
}

impl Clock for ManualClock {
    fn now_unix_ms(&self) -> i64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: u64,
    pub workspace_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ApplicationSession {
    pub id: String,
    pub client_instance_id: String,
    pub principal: String,
    pub capabilities: Vec<String>,
}

/// What one issued invocation capability authorizes.
///
/// A capability is not a permit to run an action id: it is a permit to run
/// *that* action, on *that* target, against *that* authoritative state, with
/// *that* argument contract. Every field participates, so a client cannot
/// replay a capability after the thing it described has changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionBinding {
    pub application_session_id: String,
    pub principal: String,
    pub workspace_id: Option<String>,
    pub change_id: Option<String>,
    pub action_id: String,
    /// The entity acted on, when it is not the subject itself — an extension,
    /// a catalog source, a pending authorization, a grant.
    pub target: Option<draft_ipc::console_application::ActionTarget>,
    pub workspace_revision: Option<String>,
    pub change_revision: Option<String>,
    /// The authoritative revision this capability was issued against, for
    /// actions whose subject is not a project or Change.
    pub registry_revision: Option<u64>,
    /// Digest of the input contract the client was shown. An invocation
    /// carrying arguments shaped for an older contract is rejected.
    pub input_contract_digest: String,
    /// The authoritative state this capability was offered against, and the
    /// projection the offer was derived from.
    ///
    /// This is the field that makes the capability a claim about the
    /// *project*, not merely about the exchange. `workspace_revision` and
    /// `change_revision` above can only be compared with what the client sends
    /// back, which proves the client echoed what it was given and nothing
    /// more. The precondition is re-judged against stores read at invocation
    /// time, so an action issued before somebody else committed is refused
    /// even when the client's own request is perfectly self-consistent.
    pub precondition: draft_ipc::console_application::RequestPrecondition,
    pub expires_at_unix_ms: i64,
}

pub struct SessionManager {
    next: AtomicU64,
    sessions: Mutex<HashMap<u64, Session>>,
    application_sessions: Mutex<HashMap<String, ApplicationSession>>,
    action_capabilities: Mutex<HashMap<String, ActionBinding>>,
    clock: Arc<dyn Clock>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::with_clock(Arc::new(SystemClock))
    }
}

impl SessionManager {
    pub fn new() -> Self {
        SessionManager::default()
    }

    /// A manager that reads time from `clock`.
    ///
    /// Both halves of expiry — the deadline stamped onto a capability and the
    /// check that consumes it — must read the same clock, or a test could set
    /// one and not the other and prove nothing.
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            next: AtomicU64::new(0),
            sessions: Mutex::new(HashMap::new()),
            application_sessions: Mutex::new(HashMap::new()),
            action_capabilities: Mutex::new(HashMap::new()),
            clock,
        }
    }

    /// The current time as this manager measures it.
    ///
    /// Callers that stamp a deadline onto a capability must derive it from
    /// here, so issuing and consuming agree about what time it is.
    pub fn now_unix_ms(&self) -> i64 {
        self.clock.now_unix_ms()
    }

    pub fn open(&self, workspace_path: Option<String>) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.sessions
            .lock()
            .unwrap()
            .insert(id, Session { id, workspace_path });
        id
    }

    pub fn close(&self, id: u64) {
        self.sessions.lock().unwrap().remove(&id);
    }

    pub fn count(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    pub fn open_application(
        &self,
        client_instance_id: String,
        principal: String,
        capabilities: Vec<String>,
    ) -> ApplicationSession {
        let serial = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let session = ApplicationSession {
            id: format!("console_session_{serial}"),
            client_instance_id,
            principal,
            capabilities,
        };
        let mut sessions = self.application_sessions.lock().unwrap();
        let replaced = sessions
            .values()
            .filter(|existing| existing.client_instance_id == session.client_instance_id)
            .map(|existing| existing.id.clone())
            .collect::<Vec<_>>();
        sessions.retain(|_, existing| existing.client_instance_id != session.client_instance_id);
        sessions.insert(session.id.clone(), session.clone());
        drop(sessions);
        if !replaced.is_empty() {
            self.action_capabilities
                .lock()
                .unwrap()
                .retain(|_, binding| !replaced.contains(&binding.application_session_id));
        }
        session
    }

    pub fn application(&self, id: &str) -> Option<ApplicationSession> {
        self.application_sessions.lock().unwrap().get(id).cloned()
    }

    pub fn issue_action(&self, binding: ActionBinding) -> String {
        let serial = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let token = format!("console_cap_{serial}");
        self.action_capabilities
            .lock()
            .unwrap()
            .insert(token.clone(), binding);
        token
    }

    /// Consume a short-lived action capability. Consuming on success makes a
    /// descriptor single-use; retries attach by durable operation id instead.
    pub fn consume_action(
        &self,
        token: &str,
        application_session_id: &str,
        principal: &str,
    ) -> Result<ActionBinding, &'static str> {
        let binding = self
            .action_capabilities
            .lock()
            .unwrap()
            .remove(token)
            .ok_or("unknown or already-used action capability")?;
        if binding.application_session_id != application_session_id {
            return Err("action capability belongs to another application session");
        }
        if binding.principal != principal {
            return Err("action capability belongs to another principal");
        }
        if binding.expires_at_unix_ms < self.clock.now_unix_ms() {
            return Err("action capability expired");
        }
        Ok(binding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_ipc::console_application::{Projection, ReadModelWatermark, RequestPrecondition};

    /// A precondition that depends on nothing mutable.
    ///
    /// These tests are about session bookkeeping — rollover, expiry, single
    /// use — so the freshness dimension is deliberately held constant. What a
    /// precondition does when the project moves is proved where it is judged,
    /// against real stores.
    fn nothing_mutable() -> RequestPrecondition {
        RequestPrecondition {
            projection: Projection::HistoricalBaselineComposition,
            watermark: ReadModelWatermark::default(),
        }
    }

    #[test]
    fn application_rollover_invalidates_old_descriptors() {
        let sessions = SessionManager::new();
        let first = sessions.open_application("client-a".into(), "local".into(), vec![]);
        let token = sessions.issue_action(ActionBinding {
            application_session_id: first.id.clone(),
            principal: "local".into(),
            workspace_id: None,
            change_id: None,
            action_id: "refresh".into(),
            target: None,
            workspace_revision: None,
            change_revision: None,
            registry_revision: None,
            input_contract_digest: String::new(),
            precondition: nothing_mutable(),
            expires_at_unix_ms: sessions.now_unix_ms() + 10_000,
        });
        let second = sessions.open_application("client-a".into(), "local".into(), vec![]);
        assert_ne!(first.id, second.id);
        assert!(sessions.application(&first.id).is_none());
        assert!(sessions.consume_action(&token, &first.id, "local").is_err());
    }

    #[test]
    fn capability_expiry_is_measured_against_the_manager_clock() {
        // Expiry is a real rule, and this proves it without waiting: the clock
        // moves because the test says so, not because the machine was slow.
        let clock = Arc::new(ManualClock::default());
        let sessions = SessionManager::with_clock(clock.clone());
        let app = sessions.open_application("client".into(), "local".into(), vec![]);
        let binding = || ActionBinding {
            application_session_id: app.id.clone(),
            principal: "local".into(),
            workspace_id: None,
            change_id: None,
            action_id: "refresh".into(),
            target: None,
            workspace_revision: None,
            change_revision: None,
            registry_revision: None,
            input_contract_digest: String::new(),
            precondition: nothing_mutable(),
            expires_at_unix_ms: sessions.now_unix_ms() + 30_000,
        };

        // Time has not moved, so a freshly issued capability is live however
        // much work happened in between.
        let live = sessions.issue_action(binding());
        assert!(sessions.consume_action(&live, &app.id, "local").is_ok());

        // Past the deadline it is refused, and the reason names expiry rather
        // than any of the other ways a capability can be rejected.
        let stale = sessions.issue_action(binding());
        clock.advance_ms(30_001);
        assert_eq!(
            sessions.consume_action(&stale, &app.id, "local"),
            Err("action capability expired")
        );
    }

    #[test]
    fn action_descriptors_are_single_use() {
        let sessions = SessionManager::new();
        let app = sessions.open_application("client".into(), "local".into(), vec![]);
        let token = sessions.issue_action(ActionBinding {
            application_session_id: app.id.clone(),
            principal: "local".into(),
            workspace_id: None,
            change_id: None,
            action_id: "refresh".into(),
            target: None,
            workspace_revision: None,
            change_revision: None,
            registry_revision: None,
            input_contract_digest: String::new(),
            precondition: nothing_mutable(),
            expires_at_unix_ms: sessions.now_unix_ms() + 10_000,
        });
        assert!(sessions.consume_action(&token, &app.id, "local").is_ok());
        assert!(sessions.consume_action(&token, &app.id, "local").is_err());
    }
}
