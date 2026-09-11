//! Global `~/.draft/` store — user and device-level Draft state.
//!
//! See `docs/internals/storage-and-events.md`.
//!
//! The global store holds identity, the private signing key, trusted public
//! keys, default policies, adapter configuration, the candidate/actor registry,
//! reusable caches/models, a global receipt index, and local trust metrics. It
//! never stores project-local Change data. It is hidden like the project store.
//!
//! The default location is `~/.draft/` (Unix/macOS) or `%USERPROFILE%\.draft\`
//! (Windows). Tests and sandboxes may override it with `DRAFT_GLOBAL_HOME`
//! (an absolute path to the `.draft` directory itself).

use crate::support::error::{DraftError, DraftResult};
use crate::support::fsutil::ensure_dir;
use crate::support::hidden::{self, HiddenStatus};
use std::path::{Path, PathBuf};

#[cfg(any(test, feature = "testing"))]
mod test_home {
    use super::PathBuf;
    use std::cell::RefCell;

    thread_local! {
        /// Per-test global store, when a test needs one of its own.
        ///
        /// This is deliberately thread-local rather than an environment
        /// variable: the test harness gives each test its own thread, so an
        /// override set here cannot be observed — or clobbered — by a test
        /// running concurrently. `DRAFT_GLOBAL_HOME` is process-global, and
        /// mutating it mid-run is what made the suite flaky.
        static OVERRIDE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    /// The scratch store every test resolves to unless it asked for its own.
    ///
    /// Created once per test binary and never mutated, so no test can write
    /// into the developer's real `~/.draft` and none can race another for it.
    pub(super) fn shared() -> &'static std::path::Path {
        static SHARED: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        SHARED
            .get_or_init(|| {
                tempfile::Builder::new()
                    .prefix("draft-test-global-")
                    .tempdir()
                    .expect("test global store")
            })
            .path()
    }

    pub(super) fn current() -> Option<PathBuf> {
        OVERRIDE.with(|slot| slot.borrow().clone())
    }

    /// Points this thread's global store at `root` until the guard drops.
    #[derive(Debug)]
    pub struct ScopedGlobalHome(Option<PathBuf>);

    impl ScopedGlobalHome {
        pub fn set(root: impl Into<PathBuf>) -> Self {
            let previous = OVERRIDE.with(|slot| slot.borrow_mut().replace(root.into()));
            Self(previous)
        }
    }

    impl Drop for ScopedGlobalHome {
        fn drop(&mut self) {
            let previous = self.0.take();
            OVERRIDE.with(|slot| *slot.borrow_mut() = previous);
        }
    }
}

#[cfg(any(test, feature = "testing"))]
pub use test_home::ScopedGlobalHome;

/// Handle to the global Draft store and its canonical layout.
///
/// This is the sole platform-aware resolver for user-scoped Draft state.  Code
/// should ask this type for a logical namespace instead of spelling a home or
/// XDG path itself. `DRAFT_GLOBAL_HOME` remains the supported test/sandbox
/// override and names the store root directly.
#[derive(Debug, Clone)]
pub struct DraftGlobalStore {
    root: PathBuf,
}

/// Typed logical namespaces in the global Draft store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalNamespace {
    Registry,
    Identity,
    Extensions,
    Trust,
    Runtime,
    Operations,
    Notifications,
    Audit,
    Cache,
}

impl GlobalNamespace {
    fn directory(self) -> &'static str {
        match self {
            Self::Registry => "registry",
            Self::Identity => "identity",
            Self::Extensions => "extensions",
            Self::Trust => "trust",
            Self::Runtime => "runtime",
            Self::Operations => "operations",
            Self::Notifications => "notifications",
            Self::Audit => "audit",
            Self::Cache => "cache",
        }
    }
}

impl DraftGlobalStore {
    /// Construct a handle at an explicit `.draft` directory.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        DraftGlobalStore { root: root.into() }
    }

    /// Locate the default global store (respecting `DRAFT_GLOBAL_HOME`).
    ///
    /// A test build resolves a thread-local override first, and otherwise a
    /// scratch store shared by the test binary, so tests neither read the
    /// developer's real store nor race one another over a process-global.
    pub fn locate() -> DraftResult<Self> {
        #[cfg(any(test, feature = "testing"))]
        {
            if let Some(root) = test_home::current() {
                return Ok(DraftGlobalStore::at(root));
            }
        }
        let explicit = std::env::var_os("DRAFT_GLOBAL_HOME");
        #[cfg(any(test, feature = "testing"))]
        let explicit = explicit.or_else(|| Some(test_home::shared().as_os_str().to_owned()));
        Self::resolve(explicit)
    }

    /// Resolve a store from an explicit override, falling back to the user home.
    ///
    /// Split out from [`Self::locate`] so the override rule is testable without
    /// mutating the process environment.
    fn resolve(explicit: Option<std::ffi::OsString>) -> DraftResult<Self> {
        if let Some(explicit) = explicit {
            return Ok(DraftGlobalStore::at(PathBuf::from(explicit)));
        }
        let home = user_home_dir().ok_or_else(|| {
            DraftError::storage("cannot determine home directory (set HOME/USERPROFILE)")
        })?;
        Ok(DraftGlobalStore::at(home.join(".draft")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn exists(&self) -> bool {
        self.root.is_dir()
    }

    pub fn namespace(&self, namespace: GlobalNamespace) -> PathBuf {
        self.root.join(namespace.directory())
    }

    // ---- Canonical layout ------------------------------------------------

    pub fn config_toml(&self) -> PathBuf {
        self.root.join("config.toml")
    }
    pub fn identity_dir(&self) -> PathBuf {
        self.namespace(GlobalNamespace::Identity)
    }
    pub fn actor_json(&self) -> PathBuf {
        self.identity_dir().join("actor.json")
    }
    pub fn candidates_json(&self) -> PathBuf {
        self.identity_dir().join("candidates.json")
    }
    pub fn keys_dir(&self) -> PathBuf {
        self.root.join("keys")
    }
    pub fn signing_key(&self) -> PathBuf {
        self.keys_dir().join("signing.key")
    }
    pub fn public_keys_dir(&self) -> PathBuf {
        self.keys_dir().join("public.keys")
    }
    pub fn trust_dir(&self) -> PathBuf {
        self.namespace(GlobalNamespace::Trust)
    }
    pub fn trusted_actors_json(&self) -> PathBuf {
        self.trust_dir().join("trusted_actors.json")
    }
    pub fn trusted_candidates_json(&self) -> PathBuf {
        self.trust_dir().join("trusted_candidates.json")
    }
    pub fn trusted_workspaces_json(&self) -> PathBuf {
        self.trust_dir().join("trusted_workspaces.json")
    }
    pub fn revoked_keys_json(&self) -> PathBuf {
        self.trust_dir().join("revoked_keys.json")
    }
    pub fn policies_dir(&self) -> PathBuf {
        self.root.join("policies")
    }
    pub fn default_policy_toml(&self) -> PathBuf {
        self.policies_dir().join("default-policy.toml")
    }
    pub fn adapters_dir(&self) -> PathBuf {
        self.root.join("adapters")
    }
    pub fn adapter_dir(&self, name: &str) -> PathBuf {
        self.adapters_dir().join(name)
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.namespace(GlobalNamespace::Cache)
    }
    pub fn models_dir(&self) -> PathBuf {
        self.root.join("models")
    }
    pub fn receipts_dir(&self) -> PathBuf {
        self.root.join("receipts")
    }
    pub fn global_receipt_index(&self) -> PathBuf {
        self.receipts_dir().join("global-index.json")
    }
    pub fn telemetry_dir(&self) -> PathBuf {
        self.root.join("telemetry")
    }
    pub fn registry_dir(&self) -> PathBuf {
        self.namespace(GlobalNamespace::Registry)
    }
    pub fn services_dir(&self) -> PathBuf {
        self.root.join("services")
    }
    pub fn extensions_dir(&self) -> PathBuf {
        self.namespace(GlobalNamespace::Extensions)
    }
    pub fn templates_dir(&self) -> PathBuf {
        self.root.join("templates")
    }
    pub fn indexes_dir(&self) -> PathBuf {
        self.root.join("indexes")
    }
    pub fn doctor_dir(&self) -> PathBuf {
        self.root.join("doctor")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn locks_dir(&self) -> PathBuf {
        self.root.join("locks")
    }
    pub fn recovery_dir(&self) -> PathBuf {
        self.root.join("recovery")
    }
    pub fn runtime_dir(&self) -> PathBuf {
        self.namespace(GlobalNamespace::Runtime)
    }
    pub fn operations_dir(&self) -> PathBuf {
        self.namespace(GlobalNamespace::Operations)
    }
    pub fn notifications_dir(&self) -> PathBuf {
        self.namespace(GlobalNamespace::Notifications)
    }
    pub fn audit_dir(&self) -> PathBuf {
        self.namespace(GlobalNamespace::Audit)
    }
    pub fn local_metrics_json(&self) -> PathBuf {
        self.telemetry_dir().join("local-metrics.json")
    }

    /// Create the full global tree, mark it hidden, and lock down the key dir.
    /// Idempotent: re-running is safe and does not overwrite existing files.
    pub fn create_all(&self) -> DraftResult<HiddenStatus> {
        for dir in [
            self.root.clone(),
            self.identity_dir(),
            self.keys_dir(),
            self.public_keys_dir(),
            self.trust_dir(),
            self.policies_dir(),
            self.adapters_dir(),
            self.adapter_dir("agui"),
            self.cache_dir(),
            self.models_dir(),
            self.receipts_dir(),
            self.telemetry_dir(),
            self.registry_dir(),
            self.services_dir(),
            self.services_dir().join("tokens"),
            self.extensions_dir(),
            self.templates_dir(),
            self.indexes_dir(),
            self.doctor_dir(),
            self.logs_dir(),
            self.locks_dir(),
            self.recovery_dir(),
            self.runtime_dir(),
            self.operations_dir(),
            self.notifications_dir(),
            self.audit_dir(),
        ] {
            ensure_dir(&dir)?;
        }
        // Private-key material gets 0700 on its directory.
        let _ = hidden::restrict_dir(&self.keys_dir(), 0o700);
        let status = hidden::ensure_hidden(&self.root);
        Ok(status)
    }
}

/// Resolve the current user's home directory in a platform-appropriate way.
/// Public so other modules (and `app.rs`) share one definition.
pub fn user_home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_all_builds_hidden_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let home = DraftGlobalStore::at(tmp.path().join(".draft"));
        let status = home.create_all().unwrap();
        assert!(status.is_ok());
        assert!(home.exists());
        assert!(home.keys_dir().is_dir());
        assert!(home.identity_dir().is_dir());
        assert!(home.telemetry_dir().is_dir());
        assert!(std::fs::read_dir(home.adapters_dir())
            .unwrap()
            .all(|entry| entry.unwrap().file_name().to_string_lossy() == "agui"));
        // Idempotent.
        home.create_all().unwrap();
    }

    #[test]
    fn an_explicit_override_names_the_store_root_directly() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("custom/.draft");
        let home = DraftGlobalStore::resolve(Some(target.as_os_str().to_owned())).unwrap();
        assert_eq!(home.root(), target.as_path());
    }

    #[test]
    fn a_scoped_override_is_confined_to_the_thread_that_set_it() {
        let tmp = tempfile::tempdir().unwrap();
        let mine = tmp.path().join("mine/.draft");
        let _scope = ScopedGlobalHome::set(&mine);
        assert_eq!(DraftGlobalStore::locate().unwrap().root(), mine.as_path());

        // A concurrent test would resolve its own store, never this one.
        let elsewhere =
            std::thread::spawn(|| DraftGlobalStore::locate().unwrap().root().to_path_buf())
                .join()
                .unwrap();
        assert_ne!(elsewhere, mine);
    }

    #[test]
    fn a_test_never_resolves_the_real_user_store() {
        let resolved = DraftGlobalStore::locate().unwrap();
        let real = user_home_dir().map(|home| home.join(".draft"));
        assert_ne!(Some(resolved.root().to_path_buf()), real);
    }
}
