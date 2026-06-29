//! The single orchestration entry point shared by the CLI (embedded mode) and
//! `draftd`. It ties the engines together and is the **only** place that emits
//! the operation-log records that the engines do not self-append.
//!
//! ## Who appends operations
//! - This module appends: `WorkspaceDetected`, `WorkspaceInitialized`,
//!   `ProviderSelected`, `ChangeScanned`, `ChangeGrouped`, `ReviewStarted`,
//!   `ReviewDecisionRecorded`, `RiskEvaluated`, `VerificationStarted`,
//!   `VerificationCompleted`, `UndoPlanned`, `UndoApplied`, `WorkspaceMigrated`.
//! - The engines self-append: `CheckpointCreated`/`CheckpointRestored`
//!   (checkpoint engine) and `FinalizationPlanned`/`FinalizationCompleted`
//!   (finalization engine). This module must NOT re-append those.
//! - `draftd` appends `ServiceStarted`/`ServiceStopped`.

mod reports;

use std::path::Path;

pub use reports::*;

use crate::changes::{self, DraftChange};
use crate::checkpoint;
use crate::common::ReceiptId;
use crate::conflict;
use crate::error::{DraftError, DraftErrorKind, DraftResult};
use crate::finalization::{self, FinalizationContext, FinalizationRequest};
use crate::identity::{resolve_actor, ActorRef};
use crate::operations::{NewOperation, ObjectKind, ObjectRef, OperationKind, OperationLog};
use crate::receipts::{self, DraftReceipt};
use crate::review::{self, ReviewSession, ReviewState};
use crate::risk::{self, RiskSummary};
use crate::vcs::registry::ProviderRegistry;
use crate::vcs::traits::VcsRepository;
use crate::vcs::types::{DiffInput, ProviderDelta, ProviderId, ProviderUndoInput};
use crate::verification::{self, VerificationPlan};
use crate::workspace::{self, Workspace};

/// The embedded application API over a set of registered providers.
pub struct App {
    pub registry: ProviderRegistry,
}

/// An opened workspace bundle: workspace + provider repository + op log + actor.
pub struct Opened {
    pub ws: Workspace,
    pub repo: Box<dyn VcsRepository>,
    pub log: OperationLog,
    pub actor: ActorRef,
}

impl App {
    pub fn new(registry: ProviderRegistry) -> Self {
        App { registry }
    }

    fn open(&self, path: &Path) -> DraftResult<Opened> {
        // Transparently migrate a v0.1.0 workspace before opening (FR-WS-004).
        if let Some(root) = workspace::find_workspace_root(path) {
            crate::migration::migrate_if_needed(&root)?;
        }
        let ws = Workspace::open(path)?;
        let provider = self.registry.get(&ws.provider_id).ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::ProviderNotDetected,
                format!("provider '{}' is not registered", ws.provider_id),
            )
        })?;
        let repo = provider.open(&ws)?;
        let actor = resolve_actor(&ws.draft_dir);
        let log = OperationLog::new(ws.layout(), ws.id.clone());
        Ok(Opened {
            ws,
            repo,
            log,
            actor,
        })
    }

    // --- workspace lifecycle ------------------------------------------------

    /// Detect the provider for `path` (does not require a workspace).
    pub fn detect(&self, path: &Path) -> DraftResult<DetectReport> {
        let selection = workspace::detect_provider(&self.registry, path)?;
        let caps = selection.provider.capabilities();
        Ok(DetectReport {
            provider_id: selection.detection.provider_id.to_string(),
            provider_name: selection.provider.name().to_string(),
            experimental: selection.provider.is_experimental(),
            root: selection.detection.root.display().to_string(),
            confidence: format!("{:?}", selection.detection.confidence),
            reason: selection.detection.reason,
            capabilities: caps.enabled_names().iter().map(|s| s.to_string()).collect(),
        })
    }

    /// Initialize a workspace at `path`, binding it to the detected (or
    /// overridden) provider, and exclude `.draft/` from provider history.
    pub fn init(
        &self,
        path: &Path,
        provider_override: Option<ProviderId>,
        ack_experimental: bool,
    ) -> DraftResult<InitReport> {
        if let Some(root) = workspace::find_workspace_root(path) {
            // Idempotent: opening an existing workspace (migrating a v0.1.0 one
            // first if needed, so `draft start` works for v0.1 users too).
            crate::migration::migrate_if_needed(&root)?;
            let ws = Workspace::open_at(&root)?;
            return Ok(InitReport {
                workspace_id: ws.id.to_string(),
                provider_id: ws.provider_id.to_string(),
                root: ws.root.display().to_string(),
                created: false,
                draft_excluded: true,
            });
        }

        let (provider, root) = match provider_override {
            Some(id) => {
                let p = self.registry.get(&id).ok_or_else(|| {
                    DraftError::new(
                        DraftErrorKind::ProviderNotDetected,
                        format!("provider '{id}' is not registered"),
                    )
                })?;
                let det = p.detect(path)?;
                (p, det.root)
            }
            None => {
                let sel = workspace::detect_provider(&self.registry, path)?;
                (sel.provider, sel.detection.root)
            }
        };

        if provider.is_experimental() && !ack_experimental {
            return Err(DraftError::new(
                DraftErrorKind::InvalidConfig,
                format!(
                    "provider '{}' is experimental; pass --experimental to use it",
                    provider.id()
                ),
            ));
        }

        let ws = workspace::initialize(&root, &root, provider.id(), ack_experimental)?;
        let repo = provider.open(&ws)?;
        // Exclude .draft/ from provider history (safety gate, not cosmetics).
        let ignore = repo.ignore_rules()?;
        let log = OperationLog::new(ws.layout(), ws.id.clone());
        let actor = resolve_actor(&ws.draft_dir);
        log.append(
            NewOperation::new(
                OperationKind::WorkspaceInitialized,
                actor.clone(),
                provider.id(),
            )
            .output(ObjectRef::new(ObjectKind::Workspace, ws.id.to_string()))
            .message(format!(
                "initialized workspace with provider {}",
                provider.id()
            )),
        )?;
        log.append(
            NewOperation::new(OperationKind::ProviderSelected, actor, provider.id())
                .message(format!("bound provider {}", provider.id())),
        )?;

        Ok(InitReport {
            workspace_id: ws.id.to_string(),
            provider_id: ws.provider_id.to_string(),
            root: ws.root.display().to_string(),
            created: true,
            draft_excluded: ignore.draft_dir_excluded,
        })
    }

    // --- read-only status ---------------------------------------------------

    pub fn status(&self, path: &Path) -> DraftResult<StatusReport> {
        let opened = self.open(path)?;
        let view = opened.repo.current_view()?;
        // Status is the cheap read path: persist groups but do not append
        // operations (NFR-008).
        let (delta, draft_changes, risk) = self.scan(&opened, false)?;
        let conflicts = conflict::detect(opened.repo.as_ref(), &opened.ws.layout())?;
        let verification = verification::latest(&opened.ws.layout())?.map(|r| r.summary());
        let last_receipt = receipts::list(&opened.ws.layout())?.into_iter().next();

        Ok(StatusReport {
            workspace_id: opened.ws.id.to_string(),
            provider_id: opened.ws.provider_id.to_string(),
            provider_view: view.description,
            changed_files: delta.stats.files_changed,
            additions: delta.stats.additions,
            deletions: delta.stats.deletions,
            change_groups: draft_changes
                .iter()
                .map(|c| ChangeGroupSummary {
                    id: c.id.to_string(),
                    title: c.title.clone().unwrap_or_default(),
                    files: c.file_changes.len(),
                    review_state: c.review_state.label().to_string(),
                })
                .collect(),
            risk_level: risk.level.label().to_string(),
            risk_findings: risk.findings.len(),
            verification_status: verification.map(|v| v.status.label().to_string()),
            conflicts: conflicts.conflicts.len(),
            last_receipt: last_receipt.map(|r| r.id.to_string()),
        })
    }

    /// Scan the working tree into Draft changes; persists them and (optionally)
    /// appends `ChangeScanned`/`ChangeGrouped`/`RiskEvaluated`.
    fn scan(
        &self,
        opened: &Opened,
        log_ops: bool,
    ) -> DraftResult<(ProviderDelta, Vec<DraftChange>, RiskSummary)> {
        let delta = opened.repo.diff(DiffInput::WorkingTree)?;
        let risk = risk::evaluate(&delta, &opened.ws.config.risk);
        let mut groups = changes::group_delta(&opened.ws.id, &delta);
        // Attach risk summary to each group (whole-delta risk for v0.2.0).
        for g in &mut groups {
            g.risk_summary = Some(risk.clone());
            changes::save_change(&opened.ws.layout(), g)?;
        }
        changes::save_group_index(
            &opened.ws.layout(),
            &changes::store::GroupIndex {
                source: crate::changes::GroupingSource::Automatic,
                change_ids: groups.iter().map(|g| g.id.clone()).collect(),
            },
        )?;
        if log_ops {
            opened.log.append(
                NewOperation::new(
                    OperationKind::ChangeScanned,
                    opened.actor.clone(),
                    opened.ws.provider_id.clone(),
                )
                .message(format!("{} files changed", delta.stats.files_changed)),
            )?;
            opened.log.append(
                NewOperation::new(
                    OperationKind::ChangeGrouped,
                    opened.actor.clone(),
                    opened.ws.provider_id.clone(),
                )
                .message(format!("{} change group(s)", groups.len())),
            )?;
            opened.log.append(
                NewOperation::new(
                    OperationKind::RiskEvaluated,
                    opened.actor.clone(),
                    opened.ws.provider_id.clone(),
                )
                .risk(risk.clone()),
            )?;
        }
        Ok((delta, groups, risk))
    }

    // --- review -------------------------------------------------------------

    /// Open (or refresh) a review session over the current change groups.
    pub fn review(&self, path: &Path, approve_all: bool) -> DraftResult<ReviewReport> {
        let opened = self.open(path)?;
        let (_, mut groups, _) = self.scan(&opened, true)?;
        let change_ids: Vec<_> = groups.iter().map(|g| g.id.clone()).collect();

        let mut session = ReviewSession::start(
            opened.ws.id.clone(),
            change_ids.clone(),
            opened.actor.clone(),
        );
        opened.log.append(
            NewOperation::new(
                OperationKind::ReviewStarted,
                opened.actor.clone(),
                opened.ws.provider_id.clone(),
            )
            .message(format!("review of {} change(s)", change_ids.len())),
        )?;

        if approve_all {
            for g in &mut groups {
                session.record(
                    g.id.clone(),
                    review::ReviewDecisionKind::Approved,
                    None,
                    opened.actor.clone(),
                );
                g.review_state = ReviewState::Approved;
                changes::save_change(&opened.ws.layout(), g)?;
                opened.log.append(
                    NewOperation::new(
                        OperationKind::ReviewDecisionRecorded,
                        opened.actor.clone(),
                        opened.ws.provider_id.clone(),
                    )
                    .input(ObjectRef::new(ObjectKind::Change, g.id.to_string()))
                    .message("approved"),
                )?;
            }
            session.completed_at = Some(crate::common::now());
        }
        session.save(&opened.ws.layout())?;

        Ok(ReviewReport {
            session_id: session.id.to_string(),
            change_groups: groups
                .iter()
                .map(|c| ChangeGroupSummary {
                    id: c.id.to_string(),
                    title: c.title.clone().unwrap_or_default(),
                    files: c.file_changes.len(),
                    review_state: c.review_state.label().to_string(),
                })
                .collect(),
            decisions: session.decisions.len(),
        })
    }

    // --- verification -------------------------------------------------------

    pub fn verify(&self, path: &Path, command: Option<String>) -> DraftResult<VerifyReport> {
        let opened = self.open(path)?;
        let plan = match command {
            Some(cmd) => VerificationPlan {
                id: crate::common::VerificationPlanId::generate(),
                commands: vec![parse_command(&cmd)],
            },
            None => VerificationPlan::from_config(
                &opened.ws.config.verification,
                &opened.ws.provider_root,
            ),
        };
        if plan.is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::VerificationFailed,
                "no verification command configured and none could be inferred",
            )
            .with_suggestion("Add a [[verification.commands]] entry or pass a command."));
        }
        opened.log.append(
            NewOperation::new(
                OperationKind::VerificationStarted,
                opened.actor.clone(),
                opened.ws.provider_id.clone(),
            )
            .message(
                plan.commands
                    .iter()
                    .map(|c| c.display())
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
        )?;
        let result = verification::run(&opened.ws.layout(), &opened.ws.provider_root, &plan)?;
        opened.log.append(
            NewOperation::new(
                OperationKind::VerificationCompleted,
                opened.actor.clone(),
                opened.ws.provider_id.clone(),
            )
            .verification(result.summary()),
        )?;
        Ok(VerifyReport {
            result_id: result.id.to_string(),
            status: result.status.label().to_string(),
            commands: result
                .command_results
                .iter()
                .map(|c| CommandSummary {
                    command: c.command.clone(),
                    status: c.status.label().to_string(),
                    exit_code: c.exit_code,
                })
                .collect(),
        })
    }

    // --- finalization (draft commit) ---------------------------------------

    pub fn finalize(&self, path: &Path, opts: FinalizeOptions) -> DraftResult<FinalizeReport> {
        let opened = self.open(path)?;
        let (_, groups, riskv) = self.scan(&opened, true)?;
        if groups.is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::FinalizationFailed,
                "there are no changes to finalize",
            ));
        }

        // Apply latest review decisions to the freshly-scanned groups so the
        // policy gate sees up-to-date review state.
        let latest_review = review::latest(&opened.ws.layout())?;
        let mut included = groups;
        if let Some(session) = &latest_review {
            for g in &mut included {
                if let Some(d) = session.latest_for(&g.id) {
                    g.review_state = ReviewState::from(d.kind);
                }
            }
        }

        let verification = if opts.no_verify {
            None
        } else {
            verification::latest(&opened.ws.layout())?.map(|r| r.summary())
        };
        let conflicts = conflict::detect(opened.repo.as_ref(), &opened.ws.layout())?;

        // Pre-finalization checkpoint for undo safety (best-effort).
        let checkpoint = if opened
            .registry_supports_checkpoints(&self.registry)
            .unwrap_or(false)
        {
            checkpoint::create(
                &opened.ws,
                opened.repo.as_ref(),
                &opened.log,
                opened.actor.clone(),
                Some(format!("pre-finalization: {}", opts.message)),
            )
            .ok()
            .map(|c| c.id)
        } else {
            None
        };

        let ctx = FinalizationContext {
            ws: &opened.ws,
            repo: opened.repo.as_ref(),
            log: &opened.log,
            actor: opened.actor.clone(),
            changes: included.clone(),
            risk: riskv,
            verification: verification.clone(),
            conflicts,
            checkpoint,
        };
        let request = FinalizationRequest {
            message: opts.message.clone(),
            trailers: opts.trailers.clone(),
            confirm_high_risk: opts.confirm_high_risk,
        };
        let (plan, result) = finalization::finalize(&ctx, &request)?;

        Ok(FinalizeReport {
            change_count: included.len(),
            provider_object: result.object.object_id.to_string(),
            provider_object_label: result.object.label.clone(),
            provider_object_kind: result.object.kind.clone(),
            receipt_id: result.receipt_id.to_string(),
            warnings: plan.policy_result.warnings,
        })
    }

    // --- undo ---------------------------------------------------------------

    pub fn undo(&self, path: &Path) -> DraftResult<UndoReport> {
        let opened = self.open(path)?;
        let receipt = receipts::list(&opened.ws.layout())?
            .into_iter()
            .find(|r| !r.provider_objects.is_empty())
            .ok_or_else(|| DraftError::new(DraftErrorKind::NotFound, "no finalization to undo"))?;
        let object = receipt.provider_objects[0].clone();
        opened.log.append(
            NewOperation::new(
                OperationKind::UndoPlanned,
                opened.actor.clone(),
                opened.ws.provider_id.clone(),
            )
            .message(format!("undo {}", object.object_id)),
        )?;
        let result = opened.repo.undo_provider_action(ProviderUndoInput {
            object: Some(object.clone()),
            checkpoint: None,
        })?;
        opened.log.append(
            NewOperation::new(
                OperationKind::UndoApplied,
                opened.actor.clone(),
                opened.ws.provider_id.clone(),
            )
            .message(result.message.clone()),
        )?;
        Ok(UndoReport {
            undone: result.undone,
            message: result.message,
            provider_history_changed: result.provider_history_changed,
            receipt_id: receipt.id.to_string(),
        })
    }

    // --- checkpoint ---------------------------------------------------------

    pub fn checkpoint(
        &self,
        path: &Path,
        description: Option<String>,
    ) -> DraftResult<CheckpointReport> {
        let opened = self.open(path)?;
        let cp = checkpoint::create(
            &opened.ws,
            opened.repo.as_ref(),
            &opened.log,
            opened.actor.clone(),
            description,
        )?;
        Ok(CheckpointReport {
            checkpoint_id: cp.id.to_string(),
        })
    }

    // --- providers & receipts ----------------------------------------------

    pub fn providers(&self) -> Vec<ProviderListItem> {
        self.registry
            .providers()
            .into_iter()
            .map(|p| ProviderListItem {
                id: p.id.to_string(),
                name: p.name,
                experimental: p.experimental,
                description: p.description,
                capabilities: p
                    .capabilities
                    .enabled_names()
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            })
            .collect()
    }

    pub fn provider_status(&self, path: &Path) -> DraftResult<DetectReport> {
        // For an initialized workspace, report the bound provider's capabilities.
        let ws = Workspace::open(path)?;
        let provider = self.registry.get(&ws.provider_id).ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::ProviderNotDetected,
                format!("provider '{}' is not registered", ws.provider_id),
            )
        })?;
        let repo = provider.open(&ws)?;
        let view = repo.current_view()?;
        Ok(DetectReport {
            provider_id: provider.id().to_string(),
            provider_name: provider.name().to_string(),
            experimental: provider.is_experimental(),
            root: ws.provider_root.display().to_string(),
            confidence: "bound".to_string(),
            reason: view.description,
            capabilities: provider
                .capabilities()
                .enabled_names()
                .iter()
                .map(|s| s.to_string())
                .collect(),
        })
    }

    pub fn receipt_list(&self, path: &Path) -> DraftResult<Vec<DraftReceipt>> {
        let ws = Workspace::open(path)?;
        receipts::list(&ws.layout())
    }

    pub fn receipt_show(&self, path: &Path, id: &str) -> DraftResult<DraftReceipt> {
        let ws = Workspace::open(path)?;
        receipts::load(&ws.layout(), &ReceiptId::new(id))
    }
}

impl Opened {
    fn registry_supports_checkpoints(&self, registry: &ProviderRegistry) -> Option<bool> {
        registry
            .get(&self.ws.provider_id)
            .map(|p| p.capabilities().supports_local_checkpoints)
    }
}

/// Parse a free-form command string into a structured command (split on spaces;
/// good enough for `draft verify "<cmd>"` parity with v0.1.0).
fn parse_command(cmd: &str) -> crate::verification::VerificationCommand {
    let mut parts = cmd.split_whitespace();
    let program = parts.next().unwrap_or("").to_string();
    let args: Vec<String> = parts.map(|s| s.to_string()).collect();
    crate::verification::VerificationCommand {
        name: "custom".to_string(),
        command: program,
        args,
        timeout_ms: None,
    }
}
