//! PATH exposure — the two platform models, and nothing generic.
//!
//! *Unix*: exactly two managed symlinks, `<path_bin>/draft` and
//! `<path_bin>/draftd`, pointing at the canonical binaries. Never a copy. A
//! symlink is validated with `lstat` and its exact target, and removed by
//! unlinking the link itself, never following it.
//!
//! *Windows*: `<install_root>\bin` on the User PATH, governed entirely by
//! [`windows`], which is the single low-level authority for reading,
//! tokenizing, comparing, classifying, appending and removing it.

use std::path::{Path, PathBuf};

use super::layout::{Executable, InstallLayout};
use super::{fail, InstallationFailure};
use crate::support::error::{DraftError, DraftResult};

pub mod unix {
    use super::*;
    use crate::installation::operation::{SlotPreInstall, UnixSlot};
    use crate::installation::{Identity, InstallationOperationId, LifecycleHost};

    /// What occupies one PATH slot right now, by `lstat` of the slot itself.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum SlotObservation {
        Missing,
        /// A symlink whose target is exactly the canonical binary.
        ExpectedSymlink,
        /// A symlink to anything else, or a broken one.
        ForeignSymlink,
        RegularFile,
        /// A directory, device, fifo, socket or anything else.
        Other,
    }

    /// The target a symlink names, resolved against its own directory.
    fn link_target(link: &Path) -> Option<PathBuf> {
        let target = std::fs::read_link(link).ok()?;
        Some(if target.is_absolute() {
            target
        } else {
            link.parent()?.join(target)
        })
    }

    pub fn observe(slot: &Path, expected_target: &Path) -> SlotObservation {
        let Ok(metadata) = std::fs::symlink_metadata(slot) else {
            return SlotObservation::Missing;
        };
        let kind = metadata.file_type();
        if kind.is_symlink() {
            return match link_target(slot) {
                Some(target) if target == expected_target => SlotObservation::ExpectedSymlink,
                _ => SlotObservation::ForeignSymlink,
            };
        }
        if kind.is_file() {
            return SlotObservation::RegularFile;
        }
        SlotObservation::Other
    }

    pub fn slot(path_bin: &Path, executable: Executable) -> PathBuf {
        path_bin.join(executable.stem())
    }

    /// Classify both slots into one plan before anything mutates.
    ///
    /// Missing or already the exact managed symlink is fine. A regular file is
    /// refused by default — an unrelated `draft` must never be deleted — and
    /// is migrated only under the explicit `DRAFT_MIGRATE_LEGACY_PATH=1`
    /// opt-in, and only when *both* slots are regular files that report the
    /// same parseable Draft version. Anything else refuses. Either slot
    /// refusing refuses the pair: there is no one-slot migration.
    pub fn classify_pair(
        layout: &InstallLayout,
        path_bin: &Path,
        migrate_opt_in: bool,
        operation: &InstallationOperationId,
        host: &dyn LifecycleHost,
    ) -> DraftResult<(UnixSlot, UnixSlot, bool)> {
        let observe_one = |executable: Executable| {
            let path = slot(path_bin, executable);
            (path.clone(), observe(&path, &layout.executable(executable)))
        };
        let (draft_path, draft) = observe_one(Executable::Draft);
        let (draftd_path, draftd) = observe_one(Executable::Draftd);
        let refuse = |path: &Path, why: &str| {
            fail(
                InstallationFailure::LegacyInstallationConflict,
                format!(
                    "{why} at {}; move or remove it, then re-run the installer",
                    path.display()
                ),
            )
        };
        for (path, observation) in [(&draft_path, &draft), (&draftd_path, &draftd)] {
            match observation {
                SlotObservation::ForeignSymlink => {
                    return Err(refuse(
                        path,
                        "a symlink to something other than this installation",
                    ))
                }
                SlotObservation::Other => {
                    return Err(refuse(path, "a non-file object already exists"))
                }
                _ => {}
            }
        }
        let legacy = [&draft, &draftd]
            .iter()
            .any(|observation| **observation == SlotObservation::RegularFile);
        if !legacy {
            let slot_of = |observation: &SlotObservation| UnixSlot {
                pre_install: if *observation == SlotObservation::Missing {
                    SlotPreInstall::Missing
                } else {
                    SlotPreInstall::ExpectedManagedSymlink
                },
                legacy_identity: None,
                legacy_staging_slot: None,
                created_by_operation: *observation == SlotObservation::Missing,
                moved_aside: false,
            };
            return Ok((slot_of(&draft), slot_of(&draftd), false));
        }
        if !migrate_opt_in {
            let path = if draft == SlotObservation::RegularFile {
                &draft_path
            } else {
                &draftd_path
            };
            return Err(
                refuse(path, "a non-managed file already exists").with_suggestion(
                    "A copied legacy Draft? Remove or rename both $DRAFT_INSTALL_DIR/draft and \
                 draftd, or set DRAFT_MIGRATE_LEGACY_PATH=1 to authorize replacing them.",
                ),
            );
        }
        if draft != SlotObservation::RegularFile || draftd != SlotObservation::RegularFile {
            return Err(fail(
                InstallationFailure::LegacyInstallationMigrationUnsafe,
                "legacy migration needs both draft and draftd to be regular files; nothing was changed",
            ));
        }
        let draft_version = host.binary_version(&draft_path).ok();
        let draftd_version = host.binary_version(&draftd_path).ok();
        match (&draft_version, &draftd_version) {
            (Some(a), Some(b)) if a == b && semver::Version::parse(a).is_ok() => {}
            _ => {
                return Err(fail(
                    InstallationFailure::LegacyInstallationMigrationUnsafe,
                    "the legacy draft and draftd do not both report the same Draft version; \
                     nothing was changed",
                ))
            }
        }
        let legacy_slot = |executable: Executable, path: &Path| -> DraftResult<UnixSlot> {
            Ok(UnixSlot {
                pre_install: SlotPreInstall::AuthorizedLegacyFile,
                legacy_identity: Some(Identity::of_file(path)?),
                legacy_staging_slot: Some(
                    layout
                        .legacy_slot(operation, executable)
                        .display()
                        .to_string(),
                ),
                created_by_operation: true,
                moved_aside: true,
            })
        };
        Ok((
            legacy_slot(Executable::Draft, &draft_path)?,
            legacy_slot(Executable::Draftd, &draftd_path)?,
            true,
        ))
    }

    /// Create the managed symlink. An existing exact link is success.
    pub fn create(link: &Path, target: &Path) -> DraftResult<()> {
        match observe(link, target) {
            SlotObservation::ExpectedSymlink => Ok(()),
            SlotObservation::Missing => {
                if let Some(parent) = link.parent() {
                    crate::support::fsutil::ensure_dir(parent)?;
                }
                symlink(target, link).map_err(|error| {
                    fail(
                        InstallationFailure::PermissionDenied,
                        format!(
                            "cannot create the PATH symlink {} -> {}: {error}; Draft never falls \
                             back to copying",
                            link.display(),
                            target.display()
                        ),
                    )
                })
            }
            _ => Err(fail(
                InstallationFailure::LegacyInstallationConflict,
                format!(
                    "{} is occupied by something Draft does not own",
                    link.display()
                ),
            )),
        }
    }

    #[cfg(unix)]
    fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(not(unix))]
    fn symlink(_: &Path, _: &Path) -> std::io::Result<()> {
        Err(std::io::Error::other(
            "Unix PATH symlinks exist only on Unix",
        ))
    }

    /// Remove one owned symlink by unlinking the link itself. Absent is
    /// success; a foreign or retargeted object is refused and never deleted.
    pub fn remove(link: &Path, expected_target: &Path) -> DraftResult<()> {
        match observe(link, expected_target) {
            SlotObservation::Missing => Ok(()),
            SlotObservation::ExpectedSymlink => crate::installation::remove_file_if_present(link),
            _ => Err(fail(
                InstallationFailure::PathSymlinkTargetMismatch,
                format!(
                    "{} is no longer the managed symlink to {}; nothing was removed",
                    link.display(),
                    expected_target.display()
                ),
            )),
        }
    }

    /// Move an authorized legacy file into its staging slot, verifying the
    /// identity journalled for it. Already moved is success.
    pub fn stage_legacy(slot: &Path, staging: &Path, identity: &Identity) -> DraftResult<()> {
        if identity.matches(staging) && std::fs::symlink_metadata(slot).is_err() {
            return Ok(());
        }
        if !identity.matches(slot) {
            return Err(fail(
                InstallationFailure::LegacyInstallationMigrationUnsafe,
                format!(
                    "{} no longer holds the legacy file that was classified",
                    slot.display()
                ),
            ));
        }
        if let Some(parent) = staging.parent() {
            crate::support::fsutil::ensure_dir(parent)?;
        }
        std::fs::rename(slot, staging)
            .map_err(|error| DraftError::storage(format!("stage {}: {error}", slot.display())))
    }

    /// Put a staged legacy file back. Already restored is success.
    pub fn restore_legacy(slot: &Path, staging: &Path, identity: &Identity) -> DraftResult<()> {
        if identity.matches(slot) {
            return Ok(());
        }
        if !identity.matches(staging) {
            return Err(fail(
                InstallationFailure::RecoveryFailed,
                format!(
                    "the staged legacy copy of {} is missing or altered",
                    slot.display()
                ),
            ));
        }
        std::fs::rename(staging, slot)
            .map_err(|error| DraftError::storage(format!("restore {}: {error}", slot.display())))
    }
}

pub mod windows {
    //! Windows User PATH (`HKCU\Environment\Path`) — the single authority.
    //!
    //! **Read** raw and unexpanded: `Absent`, or `Present` as `REG_SZ` /
    //! `REG_EXPAND_SZ`; any other registry type is `WindowsPathStateInvalid`.
    //! **Tokenize** on every `;`, keeping empty tokens and exact text.
    //! **Classify** each token against `C = <install_root>\bin` with
    //! `k(x) = x` minus trailing `\`: *opaque* (contains `"` or `%` — never
    //! expanded, matched or removed), *owned-match* (`k(t) == k(C)` under
    //! ordinal ignore-case — the only relation that grants removal authority),
    //! *ambiguous* (not owned-match, but equal under PowerShell-conformant
    //! culture ignore-case, or after trimming whitespace, treating `/` as `\`
    //! or collapsing `.`/`..` — detected, never applied), else *unrelated*.
    //!
    //! Every mutation is one registry write or delete between an immediate
    //! read (M1) and an immediate re-read that must satisfy the action's
    //! semantic postcondition (M5–M7), else `WindowsPathConcurrentMutation`
    //! (M8) with no compensating write and no retry. This serializes Draft's own
    //! lifecycle actors through `lifecycle.lock`; it is **not** a compare-and-set
    //! against other programs writing the registry, and makes no linearizability
    //! claim about a write racing strictly inside the read→write window.

    use super::*;
    use crate::installation::operation::SegmentPreInstall;
    use crate::installation::receipt::{WindowsPathProvenance, WindowsUserPathValueOrigin};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum RegistryType {
        Sz,
        ExpandSz,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum UserPathValue {
        Absent,
        Present { kind: RegistryType, raw: String },
    }

    impl UserPathValue {
        pub fn origin(&self) -> WindowsUserPathValueOrigin {
            match self {
                Self::Absent => WindowsUserPathValueOrigin::Absent,
                Self::Present {
                    kind: RegistryType::Sz,
                    ..
                } => WindowsUserPathValueOrigin::PresentRegSz,
                Self::Present {
                    kind: RegistryType::ExpandSz,
                    ..
                } => WindowsUserPathValueOrigin::PresentRegExpandSz,
            }
        }
    }

    /// The one registry value Draft ever touches. Real on Windows; injected in
    /// tests so every crash window and foreign writer can be exercised.
    pub trait UserPathRegistry {
        /// Raw and unexpanded. Any type but `REG_SZ` / `REG_EXPAND_SZ` is
        /// `WindowsPathStateInvalid`.
        fn read(&self) -> DraftResult<UserPathValue>;
        fn write(&self, kind: RegistryType, raw: &str) -> DraftResult<()>;
        fn delete(&self) -> DraftResult<()>;
        /// Best-effort `WM_SETTINGCHANGE` "Environment".
        fn broadcast(&self) {}
    }

    /// `C` — the canonical segment, always re-derived from the validated root.
    pub fn canonical_segment(layout: &InstallLayout) -> String {
        let root = layout.root().display().to_string().replace('/', "\\");
        format!("{}\\bin", root.trim_end_matches('\\'))
    }

    fn key(value: &str) -> &str {
        value.trim_end_matches('\\')
    }

    /// .NET `OrdinalIgnoreCase`: per-character simple uppercase mapping, as the
    /// OS upcase table does it. Observed on Windows (the conformance test):
    /// final sigma is not folded, and no non-ASCII character folds onto ASCII
    /// (dotless `ı` is not `I`, long `ſ` is not `S`).
    #[cfg_attr(windows, allow(dead_code))]
    fn ordinal_fold(value: &str) -> String {
        value
            .chars()
            .map(|c| {
                if c == 'ς' {
                    return c;
                }
                let mut upper = c.to_uppercase();
                match (upper.next(), upper.next()) {
                    (Some(single), None) if c.is_ascii() || !single.is_ascii() => single,
                    _ => c,
                }
            })
            .collect()
    }

    /// PowerShell-conformant culture ignore-case (`-ieq`), which additionally
    /// equates `ß`/`ss`, canonically equivalent sequences, the Kelvin sign,
    /// final sigma and compatibility ligatures. The observed vectors, not the
    /// API name, are the specification.
    #[cfg(not(windows))]
    fn culture_equal(left: &str, right: &str) -> bool {
        fn fold(value: &str) -> String {
            let mut out = String::new();
            for c in value.chars() {
                match c {
                    // Combining marks are compared as if composed onto their base.
                    '\u{0300}'..='\u{036f}' => {}
                    'ß' | 'ẞ' => out.push_str("ss"),
                    'ς' => out.push('σ'),
                    'ﬀ' => out.push_str("ff"),
                    'ﬁ' => out.push_str("fi"),
                    'ﬂ' => out.push_str("fl"),
                    'ﬃ' => out.push_str("ffi"),
                    'ﬄ' => out.push_str("ffl"),
                    other => {
                        for lower in other.to_lowercase() {
                            out.push(base_letter(lower));
                        }
                    }
                }
            }
            out
        }
        fold(left) == fold(right)
    }

    /// Precomposed Latin letters reduced to their base, so `é` (U+00E9) and
    /// `e` + U+0301 fold alike. A conservative over-approximation: it can only
    /// make more tokens *ambiguous*, which only ever makes Draft own less.
    #[cfg(not(windows))]
    fn base_letter(c: char) -> char {
        match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            other => other,
        }
    }

    #[cfg(windows)]
    fn culture_equal(left: &str, right: &str) -> bool {
        native::compare_culture_ignore_case(left, right)
    }

    #[cfg(windows)]
    fn ordinal_equal(left: &str, right: &str) -> bool {
        native::compare_ordinal_ignore_case(left, right)
    }

    #[cfg(not(windows))]
    fn ordinal_equal(left: &str, right: &str) -> bool {
        ordinal_fold(left) == ordinal_fold(right)
    }

    /// The only relation that grants removal authority.
    pub fn is_owned_match(token: &str, segment: &str) -> bool {
        !is_opaque(token) && ordinal_equal(key(token), key(segment))
    }

    fn is_opaque(token: &str) -> bool {
        token.contains('"') || token.contains('%')
    }

    /// Lexical collapse of `.` / `..` segments. Detection only.
    fn collapse(value: &str) -> String {
        let mut parts: Vec<&str> = Vec::new();
        for part in value.split('\\') {
            match part {
                "." => {}
                ".." => {
                    parts.pop();
                }
                other => parts.push(other),
            }
        }
        parts.join("\\")
    }

    fn loose(value: &str) -> String {
        let normalized = value.trim().replace('/', "\\");
        key(&collapse(&normalized)).to_string()
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TokenClass {
        Opaque,
        OwnedMatch,
        Ambiguous,
        Unrelated,
    }

    pub fn classify(token: &str, segment: &str) -> TokenClass {
        if is_opaque(token) {
            TokenClass::Opaque
        } else if is_owned_match(token, segment) {
            TokenClass::OwnedMatch
        } else if culture_equal(key(token), key(segment))
            || culture_equal(&loose(token), &loose(segment))
        {
            TokenClass::Ambiguous
        } else {
            TokenClass::Unrelated
        }
    }

    pub fn tokenize(raw: &str) -> Vec<&str> {
        raw.split(';').collect()
    }

    /// Owned-match and ambiguous counts of a present value.
    pub fn counts(raw: &str, segment: &str) -> (usize, usize) {
        let mut owned = 0;
        let mut ambiguous = 0;
        for token in tokenize(raw) {
            match classify(token, segment) {
                TokenClass::OwnedMatch => owned += 1,
                TokenClass::Ambiguous => ambiguous += 1,
                _ => {}
            }
        }
        (owned, ambiguous)
    }

    /// The one-time pre-install classification (before `Resolved`).
    ///
    /// Exposure — an owned-match or ambiguous User PATH token, or a
    /// process-effective PATH match under `install.ps1`'s own rule
    /// (`TrimEnd('\') -ieq`) — makes it `PreExisting`: no append, no
    /// reservation, no removal authority ever. Otherwise the request decides
    /// between `AddedByDraft` (the durable reservation) and `NotManaged`.
    pub fn classify_pre_install(
        registry: &dyn UserPathRegistry,
        segment: &str,
        process_path: Option<&str>,
        update_requested: bool,
    ) -> DraftResult<(
        WindowsUserPathValueOrigin,
        SegmentPreInstall,
        WindowsPathProvenance,
    )> {
        let value = registry.read()?;
        let user_exposure = match &value {
            UserPathValue::Absent => false,
            UserPathValue::Present { raw, .. } => {
                let (owned, ambiguous) = counts(raw, segment);
                owned + ambiguous > 0
            }
        };
        let process_exposure = process_path.is_some_and(|path| {
            path.split(';')
                .filter(|part| !part.is_empty())
                .any(|part| culture_equal(key(part), key(segment)))
        });
        let exposure = user_exposure || process_exposure;
        let provenance = match (exposure, update_requested) {
            (true, _) => WindowsPathProvenance::PreExisting,
            (false, true) => WindowsPathProvenance::AddedByDraft,
            (false, false) => WindowsPathProvenance::NotManaged,
        };
        let segment_state = if exposure {
            SegmentPreInstall::Present
        } else {
            SegmentPreInstall::Absent
        };
        Ok((value.origin(), segment_state, provenance))
    }

    fn concurrent(message: impl Into<String>) -> DraftError {
        fail(InstallationFailure::WindowsPathConcurrentMutation, message)
    }

    fn ambiguous_state(message: impl Into<String>) -> DraftError {
        fail(InstallationFailure::UninstallPlanInvalid, message)
    }

    /// Whether an `AddedByDraft` reservation is satisfied right now: exactly
    /// one owned-match and no ambiguity. Zero is "not yet"; anything else
    /// fails closed.
    pub fn reservation_satisfied(
        registry: &dyn UserPathRegistry,
        segment: &str,
    ) -> DraftResult<bool> {
        match registry.read()? {
            UserPathValue::Absent => Ok(false),
            UserPathValue::Present { raw, .. } => match counts(&raw, segment) {
                (1, 0) => Ok(true),
                (0, 0) => Ok(false),
                _ => Err(ambiguous_state(
                    "the User PATH holds a duplicate or ambiguous copy of Draft's segment",
                )),
            },
        }
    }

    /// Ensure the reserved exposure exists (`PathIntegrationApplied` for an
    /// `AddedByDraft` install). Idempotent: an existing single owned-match —
    /// even one another actor wrote after `Resolved` — satisfies the
    /// reservation and writes nothing.
    pub fn apply(registry: &dyn UserPathRegistry, segment: &str) -> DraftResult<()> {
        // M1–M3: read the current snapshot and derive exactly one mutation.
        let (kind, raw) = match registry.read()? {
            UserPathValue::Absent => (RegistryType::Sz, segment.to_string()),
            UserPathValue::Present { kind, raw } => match counts(&raw, segment) {
                (1, 0) => return Ok(()),
                (0, 0) if raw.is_empty() => (kind, segment.to_string()),
                (0, 0) => (kind, format!("{raw};{segment}")),
                _ => {
                    return Err(ambiguous_state(
                        "the User PATH already holds a duplicate or ambiguous copy of Draft's \
                         segment; nothing was written",
                    ))
                }
            },
        };
        // M4: exactly one write.
        registry.write(kind, &raw)?;
        // M5–M8: immediate re-read against the postcondition.
        match registry.read() {
            Ok(UserPathValue::Present { raw, .. }) if counts(&raw, segment) == (1, 0) => {
                registry.broadcast();
                Ok(())
            }
            _ => Err(concurrent(
                "the User PATH changed while Draft appended its segment; it was not re-written",
            )),
        }
    }

    /// The I75 undo case the current value is in.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum UndoCase {
        /// U1: the value is absent — the delta is gone.
        Absent,
        /// U2: originally absent and exactly the reservation-only singleton.
        DeleteValue,
        /// U3: exactly one owned token among other content.
        RemoveToken,
        /// U4: no owned-match and no ambiguity — already undone.
        AlreadyUndone,
        /// U5: duplicates or ambiguity — fail closed.
        Ambiguous,
    }

    pub fn undo_case(
        value: &UserPathValue,
        segment: &str,
        origin: WindowsUserPathValueOrigin,
    ) -> UndoCase {
        match value {
            UserPathValue::Absent => UndoCase::Absent,
            UserPathValue::Present { kind, raw } => {
                if origin == WindowsUserPathValueOrigin::Absent
                    && *kind == RegistryType::Sz
                    && raw.as_str() == segment
                {
                    return UndoCase::DeleteValue;
                }
                match counts(raw, segment) {
                    (1, 0) => UndoCase::RemoveToken,
                    (0, 0) => UndoCase::AlreadyUndone,
                    _ => UndoCase::Ambiguous,
                }
            }
        }
    }

    /// Compare-and-undo from the CURRENT state — one rule for `FreshInstall`
    /// rollback and `Uninstall` removal. Never restores a historical PATH, never
    /// deletes a value that pre-dated Draft or is merely empty, and performs at
    /// most one registry mutation per attempt.
    pub fn undo(
        registry: &dyn UserPathRegistry,
        segment: &str,
        origin: WindowsUserPathValueOrigin,
    ) -> DraftResult<()> {
        let value = registry.read()?;
        match undo_case(&value, segment, origin) {
            UndoCase::Absent | UndoCase::AlreadyUndone => Ok(()),
            UndoCase::Ambiguous => Err(ambiguous_state(
                "the User PATH holds a duplicate or ambiguous copy of Draft's segment; nothing \
                 was removed",
            )),
            UndoCase::DeleteValue => {
                registry.delete()?;
                match registry.read() {
                    Ok(UserPathValue::Absent) => Ok(()),
                    Ok(UserPathValue::Present { raw, .. }) if counts(&raw, segment) == (0, 0) => {
                        Ok(())
                    }
                    _ => Err(concurrent(
                        "the User PATH changed while Draft removed its value",
                    )),
                }
            }
            UndoCase::RemoveToken => {
                let UserPathValue::Present { kind, raw } = value else {
                    unreachable!("RemoveToken is only derived from a present value")
                };
                let remaining: Vec<&str> = tokenize(&raw)
                    .into_iter()
                    .filter(|token| classify(token, segment) != TokenClass::OwnedMatch)
                    .collect();
                registry.write(kind, &remaining.join(";"))?;
                match registry.read() {
                    Ok(UserPathValue::Absent) => Ok(()),
                    Ok(UserPathValue::Present { raw, .. }) if counts(&raw, segment) == (0, 0) => {
                        registry.broadcast();
                        Ok(())
                    }
                    _ => Err(concurrent(
                        "the User PATH changed while Draft removed its segment",
                    )),
                }
            }
        }
    }

    /// An in-memory registry for tests: records every mutation and can let a
    /// "foreign writer" act between Draft's write and its verification read.
    pub struct MemoryRegistry {
        pub value: std::cell::RefCell<Result<UserPathValue, ()>>,
        pub mutations: std::cell::RefCell<Vec<String>>,
        #[allow(clippy::type_complexity)]
        pub after_mutation: std::cell::RefCell<Option<Box<dyn FnMut(&mut UserPathValue)>>>,
    }

    impl MemoryRegistry {
        pub fn new(value: UserPathValue) -> Self {
            Self {
                value: std::cell::RefCell::new(Ok(value)),
                mutations: Default::default(),
                after_mutation: Default::default(),
            }
        }

        /// A value of an unsupported registry type (e.g. `REG_BINARY`).
        pub fn unsupported() -> Self {
            let registry = Self::new(UserPathValue::Absent);
            *registry.value.borrow_mut() = Err(());
            registry
        }

        pub fn current(&self) -> UserPathValue {
            self.value.borrow().clone().expect("a supported value")
        }

        fn interfere(&self) {
            if let Some(hook) = self.after_mutation.borrow_mut().as_mut() {
                if let Ok(value) = self.value.borrow_mut().as_mut() {
                    hook(value);
                }
            }
        }
    }

    impl UserPathRegistry for MemoryRegistry {
        fn read(&self) -> DraftResult<UserPathValue> {
            self.value.borrow().clone().map_err(|()| {
                fail(
                    InstallationFailure::WindowsPathStateInvalid,
                    "HKCU\\Environment\\Path has an unsupported registry type",
                )
            })
        }

        fn write(&self, kind: RegistryType, raw: &str) -> DraftResult<()> {
            self.mutations
                .borrow_mut()
                .push(format!("write {kind:?} {raw}"));
            *self.value.borrow_mut() = Ok(UserPathValue::Present {
                kind,
                raw: raw.to_string(),
            });
            self.interfere();
            Ok(())
        }

        fn delete(&self) -> DraftResult<()> {
            self.mutations.borrow_mut().push("delete".into());
            *self.value.borrow_mut() = Ok(UserPathValue::Absent);
            self.interfere();
            Ok(())
        }
    }

    /// The real `HKCU\Environment\Path`.
    #[cfg(windows)]
    pub struct SystemRegistry;

    #[cfg(windows)]
    impl UserPathRegistry for SystemRegistry {
        fn read(&self) -> DraftResult<UserPathValue> {
            native::read()
        }
        fn write(&self, kind: RegistryType, raw: &str) -> DraftResult<()> {
            native::write(kind, raw)
        }
        fn delete(&self) -> DraftResult<()> {
            native::delete()
        }
        fn broadcast(&self) {
            native::broadcast();
        }
    }

    #[cfg(windows)]
    mod native {
        use super::*;
        use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
        use windows_sys::Win32::Globalization::{
            CompareStringEx, CompareStringOrdinal, CSTR_EQUAL, NORM_IGNORECASE,
        };
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
            HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ, REG_SZ,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
        };

        fn wide(value: &str) -> Vec<u16> {
            value.encode_utf16().chain(std::iter::once(0)).collect()
        }

        struct Key(HKEY);
        impl Drop for Key {
            fn drop(&mut self) {
                unsafe { RegCloseKey(self.0) };
            }
        }

        fn open(access: u32) -> DraftResult<Key> {
            let subkey = wide("Environment");
            let mut key: HKEY = std::ptr::null_mut();
            let status =
                unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, access, &mut key) };
            if status != ERROR_SUCCESS {
                return Err(DraftError::storage(format!(
                    "open HKCU\\Environment: error {status}"
                )));
            }
            Ok(Key(key))
        }

        pub fn read() -> DraftResult<UserPathValue> {
            let key = open(KEY_QUERY_VALUE)?;
            let name = wide("Path");
            let mut kind = 0u32;
            let mut size = 0u32;
            let status = unsafe {
                RegQueryValueExW(
                    key.0,
                    name.as_ptr(),
                    std::ptr::null(),
                    &mut kind,
                    std::ptr::null_mut(),
                    &mut size,
                )
            };
            if status == ERROR_FILE_NOT_FOUND {
                return Ok(UserPathValue::Absent);
            }
            if status != ERROR_SUCCESS {
                return Err(DraftError::storage(format!(
                    "read User PATH: error {status}"
                )));
            }
            let kind = match kind {
                REG_SZ => RegistryType::Sz,
                REG_EXPAND_SZ => RegistryType::ExpandSz,
                _ => {
                    return Err(fail(
                        InstallationFailure::WindowsPathStateInvalid,
                        "HKCU\\Environment\\Path has a registry type other than REG_SZ / \
                         REG_EXPAND_SZ",
                    ))
                }
            };
            let mut buffer = vec![0u16; (size as usize).div_ceil(2) + 1];
            let mut bytes = (buffer.len() * 2) as u32;
            let status = unsafe {
                RegQueryValueExW(
                    key.0,
                    name.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    buffer.as_mut_ptr().cast(),
                    &mut bytes,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(DraftError::storage(format!(
                    "read User PATH: error {status}"
                )));
            }
            let mut units = &buffer[..(bytes as usize / 2)];
            while let Some((0, rest)) = units.split_last() {
                units = rest;
            }
            Ok(UserPathValue::Present {
                kind,
                raw: String::from_utf16_lossy(units),
            })
        }

        pub fn write(kind: RegistryType, raw: &str) -> DraftResult<()> {
            let key = open(KEY_SET_VALUE)?;
            let name = wide("Path");
            let data = wide(raw);
            let kind = match kind {
                RegistryType::Sz => REG_SZ,
                RegistryType::ExpandSz => REG_EXPAND_SZ,
            };
            let status = unsafe {
                RegSetValueExW(
                    key.0,
                    name.as_ptr(),
                    0,
                    kind,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(DraftError::storage(format!(
                    "write User PATH: error {status}"
                )));
            }
            Ok(())
        }

        pub fn delete() -> DraftResult<()> {
            let key = open(KEY_SET_VALUE)?;
            let name = wide("Path");
            let status = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
            if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
                return Err(DraftError::storage(format!(
                    "delete User PATH: error {status}"
                )));
            }
            Ok(())
        }

        pub fn broadcast() {
            let environment = wide("Environment");
            let mut result = 0usize;
            unsafe {
                SendMessageTimeoutW(
                    HWND_BROADCAST,
                    WM_SETTINGCHANGE,
                    0,
                    environment.as_ptr() as isize,
                    SMTO_ABORTIFHUNG,
                    5000,
                    &mut result,
                )
            };
        }

        pub fn compare_ordinal_ignore_case(left: &str, right: &str) -> bool {
            let (left, right): (Vec<u16>, Vec<u16>) = (
                left.encode_utf16().collect(),
                right.encode_utf16().collect(),
            );
            unsafe {
                CompareStringOrdinal(
                    left.as_ptr(),
                    left.len() as i32,
                    right.as_ptr(),
                    right.len() as i32,
                    1,
                ) == CSTR_EQUAL
            }
        }

        pub fn compare_culture_ignore_case(left: &str, right: &str) -> bool {
            let (left, right): (Vec<u16>, Vec<u16>) = (
                left.encode_utf16().collect(),
                right.encode_utf16().collect(),
            );
            let invariant = wide("");
            unsafe {
                CompareStringEx(
                    invariant.as_ptr(),
                    NORM_IGNORECASE,
                    left.as_ptr(),
                    left.len() as i32,
                    right.as_ptr(),
                    right.len() as i32,
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                ) == CSTR_EQUAL
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use WindowsUserPathValueOrigin as Origin;

        const C: &str = r"C:\Users\ada\AppData\Local\Programs\Draft\bin";

        fn present(kind: RegistryType, raw: &str) -> UserPathValue {
            UserPathValue::Present {
                kind,
                raw: raw.to_string(),
            }
        }

        fn sz(raw: &str) -> UserPathValue {
            present(RegistryType::Sz, raw)
        }

        #[test]
        fn the_frozen_comparator_vectors() {
            let owned = [
                C.to_string(),
                C.to_ascii_uppercase(),
                format!(r"{C}\"),
                format!(r"{C}\\\"),
            ];
            for token in &owned {
                assert_eq!(classify(token, C), TokenClass::OwnedMatch, "{token}");
            }
            for token in [
                format!("\"{C}\""),
                r"%LOCALAPPDATA%\Programs\Draft\bin".to_string(),
            ] {
                assert_eq!(classify(&token, C), TokenClass::Opaque, "{token}");
            }
            for token in [
                format!(" {C} "),
                C.replace('\\', "/"),
                format!(r"{C}\..\bin"),
                r"C:\Users\ada\AppData\Local\Programs\Draft\.\bin".to_string(),
            ] {
                assert_eq!(classify(&token, C), TokenClass::Ambiguous, "{token}");
            }
            for token in [
                r"\\server\share\bin",
                r"D:\Users\ada\AppData\Local\Programs\Draft\bin",
                r"C:\Windows",
            ] {
                assert_eq!(classify(token, C), TokenClass::Unrelated, "{token}");
            }
            // `-ieq` true, ordinal false: never an owned-match, always ambiguous.
            for (segment, token) in [
                (r"C:\straße\bin", r"C:\STRASSE\bin"),
                ("C:\\caf\u{e9}\\bin", "C:\\cafe\u{301}\\bin"),
                (r"C:\k\bin", "C:\\\u{212a}\\bin"),
                ("C:\\\u{3c3}\\bin", "C:\\\u{3c2}\\bin"),
                (r"C:\fi\bin", "C:\\\u{fb01}\\bin"),
            ] {
                assert_eq!(classify(token, segment), TokenClass::Ambiguous, "{token}");
            }
            // Ordinal ignore-case agrees with `-ieq` on these.
            for (segment, token) in [
                ("C:\\\u{c9}t\u{e9}\\bin", "C:\\\u{e9}T\u{c9}\\bin"),
                ("C:\\\u{416}\\bin", "C:\\\u{436}\\bin"),
            ] {
                assert_eq!(classify(token, segment), TokenClass::OwnedMatch, "{token}");
            }
            // Zero-width space and full-width letters are different paths.
            assert_eq!(
                classify("C:\\\u{200b}x\\bin", r"C:\x\bin"),
                TokenClass::Unrelated
            );
            assert_eq!(
                classify("C:\\\u{ff58}\\bin", r"C:\x\bin"),
                TokenClass::Unrelated
            );
        }

        /// The frozen comparator vectors, run through real PowerShell
        /// (`$a.TrimEnd('\') -ieq $b.TrimEnd('\')`) and through the Rust keys.
        /// Mandatory on Windows; also runs wherever `powershell.exe` is
        /// reachable (for example WSL).
        #[test]
        fn powershell_ieq_and_the_rust_keys_agree_on_the_frozen_vectors() {
            let vectors: Vec<(String, String)> = [
                (C, C),
                (C, &C.to_ascii_uppercase()),
                (C, &format!("{C}\\")),
                (C, &format!("{C}\\\\")),
                (C, &C.replace('\\', "/")),
                (C, &format!(" {C} ")),
                (C, &format!("\"{C}\"")),
                (C, r"%LOCALAPPDATA%\Programs\Draft\bin"),
                (C, r"\\server\share\bin"),
                (C, r"D:\Users\ada\AppData\Local\Programs\Draft\bin"),
                ("C:\\\u{c9}t\u{e9}", "C:\\\u{e9}T\u{c9}"),
                ("C:\\\u{416}", "C:\\\u{436}"),
                ("C:\\i", "C:\\\u{131}"),
                ("C:\\i", "C:\\\u{130}"),
                ("C:\\x", "C:\\\u{200b}x"),
                ("C:\\x", "C:\\\u{ff58}"),
                ("C:\\stra\u{df}e", "C:\\STRASSE"),
                ("C:\\caf\u{e9}", "C:\\cafe\u{301}"),
                ("C:\\k", "C:\\\u{212a}"),
                ("C:\\\u{3c3}", "C:\\\u{3c2}"),
                ("C:\\fi", "C:\\\u{fb01}"),
            ]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
            let powershell = ["powershell.exe", "powershell"].into_iter().find(|exe| {
                std::process::Command::new(exe)
                    .arg("-Help")
                    .output()
                    .is_ok()
            });
            let Some(powershell) = powershell else {
                if cfg!(windows) {
                    panic!("the conformance test must run on Windows");
                }
                eprintln!(
                    "SKIPPED: no powershell.exe reachable; the Windows CI lane runs this test"
                );
                return;
            };
            let literal = |text: &str| -> String {
                let codes: Vec<String> = text
                    .encode_utf16()
                    .map(|unit| format!("[char]{unit}"))
                    .collect();
                format!("(-join @({}))", codes.join(","))
            };
            let mut script = String::new();
            for (a, b) in &vectors {
                script.push_str(&format!(
                    "$a={};$b={};$i=($a.TrimEnd([char]92) -ieq $b.TrimEnd([char]92));$o=[string]::Equals($a.TrimEnd([char]92),$b.TrimEnd([char]92),[StringComparison]::OrdinalIgnoreCase);Write-Output (\"$i $o\");",
                    literal(a),
                    literal(b)
                ));
            }
            // The script is pure ASCII (every vector is spelled as [char]
            // codes), so it travels over stdin without any encoding question.
            let output = {
                use std::io::Write;
                let mut child = std::process::Command::new(powershell)
                    .args(["-NoProfile", "-NonInteractive", "-Command", "-"])
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .unwrap();
                let mut stdin = child.stdin.take().unwrap();
                for line in script.split(';').filter(|line| !line.is_empty()) {
                    writeln!(stdin, "{line}").unwrap();
                }
                drop(stdin);
                child.wait_with_output().unwrap()
            };
            let text = String::from_utf8_lossy(&output.stdout);
            let results: Vec<(bool, bool)> = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    let mut parts = line.split_whitespace();
                    (parts.next() == Some("True"), parts.next() == Some("True"))
                })
                .collect();
            assert_eq!(
                results.len(),
                vectors.len(),
                "{text}{}",
                String::from_utf8_lossy(&output.stderr)
            );
            for ((segment, token), (ieq, ordinal)) in vectors.iter().zip(results) {
                let class = classify(token, segment);
                if class == TokenClass::OwnedMatch {
                    assert!(ieq, "Rust owns {token:?} but PowerShell -ieq rejects it");
                    assert!(
                        ordinal,
                        "Rust owns {token:?} but ordinal ignore-case rejects it"
                    );
                }
                if !ieq && !token.contains('"') && !token.contains('%') {
                    assert_ne!(class, TokenClass::OwnedMatch, "{token:?}");
                }
                if ieq && !ordinal {
                    assert_eq!(
                        class,
                        TokenClass::Ambiguous,
                        "-ieq-only equivalence must be ambiguous: {token:?}"
                    );
                }
            }
        }

        #[test]
        fn pre_install_classification_table() {
            let absent = MemoryRegistry::new(UserPathValue::Absent);
            let cases = [
                (&absent, None, false, WindowsPathProvenance::NotManaged),
                (&absent, None, true, WindowsPathProvenance::AddedByDraft),
                (&absent, Some(C), false, WindowsPathProvenance::PreExisting),
                (&absent, Some(C), true, WindowsPathProvenance::PreExisting),
            ];
            for (registry, process, requested, expected) in cases {
                let (_, _, provenance) =
                    classify_pre_install(registry, C, process, requested).unwrap();
                assert_eq!(provenance, expected);
            }
            let owned = MemoryRegistry::new(sz(&format!(r"C:\Tools;{C}")));
            assert_eq!(
                classify_pre_install(&owned, C, None, true).unwrap().2,
                WindowsPathProvenance::PreExisting
            );
            let ambiguous = MemoryRegistry::new(sz(&C.replace('\\', "/")));
            assert_eq!(
                classify_pre_install(&ambiguous, C, None, true).unwrap(),
                (
                    Origin::PresentRegSz,
                    SegmentPreInstall::Present,
                    WindowsPathProvenance::PreExisting
                )
            );
            assert_eq!(
                crate::installation::failure_of(
                    &classify_pre_install(&MemoryRegistry::unsupported(), C, None, true)
                        .unwrap_err()
                ),
                Some(InstallationFailure::WindowsPathStateInvalid)
            );
        }

        #[test]
        fn an_absent_value_is_created_as_reg_sz_and_rollback_deletes_it_directly() {
            let registry = MemoryRegistry::new(UserPathValue::Absent);
            apply(&registry, C).unwrap();
            assert_eq!(registry.current(), sz(C));
            // A crash after the write: the probe sees the reservation satisfied.
            assert!(reservation_satisfied(&registry, C).unwrap());
            apply(&registry, C).unwrap();
            assert_eq!(registry.mutations.borrow().len(), 1);
            undo(&registry, C, Origin::Absent).unwrap();
            assert_eq!(registry.current(), UserPathValue::Absent);
            // One delete, no empty-string write first.
            assert_eq!(
                *registry.mutations.borrow(),
                [format!("write Sz {C}"), "delete".to_string()]
            );
            // Crash after the delete: completed, nothing more written.
            undo(&registry, C, Origin::Absent).unwrap();
            assert_eq!(registry.mutations.borrow().len(), 2);
        }

        #[test]
        fn compare_and_undo_preserves_foreign_state_in_the_snapshot() {
            for (origin, before, expected) in [
                (Origin::Absent, sz(&format!("{C};Other")), sz("Other")),
                (Origin::Absent, sz(&format!("Other;{C}")), sz("Other")),
                (Origin::Absent, sz("Other"), sz("Other")),
                (
                    Origin::Absent,
                    present(RegistryType::ExpandSz, &format!("{C};Other")),
                    present(RegistryType::ExpandSz, "Other"),
                ),
                (
                    Origin::Absent,
                    present(RegistryType::ExpandSz, C),
                    present(RegistryType::ExpandSz, ""),
                ),
                (Origin::PresentRegSz, sz(C), sz("")),
                (
                    Origin::PresentRegExpandSz,
                    present(RegistryType::ExpandSz, C),
                    present(RegistryType::ExpandSz, ""),
                ),
                (Origin::PresentRegSz, sz(&format!("A;;{C};B")), sz("A;;B")),
                (Origin::PresentRegSz, sz(&format!(";{C};")), sz(";")),
                (Origin::PresentRegSz, sz(""), sz("")),
            ] {
                let registry = MemoryRegistry::new(before.clone());
                undo(&registry, C, origin).unwrap();
                assert_eq!(registry.current(), expected, "{before:?}");
            }
            for ambiguous in [
                sz(&format!("{C};{C}")),
                sz(&format!("{C};{}", C.replace('\\', "/"))),
            ] {
                let registry = MemoryRegistry::new(ambiguous);
                assert!(undo(&registry, C, Origin::Absent).is_err());
                assert!(registry.mutations.borrow().is_empty());
            }
            assert!(undo(&MemoryRegistry::unsupported(), C, Origin::Absent).is_err());
        }

        #[test]
        fn append_preserves_type_and_every_unrelated_token() {
            for (before, after) in [
                (sz(""), sz(C)),
                (
                    present(RegistryType::ExpandSz, ""),
                    present(RegistryType::ExpandSz, C),
                ),
                (sz("A;;B"), sz(&format!("A;;B;{C}"))),
                (
                    present(RegistryType::ExpandSz, "%X%"),
                    present(RegistryType::ExpandSz, &format!("%X%;{C}")),
                ),
            ] {
                let registry = MemoryRegistry::new(before);
                apply(&registry, C).unwrap();
                assert_eq!(registry.current(), after);
            }
            let duplicate = MemoryRegistry::new(sz(&format!("{C};{C}")));
            assert!(apply(&duplicate, C).is_err());
            assert!(duplicate.mutations.borrow().is_empty());
        }

        #[test]
        fn observable_interference_fails_closed_without_a_second_write() {
            // A duplicate appears between Draft's append and its verification read.
            let registry = MemoryRegistry::new(sz("A"));
            *registry.after_mutation.borrow_mut() = Some(Box::new(|value| {
                if let UserPathValue::Present { raw, .. } = value {
                    raw.push_str(&format!(";{C}"));
                }
            }));
            assert_eq!(
                crate::installation::failure_of(&apply(&registry, C).unwrap_err()),
                Some(InstallationFailure::WindowsPathConcurrentMutation)
            );
            assert_eq!(registry.mutations.borrow().len(), 1);

            // The token reappears after removal.
            let registry = MemoryRegistry::new(sz(&format!("A;{C}")));
            *registry.after_mutation.borrow_mut() = Some(Box::new(|value| {
                *value = UserPathValue::Present {
                    kind: RegistryType::Sz,
                    raw: format!("A;{C}"),
                };
            }));
            assert!(undo(&registry, C, Origin::PresentRegSz).is_err());
            assert_eq!(registry.mutations.borrow().len(), 1);

            // A foreign token added after the write is accepted when Draft's own
            // postcondition still holds.
            let registry = MemoryRegistry::new(sz("A"));
            *registry.after_mutation.borrow_mut() = Some(Box::new(|value| {
                if let UserPathValue::Present { raw, .. } = value {
                    raw.push_str(";Later");
                }
            }));
            apply(&registry, C).unwrap();
            assert_eq!(registry.current(), sz(&format!("A;{C};Later")));
        }

        #[test]
        fn a_reservation_is_satisfied_by_one_owned_token_whoever_wrote_it() {
            // This asserts the operation's reservation, not physical authorship.
            let registry = MemoryRegistry::new(sz(&format!("A;{C}")));
            assert!(reservation_satisfied(&registry, C).unwrap());
            apply(&registry, C).unwrap();
            assert!(registry.mutations.borrow().is_empty());
            let two = MemoryRegistry::new(sz(&format!("{C};{C}")));
            assert!(reservation_satisfied(&two, C).is_err());
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::unix::*;

    #[test]
    fn slots_are_observed_by_lstat_and_removed_by_the_link_itself() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("root/bin/draft");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"binary").unwrap();
        let bin = dir.path().join("pathbin");
        std::fs::create_dir_all(&bin).unwrap();
        let link = bin.join("draft");
        assert_eq!(observe(&link, &target), SlotObservation::Missing);
        create(&link, &target).unwrap();
        assert_eq!(observe(&link, &target), SlotObservation::ExpectedSymlink);
        create(&link, &target).unwrap();
        remove(&link, &target).unwrap();
        assert!(target.exists(), "removing a link never touches its target");
        assert_eq!(observe(&link, &target), SlotObservation::Missing);

        std::os::unix::fs::symlink(dir.path().join("elsewhere"), &link).unwrap();
        assert_eq!(observe(&link, &target), SlotObservation::ForeignSymlink);
        assert!(remove(&link, &target).is_err());
        assert!(
            std::fs::symlink_metadata(&link).is_ok(),
            "a foreign link survives"
        );
    }
}
