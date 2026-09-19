mod installation;
mod output;
mod service;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use draft_core::app::App;
use draft_core::support::error::{DraftError, DraftErrorKind};

#[derive(Parser)]
#[command(name = "draft", version = draft_core::DRAFT_VERSION, about = "Draft - Human control for agent-scale change")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Update this Draft installation to a newer signed release.
    ///
    /// Only official standalone installations update themselves. Every
    /// artifact is verified against the signed release manifest before it
    /// replaces anything, and a running daemon is stopped and restarted.
    Update {
        /// Report the latest eligible release and change nothing.
        #[arg(long)]
        check: bool,
        /// Install exactly this release, whatever the channel.
        #[arg(long, conflicts_with = "channel")]
        version: Option<String>,
        /// Follow this release track: stable or prerelease.
        #[arg(long)]
        channel: Option<String>,
        /// Permit installing a version older than the installed one.
        #[arg(long, requires = "version", conflicts_with = "check")]
        allow_downgrade: bool,
        /// Trust-bridge hops already taken (set when re-executing).
        #[arg(long, hide = true, default_value_t = 0)]
        trust_hop: u32,
        #[arg(long)]
        json: bool,
    },
    /// Remove this Draft installation.
    ///
    /// Removes the executables, their PATH exposure and the installation's own
    /// metadata. Projects are never scanned or touched, and the global user
    /// store is kept unless `--purge` is given.
    Uninstall {
        /// Print exactly what would be removed and preserved; change nothing.
        #[arg(long)]
        dry_run: bool,
        /// Also delete the global user store, once it is proven Draft's.
        #[arg(long)]
        purge: bool,
        /// Confirm `--purge` without prompting. Waives no safety check.
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    #[command(name = "__lifecycle-helper", hide = true)]
    LifecycleHelper {
        #[arg(long)]
        installation_id: String,
        #[arg(long)]
        operation_id: String,
        #[arg(long, conflicts_with = "bootstrap_recovery")]
        parent_pid: Option<u32>,
        #[arg(long)]
        bootstrap_recovery: bool,
    },
    #[command(name = "release-trust-set", hide = true)]
    ReleaseTrustSet {
        #[arg(long)]
        json: bool,
    },
    #[command(name = "__installer", hide = true)]
    Installer {
        #[command(subcommand)]
        action: InstallerAction,
    },
    /// Manage the long-lived local Draft daemon.
    Daemon {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Manage registered Draft projects.
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },
    /// Open and seal ChangePacks.
    ///
    /// A ChangePack is the unit of proposed work: what it is for, and exactly what
    /// it may touch. Sealing captures the workspace as a revision of it.
    ///
    /// What happens to a sealed revision is the rest of the chain — `evidence`,
    /// `assessment`, `gate`, `decision`, `promote` — each its own command,
    /// because each is its own act.
    Pack {
        #[command(subcommand)]
        action: PackAction,
    },
    /// Inspect resources and how they are observed.
    Resource {
        #[command(subcommand)]
        action: ResourceAction,
    },
    /// Read the Activity Ledger — what actually happened.
    Activity {
        #[command(subcommand)]
        action: ActivityAction,
    },
    /// Restore a past state.
    Recover {
        #[command(subcommand)]
        action: RecoverAction,
    },
    #[command(flatten)]
    Workspace(WorkspaceCommand),
    #[command(flatten)]
    Tasks(TaskCommand),
    /// Show items requiring attention and their next safe action.
    Inbox {
        #[arg(long)]
        json: bool,
    },
    /// Run local maintenance.
    Maintenance {
        #[command(subcommand)]
        action: MaintenanceAction,
    },

    // -----------------------------------------------------------------------
    // The Draft Change Graph.
    //
    // Each of these is one stage of the one chain, and each is a separate
    // command because each is a separate act:
    //
    //   evidence → assessment → gate → decision → promote → publish
    //
    // Deciding authorizes. Promoting changes what the project accepts.
    // Publishing delivers it somewhere and changes nothing. Collapsing any two
    // of these into one command would make it impossible to do one without
    // the other.
    // -----------------------------------------------------------------------
    /// Read the Baselines this project has accepted, and deliver them.
    ///
    /// The Baseline is the project's authoritative state. There is no other.
    Baseline {
        #[command(subcommand)]
        action: Option<BaselineAction>,
    },
    /// Promote an approved revision, advancing the accepted Baseline.
    ///
    /// The only operation that changes what this project accepts.
    Promote {
        /// The ChangePack whose work is being accepted.
        change_pack_id: String,
        /// The exact revision being promoted.
        revision_pack_id: String,
        /// The approving Decision. Defaults to the one on record for this
        /// revision citing a satisfied gate.
        #[arg(long)]
        decision: Option<String>,
        /// The gate that Decision was made over.
        #[arg(long)]
        gate: Option<String>,
        /// The Baseline you believe is accepted.
        ///
        /// Read from the project when omitted, which is right for a command
        /// invoked now. Pass it explicitly when acting on a view you took
        /// earlier: a promotion decided against state that has since moved is
        /// refused rather than silently rebased.
        #[arg(long)]
        expected_baseline: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show what a promotion did, and how far it got.
    Promotion {
        promotion: String,
        #[arg(long)]
        json: bool,
    },
    /// Grant, inspect and withdraw what this project is permitted to do.
    ///
    /// A capability is granted explicitly and recorded as a fact each use
    /// cites. Being permitted to accept work into a Baseline is not being
    /// permitted to announce it to the outside world, and collapsing the two
    /// would make every approver an unwitting publisher.
    Authority {
        #[command(subcommand)]
        action: AuthorityAction,
    },
}

#[derive(Subcommand)]
enum InstallerAction {
    Install {
        #[arg(long)]
        install_root: PathBuf,
        #[arg(long)]
        path_bin: Option<PathBuf>,
        #[arg(long)]
        update_path: bool,
        #[arg(long)]
        migrate_legacy: bool,
        #[arg(long)]
        json: bool,
    },
    Recover {
        #[arg(long)]
        install_root: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum BaselineAction {
    /// Every Baseline this project has accepted, newest first.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Deliver an already-published Baseline again, under a stated intent.
    ///
    /// A different Publication, not a retry of the old one: the intent is part
    /// of the request key, so the second delivery has its own identity, its
    /// own attempts and its own history. Retrying the *same* Publication is
    /// `baseline publish retry`, which needs an authorization rather than an
    /// intent.
    Republish {
        /// The Baseline to deliver. The accepted one when omitted.
        baseline: Option<String>,
        /// Why it is being delivered again.
        #[arg(long = "republish-intent")]
        intent: String,
        #[arg(long, default_value = "draft.publish/export")]
        purpose: String,
        #[arg(long)]
        attempt: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show the accepted Baseline and everything that established it.
    Show {
        #[arg(long)]
        json: bool,
    },
    /// The accepted Baseline's identity, and nothing else.
    Current {
        #[arg(long)]
        json: bool,
    },
    /// The root over the material state this Baseline accepts.
    StateRoot {
        #[arg(long)]
        json: bool,
    },
    /// The root over the exact provenance establishing that state.
    EvidenceRoot {
        #[arg(long)]
        json: bool,
    },
    /// What was observed, per provider and domain — and what was not.
    ///
    /// Distinguishes *not observed because nothing was attempted* from *not
    /// observed because the attempt failed*. An empty Resource set never
    /// proves complete observation.
    Coverage {
        #[arg(long)]
        json: bool,
    },
    /// This Baseline back to the project's first, newest first.
    Lineage {
        #[arg(long)]
        json: bool,
    },
    /// Which provider established each accepted Resource state.
    ///
    /// Immutable accepted provenance, never a route. It does not change when a
    /// binding is later reprofiled or unbound; whether delivery could be
    /// routed right now is reported separately.
    Composition {
        #[arg(long)]
        json: bool,
    },
    /// The receipts issued for this project's promotions and publications.
    ///
    /// Naming one shows it in full. Reading a receipt is not verifying it —
    /// that is a diagnosis, and it lives under `draft doctor receipts`.
    Receipts {
        receipt: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Every Publication and where each one is.
    Publications {
        #[arg(long)]
        json: bool,
    },
    /// Deliver an accepted Baseline outside Draft.
    Publish {
        #[command(subcommand)]
        action: PublishAction,
    },
}

#[derive(Subcommand)]
enum PublishAction {
    /// Deliver a promoted Baseline to a target.
    ///
    /// Optional and repeatable. A failed publication leaves the Baseline
    /// exactly as accepted as it was — delivery has no authority over what the
    /// project agreed.
    Run {
        /// The promoted Baseline to deliver. Defaults to the accepted one.
        #[arg(long)]
        baseline: Option<String>,
        /// A retry authorization permitting another attempt at a delivery
        /// Draft could not establish.
        ///
        /// Needed only after an indeterminate outcome against a target whose
        /// semantics cannot rule out duplication. Obtain one with
        /// `draft baseline publish authorize-retry`; it permits exactly one
        /// attempt.
        #[arg(long)]
        retry_authorization: Option<String>,
        /// What this delivery is for.
        #[arg(long, default_value = "draft.publish/export")]
        purpose: String,
        /// The identity of this attempt.
        ///
        /// Re-running with the same value converges on what that attempt
        /// concluded instead of delivering again. Omit it for a new send.
        #[arg(long)]
        attempt: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List this project's Publications and where each one is.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Attempt the same Publication again, on the same route and key.
    ///
    /// The same Publication, a new attempt. Requires a retry authorization,
    /// because the only reason to reach for this is a delivery whose effect
    /// Draft could not establish — and re-sending one of those may duplicate
    /// a real-world effect. Delivering a Baseline again *on purpose* is
    /// `draft baseline republish`, which is a different Publication.
    Retry {
        /// The one-shot authorization permitting this attempt.
        #[arg(long)]
        retry_authorization: String,
        #[arg(long, default_value = "draft.publish/export")]
        purpose: String,
        #[arg(long)]
        attempt: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Record what was later established about an uncertain delivery.
    ///
    /// The primary outcome is never rewritten. "We did not know, then we
    /// learned" is a different history from "we knew all along", and only one
    /// of them explains why a retry authorization was issued in between — so
    /// this writes a Resolution beside the outcome, under current authority.
    Resolve {
        /// The exact outcome being resolved. The Publication's most recent
        /// conclusion when omitted.
        #[arg(long)]
        outcome: Option<String>,
        /// The delivery did happen, and this is what it produced.
        #[arg(long, conflicts_with = "mark_failed")]
        mark_succeeded: Option<String>,
        /// The delivery did not happen, and this is why.
        #[arg(long)]
        mark_failed: Option<String>,
        #[arg(long)]
        rationale: String,
        #[arg(long, default_value = "draft.publish/export")]
        purpose: String,
        #[arg(long)]
        attempt: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Authorize another attempt at a delivery Draft could not establish.
    ///
    /// A delivery that ended indeterminately against a target whose semantics
    /// cannot rule out duplication is stuck on purpose: re-sending might
    /// duplicate a real-world effect, and nothing Draft can read locally says
    /// whether it would. This is the decision that unsticks it, recorded with
    /// who made it and why, and it permits exactly one further attempt.
    AuthorizeRetry {
        /// Why this is worth the risk, in your words.
        #[arg(long)]
        rationale: String,
        #[arg(long, default_value = "draft.publish/export")]
        purpose: String,
        #[arg(long)]
        json: bool,
    },
    /// Withdraw a publication attempt that stalled before it was sent.
    ///
    /// An attempt interrupted between its allocation and the moment it would
    /// have been delivered blocks its Publication: the barrier cannot prove it
    /// caused no effect. This proves it — the journal never reached the
    /// dispatch boundary — and frees the Publication. An attempt that was
    /// already sent is refused, because withdrawing it would assert something
    /// Draft cannot know.
    Withdraw {
        /// The attempt identity to withdraw, as passed to `publish run
        /// --attempt`.
        #[arg(long)]
        attempt: String,
        #[arg(long)]
        reason: String,
        #[arg(long, default_value = "draft.publish/export")]
        purpose: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum AuthorityAction {
    /// Grant a capability over this project.
    Grant {
        /// The capability being permitted, as a namespaced id.
        ///
        /// Named rather than assumed: publishing and operating are different
        /// permissions, and a grant that did not say which would leave every
        /// reader of the record guessing what was allowed.
        #[arg(long, default_value = "draft.publish/v1")]
        capability: String,
        /// Who is being permitted. This project's own actor when omitted.
        #[arg(long = "to")]
        grantee: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Every grant this project has issued, and whether it is still in force.
    ///
    /// Revoked grants are listed too. A record of authority that drops what
    /// was withdrawn cannot answer the question an audit actually asks.
    List {
        #[arg(long)]
        json: bool,
    },
    /// One grant, and its current standing.
    Show {
        grant: String,
        #[arg(long)]
        json: bool,
    },
    /// Withdraw a grant.
    ///
    /// Records an immutable revocation naming the grant exactly, then moves it
    /// out of the project's active security state. Nothing is deleted, and a
    /// receipt issued while the grant was live stays valid history.
    Revoke {
        grant: String,
        /// Why. "Revoked for cause" and "revoked because the project finished"
        /// lead to different follow-up, and neither is recoverable from the
        /// bare fact that a revocation exists.
        #[arg(long)]
        reason: String,
        #[arg(long)]
        json: bool,
    },
    /// Authority to carry out operations, as opposed to authority to publish.
    ///
    /// The same grant machinery over `draft.change.operate/v1`: an agent
    /// permitted to do work on this project has not thereby been permitted to
    /// announce anything outside it.
    Executor {
        #[command(subcommand)]
        action: ExecutorAuthorityAction,
    },
}

#[derive(Subcommand)]
enum ExecutorAuthorityAction {
    /// Permit an actor to carry out operations on this project.
    Grant {
        /// Who is being permitted. This project's own actor when omitted.
        #[arg(long = "to")]
        grantee: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Every operating grant, and whether it is still in force.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Withdraw an operating grant.
    Revoke {
        grant: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum EvidenceAction {
    /// Run the project's checks against a revision and record the result.
    Run {
        revision_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Every piece of Evidence recorded about a revision.
    List {
        revision_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// One piece of Evidence: what it read, who produced it, what it concluded.
    Show {
        evidence: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum AssessmentAction {
    /// Record a risk judgement over a revision's evidence.
    Record {
        revision_pack_id: String,
        /// low, medium, high or critical.
        ///
        /// There is deliberately no way to record "unassessed": that is what
        /// Draft concludes when nobody has looked, not something to assert.
        #[arg(long)]
        risk: String,
        #[arg(long, default_value = "assessed from the command line")]
        rationale: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum GateAction {
    /// Evaluate the project's gate over a revision.
    Evaluate {
        revision_pack_id: String,
        /// Waivers to offer. A waiver excuses a condition and is always named
        /// in the result — "somebody allowed this" never looks like "this
        /// passed".
        #[arg(long = "waiver")]
        waivers: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Everything decided about a revision, and what may legally follow.
    List {
        change_pack_id: String,
        revision_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Excuse one gate condition on one exact revision.
    ///
    /// Bound to the revision, not the ChangePack: an exception accepted for the
    /// work as it stood is not an exception for whatever it becomes.
    Waive {
        revision_pack_id: String,
        /// The condition being excused, e.g. `draft.gate/verified`.
        condition: String,
        #[arg(long)]
        reason: String,
        /// How long the exception lasts, in days.
        #[arg(long, default_value_t = 7)]
        days: u32,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum DecisionAction {
    /// Approve a revision, authorizing its promotion.
    Approve {
        revision_pack_id: String,
        /// The gate this approval is made over. Defaults to a satisfied gate
        /// on record for this revision.
        #[arg(long)]
        gate: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Reject a revision.
    ///
    /// Needs no gate: refusing work is legitimate whatever the checks say.
    Reject {
        revision_pack_id: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        json: bool,
    },
}

/// Everything done to or about a ChangePack.
#[derive(Subcommand)]
enum PackAction {
    /// Create a ChangePack: what it is for, and exactly what it may touch.
    ///
    /// A declaration without a boundary is not a scope, so both are stated
    /// here and resolved against the accepted Baseline. Re-running with the
    /// same intent converges on the ChangePack it already created.
    New {
        /// What the ChangePack is for, in your words.
        intent: String,
        /// The Resources it may touch, as paths or Resource ids.
        ///
        /// Resolution narrows this to what the accepted Baseline holds and
        /// what the project holds now — so a file that does not exist yet can
        /// still be declared, and adding one is an ordinary change. It may
        /// never widen the declaration.
        #[arg(long = "scope", num_args = 1..)]
        scope: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// List ChangePacks and the revisions sealed against them.
    List {
        #[arg(long)]
        json: bool,
    },
    /// One ChangePack: its lifecycle, what it is for, what it may touch, and every
    /// revision sealed against it.
    Show {
        change_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// What a ChangePack is for, in the author's words.
    Intent {
        #[command(subcommand)]
        action: IntentAction,
    },
    /// Choose the ChangePack subsequent commands default to.
    ///
    /// A convenience, never an authority: every command that acts still names
    /// the exact revision it acts on.
    Select {
        change_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Everything recorded about a ChangePack and its newest revision at once.
    ///
    /// `show` answers what the ChangePack is; this answers what has happened to
    /// it — every judgement, the explanation of its work, and what it
    /// currently interferes with.
    Inspect {
        change_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// The accepted history a ChangePack was worked from.
    ///
    /// Lineage, not proximity. Two ChangePacks touching neighbouring Resources
    /// depend on nothing of each other; `conflicts` is the question that asks
    /// about them.
    Depends {
        change_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Every other ChangePack whose newest revision interferes with this one's.
    Conflicts {
        change_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Check whether several ChangePacks hold together against one Baseline.
    ///
    /// Composes only when every pair is independent and every member was
    /// sealed from the same Baseline. A pair Draft cannot show separable is
    /// reported as interfering rather than assumed composable.
    Compose {
        #[arg(num_args = 2..)]
        change_pack_ids: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Report which members of a composition can be advanced on their own.
    ///
    /// The inverse of `compose`, and not a mutation: a member held by another
    /// is named along with the relation holding it, so the answer says what to
    /// resolve rather than merely refusing.
    Disperse {
        #[arg(num_args = 2..)]
        change_pack_ids: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// What a revision reaches, through contributed elements and relations.
    ///
    /// Nothing is inferred. An element exists because an authorized extractor
    /// said so; directory layout, dependency edges and name similarity produce
    /// no elements at all.
    Impact {
        revision_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// What the evidence about a revision actually speaks for.
    ///
    /// Deliberately hard to satisfy: a Resource is covered only where evidence
    /// read an observation of that exact Resource.
    Coverage {
        revision_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// The derived explanation of what a revision did.
    Representation {
        #[command(subcommand)]
        action: RepresentationAction,
    },
    /// The receipts issued for this ChangePack's promotions.
    Receipts {
        change_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// What a ChangePack may touch, as resolved against the Baseline it was opened
    /// from.
    ///
    /// Resolution may narrow a declaration but never widen it, and it is
    /// resolved once and verified again at seal — so a definition amended
    /// afterwards leaves the resolution stale rather than silently widening
    /// what the work may reach.
    Scope {
        change_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// The revisions sealed against a ChangePack.
    Revision {
        #[command(subcommand)]
        action: RevisionAction,
    },
    /// Create a checkpoint.
    Checkpoint {
        message: String,
        #[arg(long)]
        json: bool,
    },
    /// Manage candidates.
    Candidate {
        #[command(subcommand)]
        action: CandidateAction,
    },
    /// Stop work on a ChangePack, keeping everything recorded about it.
    ///
    /// "We tried this and stopped" is frequently the most useful thing in a
    /// project's history, so nothing is deleted. There is no `delete`.
    Abandon {
        change_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Resume an abandoned ChangePack.
    Reopen {
        change_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Report how two ChangePacks interfere over the resources they both touch.
    ///
    /// Resources only one side touches are left out: silence is the answer for
    /// them, and listing them would bury the ones that actually interfere.
    Compare {
        left: String,
        right: String,
        #[arg(long)]
        json: bool,
    },
    /// Record Evidence about a sealed revision by running the project's checks.
    ///
    /// Evidence is a statement, not a permission. Five outcomes, because "no
    /// checks ran" and "checks ran and passed" are different facts.
    Evidence {
        #[command(subcommand)]
        action: EvidenceAction,
    },
    /// Judge a revision's evidence.
    ///
    /// An assessment is a judgement about risk. It does not authorize anything.
    Assess {
        revision_pack_id: String,
        #[arg(long)]
        risk: String,
        #[arg(long, default_value = "")]
        rationale: String,
        #[arg(long)]
        json: bool,
    },
    /// Record that you examined a revision.
    ///
    /// A review is the act of looking; a Decision is the conclusion. They come
    /// apart in both directions — a reviewer can read a revision and conclude
    /// nothing yet, and a decision with no recorded review behind it is
    /// precisely what an audit wants to notice.
    Review {
        revision_pack_id: String,
        /// What you want recorded. Repeatable.
        #[arg(long = "comment")]
        comments: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Record an immutable Decision about a revision.
    ///
    /// Approving authorizes a promotion. It does not perform one.
    Decide {
        revision_pack_id: String,
        /// Approve, citing the gate the judgement was made over.
        #[arg(long, conflicts_with = "reject")]
        approve: bool,
        /// Reject. `--reason` says why.
        #[arg(long)]
        reject: bool,
        /// The gate an approval cites. Defaults to the one on record.
        #[arg(long)]
        gate: Option<String>,
        /// Required when rejecting: a rejection nobody can read is not a
        /// decision anyone can act on.
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Evaluate this project's gate over a revision, or read what it decided.
    ///
    /// A gate says whether conditions are satisfied. It never authorizes: that
    /// is what a Decision is for.
    Gates {
        #[command(subcommand)]
        action: GateAction,
    },
}

#[derive(Subcommand)]
enum IntentAction {
    /// What a ChangePack is for, as declared.
    Show {
        change_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Declare what a ChangePack is for.
    ///
    /// The same act as amending: a ChangePack's intent lives in its definition, and
    /// changing it mints a new definition rather than editing the old one. Kept
    /// as two spellings because "state it for the first time" and "change what
    /// it says" read differently to the person doing it.
    Set {
        change_pack_id: String,
        intent: String,
        #[arg(long)]
        json: bool,
    },
    /// Amend what a ChangePack is for.
    Amend {
        change_pack_id: String,
        intent: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RepresentationAction {
    /// Every explanation this project has recorded.
    List {
        #[arg(long)]
        json: bool,
    },
    /// The explanation of one sealed revision.
    Show {
        revision_pack_id: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RevisionAction {
    /// Seal the workspace's current state as a revision of a ChangePack.
    ///
    /// The state is observed, not asserted. Sealing the same state twice is
    /// the same revision, so a re-run is not a second thing to review.
    Seal {
        change_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Every revision sealed against a ChangePack, newest first.
    List {
        change_pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// One sealed revision: the exact definition and scope it was sealed
    /// against, the Baseline it was worked from, and what it touched.
    Show {
        change_pack_id: String,
        revision_pack_id: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ResourceAction {
    /// Inspect the observation semantics in force, and what they saw.
    Observation {
        #[command(subcommand)]
        action: ObservationAction,
    },
    /// One Resource's accepted state, and what established it.
    ///
    /// Shows what the workspace holds now alongside it: a reader given only
    /// one of the two cannot tell whether the Resource has moved.
    State {
        resource: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ActivityAction {
    /// List Activity events, most recent first.
    List {
        #[arg(long)]
        page: Option<usize>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        raw: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show one Activity event.
    Show {
        event_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Verify the Activity chain.
    Verify {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RecoverAction {
    /// Resolve the target and report the plan without mutating anything.
    Plan {
        reference: String,
        #[arg(long)]
        json: bool,
    },
    /// Restore a past state.
    Run {
        reference: String,
        #[arg(long)]
        json: bool,
    },
    /// Report every safety check the restore would run, without running it.
    ///
    /// Distinct from `plan`: `plan` says what would change, this says whether
    /// Draft could prove the result.
    DryRun {
        reference: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum MaintenanceAction {
    /// Collect unreachable storage. Never deletes canonical history.
    Gc,
    /// Compact object storage.
    Compact {
        #[arg(long)]
        json: bool,
    },
    /// Report storage statistics.
    Stats {
        #[arg(long)]
        json: bool,
    },
    /// Rebuild derived indexes from their authoritative source.
    IndexRebuild {
        #[arg(long)]
        json: bool,
    },
    /// Remove Draft metadata from this project.
    RemoveProject {
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum WorkspaceCommand {
    /// Initialize a Draft workspace (or the global store with --global).
    Init {
        #[arg(short = 'b')]
        base: Option<String>,
        /// Initialize the global `~/.draft/` store instead of a project.
        #[arg(long)]
        global: bool,
        #[arg(long)]
        json: bool,
    },
    /// Validate global and project Draft state.
    Doctor {
        #[command(subcommand)]
        action: Option<DoctorAction>,
        /// Validate only the global store.
        #[arg(long)]
        global: bool,
        #[arg(long)]
        json: bool,
    },
    /// Launch the local Draft Console.
    Console {
        #[command(subcommand)]
        mode: Option<ConsoleMode>,
    },
    /// Manage Draft extensions.
    Extension {
        #[command(subcommand)]
        action: ExtensionAction,
    },
    /// Manage workspace config.
    Config {
        #[arg(short = 'k')]
        key: Option<String>,
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// Show Draft-native workspace status.
    Status {
        #[arg(short = 'p')]
        change: Option<String>,
        #[arg(short = 'c')]
        component: Option<String>,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ConsoleMode {
    /// Launch Draft Console in a browser.
    Web(ConsoleWebArgs),
    /// Launch Draft Console in the current terminal.
    Tui(ConsoleTuiArgs),
}

#[derive(Args)]
struct ConsoleWebArgs {
    /// Preselect a registered project by exact workspace id or canonical path.
    #[arg(long, conflicts_with = "no_preselect")]
    project: Option<String>,
    /// Always start in GLOBAL context.
    #[arg(long)]
    no_preselect: bool,
    /// Port to bind (loopback only).
    #[arg(long, default_value_t = 4317)]
    port: u16,
    /// Print the bootstrap URL without opening a browser.
    #[arg(long)]
    no_open: bool,
}

#[derive(Args)]
struct ConsoleTuiArgs {
    /// Preselect a registered project by exact workspace id or canonical path.
    #[arg(long, conflicts_with = "no_preselect")]
    project: Option<String>,
    /// Always start in GLOBAL context.
    #[arg(long)]
    no_preselect: bool,
}

#[derive(Subcommand)]
enum TaskCommand {
    /// Manage tasks.
    Task {
        #[command(subcommand)]
        action: Option<TaskAction>,
    },
}

#[derive(Subcommand)]
enum ObservationAction {
    /// The adapter and view-rule bindings currently in force.
    Show {
        #[arg(long)]
        json: bool,
    },
    /// Which domains the current observation covers, and what it could not see.
    Coverage {
        #[arg(long)]
        json: bool,
    },
    /// Which implementation actually performed an observation.
    Provenance {
        /// The observed state to look up. Defaults to the current one.
        #[arg(long)]
        snapshot_digest: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Semantics an installed extension would observe under, if adopted.
    ///
    /// Reports nothing when the installed extensions observe exactly as the
    /// adopted semantics do — the ordinary case, including a package update
    /// that changed nothing about what is observable.
    Pending {
        #[arg(long)]
        json: bool,
    },
    /// What adopting the pending semantics would do, without doing it.
    ///
    /// Runs a trial observation that is thrown away: no snapshot, no
    /// provenance record, no change to what is in force.
    Preview {
        #[arg(long)]
        json: bool,
    },
    /// Adopt the pending semantics: a new baseline, atomically and on the
    /// record.
    ///
    /// Work derived under the old semantics stays readable but must be
    /// re-derived before it can change anything.
    Adopt {
        #[arg(long)]
        json: bool,
    },
    /// Every adoption this project has made.
    Transitions {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum MaintenanceCommand {
    /// Remove Draft metadata from this workspace.
    Close {
        #[arg(long)]
        force: bool,
    },
    /// Run safe Draft metadata maintenance.
    Gc,
    /// Manage storage.
    Storage {
        #[command(subcommand)]
        action: StorageAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Read a config value using CLI > project > global > default precedence.
    Get {
        key: String,
        /// Read only from the global `~/.draft/config.toml` layer.
        #[arg(long)]
        global: bool,
        #[arg(long)]
        json: bool,
    },
    Set {
        key: String,
        value: String,
        /// Write to the global `~/.draft/config.toml` instead of the project.
        #[arg(long)]
        global: bool,
        #[arg(long)]
        json: bool,
    },
    Unset {
        key: String,
        /// Clear the value in the global `~/.draft/config.toml` layer.
        #[arg(long)]
        global: bool,
        #[arg(long)]
        json: bool,
    },
    /// Manage hooks.
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
    /// Manage `.draft/.ignore`.
    Ignore {
        #[command(subcommand)]
        action: IgnoreAction,
    },
}

#[derive(Subcommand)]
enum DoctorAction {
    Sync {
        #[arg(long)]
        fix: bool,
        #[arg(long)]
        json: bool,
    },
    /// Report on object storage without changing it.
    Storage {
        #[arg(long)]
        json: bool,
    },
    Index {
        #[arg(long)]
        refresh: bool,
        #[arg(long)]
        global: bool,
        #[arg(long)]
        json: bool,
    },
    /// Read the Activity Ledger back and report what still holds.
    ///
    /// `--replay` is an in-memory check: it says whether the stored chain is
    /// coherent and what it contains. It never rewrites anything —
    /// `draft maintenance index-rebuild` is the transactional repair.
    Activity {
        #[arg(long)]
        replay: bool,
        #[arg(long)]
        json: bool,
    },
    /// Verify receipts: structure, canonical form, signature, and trust.
    ///
    /// Historical trust and current trust are reported separately, and what
    /// cannot be determined reads `unknown` rather than `valid`.
    Receipts {
        /// Receipt id (rcp_...). Omit with --all to verify everything.
        receipt_id: Option<String>,
        /// Verify the event chain, transparency chain, and every receipt.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ServiceAction {
    Start,
    Stop,
    Restart,
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ProjectAction {
    List {
        #[arg(long)]
        json: bool,
    },
    Register {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Init {
        path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Relocate {
        workspace_id: String,
        destination: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Unregister {
        workspace_id: String,
        #[arg(long)]
        json: bool,
    },
    AdoptCopy {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Show what this project currently accepts.
    Control {
        #[arg(long)]
        json: bool,
    },
    /// Close this project to new work.
    ///
    /// A lifecycle transition, never a deletion: the history stays readable and
    /// verifiable. Removing Draft's metadata is `draft maintenance
    /// remove-project`, which is a different act.
    Close {
        #[arg(long)]
        json: bool,
    },
    /// Manage what this project is attached to.
    ///
    /// A provider is three facts: an immutable definition of what its
    /// namespace and endpoints mean, an immutable profile of how it is driven,
    /// and a revisioned binding pointing at both. Only the binding moves.
    Provider {
        #[command(subcommand)]
        action: ProviderAction,
    },
}

/// A definition, a profile and the binding that points at them.
///
/// The two immutable facts are supplied as canonical JSON documents, read from
/// files: each is deserialized *exactly* into the frozen structure it names, so
/// the command line adds no vocabulary of its own and an unknown field is
/// refused rather than ignored.
#[derive(Subcommand)]
enum ProviderAction {
    /// Every binding this project has, withdrawn ones included.
    List {
        #[arg(long)]
        json: bool,
    },
    /// One binding, with the immutable facts it points at.
    Show {
        binding: String,
        #[arg(long)]
        json: bool,
    },
    /// Attach this project to a provider.
    Bind {
        /// What to call this attachment. Two attachments of the same kind are
        /// legitimate, and without a name they would share one identity.
        name: String,
        /// The `ResourceStateSemanticsContract` its observations are read
        /// under, as canonical JSON.
        #[arg(long)]
        semantics: PathBuf,
        /// The `ProviderSemanticDefinition`, as canonical JSON.
        #[arg(long)]
        definition: PathBuf,
        /// The `ProviderOperationalProfile`, as canonical JSON.
        #[arg(long)]
        profile: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Point a binding at a different semantic definition.
    ///
    /// History is untouched: every Baseline keeps naming the definition its
    /// observations were actually made under, and a route planned against the
    /// old one becomes stale rather than silently adopting the new one.
    Redefine {
        binding: String,
        #[arg(long)]
        semantics: PathBuf,
        #[arg(long)]
        definition: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Point a binding at a different operational profile.
    Profile {
        binding: String,
        #[arg(long)]
        profile: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Withdraw a binding from new work.
    ///
    /// Deletes nothing. Historical verification, reads, GC reachability and
    /// explicit recovery all keep working; new observations, routing and
    /// delivery are refused until it is rebound.
    Unbind {
        binding: String,
        #[arg(long)]
        json: bool,
    },
    /// Reactivate a withdrawn binding.
    Rebind {
        binding: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ToolAction {
    /// Every tool action installed extensions contribute.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Run one contributed tool action.
    Invoke {
        /// Namespaced action id.
        action_id: String,
        /// Apply the proposed mutations as a Draft-authored operation.
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ExtensionAction {
    /// Run a contributed tool action.
    ///
    /// The tool returns findings and proposed mutations; Draft authors the
    /// operation that applies them, under its own id, attribution,
    /// preconditions and protections. Without `--apply` the proposals are
    /// only reported.
    Tool {
        #[command(subcommand)]
        action: ToolAction,
    },
    Source {
        #[command(subcommand)]
        action: ExtensionSourceAction,
    },
    Search {
        #[arg(default_value = "")]
        query: String,
        /// Restrict the search to one configured source.
        #[arg(long)]
        source: Option<String>,
        /// Restrict the search to packages contributing this capability.
        #[arg(long)]
        capability: Option<String>,
        #[arg(long, default_value_t = 1)]
        page: usize,
        #[arg(long, default_value_t = 25)]
        limit: usize,
        /// Contact configured sources before searching. Without this, search
        /// reads verified cached metadata and works offline.
        #[arg(long)]
        refresh: bool,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    Install {
        target: String,
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        version: Option<String>,
        /// Authorize a permission the package declares, in the same command.
        /// Installation and authorization stay separate, audited decisions.
        #[arg(long = "grant")]
        grants: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    Update {
        id: Option<String>,
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        all: bool,
        /// Re-authorize a permission for the updated artifact. An update always
        /// retires the previous grant, even when it asks for the same thing.
        #[arg(long = "grant")]
        grants: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Authorize capabilities for an installed extension.
    Authorize {
        id: String,
        #[arg(long = "grant", required = true)]
        grants: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Withdraw authorization from an installed extension. The package stays
    /// installed and enabled; only the capability is withdrawn.
    Revoke {
        id: String,
        /// Withdraw one permission instead of all of them.
        #[arg(long)]
        permission: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Uninstall {
        id: String,
        #[arg(long)]
        json: bool,
    },
    Enable {
        id: String,
        #[arg(long)]
        json: bool,
    },
    Disable {
        id: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ExtensionSourceAction {
    Add {
        id: String,
        location: String,
        /// Treat the location as a local directory catalog.
        #[arg(long, conflicts_with = "https")]
        local: bool,
        /// Treat the location as an HTTPS catalog origin.
        #[arg(long, conflicts_with = "local")]
        https: bool,
        /// Trust anchor for the new source, as signed root metadata.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Trust anchor for the new source, as the root's pinned digest.
        #[arg(long = "root-sha256")]
        root_sha256: Option<String>,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one configured source and its current usability.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    Remove {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Stop discovering and updating from a source, keeping what it installed.
    Delete {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Resume using a previously disabled source.
    Enable {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Stop using a source without removing its configuration or installs.
    Disable {
        id: String,
        #[arg(long)]
        json: bool,
    },
    Trust {
        id: String,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        fingerprint: String,
        #[arg(long)]
        reset: bool,
        #[arg(long)]
        json: bool,
    },
    /// Refresh one source, or every enabled source when no id is given.
    Refresh {
        id: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum HookAction {
    /// List configured hooks, or read one with a key.
    List {
        #[arg(short = 'k')]
        key: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Set {
        key: String,
        value: String,
        #[arg(long)]
        json: bool,
    },
    Unset {
        key: String,
        #[arg(long)]
        json: bool,
    },
    Run {
        hook_name: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum IgnoreAction {
    Add {
        pattern: String,
        #[arg(long)]
        json: bool,
    },
    Remove {
        pattern: String,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TaskAction {
    #[command(flatten)]
    Definition(TaskDefinitionAction),
    #[command(flatten)]
    Execution(TaskExecutionAction),
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(Subcommand)]
enum TaskDefinitionAction {
    /// List the task templates installed extensions contribute.
    ///
    /// Draft ships none: a template describes what a kind of work looks like in
    /// a particular domain.
    Templates {
        #[arg(long)]
        json: bool,
    },
    /// Create a stored deterministic task definition.
    Create {
        name: String,
        #[arg(long)]
        goal: String,
        #[arg(long)]
        template: Option<String>,
        #[arg(long = "allow")]
        allowed_zones: Vec<String>,
        #[arg(long = "forbid")]
        forbidden_zones: Vec<String>,
        #[arg(long = "success")]
        success_criteria: Vec<String>,
        #[arg(long = "candidate-preset")]
        candidate_preset: Option<String>,
        #[arg(long, value_parser = ["low", "medium", "high", "critical"])]
        risk: Option<String>,
        #[arg(long, value_parser = ["normal", "safe", "plan-first"])]
        mode: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Update canonical task lifecycle metadata.
    Update {
        task: String,
        #[arg(long, value_parser = ["open", "in-progress", "blocked", "completed", "cancelled"])]
        status: Option<String>,
        #[arg(long, value_parser = ["low", "normal", "high", "urgent"])]
        priority: Option<String>,
        #[arg(long)]
        due: Option<String>,
        #[arg(long)]
        clear_due: bool,
        #[arg(long)]
        assignee: Option<String>,
        #[arg(long, value_parser = ["actor", "candidate"])]
        assignee_kind: Option<String>,
        #[arg(long)]
        clear_assignee: bool,
        #[arg(long)]
        json: bool,
    },
    /// Add or complete a checklist-style next action.
    NextAction {
        task: String,
        #[command(subcommand)]
        action: NextActionCommand,
    },
    /// Create a task through deterministic stdin prompts.
    Wizard {
        #[arg(long)]
        json: bool,
    },
    /// Clear task runtime state, or remove the definition too with --hard.
    Drop {
        task: String,
        #[arg(long)]
        hard: bool,
        #[arg(long)]
        json: bool,
    },
    Export {
        task: String,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Import {
        path: PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    /// Inspect a task by id or name.
    Show {
        task: String,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        executions: bool,
        #[arg(long)]
        changes: bool,
        #[arg(long)]
        conflicts: bool,
        #[arg(long)]
        lanes: bool,
        #[arg(long)]
        evidence: bool,
        #[arg(long)]
        timeline: bool,
        #[arg(long)]
        explain: bool,
        #[arg(long)]
        decompose: bool,
        #[arg(long = "compare-stable")]
        compare_stable: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum NextActionCommand {
    Add {
        label: String,
        #[arg(long)]
        json: bool,
    },
    Complete {
        action_id: String,
        #[arg(long)]
        reopen: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TaskExecutionAction {
    Spawn {
        name: String,
        #[arg(short = 'p')]
        change: Option<String>,
        #[arg(short = 'c')]
        candidates: Vec<String>,
        #[arg(long)]
        preset: Option<String>,
        #[arg(long)]
        resume: Option<String>,
        #[arg(long)]
        cancel: Option<String>,
        #[arg(long)]
        retry: Option<String>,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        cron: Option<String>,
        #[arg(last = true)]
        instruction: Vec<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum CandidateAction {
    List {
        #[arg(long)]
        json: bool,
    },
    Show {
        candidate_name: String,
        #[arg(long)]
        json: bool,
    },
    Add(CandidateMutationArgs),
    Update(CandidateMutationArgs),
    Remove {
        candidate_name: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct CandidateMutationArgs {
    candidate_name: String,
    #[arg(long, value_parser = ["command", "chat", "manual"])]
    kind: Option<String>,
    #[arg(last = true, required = true)]
    template: Vec<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum StorageAction {
    Stats {
        #[arg(long)]
        json: bool,
    },
    Gc {
        #[arg(long)]
        json: bool,
    },
    Compact {
        #[arg(long)]
        json: bool,
    },
    Prune {
        #[arg(long)]
        json: bool,
    },
    Doctor {
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", output::format_error(&e));
            match e.kind {
                DraftErrorKind::VerificationFailed => ExitCode::from(5),
                DraftErrorKind::RiskPolicyBlocked => ExitCode::from(6),
                DraftErrorKind::ReviewRequired => ExitCode::from(7),
                // The work is intact in every one of these: a gate that is not
                // satisfied, coverage nobody established, an authorization
                // about a different revision, a Baseline that moved, a hook
                // that failed. None of them is a corruption, and none is a
                // reason to look at the change itself.
                DraftErrorKind::HookFailed
                | DraftErrorKind::GateUnsatisfied
                | DraftErrorKind::CoverageIncomplete
                | DraftErrorKind::StaleRevision
                | DraftErrorKind::StaleBaseline => ExitCode::from(8),
                DraftErrorKind::Storage => ExitCode::from(9),
                DraftErrorKind::ConflictDetected => ExitCode::from(2),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

fn run(cli: Cli) -> Result<(), DraftError> {
    // Installation lifecycle runs before, and without, any project discovery.
    match cli.command {
        Command::Update {
            check,
            version,
            channel,
            allow_downgrade,
            trust_hop,
            json,
        } => {
            return installation::run_update(
                check,
                version,
                channel,
                allow_downgrade,
                trust_hop,
                json,
            )
        }
        Command::Uninstall {
            dry_run,
            purge,
            yes,
            json,
        } => return installation::run_uninstall(dry_run, purge, yes, json),
        Command::LifecycleHelper {
            installation_id,
            operation_id,
            parent_pid,
            bootstrap_recovery,
        } => {
            return installation::run_helper(
                installation_id,
                operation_id,
                parent_pid,
                bootstrap_recovery,
            )
        }
        Command::ReleaseTrustSet { json } => return installation::run_release_trust_set(json),
        Command::Installer { action } => {
            return match action {
                InstallerAction::Install {
                    install_root,
                    path_bin,
                    update_path,
                    migrate_legacy,
                    json,
                } => installation::run_installer_install(
                    install_root,
                    path_bin,
                    update_path,
                    migrate_legacy,
                    json,
                ),
                InstallerAction::Recover { install_root } => {
                    installation::run_installer_recover(install_root)
                }
            }
        }
        _ => {}
    }
    let cwd = std::env::current_dir().map_err(DraftError::from)?;
    ensure_project_scope(&cli.command, cwd.as_path())?;
    let app = app();
    // The surface is the §8.1 command tree. The handler enums below it are an
    // internal detail carried over from the previous surface, so this
    // translation is a mapping between them rather than a second user-facing
    // vocabulary — there is exactly one way to spell each command.
    match cli.command {
        Command::Update { .. }
        | Command::Uninstall { .. }
        | Command::LifecycleHelper { .. }
        | Command::ReleaseTrustSet { .. }
        | Command::Installer { .. } => unreachable!("dispatched before project discovery"),
        Command::Daemon { action } => service::handle(action, cwd.as_path()),
        Command::Project { action } => run_project(&app, cwd.as_path(), action),
        Command::Workspace(command) => run_workspace(&app, cwd.as_path(), command),
        Command::Tasks(command) => run_tasks(&app, cwd.as_path(), command),
        Command::Pack { action } => run_pack(&app, cwd.as_path(), action),
        Command::Baseline { action } => run_baseline(
            &app,
            cwd.as_path(),
            action.unwrap_or(BaselineAction::Show { json: false }),
        ),
        Command::Promote {
            change_pack_id: change,
            revision_pack_id: revision,
            decision,
            gate,
            expected_baseline,
            json,
        } => run_promote(
            &app,
            cwd.as_path(),
            &change,
            &revision,
            decision,
            gate,
            expected_baseline,
            json,
        ),
        Command::Promotion { promotion, json } => render_json_or_text(
            app.dcg_promotion(cwd.as_path(), &promotion)?
                .ok_or_else(|| {
                    DraftError::new(
                        DraftErrorKind::NotFound,
                        format!("no promotion '{promotion}' has been started"),
                    )
                })?,
            json,
            "Promotion",
        ),
        Command::Authority { action } => run_authority(&app, cwd.as_path(), action),
        Command::Resource { action } => match action {
            ResourceAction::Observation { action } => run_observation(&app, cwd.as_path(), action),
            ResourceAction::State { resource, json } => render_json_or_text(
                app.dcg_resource_state(cwd.as_path(), &resource)?,
                json,
                "Resource state",
            ),
        },
        Command::Inbox { json } => render_json_or_text(app.inbox(cwd.as_path())?, json, "Inbox"),
        Command::Activity { action } => match action {
            ActivityAction::List {
                page,
                limit,
                raw,
                json,
            } => render_events(
                app.events_page(cwd.as_path(), false, false, page, limit, None)?,
                json,
                raw,
            ),
            ActivityAction::Show { event_id, json } => render_json_or_text(
                app.event_show(cwd.as_path(), &event_id)?,
                json,
                "Activity event",
            ),
            ActivityAction::Verify { json } => render_json_or_text(
                app.verify_events(cwd.as_path())?,
                json,
                "Activity chain verified",
            ),
        },
        Command::Recover { action } => match action {
            RecoverAction::Plan { reference, json } => {
                run_rollback(&app, cwd.as_path(), reference, true, json)
            }
            RecoverAction::Run { reference, json } => {
                run_rollback(&app, cwd.as_path(), reference, false, json)
            }
            RecoverAction::DryRun { reference, json } => render_json_or_text(
                app.rollback_dry_run(cwd.as_path(), &reference)?,
                json,
                "Recovery dry run",
            ),
        },
        Command::Maintenance { action } => match action {
            MaintenanceAction::Gc => run_maintenance(&app, cwd.as_path(), MaintenanceCommand::Gc),
            MaintenanceAction::Compact { json } => run_maintenance(
                &app,
                cwd.as_path(),
                MaintenanceCommand::Storage {
                    action: StorageAction::Compact { json },
                },
            ),
            MaintenanceAction::Stats { json } => run_maintenance(
                &app,
                cwd.as_path(),
                MaintenanceCommand::Storage {
                    action: StorageAction::Stats { json },
                },
            ),
            MaintenanceAction::IndexRebuild { json } => {
                render_json_or_text(app.index_rebuild(cwd.as_path())?, json, "Index rebuilt")
            }
            MaintenanceAction::RemoveProject { force } => {
                run_maintenance(&app, cwd.as_path(), MaintenanceCommand::Close { force })
            }
        },
    }
}

fn run_workspace(app: &App, cwd: &Path, command: WorkspaceCommand) -> Result<(), DraftError> {
    match command {
        WorkspaceCommand::Init { base, global, json } => {
            if global {
                render_init_global(app.init_global()?, json)
            } else {
                render_init(
                    app.init_with_base(cwd, base.as_deref().unwrap_or("base"))?,
                    json,
                )
            }
        }
        WorkspaceCommand::Doctor {
            action: Some(DoctorAction::Sync { fix, json }),
            ..
        } => {
            let registry = draft_core::project::registry::ProjectRegistry::global()?;
            let report = if fix {
                serde_json::to_value(registry.fix_stale()?)
            } else {
                serde_json::to_value(registry.inspect()?)
            }
            .map_err(|e| DraftError::storage(e.to_string()))?;
            render_json_or_text(
                report,
                json,
                if fix {
                    "Registry synchronized"
                } else {
                    "Registry status"
                },
            )
        }
        WorkspaceCommand::Doctor {
            action: Some(DoctorAction::Storage { json }),
            ..
        } => render_json_or_text(app.storage_doctor(cwd)?, json, "Storage doctor complete"),
        WorkspaceCommand::Doctor {
            action: Some(DoctorAction::Activity { replay, json }),
            ..
        } => {
            if replay {
                render_json_or_text(app.replay_events(cwd)?, json, "Activity replay")
            } else {
                render_json_or_text(app.verify_events(cwd)?, json, "Activity chain")
            }
        }
        WorkspaceCommand::Doctor {
            action:
                Some(DoctorAction::Index {
                    refresh,
                    global,
                    json,
                }),
            ..
        } => {
            let report = if global {
                app.doctor_index_global(refresh)?
            } else {
                app.doctor_index(cwd, refresh)?
            };
            render_json_or_text(report, json, "Index status")
        }
        WorkspaceCommand::Doctor {
            action:
                Some(DoctorAction::Receipts {
                    receipt_id,
                    all,
                    json,
                }),
            ..
        } => {
            if all {
                render_ledger_verification(app.receipt_verify_all(cwd)?, json)
            } else if let Some(id) = receipt_id {
                render_receipt_verification(app.receipt_verify(cwd, &id)?, json)
            } else {
                Err(DraftError::invalid_config(
                    "provide a receipt id (rcp_...) or --all",
                ))
            }
        }
        WorkspaceCommand::Doctor {
            action: None,
            global,
            json,
        } => {
            let report = if global {
                app.doctor_global()?
            } else {
                app.doctor(cwd)?
            };
            render_doctor(report, json)
        }
        WorkspaceCommand::Console { mode } => {
            let mode = mode.ok_or_else(|| {
                DraftError::invalid_config(
                    "console mode is required; use `draft console web` or `draft console tui`",
                )
            })?;
            match mode {
                ConsoleMode::Web(args) => {
                    let selection =
                        resolve_console_context(cwd, args.project.as_deref(), args.no_preselect)?;
                    service::ensure_daemon()?;
                    if let Some(diagnostic) = selection.diagnostic {
                        output::warn(&diagnostic);
                    }
                    draft_console::serve_console(draft_console::ConsoleLaunchOptions {
                        bind: "127.0.0.1".into(),
                        port: args.port,
                        preselected_workspace_id: selection.workspace_id,
                        open_browser: !args.no_open,
                    })
                    .map_err(|e| DraftError::new(DraftErrorKind::Internal, e))
                }
                ConsoleMode::Tui(args) => {
                    let selection =
                        resolve_console_context(cwd, args.project.as_deref(), args.no_preselect)?;
                    service::ensure_daemon()?;
                    draft_console_tui::run_console(draft_console_tui::LaunchOptions {
                        preselected_workspace_id: selection.workspace_id,
                        startup_diagnostic: selection.diagnostic,
                    })
                    .map_err(|e| DraftError::new(DraftErrorKind::Internal, e))
                }
            }
        }
        WorkspaceCommand::Extension { action } => match action {
            ExtensionAction::Tool { action } => match action {
                ToolAction::List { json } => {
                    render_json_or_text(app.tool_list(cwd)?, json, "Tool actions")
                }
                ToolAction::Invoke {
                    action_id,
                    apply,
                    json,
                } => render_json_or_text(
                    app.tool_invoke(cwd, &action_id, apply)?,
                    json,
                    if apply { "Tool applied" } else { "Tool run" },
                ),
            },
            ExtensionAction::Source { action } => match action {
                ExtensionSourceAction::Add {
                    id,
                    location,
                    local,
                    https,
                    root,
                    root_sha256,
                    json,
                } => {
                    let configured = draft_extension_service::catalog::source_add_as(
                        &id,
                        &location,
                        source_form(local, https),
                    )?;
                    // Configuring a source is not trusting it. An anchor given
                    // here performs the separate trust decision immediately;
                    // without one the source stays untrusted until `source
                    // trust` is run.
                    let status = match (root, root_sha256) {
                        (None, None) => configured,
                        (Some(root), fingerprint) => {
                            draft_extension_service::catalog::trust_source(
                                &id,
                                &root,
                                fingerprint.as_deref().unwrap_or_default(),
                                false,
                            )?
                        }
                        (None, Some(_)) => {
                            return Err(DraftError::invalid_config(
                                "--root-sha256 pins a root file, so --root is required with it",
                            ))
                        }
                    };
                    render_json_or_text(status, json, "Extension source configured")
                }
                ExtensionSourceAction::Show { id, json } => render_json_or_text(
                    draft_extension_service::catalog::source_show(&id)?,
                    json,
                    "Extension source",
                ),
                ExtensionSourceAction::Enable { id, json } => render_json_or_text(
                    draft_extension_service::catalog::source_set_enabled(&id, true)?,
                    json,
                    "Extension source enabled",
                ),
                ExtensionSourceAction::Disable { id, json } => render_json_or_text(
                    draft_extension_service::catalog::source_set_enabled(&id, false)?,
                    json,
                    "Extension source disabled",
                ),
                ExtensionSourceAction::List { json } => render_json_or_text(
                    draft_extension_service::catalog::source_list()?,
                    json,
                    "Extension sources",
                ),
                ExtensionSourceAction::Remove { id, json }
                | ExtensionSourceAction::Delete { id, json } => render_json_or_text(
                    draft_extension_service::catalog::source_remove(&id)?,
                    json,
                    "Extension source removed; installed extensions and their provenance are kept",
                ),
                ExtensionSourceAction::Trust {
                    id,
                    root,
                    fingerprint,
                    reset,
                    json,
                } => render_json_or_text(
                    draft_extension_service::catalog::trust_source(
                        &id,
                        &root,
                        &fingerprint,
                        reset,
                    )?,
                    json,
                    "Extension source trust accepted",
                ),
                ExtensionSourceAction::Refresh { id, json } => match id {
                    Some(id) => render_json_or_text(
                        draft_extension_service::catalog::source_refresh(&id)?,
                        json,
                        "Extension source refreshed",
                    ),
                    None => render_json_or_text(
                        draft_extension_service::catalog::source_refresh_all()?,
                        json,
                        "Extension sources refreshed",
                    ),
                },
            },
            ExtensionAction::Search {
                query,
                source,
                capability,
                page,
                limit,
                refresh,
                json,
            } => {
                if refresh {
                    // The only path that touches the network, and only because
                    // the user asked.
                    draft_extension_service::catalog::source_refresh_all()?;
                }
                render_json_or_text(
                    draft_extension_service::discovery::search(
                        &draft_extension_service::discovery::DiscoveryQuery {
                            text: query,
                            source_id: source,
                            capability,
                            page,
                            limit,
                        },
                    )?,
                    json,
                    "Extension discovery",
                )
            }
            ExtensionAction::List { json } => render_json_or_text(
                draft_extension_service::authorization::views(
                    draft_extension_service::extension::list()?,
                )?,
                json,
                "Extensions",
            ),
            ExtensionAction::Show { id, json } => {
                render_json_or_text(extension_view(&id)?, json, "Extension")
            }
            ExtensionAction::Install {
                target,
                source,
                version,
                grants,
                json,
            } => {
                // An existing directory is a local package; anything else is a
                // canonical extension id to resolve against configured sources.
                let local_package = source.is_none() && Path::new(&target).is_dir();
                let installed = if local_package {
                    if version.is_some() {
                        return Err(DraftError::invalid_config(
                            "--version requires --source for a catalog install",
                        ));
                    }
                    draft_extension_service::extension::install(Path::new(&target))?
                } else {
                    // Resolution is exact: an id published by more than one
                    // source is refused with the candidates rather than picked.
                    let source = draft_extension_service::discovery::resolve_source(
                        &target,
                        source.as_deref(),
                    )?;
                    draft_extension_service::catalog::install_from_source(
                        &source,
                        &target,
                        version.as_deref(),
                    )?
                };
                let installed = apply_grants(installed, &grants)?;
                render_json_or_text(
                    draft_extension_service::authorization::view(installed)?,
                    json,
                    "Extension installed",
                )
            }
            ExtensionAction::Update {
                id,
                source,
                version,
                all,
                grants,
                json,
            } => {
                if all {
                    if id.is_some() || source.is_some() || version.is_some() {
                        return Err(DraftError::invalid_config(
                            "--all cannot be combined with an id, --source, or --version",
                        ));
                    }
                    if !grants.is_empty() {
                        return Err(DraftError::invalid_config(
                            "--grant names one extension, so it cannot be combined with --all",
                        ));
                    }
                    render_json_or_text(
                        draft_extension_service::catalog::update_all()?,
                        json,
                        "Extensions updated",
                    )
                } else {
                    let id = id.ok_or_else(|| {
                        DraftError::invalid_config("extension update requires <id> or --all")
                    })?;
                    // Updates follow the source recorded at install time; an
                    // explicit --source must agree with it.
                    let source =
                        draft_extension_service::discovery::update_source(&id, source.as_deref())?;
                    let updated = draft_extension_service::catalog::update_from_source(
                        &source,
                        &id,
                        version.as_deref(),
                    )?;
                    let updated = apply_grants(updated, &grants)?;
                    render_json_or_text(
                        draft_extension_service::authorization::view(updated)?,
                        json,
                        "Extension updated",
                    )
                }
            }
            ExtensionAction::Authorize { id, grants, json } => {
                let permissions = parse_permissions(&grants)?;
                draft_extension_service::authorization::authorize(
                    &id,
                    &permissions,
                    &draft_core::support::common::OperationId::generate(),
                )?;
                render_json_or_text(extension_view(&id)?, json, "Extension authorized")
            }
            ExtensionAction::Revoke {
                id,
                permission,
                json,
            } => {
                let permission = permission.as_deref().map(parse_permission).transpose()?;
                draft_extension_service::authorization::revoke(
                    &id,
                    permission,
                    &draft_core::support::common::OperationId::generate(),
                )?;
                render_json_or_text(
                    extension_view(&id)?,
                    json,
                    "Extension authorization revoked",
                )
            }
            ExtensionAction::Uninstall { id, json } => render_json_or_text(
                draft_extension_service::extension::uninstall(&id)?,
                json,
                "Extension uninstalled",
            ),
            ExtensionAction::Enable { id, json } => render_json_or_text(
                draft_extension_service::authorization::view(
                    draft_extension_service::extension::set_enabled(&id, true)?,
                )?,
                json,
                "Extension enabled",
            ),
            ExtensionAction::Disable { id, json } => render_json_or_text(
                draft_extension_service::authorization::view(
                    draft_extension_service::extension::set_enabled(&id, false)?,
                )?,
                json,
                "Extension disabled",
            ),
        },
        WorkspaceCommand::Config { key, action } => match action {
            Some(ConfigAction::Get { key, global, json }) => {
                let report = if global {
                    app.config_get_global(&key)?
                } else {
                    app.config_get_layered(cwd, &key)?
                };
                render_config(report, json)
            }
            Some(ConfigAction::Set {
                key,
                value,
                global,
                json,
            }) => {
                let report = if global {
                    app.config_set_global(&key, &value)?
                } else {
                    app.config_set(cwd, &key, &value)?
                };
                render_config(report, json)
            }
            Some(ConfigAction::Unset { key, global, json }) => {
                let report = if global {
                    app.config_unset_global(&key)?
                } else {
                    app.config_unset(cwd, &key)?
                };
                render_config(report, json)
            }
            Some(ConfigAction::Hook { action }) => match action {
                HookAction::List { key, json } => match key {
                    Some(key) => render_config(app.hook_get(cwd, &key)?, json),
                    None => render_config(app.hook_list(cwd)?, json),
                },
                HookAction::Set { key, value, json } => {
                    render_config(app.hook_set(cwd, &key, &value)?, json)
                }
                HookAction::Unset { key, json } => render_config(app.hook_unset(cwd, &key)?, json),
                HookAction::Run { hook_name, json } => {
                    render_json_or_text(app.hook_run(cwd, &hook_name)?, json, "Hook complete")
                }
            },
            Some(ConfigAction::Ignore { action }) => match action {
                IgnoreAction::Add { pattern, json } => {
                    render_ignore(app.ignore_add(cwd, &pattern)?, json)
                }
                IgnoreAction::Remove { pattern, json } => {
                    render_ignore(app.ignore_remove(cwd, &pattern)?, json)
                }
                IgnoreAction::List { json } => render_ignore(app.ignore_list(cwd)?, json),
            },
            None => {
                if let Some(key) = key {
                    render_config(app.config_get(cwd, &key)?, false)
                } else {
                    render_config(app.config_list(cwd)?, false)
                }
            }
        },
        WorkspaceCommand::Status {
            change,
            component,
            full,
            json,
        } => {
            if change.is_some() || component.is_some() || full {
                let component = component
                    .as_deref()
                    .map(draft_core::app::StatusComponent::parse)
                    .transpose()?;
                render_status_report(
                    app.status_with_options(
                        cwd,
                        draft_core::app::StatusOptions {
                            change_pack_id: change,
                            component,
                            full,
                        },
                    )?,
                    json,
                )
            } else {
                render_status(app.status(cwd)?, json)
            }
        }
    }
}

fn run_project(app: &App, cwd: &Path, action: ProjectAction) -> Result<(), DraftError> {
    let registry = draft_core::project::registry::ProjectRegistry::global()?;
    match action {
        ProjectAction::List { json } => render_json_or_text(registry.list()?, json, "Projects"),
        ProjectAction::Register { path, json } => {
            let workspace = app.open(&path)?;
            render_json_or_text(
                registry.upsert(workspace.workspace_id.as_str(), &workspace.root, None, None)?,
                json,
                "Project registered",
            )
        }
        ProjectAction::Init { path, json } => {
            let path = path.unwrap_or_else(|| cwd.to_path_buf());
            if !path.exists() {
                std::fs::create_dir_all(&path).map_err(DraftError::from)?;
            }
            let initialized = app.init(&path)?;
            registry.upsert(&initialized.workspace_id, &path, None, None)?;
            render_json_or_text(initialized, json, "Project initialized")
        }
        ProjectAction::Relocate {
            workspace_id,
            destination,
            json,
        } => render_json_or_text(
            registry.relocate(&workspace_id, &destination)?,
            json,
            "Project relocated",
        ),
        ProjectAction::Unregister { workspace_id, json } => render_json_or_text(
            serde_json::json!({
                "workspace_id": workspace_id,
                "unregistered": registry.remove(&workspace_id)?,
            }),
            json,
            "Project unregistered",
        ),
        ProjectAction::AdoptCopy { path, json } => render_json_or_text(
            draft_core::app::adoption::adopt_copy(&path)?,
            json,
            "Project copy adopted",
        ),
        ProjectAction::Control { json } => {
            render_json_or_text(app.project_control(cwd)?, json, "Project control")
        }
        ProjectAction::Provider { action } => run_provider(app, cwd, action),
        ProjectAction::Close { json } => render_json_or_text(
            app.project_close(cwd)?,
            json,
            "Project closed to new work — its history is kept",
        ),
    }
}

#[derive(Debug)]
struct ConsoleSelection {
    workspace_id: Option<String>,
    diagnostic: Option<String>,
}

fn resolve_console_context(
    cwd: &Path,
    explicit: Option<&str>,
    no_preselect: bool,
) -> Result<ConsoleSelection, DraftError> {
    if explicit.is_some() && no_preselect {
        return Err(DraftError::invalid_config(
            "--project cannot be combined with --no-preselect",
        ));
    }
    if no_preselect {
        return Ok(ConsoleSelection {
            workspace_id: None,
            diagnostic: None,
        });
    }
    let registry = draft_core::project::registry::ProjectRegistry::global()?;
    if let Some(candidate) = explicit {
        let entry = registry.resolve(candidate)?;
        validate_console_selection(&registry, &entry.workspace_id, true)?;
        return Ok(ConsoleSelection {
            workspace_id: Some(entry.workspace_id),
            diagnostic: None,
        });
    }

    let canonical_cwd = match cwd.canonicalize() {
        Ok(cwd) => cwd,
        Err(error) => {
            return Ok(ConsoleSelection {
                workspace_id: None,
                diagnostic: Some(format!(
                    "Could not safely resolve the current directory ({error}); starting in GLOBAL context"
                )),
            })
        }
    };
    let mut candidates = registry
        .list()?
        .into_iter()
        .filter_map(|entry| {
            let registered = Path::new(&entry.project_path).canonicalize().ok()?;
            canonical_cwd
                .starts_with(&registered)
                .then_some((registered, entry))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    let Some((_, entry)) = candidates.into_iter().next() else {
        return Ok(ConsoleSelection {
            workspace_id: None,
            diagnostic: None,
        });
    };
    match validate_console_selection(&registry, &entry.workspace_id, false) {
        Ok(()) => Ok(ConsoleSelection {
            workspace_id: Some(entry.workspace_id),
            diagnostic: None,
        }),
        Err(error) => Ok(ConsoleSelection {
            workspace_id: None,
            diagnostic: Some(format!(
                "Automatic project selection was unsafe ({}); starting in GLOBAL context",
                error.message
            )),
        }),
    }
}

fn validate_console_selection(
    registry: &draft_core::project::registry::ProjectRegistry,
    workspace_id: &str,
    explicit: bool,
) -> Result<(), DraftError> {
    let entry = registry.resolve(workspace_id)?;
    let path = Path::new(&entry.project_path);
    if !path.exists() {
        return Err(DraftError::new(
            DraftErrorKind::NotFound,
            format!(
                "registered project '{}' is inaccessible at '{}'",
                workspace_id, entry.project_path
            ),
        )
        .with_suggestion(
            "repair or relocate the registry entry, or launch Console without --project",
        ));
    }
    path.canonicalize().map_err(|error| {
        DraftError::new(
            DraftErrorKind::Storage,
            format!("cannot safely resolve '{}': {error}", path.display()),
        )
    })?;
    let unsafe_issue = registry.inspect()?.into_iter().find(|issue| {
        issue.workspace_id == workspace_id
            && (matches!(
                issue.kind.as_str(),
                "workspace_identity_corrupt" | "path_reused_by_different_workspace"
            ) || issue.kind.starts_with("identity_conflict_with:")
                || issue.kind.starts_with("duplicate_path_with:"))
    });
    if let Some(issue) = unsafe_issue {
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            format!(
                "registered project '{}' has unsafe identity state: {}",
                workspace_id, issue.kind
            ),
        )
        .with_suggestion(if explicit {
            "resolve the registry identity conflict before selecting this project"
        } else {
            "open Doctor from GLOBAL context to resolve the registry identity conflict"
        }));
    }
    Ok(())
}

fn parse_task_status(value: &str) -> Result<draft_core::task::TaskLifecycleStatus, DraftError> {
    use draft_core::task::TaskLifecycleStatus;
    match value {
        "open" => Ok(TaskLifecycleStatus::Open),
        "in-progress" => Ok(TaskLifecycleStatus::InProgress),
        "blocked" => Ok(TaskLifecycleStatus::Blocked),
        "completed" => Ok(TaskLifecycleStatus::Completed),
        "cancelled" => Ok(TaskLifecycleStatus::Cancelled),
        _ => Err(DraftError::invalid_config("invalid task status")),
    }
}

fn parse_task_priority(value: &str) -> Result<draft_core::task::TaskPriority, DraftError> {
    use draft_core::task::TaskPriority;
    match value {
        "low" => Ok(TaskPriority::Low),
        "normal" => Ok(TaskPriority::Normal),
        "high" => Ok(TaskPriority::High),
        "urgent" => Ok(TaskPriority::Urgent),
        _ => Err(DraftError::invalid_config("invalid task priority")),
    }
}

fn run_tasks(app: &App, cwd: &Path, command: TaskCommand) -> Result<(), DraftError> {
    match command {
        TaskCommand::Task { action } => match action {
            Some(TaskAction::Definition(TaskDefinitionAction::Templates { json })) => {
                render_json_or_text(app.task_templates(cwd)?, json, "Task templates")
            }
            Some(TaskAction::Definition(TaskDefinitionAction::Create {
                name,
                goal,
                template,
                allowed_zones,
                forbidden_zones,
                success_criteria,
                candidate_preset,
                risk,
                mode,
                json,
            })) => render_json_or_text(
                app.task_create(
                    cwd,
                    &name,
                    &goal,
                    template,
                    allowed_zones,
                    forbidden_zones,
                    success_criteria,
                    risk.as_deref(),
                    mode.as_deref(),
                    candidate_preset,
                )?,
                json,
                "Task created",
            ),
            Some(TaskAction::Definition(TaskDefinitionAction::Update {
                task,
                status,
                priority,
                due,
                clear_due,
                assignee,
                assignee_kind,
                clear_assignee,
                json,
            })) => {
                if clear_due && due.is_some() {
                    return Err(DraftError::invalid_config(
                        "--due and --clear-due cannot be used together",
                    ));
                }
                if clear_assignee && assignee.is_some() {
                    return Err(DraftError::invalid_config(
                        "--assignee and --clear-assignee cannot be used together",
                    ));
                }
                let due_at = if clear_due {
                    Some(None)
                } else if let Some(due) = due {
                    Some(Some(
                        chrono::DateTime::parse_from_rfc3339(&due)
                            .map_err(|_| DraftError::invalid_config("--due must be RFC 3339"))?
                            .with_timezone(&chrono::Utc),
                    ))
                } else {
                    None
                };
                let assignee_ref = if clear_assignee {
                    Some(None)
                } else {
                    assignee.map(|id| {
                        Some(draft_core::task::AssigneeRef {
                            kind: assignee_kind.unwrap_or_else(|| "actor".into()),
                            id,
                        })
                    })
                };
                render_json_or_text(
                    app.task_update(
                        cwd,
                        &task,
                        status.as_deref().map(parse_task_status).transpose()?,
                        priority.as_deref().map(parse_task_priority).transpose()?,
                        due_at,
                        assignee_ref,
                    )?,
                    json,
                    "Task updated",
                )
            }
            Some(TaskAction::Definition(TaskDefinitionAction::NextAction { task, action })) => {
                match action {
                    NextActionCommand::Add { label, json } => render_json_or_text(
                        app.task_add_next_action(cwd, &task, &label)?,
                        json,
                        "Next action added",
                    ),
                    NextActionCommand::Complete {
                        action_id,
                        reopen,
                        json,
                    } => render_json_or_text(
                        app.task_set_next_action(cwd, &task, &action_id, !reopen)?,
                        json,
                        "Next action updated",
                    ),
                }
            }
            Some(TaskAction::Execution(TaskExecutionAction::Spawn {
                name,
                change,
                candidates,
                preset,
                resume,
                cancel,
                retry,
                reason,
                cron,
                instruction,
                json,
            })) => {
                if let Some(execution_id) = cancel {
                    return render_json_or_text(
                        app.task_cancel_execution(cwd, &execution_id, reason)?,
                        json,
                        "Execution cancelled",
                    );
                }
                if let Some(execution_id) = resume {
                    return render_json_or_text(
                        app.task_resume_execution(cwd, &execution_id)?,
                        json,
                        "Execution resume queued",
                    );
                }
                if let Some(execution_id) = retry {
                    return render_json_or_text(
                        app.task_retry_execution(cwd, &execution_id)?,
                        json,
                        "Execution retry queued",
                    );
                }
                render_json_or_text(
                    app.task_spawn_with_preset(
                        cwd,
                        &name,
                        change.as_deref(),
                        candidates,
                        preset,
                        cron,
                        instruction,
                    )?,
                    json,
                    "Task spawned",
                )
            }
            Some(TaskAction::Definition(TaskDefinitionAction::Wizard { json })) => {
                let task = run_task_wizard(app, cwd, json)?;
                render_json_or_text(task, json, "Task created")
            }
            Some(TaskAction::Definition(TaskDefinitionAction::Drop { task, hard, json })) => {
                render_json_or_text(app.task_drop(cwd, &task, hard)?, json, "Task dropped")
            }
            Some(TaskAction::Definition(TaskDefinitionAction::Export { task, output, json })) => {
                render_json_or_text(
                    app.task_export(cwd, &task, output.as_deref())?,
                    json,
                    "Task exported",
                )
            }
            Some(TaskAction::Definition(TaskDefinitionAction::Import { path, name, json })) => {
                render_json_or_text(app.task_import(cwd, &path, name)?, json, "Task imported")
            }
            Some(TaskAction::Definition(TaskDefinitionAction::List { json })) => {
                render_json_or_text(app.task_list(cwd)?, json, "Tasks")
            }
            Some(TaskAction::Definition(TaskDefinitionAction::Show {
                task,
                full,
                executions,
                changes,
                conflicts,
                lanes,
                evidence,
                timeline,
                explain,
                decompose,
                compare_stable,
                json,
            })) => {
                let options = draft_core::app::TaskViewOptions {
                    full,
                    executions,
                    changes,
                    conflicts,
                    lanes,
                    evidence,
                    timeline,
                    explain,
                    decompose,
                    compare_stable,
                };
                if full
                    || executions
                    || changes
                    || conflicts
                    || lanes
                    || evidence
                    || timeline
                    || explain
                    || decompose
                    || compare_stable
                {
                    render_json_or_text(
                        app.task_view_with_options(cwd, &task, options)?,
                        json,
                        "Task",
                    )
                } else {
                    render_json_or_text(app.task_view(cwd, &task)?, json, "Task")
                }
            }
            Some(TaskAction::External(args)) => {
                let task_id = args
                    .first()
                    .ok_or_else(|| DraftError::invalid_config("missing task id"))?;
                render_json_or_text(app.task_view(cwd, task_id)?, false, "Task")
            }
            None => render_json_or_text(app.task_current(cwd)?, false, "Task"),
        },
    }
}

/// `draft pack ...` — everything done to or about a ChangePack.
///
/// A ChangePack's identity and its work are still carried by the record
/// underneath; this is the surface §8.1 specifies over it, not a second
/// vocabulary for the same commands.
fn run_baseline(app: &App, cwd: &Path, action: BaselineAction) -> Result<(), DraftError> {
    // Every projection below reads the one accepted Baseline. Asking for a
    // root or a lineage when the project has never accepted anything is not an
    // empty answer — there is no Baseline to describe.
    let accepted = |app: &App| -> Result<draft_core::app::workflow::BaselineView, DraftError> {
        app.dcg_baseline(cwd)?.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                "this project accepts no baseline yet",
            )
        })
    };

    match action {
        BaselineAction::List { json } => {
            render_json_or_text(app.dcg_baselines(cwd)?, json, "Baselines")
        }
        BaselineAction::Republish {
            baseline,
            intent,
            purpose,
            attempt,
            json,
        } => {
            let outcome = app.dcg_republish(
                cwd,
                baseline.as_deref(),
                &purpose,
                &intent,
                &attempt.unwrap_or_else(new_attempt_id),
            )?;
            if !json && !outcome.is_delivered() {
                output::warn("the delivery did not succeed; the accepted Baseline is unchanged");
            }
            render_json_or_text(outcome, json, "Republication")
        }
        BaselineAction::Show { json } => {
            render_json_or_text(accepted(app)?, json, "Accepted Baseline")
        }
        BaselineAction::Current { json } => {
            let view = accepted(app)?;
            render_json_or_text(
                serde_json::json!({ "baseline": view.baseline.to_string() }),
                json,
                "Accepted Baseline",
            )
        }
        BaselineAction::StateRoot { json } => {
            let view = accepted(app)?;
            render_json_or_text(
                serde_json::json!({
                    "baseline": view.baseline.to_string(),
                    "project_state_root": view.manifest.project_state_root,
                }),
                json,
                "Project state root",
            )
        }
        BaselineAction::EvidenceRoot { json } => {
            let view = accepted(app)?;
            render_json_or_text(
                serde_json::json!({
                    "baseline": view.baseline.to_string(),
                    "state_evidence_root": view.manifest.state_evidence_root,
                }),
                json,
                "State evidence root",
            )
        }
        BaselineAction::Coverage { json } => {
            let view = accepted(app)?;
            render_json_or_text(
                serde_json::json!({
                    "baseline": view.baseline.to_string(),
                    "coverage_evidence_root": view.manifest.coverage_evidence_root,
                    "observed": app.observation_coverage(cwd)?,
                }),
                json,
                "Coverage",
            )
        }
        BaselineAction::Lineage { json } => {
            let view = accepted(app)?;
            render_json_or_text(
                view.lineage
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
                json,
                "Baseline lineage",
            )
        }
        BaselineAction::Composition { json } => {
            let view = accepted(app)?;
            // Accepted provenance and current routability are answers to two
            // different questions, and are reported as two fields so neither
            // can be read as the other.
            render_json_or_text(
                serde_json::json!({
                    "baseline": view.baseline.to_string(),
                    "accepted_provider_provenance": view.composition,
                    "currently_routable": view.routable,
                    "routability_reason": view.route_refusal,
                }),
                json,
                "Baseline composition",
            )
        }
        BaselineAction::Receipts { receipt, json } => match receipt {
            Some(id) => render_receipt_show(app.receipt_show(cwd, &id)?, json),
            None => render_json_or_text(app.receipts(cwd)?, json, "Receipts"),
        },
        BaselineAction::Publications { json } => {
            render_json_or_text(app.dcg_publications(cwd)?, json, "Publications")
        }
        BaselineAction::Publish { action } => run_publish(app, cwd, action),
    }
}

/// The capability an executor grant is over.
///
/// Frozen, and named rather than configurable: `draft.change.operate/v1` is
/// what "may carry out operations here" means in the reserved vocabulary, and
/// a flag would invite somebody to grant something else under a name that says
/// executor.
const EXECUTOR_CAPABILITY: &str = "draft.change.operate/v1";

fn run_authority(app: &App, cwd: &Path, action: AuthorityAction) -> Result<(), DraftError> {
    match action {
        AuthorityAction::Grant {
            capability,
            grantee,
            json,
        } => {
            let grant = app.authority_grant(cwd, &capability, grantee.as_deref())?;
            if !json {
                output::warn(
                    "this is a permission, not an action: it authorizes future work and performs \
                     none",
                );
            }
            render_json_or_text(grant, json, "Authority granted")
        }
        AuthorityAction::List { json } => {
            render_json_or_text(app.authority_list(cwd)?, json, "Authority")
        }
        AuthorityAction::Show { grant, json } => {
            render_json_or_text(app.authority_show(cwd, &grant)?, json, "Grant")
        }
        AuthorityAction::Revoke {
            grant,
            reason,
            json,
        } => render_json_or_text(
            app.authority_revoke(cwd, &grant, &reason)?,
            json,
            "Authority revoked — the grant stays on the record",
        ),
        AuthorityAction::Executor { action } => match action {
            ExecutorAuthorityAction::Grant { grantee, json } => render_json_or_text(
                app.authority_grant(cwd, EXECUTOR_CAPABILITY, grantee.as_deref())?,
                json,
                "Executor authority granted",
            ),
            ExecutorAuthorityAction::List { json } => {
                let grants: Vec<_> = app
                    .authority_list(cwd)?
                    .into_iter()
                    .filter(|view| view.grant.capability.to_string() == EXECUTOR_CAPABILITY)
                    .collect();
                render_json_or_text(grants, json, "Executor authority")
            }
            ExecutorAuthorityAction::Revoke {
                grant,
                reason,
                json,
            } => render_json_or_text(
                app.authority_revoke(cwd, &grant, &reason)?,
                json,
                "Executor authority revoked",
            ),
        },
    }
}

/// Read a canonical JSON document into exactly the structure it names.
///
/// `deny_unknown_fields` is the point: a field Draft does not recognise is
/// refused rather than dropped, so a definition written against a different
/// idea of the format fails loudly instead of binding a provider to semantics
/// nobody stated.
fn read_canonical<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, DraftError> {
    let bytes = std::fs::read(path).map_err(|error| {
        DraftError::new(
            DraftErrorKind::NotFound,
            format!("{}: {error}", path.display()),
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        DraftError::new(
            DraftErrorKind::Validation,
            format!(
                "{} is not a valid document for this command: {error}",
                path.display()
            ),
        )
    })
}

fn run_provider(app: &App, cwd: &Path, action: ProviderAction) -> Result<(), DraftError> {
    match action {
        ProviderAction::List { json } => {
            render_json_or_text(app.provider_list(cwd)?, json, "Provider bindings")
        }
        ProviderAction::Show { binding, json } => {
            render_json_or_text(app.provider_show(cwd, &binding)?, json, "Provider binding")
        }
        ProviderAction::Bind {
            name,
            semantics,
            definition,
            profile,
            json,
        } => render_json_or_text(
            app.provider_bind(
                cwd,
                &name,
                &read_canonical(&semantics)?,
                &read_canonical(&definition)?,
                &read_canonical(&profile)?,
            )?,
            json,
            "Provider bound",
        ),
        ProviderAction::Redefine {
            binding,
            semantics,
            definition,
            json,
        } => {
            let updated = app.provider_redefine(
                cwd,
                &binding,
                &read_canonical(&semantics)?,
                &read_canonical(&definition)?,
            )?;
            if !json {
                output::warn(
                    "routes planned against the previous definition are now stale and will be \
                     refused rather than silently re-pointed",
                );
            }
            render_json_or_text(updated, json, "Provider redefined")
        }
        ProviderAction::Profile {
            binding,
            profile,
            json,
        } => render_json_or_text(
            app.provider_profile(cwd, &binding, &read_canonical(&profile)?)?,
            json,
            "Provider reprofiled",
        ),
        ProviderAction::Unbind { binding, json } => render_json_or_text(
            app.provider_unbind(cwd, &binding)?,
            json,
            "Provider unbound — its history is kept and stays verifiable",
        ),
        ProviderAction::Rebind { binding, json } => render_json_or_text(
            app.provider_rebind(cwd, &binding)?,
            json,
            "Provider rebound",
        ),
    }
}

fn run_publish(app: &App, cwd: &Path, action: PublishAction) -> Result<(), DraftError> {
    match action {
        PublishAction::Run {
            baseline,
            retry_authorization,
            purpose,
            attempt,
            json,
        } => {
            let outcome = app.dcg_publish(
                cwd,
                baseline.as_deref(),
                &purpose,
                // A fresh attempt by default; the same attempt when named, so
                // a re-run after an uncertain interruption converges instead
                // of delivering twice.
                &attempt.unwrap_or_else(new_attempt_id),
                retry_authorization.as_deref(),
            )?;
            if !json {
                // The Baseline is not what failed, and must never read as
                // though it were.
                match &outcome {
                    draft_core::app::publish::PublishOutcome::Concluded { .. }
                        if outcome.is_delivered() => {}
                    _ => output::warn(
                        "the delivery did not succeed; the accepted Baseline is unchanged",
                    ),
                }
            }
            render_json_or_text(outcome, json, "Publication")
        }
        PublishAction::List { json } => {
            render_json_or_text(app.dcg_publications(cwd)?, json, "Publications")
        }
        PublishAction::AuthorizeRetry {
            rationale,
            purpose,
            json,
        } => {
            let digest = app.dcg_authorize_retry(cwd, &purpose, &new_attempt_id(), &rationale)?;
            if !json {
                output::warn(
                    "this permits one further attempt that may duplicate a real-world effect",
                );
            }
            render_json_or_text(
                serde_json::json!({ "retry_authorization": digest }),
                json,
                "Retry authorized — pass it to `draft baseline publish run \
                 --retry-authorization`",
            )
        }
        PublishAction::Retry {
            retry_authorization,
            purpose,
            attempt,
            json,
        } => {
            let outcome = app.dcg_publish(
                cwd,
                None,
                &purpose,
                &attempt.unwrap_or_else(new_attempt_id),
                Some(&retry_authorization),
            )?;
            if !json && !outcome.is_delivered() {
                output::warn("the delivery did not succeed; the accepted Baseline is unchanged");
            }
            render_json_or_text(outcome, json, "Publication")
        }
        PublishAction::Resolve {
            outcome,
            mark_succeeded,
            mark_failed,
            rationale,
            purpose,
            attempt,
            json,
        } => {
            let digest = app.dcg_resolve_outcome(
                cwd,
                &purpose,
                &attempt.unwrap_or_else(new_attempt_id),
                outcome.as_deref(),
                mark_succeeded.as_deref(),
                mark_failed.as_deref(),
                &rationale,
            )?;
            render_json_or_text(
                serde_json::json!({ "resolution": digest }),
                json,
                "Outcome resolved — the primary outcome is unchanged",
            )
        }
        PublishAction::Withdraw {
            attempt,
            reason,
            purpose,
            json,
        } => render_json_or_text(
            app.dcg_withdraw_attempt(cwd, &purpose, &attempt, &reason)?,
            json,
            "Attempt withdrawn",
        ),
    }
}

fn run_pack(app: &App, cwd: &Path, action: PackAction) -> Result<(), DraftError> {
    match action {
        PackAction::New {
            intent,
            scope,
            json,
        } => render_json_or_text(
            app.dcg_open_change_pack(cwd, &intent, &scope)?,
            json,
            "ChangePack created",
        ),
        PackAction::List { json } => {
            render_json_or_text(app.dcg_change_packs(cwd)?, json, "ChangePacks")
        }
        PackAction::Checkpoint { message, json } => {
            render_json_or_text(app.checkpoint(cwd, &message)?, json, "Checkpoint created")
        }
        PackAction::Candidate { action } => run_candidate(app, cwd, action),
        PackAction::Show {
            change_pack_id: change,
            json,
        } => render_json_or_text(app.dcg_change_pack(cwd, &change)?, json, "ChangePack"),
        PackAction::Intent { action } => match action {
            IntentAction::Show {
                change_pack_id: change,
                json,
            } => {
                let view = app.dcg_change_pack(cwd, &change)?;
                render_json_or_text(
                    serde_json::json!({
                        "change_pack_id": view["change_pack_id"],
                        "intent": view["definition"]["intent"],
                    }),
                    json,
                    "Intent",
                )
            }
            IntentAction::Set {
                change_pack_id: change,
                intent,
                json,
            }
            | IntentAction::Amend {
                change_pack_id: change,
                intent,
                json,
            } => render_json_or_text(
                app.dcg_amend_intent(cwd, &change, &intent)?,
                json,
                "Intent recorded — the scope it declared is unchanged",
            ),
        },
        PackAction::Select {
            change_pack_id: change,
            json,
        } => render_json_or_text(
            serde_json::json!({ "selected_change": app.dcg_select_change_pack(cwd, &change)? }),
            json,
            "ChangePack selected",
        ),
        PackAction::Inspect {
            change_pack_id: change,
            json,
        } => render_json_or_text(app.dcg_inspect(cwd, &change)?, json, "ChangePack"),
        PackAction::Depends {
            change_pack_id: change,
            json,
        } => render_json_or_text(app.dcg_depends(cwd, &change)?, json, "Dependencies"),
        PackAction::Conflicts {
            change_pack_id: change,
            json,
        } => render_json_or_text(app.dcg_conflicts(cwd, &change)?, json, "Conflicts"),
        PackAction::Compose {
            change_pack_ids: changes,
            json,
        } => {
            let composition = app.dcg_compose(cwd, &changes)?;
            if !json && composition.status == draft_core::dcg::compose::CompositionStatus::Failed {
                output::warn("these ChangePacks do not compose; `disperse` says which are held");
            }
            render_json_or_text(composition, json, "Composition")
        }
        PackAction::Disperse {
            change_pack_ids: changes,
            json,
        } => render_json_or_text(app.dcg_disperse(cwd, &changes)?, json, "Dispersal"),
        PackAction::Impact {
            revision_pack_id: revision,
            json,
        } => render_json_or_text(app.dcg_impact(cwd, &revision)?, json, "Impact"),
        PackAction::Coverage {
            revision_pack_id: revision,
            json,
        } => render_json_or_text(app.dcg_coverage(cwd, &revision)?, json, "Proof coverage"),
        PackAction::Representation { action } => match action {
            RepresentationAction::List { json } => {
                render_json_or_text(app.dcg_representations(cwd)?, json, "Representations")
            }
            RepresentationAction::Show {
                revision_pack_id: revision,
                json,
            } => render_json_or_text(
                app.dcg_representation(cwd, &revision)?.ok_or_else(|| {
                    DraftError::new(
                        DraftErrorKind::NotFound,
                        format!("no explanation is recorded for revision '{revision}'"),
                    )
                })?,
                json,
                "Representation",
            ),
        },
        PackAction::Receipts {
            change_pack_id: change,
            json,
        } => render_json_or_text(
            app.dcg_change_pack_receipts(cwd, &change)?,
            json,
            "Receipts",
        ),
        PackAction::Scope {
            change_pack_id: change,
            json,
        } => {
            let view = app.dcg_change_pack(cwd, &change)?;
            // Declared and resolved are both shown. A reader given only the
            // resolved set cannot tell whether a declaration was narrowed.
            render_json_or_text(
                serde_json::json!({
                    "change_pack_id": view["change_pack_id"],
                    "declared": view["definition"]["scope_declaration"],
                    "resolved": view["scope_resolution"],
                }),
                json,
                "Scope",
            )
        }
        PackAction::Revision { action } => match action {
            RevisionAction::Seal {
                change_pack_id: change,
                json,
            } => render_json_or_text(app.dcg_seal(cwd, &change)?, json, "Revision sealed"),
            RevisionAction::List {
                change_pack_id: change,
                json,
            } => {
                let view = app.dcg_change_pack(cwd, &change)?;
                render_json_or_text(view["revisions"].clone(), json, "Revisions")
            }
            RevisionAction::Show {
                change_pack_id: change,
                revision_pack_id: revision,
                json,
            } => {
                let view = app.dcg_change_pack(cwd, &change)?;
                let sealed = view["revisions"]
                    .as_array()
                    .and_then(|all| {
                        all.iter()
                            .find(|value| value["id"].as_str() == Some(revision.as_str()))
                    })
                    .cloned()
                    .ok_or_else(|| {
                        DraftError::new(
                            DraftErrorKind::NotFound,
                            format!("ChangePack '{change}' has no revision '{revision}'"),
                        )
                    })?;
                render_json_or_text(sealed, json, "Revision")
            }
        },
        PackAction::Abandon {
            change_pack_id: change,
            json,
        } => render_json_or_text(
            app.dcg_abandon_change_pack(cwd, &change)?,
            json,
            "ChangePack abandoned — its history is kept",
        ),
        PackAction::Reopen {
            change_pack_id: change,
            json,
        } => render_json_or_text(
            app.dcg_reopen_change_pack(cwd, &change)?,
            json,
            "ChangePack reopened",
        ),
        PackAction::Compare { left, right, json } => render_json_or_text(
            app.dcg_compare_change_packs(cwd, &left, &right)?,
            json,
            "Interference",
        ),
        PackAction::Evidence { action } => match action {
            EvidenceAction::Run {
                revision_pack_id: revision,
                json,
            } => render_json_or_text(app.dcg_verify(cwd, &revision)?, json, "Evidence recorded"),
            EvidenceAction::List {
                revision_pack_id: revision,
                json,
            } => {
                let evidence = app.dcg_evidence_for(cwd, &revision)?;
                render_json_or_text(evidence, json, "Evidence")
            }
            EvidenceAction::Show { evidence, json } => render_json_or_text(
                app.dcg_evidence(cwd, &evidence)?.ok_or_else(|| {
                    DraftError::new(
                        DraftErrorKind::NotFound,
                        format!("no evidence '{evidence}'"),
                    )
                })?,
                json,
                "Evidence",
            ),
        },
        PackAction::Assess {
            revision_pack_id: revision,
            risk,
            rationale,
            json,
        } => render_json_or_text(
            app.dcg_assess(cwd, &revision, &risk, &rationale)?,
            json,
            "Assessment recorded",
        ),
        PackAction::Review {
            revision_pack_id: revision,
            comments,
            json,
        } => render_json_or_text(
            app.dcg_review(cwd, &revision, &comments)?,
            json,
            "Review recorded — this is not a decision",
        ),
        PackAction::Decide {
            revision_pack_id: revision,
            approve,
            reject,
            gate,
            reason,
            json,
        } => {
            if approve == reject {
                return Err(DraftError::new(
                    DraftErrorKind::Validation,
                    "say which: --approve or --reject",
                ));
            }
            if approve {
                render_json_or_text(
                    app.dcg_decide(cwd, &revision, gate.as_deref(), true, None)?,
                    json,
                    "Approved — this authorizes a promotion; it does not perform one",
                )
            } else {
                let reason = reason.ok_or_else(|| {
                    DraftError::new(
                        DraftErrorKind::Validation,
                        "--reject requires --reason: a rejection nobody can read is not a \
                         decision anyone can act on",
                    )
                })?;
                render_json_or_text(
                    app.dcg_decide(cwd, &revision, None, false, Some(&reason))?,
                    json,
                    "Rejected",
                )
            }
        }
        PackAction::Gates { action } => match action {
            GateAction::Evaluate {
                revision_pack_id: revision,
                waivers,
                json,
            } => {
                let evaluation = app.dcg_evaluate_gate(cwd, &revision, &waivers)?;
                // Said plainly, because a gate is the one place where "not
                // satisfied" is the useful answer as often as "satisfied" is.
                if !json && !evaluation.is_satisfied() {
                    output::warn("the gate is not satisfied");
                }
                render_json_or_text(evaluation, json, "Gate evaluated")
            }
            GateAction::List {
                change_pack_id: change,
                revision_pack_id: revision,
                json,
            } => render_json_or_text(
                app.dcg_authorization(cwd, &change, &revision)?,
                json,
                "Authorization",
            ),
            GateAction::Waive {
                revision_pack_id: revision,
                condition,
                reason,
                days,
                json,
            } => render_json_or_text(
                app.dcg_waive(cwd, &revision, &condition, &reason, days)?,
                json,
                "Waiver granted — offer it with `draft pack gates evaluate --waiver`",
            ),
        },
    }
}

fn run_observation(app: &App, cwd: &Path, action: ObservationAction) -> Result<(), DraftError> {
    match action {
        ObservationAction::Show { json } => {
            render_json_or_text(app.observation_context(cwd)?, json, "Observation context")
        }
        ObservationAction::Coverage { json } => {
            render_json_or_text(app.observation_coverage(cwd)?, json, "Observation coverage")
        }
        ObservationAction::Provenance {
            snapshot_digest,
            json,
        } => render_json_or_text(
            app.observation_provenance(cwd, snapshot_digest.as_deref())?,
            json,
            "Observation provenance",
        ),
        ObservationAction::Pending { json } => match app.observation_pending(cwd)? {
            Some(pending) => render_json_or_text(pending, json, "Pending observation context"),
            None => {
                if json {
                    render_json_or_text(serde_json::json!(null), true, "")
                } else {
                    output::print_human(
                        &"No pending observation change: the installed extensions observe \
                              exactly as the adopted semantics do.",
                    );
                    Ok(())
                }
            }
        },
        ObservationAction::Preview { json } => render_json_or_text(
            app.observation_preview(cwd)?,
            json,
            "Observation context preview",
        ),
        ObservationAction::Adopt { json } => render_json_or_text(
            app.observation_adopt(cwd)?,
            json,
            "Adopted observation context",
        ),
        ObservationAction::Transitions { json } => render_json_or_text(
            app.observation_transitions(cwd)?,
            json,
            "Observation context transitions",
        ),
    }
}

fn run_candidate(app: &App, cwd: &Path, action: CandidateAction) -> Result<(), DraftError> {
    match action {
        CandidateAction::List { json } => {
            render_json_or_text(app.candidate_list(cwd)?, json, "Candidates")
        }
        CandidateAction::Show {
            candidate_name,
            json,
        } => render_json_or_text(app.candidate_show(cwd, &candidate_name)?, json, "Candidate"),
        CandidateAction::Add(args) => render_json_or_text(
            app.candidate_add(
                cwd,
                &args.candidate_name,
                args.kind.as_deref(),
                args.template,
            )?,
            args.json,
            "Candidate added",
        ),
        CandidateAction::Update(args) => render_json_or_text(
            app.candidate_update(
                cwd,
                &args.candidate_name,
                args.kind.as_deref(),
                args.template,
            )?,
            args.json,
            "Candidate updated",
        ),
        CandidateAction::Remove {
            candidate_name,
            json,
        } => render_json_or_text(
            app.candidate_remove(cwd, &candidate_name)?,
            json,
            "Candidate removed",
        ),
    }
}

/// Restore a past state, or report what restoring would do.
fn run_rollback(
    app: &App,
    cwd: &Path,
    reference: String,
    dry_run: bool,
    json: bool,
) -> Result<(), DraftError> {
    if dry_run {
        render_dry_run(app.rollback_dry_run(cwd, &reference)?, json)
    } else {
        render_json_or_text(
            app.rollback(cwd, &reference, true)?,
            json,
            "Rollback complete",
        )
    }
}

fn run_maintenance(app: &App, cwd: &Path, command: MaintenanceCommand) -> Result<(), DraftError> {
    match command {
        MaintenanceCommand::Close { force } => render_close(app.close(cwd, force)?),
        MaintenanceCommand::Gc => render_gc(app.gc(cwd)?),
        MaintenanceCommand::Storage { action } => match action {
            StorageAction::Stats { json } => {
                render_json_or_text(app.storage_stats(cwd)?, json, "Storage stats")
            }
            StorageAction::Gc { json } => {
                render_json_or_text(app.storage_gc(cwd)?, json, "Storage GC complete")
            }
            StorageAction::Compact { json } => {
                render_json_or_text(app.storage_compact(cwd)?, json, "Storage compact complete")
            }
            StorageAction::Prune { json } => {
                render_json_or_text(app.storage_prune(cwd)?, json, "Storage prune complete")
            }
            StorageAction::Doctor { json } => {
                render_json_or_text(app.storage_doctor(cwd)?, json, "Storage doctor complete")
            }
        },
    }
}

fn ensure_project_scope(command: &Command, cwd: &Path) -> Result<(), DraftError> {
    if !requires_project_scope(command) || find_workspace_root(cwd).is_some() {
        return Ok(());
    }
    Err(DraftError::new(
        DraftErrorKind::ProjectScopeRequired,
        "this command must be run inside a Draft workspace",
    )
    .with_context(format!("current directory: {}", cwd.display()))
    .with_suggestion("run `draft init` here or change into an existing Draft workspace"))
}

fn requires_project_scope(command: &Command) -> bool {
    match command {
        Command::Daemon { .. } | Command::Project { .. } => false,
        Command::Workspace(command) => requires_workspace_scope(command),
        _ => true,
    }
}

fn requires_workspace_scope(command: &WorkspaceCommand) -> bool {
    match command {
        WorkspaceCommand::Init { .. } => false,
        WorkspaceCommand::Doctor {
            action: Some(DoctorAction::Sync { .. }),
            ..
        }
        | WorkspaceCommand::Doctor { global: true, .. } => false,
        WorkspaceCommand::Doctor { .. } => true,
        WorkspaceCommand::Console { .. } => false,
        WorkspaceCommand::Extension { .. } => false,
        WorkspaceCommand::Config {
            action:
                Some(ConfigAction::Get { global: true, .. })
                | Some(ConfigAction::Set { global: true, .. })
                | Some(ConfigAction::Unset { global: true, .. })
                | None,
            ..
        } => false,
        WorkspaceCommand::Config { .. } => true,
        WorkspaceCommand::Status { .. } => false,
    }
}

fn find_workspace_root(cwd: &Path) -> Option<PathBuf> {
    let mut cur = cwd.to_path_buf();
    loop {
        if cur.join(".draft").join("project.json").exists() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

fn render_init(report: draft_core::app::InitReport, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
        return Ok(());
    }
    if report.created {
        output::success("Initialized Draft workspace.");
    } else {
        output::warn("Draft workspace already initialized here.");
    }
    output::field("Workspace", &report.workspace_id);
    output::field("Root", &report.root);
    output::field(".draft", &report.draft_dir);
    output::field("Baseline", &report.baseline_id);
    output::field("State root", &report.project_state_root);
    if !report.next_actions.is_empty() {
        output::section("Next actions");
        for action in &report.next_actions {
            output::bullet(action);
        }
    }
    if !report.candidate_guidance.is_empty() {
        output::section("Candidates");
        output::line(&report.candidate_guidance);
    }
    Ok(())
}

fn render_init_global(
    report: draft_core::app::InitGlobalReport,
    json: bool,
) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
        return Ok(());
    }
    if report.created {
        output::success("Initialized global Draft store.");
    } else {
        output::warn("Global Draft store already initialized.");
    }
    output::field("Root", &report.root);
    output::field("Hidden", &report.hidden.to_string());
    output::field("Actor", &report.actor_id);
    output::field("Public key", &report.public_key_id);
    Ok(())
}

fn render_doctor(report: draft_core::app::DoctorReport, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
    } else {
        print_doctor_scope(&report.global);
        if let Some(project) = &report.project {
            print_doctor_scope(project);
        }
    }
    if report.healthy() {
        Ok(())
    } else {
        Err(DraftError::new(
            DraftErrorKind::Storage,
            "draft doctor found problems",
        ))
    }
}

fn print_doctor_scope(scope: &draft_core::app::DoctorScope) {
    output::header(&format!("{} store", scope.label));
    output::field("Root", &scope.root);
    output::field("Exists", &scope.exists.to_string());
    output::field("Hidden", &scope.hidden.to_string());
    for c in &scope.checks {
        let mark = if c.ok { "ok " } else { "FAIL" };
        let category = c
            .category
            .as_deref()
            .map(|value| format!(" [{value}]"))
            .unwrap_or_default();
        println!("  [{mark}] {:<18}{category} {}", c.name, c.detail);
    }
}

fn render_dry_run(report: draft_core::app::DryRunReport, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
        return Ok(());
    }
    output::header(&format!("Dry run: {} {}", report.action, report.target));
    output::field("Would proceed", &report.would_proceed.to_string());
    output::field("Resulting state", &report.resulting_state);
    for c in &report.checks {
        let mark = if c.ok { "ok " } else { "FAIL" };
        println!("  [{mark}] {:<18} {}", c.name, c.detail);
    }
    if !report.affected_resources.is_empty() {
        output::field(
            "Affected resources",
            &report.affected_resources.len().to_string(),
        );
        for locator in &report.affected_resources {
            println!("    {locator}");
        }
    }
    Ok(())
}

fn render_receipt_show(value: serde_json::Value, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&value);
        return Ok(());
    }
    let payload = value.get("payload").cloned().unwrap_or(value.clone());
    let signer = value.get("signer").cloned().unwrap_or_default();
    let receipt_id = json_string(&payload, &["receipt_id"]).unwrap_or("unknown");

    output::header(&format!("Receipt {receipt_id}"));
    output::section("Proof");
    // What the receipt attests, said as the one exact fact it witnessed.
    // A receipt that could name a set of things would be unable to say which
    // of them it actually saw.
    if let Some(subject) = payload.get("subject") {
        match json_string(subject, &["kind"]) {
            Some("promotion") => {
                output::bullet("Attests: a Promotion accepted a Baseline");
                if let Some(promotion) = json_string(subject, &["promotion"]) {
                    output::bullet(&format!("Promotion: {promotion}"));
                }
                if let Some(baseline) = json_string(subject, &["baseline"]) {
                    output::bullet(&format!("Baseline: {baseline}"));
                }
            }
            Some("publication_outcome") => {
                output::bullet("Attests: a publication attempt's primary outcome");
                if let Some(outcome) = json_string(subject, &["outcome"]) {
                    output::bullet(&format!("Outcome: {outcome}"));
                }
            }
            Some("publication_resolution") => {
                output::bullet("Attests: an authorized resolution of an outcome");
                if let Some(resolution) = json_string(subject, &["resolution"]) {
                    output::bullet(&format!("Resolution: {resolution}"));
                }
            }
            _ => {}
        }
    }
    if let Some(actor) = json_string(&payload, &["issued_by"]) {
        output::bullet(&format!("Issued by: {actor}"));
    }
    if let Some(key) = json_string(&signer, &["signing_key_id"]) {
        output::bullet(&format!("Signing key: {key}"));
    }
    if let Some(algorithm) = json_string(&signer, &["signature_algorithm"]) {
        output::bullet(&format!("Algorithm: {algorithm}"));
    }
    if json_string(&value, &["signature"]).is_some() {
        // Present, not valid. Reading a receipt is not verifying it, and
        // `draft doctor receipts` is where that question is answered.
        output::bullet("Signature: present (verify with `draft doctor receipts`)");
    }
    output::section("Receipt IDs");
    output::bullet(&format!("Receipt: {receipt_id}"));
    if let Some(issued_at) = payload.get("issued_at").and_then(serde_json::Value::as_i64) {
        output::bullet(&format!("Issued at: {issued_at}"));
    }
    Ok(())
}

fn json_string<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| value.get(*key)?.as_str())
}

fn render_receipt_verification(
    v: draft_core::receipt::ReceiptVerification,
    json: bool,
) -> Result<(), DraftError> {
    if json {
        output::print_json(&v);
    } else {
        output::header(&format!("Receipt {} ({})", v.receipt_id, v.subject));
        for c in &v.checks {
            // Three-valued on purpose: "unknown" is never rendered as a pass.
            println!("  {:<8} {:<18} {}", c.status.as_str(), c.name, c.detail);
        }
    }
    if v.ok {
        Ok(())
    } else {
        Err(DraftError::new(
            DraftErrorKind::Storage,
            format!("receipt {} failed verification", v.receipt_id),
        ))
    }
}

fn render_ledger_verification(
    v: draft_core::read_model::LedgerVerification,
    json: bool,
) -> Result<(), DraftError> {
    if json {
        output::print_json(&v);
    } else {
        output::header("Receipt and Activity verification");
        println!(
            "  Activity:      {} ({} events)",
            ok_word(v.activity_chain_ok),
            v.activity_count
        );
        println!(
            "  transparency:  {} ({} entries)",
            ok_word(v.transparency_ok),
            v.transparency_count
        );
        let bad = v.receipts.iter().filter(|r| !r.ok).count();
        println!(
            "  receipts:      {} ({} ok / {} failed)",
            ok_word(bad == 0),
            v.receipts.len() - bad,
            bad
        );
    }
    if v.all_ok {
        Ok(())
    } else {
        Err(DraftError::new(
            DraftErrorKind::Storage,
            "the Activity chain, a receipt, or the transparency chain did not verify",
        ))
    }
}

fn render_close(report: draft_core::app::CloseReport) -> Result<(), DraftError> {
    output::success("Draft closed");
    output::field(".draft removed", &report.draft_dir);
    output::field("Forced", &report.forced.to_string());
    output::field("Pending changes", &report.pending_changes.to_string());
    Ok(())
}

fn render_gc(report: draft_core::app::maintenance::GcReport) -> Result<(), DraftError> {
    output::success("Draft GC complete");
    output::field("Removed entries", &report.removed_entries.to_string());
    output::field(
        "Accepted Baseline valid",
        &report.accepted_baseline_valid.to_string(),
    );
    output::field(
        "Active changes preserved",
        &report.active_changes_preserved.to_string(),
    );
    Ok(())
}

fn ok_word(ok: bool) -> &'static str {
    if ok {
        "OK"
    } else {
        "FAIL"
    }
}

fn render_config(report: draft_core::app::ConfigReport, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
        return Ok(());
    }
    for (k, v) in report.entries {
        output::field(&k, &v);
    }
    Ok(())
}

fn render_ignore(report: draft_core::app::IgnoreReport, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
        return Ok(());
    }
    for p in report.patterns {
        println!("{p}");
    }
    Ok(())
}

fn render_status(
    report: draft_core::dcg::state::WorkspaceStatus,
    json: bool,
) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
        return Ok(());
    }
    output::header("Workspace Status");
    output::field("Workspace", &report.workspace_id.to_string());
    output::field("ChangePacks", &report.changes.len().to_string());
    for change in report.changes {
        let aspects = change
            .aspects
            .iter()
            .map(|aspect| aspect.as_str())
            .collect::<Vec<_>>()
            .join(",");
        println!("  {:<20} {}", aspects, change.locator.body);
    }
    Ok(())
}

fn render_status_report(
    report: draft_core::app::StatusReport,
    json: bool,
) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
        return Ok(());
    }
    output::header("Workspace Status");
    output::field("Workspace", &report.workspace.workspace_id.to_string());
    output::field("ChangePacks", &report.workspace.changes.len().to_string());
    if let Some(component) = &report.component {
        output::field("Component", component);
    }
    if let Some(change) = &report.change_pack_id {
        output::field("ChangePack", change);
    }
    for (name, value) in report.sections {
        output::section(&name);
        output::print_human(&value);
    }
    Ok(())
}

fn render_events(
    events: Vec<draft_core::read_model::ActivityEntry>,
    json: bool,
    raw: bool,
) -> Result<(), DraftError> {
    if json {
        output::print_json(&events);
        return Ok(());
    }
    if raw {
        for e in events {
            println!("{}", serde_json::to_string(&e).map_err(DraftError::from)?);
        }
        return Ok(());
    }
    for e in events {
        println!(
            "{} {} {}",
            e.event_id,
            e.kind,
            e.subject.unwrap_or_default()
        );
    }
    Ok(())
}

fn run_task_wizard(
    app: &App,
    cwd: &Path,
    json: bool,
) -> Result<draft_core::task::TaskDefinition, DraftError> {
    let name = prompt_line("Task name", json)?;
    // Show what is actually installable rather than asking for an id blind.
    // With no vocabulary installed there is nothing to offer, and the wizard
    // says so instead of implying the answer is a name the user forgot.
    let available = app.task_templates(cwd)?;
    if !json {
        if available.is_empty() {
            output::field(
                "Templates",
                "none installed (install an extension contributing task_template)",
            );
        } else {
            output::field(
                "Templates",
                &available
                    .iter()
                    .map(|template| template.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
    }
    let template = prompt_line("Task type/template (blank for none)", json)?;
    let goal = prompt_line("Goal", json)?;
    let should_not_change = prompt_line("What should not change?", json)?;
    let allowed = prompt_line("Allowed zones (comma-separated)", json)?;
    let forbidden = prompt_line("Forbidden zones (comma-separated)", json)?;
    let success = prompt_line("Success checks (comma-separated)", json)?;
    let risk = prompt_line("Risk [low|medium|high|critical]", json)?;
    let plan_first = prompt_line("Plan first? [y/N]", json)?;
    let candidate_preset = prompt_line("Candidate preset", json)?;
    let mode = if matches!(plan_first.trim(), "y" | "Y" | "yes" | "YES") {
        Some("plan-first".to_string())
    } else {
        None
    };
    let mut forbidden_zones = split_csv(&forbidden);
    forbidden_zones.extend(split_csv(&should_not_change));
    if !json {
        output::section("Preview");
        output::field("Name", name.trim());
        output::field("Template", template.trim());
        output::field("Goal", goal.trim());
        output::field("Allowed", &split_csv(&allowed).join(", "));
        output::field("Forbidden", &forbidden_zones.join(", "));
        output::field("Success", &split_csv(&success).join(", "));
        output::field("Risk", risk.trim());
        output::field("Mode", mode.as_deref().unwrap_or("normal"));
        output::field("Preset", candidate_preset.trim());
    }
    let confirm = prompt_line("Create task? [y/N]", json)?;
    if !matches!(confirm.trim(), "y" | "Y" | "yes" | "YES") {
        return Err(DraftError::invalid_config("task wizard cancelled"));
    }
    let template = (!template.trim().is_empty()).then(|| template.trim().to_string());
    let risk = (!risk.trim().is_empty()).then(|| risk.trim().to_string());
    let candidate_preset =
        (!candidate_preset.trim().is_empty()).then(|| candidate_preset.trim().to_string());
    app.task_create(
        cwd,
        name.trim(),
        goal.trim(),
        template,
        split_csv(&allowed),
        forbidden_zones,
        split_csv(&success),
        risk.as_deref(),
        mode.as_deref(),
        candidate_preset,
    )
}

fn split_csv(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn prompt_line(label: &str, quiet: bool) -> Result<String, DraftError> {
    if !quiet {
        print!("{label}: ");
        io::stdout().flush().map_err(DraftError::from)?;
    }
    let mut buf = String::new();
    io::stdin().read_line(&mut buf).map_err(DraftError::from)?;
    Ok(buf.trim_end().to_string())
}

/// Parse one permission by its wire name, so a typo is refused rather than
/// silently granting nothing.
fn extension_view(
    id: &str,
) -> Result<draft_extension_service::authorization::ExtensionView, DraftError> {
    draft_extension_service::authorization::view(draft_extension_service::extension::show(id)?)
}

/// Which transport the user said a catalog location names.
fn source_form(local: bool, https: bool) -> draft_extension_service::catalog::SourceForm {
    match (local, https) {
        (true, _) => draft_extension_service::catalog::SourceForm::LocalDirectory,
        (_, true) => draft_extension_service::catalog::SourceForm::Https,
        _ => draft_extension_service::catalog::SourceForm::Inferred,
    }
}

/// An app that reads contributed domain knowledge from the extensions the user
/// has installed, enabled and authorized.
fn app() -> App {
    App::with_extension_contributions(std::sync::Arc::new(
        draft_extension_service::contributions::InstalledExtensions,
    ))
}

fn parse_permission(name: &str) -> Result<draft_core::extension::ExtensionPermission, DraftError> {
    draft_core::extension::ExtensionPermission::parse(name)
        .map_err(draft_core::extension::from_format_error)
}

fn parse_permissions(
    names: &[String],
) -> Result<Vec<draft_core::extension::ExtensionPermission>, DraftError> {
    names
        .iter()
        .map(|name| parse_permission(name))
        .collect::<Result<Vec<_>, _>>()
}

/// Apply any `--grant` the user passed alongside an install or update.
///
/// Installing and authorizing stay two decisions: this runs the second one
/// only because the user asked for it in the same breath, and it is recorded
/// as its own audited grant. Passing no `--grant` leaves the package installed
/// and enabled with its command-bearing contributions inert.
fn apply_grants(
    installed: draft_core::extension::InstalledExtension,
    grants: &[String],
) -> Result<draft_core::extension::InstalledExtension, DraftError> {
    if grants.is_empty() {
        return Ok(installed);
    }
    let permissions = parse_permissions(grants)?;
    draft_extension_service::authorization::authorize(
        installed.id(),
        &permissions,
        &draft_core::support::common::OperationId::generate(),
    )?;
    Ok(installed)
}

/// Resolve what a promotion needs, then run it.
///
/// The decision and gate default to what is on record for this revision, so
/// the common case does not require copying ids around. Nothing here decides
/// whether the promotion is authorized: it resolves references and hands them
/// to the application operation, which asks the domain.
#[allow(clippy::too_many_arguments)]
fn run_promote(
    app: &App,
    cwd: &Path,
    change: &str,
    revision: &str,
    decision: Option<String>,
    gate: Option<String>,
    expected_baseline: Option<String>,
    json: bool,
) -> Result<(), DraftError> {
    let view = app.dcg_authorization(cwd, change, revision)?;

    let (decision, gate) = match (decision, gate) {
        (Some(decision), Some(gate)) => (decision, gate),
        (decision, gate) => {
            let approving = view.approving_decision().ok_or_else(|| {
                DraftError::new(
                    DraftErrorKind::ReviewRequired,
                    format!(
                        "no approving decision cites a satisfied gate over revision '{revision}'"
                    ),
                )
                .with_suggestion("Run `draft gate evaluate`, then `draft decision approve`.")
            })?;
            let satisfied = view
                .gates
                .iter()
                .find(|value| value.satisfied && value.evaluation.covers(&approving.revision_pack))
                .ok_or_else(|| {
                    DraftError::new(
                        DraftErrorKind::ReviewRequired,
                        format!("no satisfied gate covers revision '{revision}'"),
                    )
                })?;
            (
                decision.unwrap_or_else(|| approving.id.to_string()),
                gate.unwrap_or_else(|| satisfied.evaluation.id.clone()),
            )
        }
    };

    // Read now, because a command invoked now is acting on the project as it
    // is. A caller acting on an older view states its own.
    let expected = match expected_baseline {
        Some(value) => Some(value),
        None => app
            .dcg_baseline(cwd)?
            .map(|baseline| baseline.baseline.digest().to_string()),
    };

    let outcome = app.dcg_promote(cwd, change, revision, &decision, &gate, expected.as_deref())?;
    if !json {
        output::success(&format!(
            "the project now accepts baseline {}",
            outcome.baseline()
        ));
    }
    render_json_or_text(outcome, json, "Promotion")
}

/// A fresh identity for one publication attempt.
///
/// Deliberately not derived from the Publication: two deliberate sends of the
/// same Publication are two attempts, and deriving this would silently make
/// the second one converge on the first.
fn new_attempt_id() -> String {
    format!(
        "cli-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or_default()
    )
}

fn render_json_or_text<T: serde::Serialize>(
    value: T,
    json: bool,
    label: &str,
) -> Result<(), DraftError> {
    if json {
        output::print_json(&value);
    } else {
        // Human-readable by default: never JSON without an explicit flag.
        output::success(label);
        output::print_human(&value);
    }
    Ok(())
}

#[allow(dead_code)]
fn _assert_path(_: &Path) {}

#[cfg(test)]
mod console_selection_tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct GlobalHomeGuard(Option<std::ffi::OsString>);

    impl GlobalHomeGuard {
        fn set(path: &Path) -> Self {
            let previous = std::env::var_os("DRAFT_GLOBAL_HOME");
            std::env::set_var("DRAFT_GLOBAL_HOME", path);
            Self(previous)
        }
    }

    impl Drop for GlobalHomeGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.0.take() {
                std::env::set_var("DRAFT_GLOBAL_HOME", previous);
            } else {
                std::env::remove_var("DRAFT_GLOBAL_HOME");
            }
        }
    }

    #[test]
    fn console_selection_is_registry_only_and_unsafe_automatic_state_falls_back() {
        let _lock = env_lock().lock().unwrap();
        let global = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let _home = GlobalHomeGuard::set(&global.path().join("draft-home"));
        let app = app();
        let initialized = app.init(project.path()).unwrap();
        let registry = draft_core::project::registry::ProjectRegistry::global().unwrap();
        registry
            .upsert(&initialized.workspace_id, project.path(), None, None)
            .unwrap();
        let nested = project.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let revision_before = registry.envelope().unwrap().revision;

        let automatic = resolve_console_context(&nested, None, false).unwrap();
        assert_eq!(
            automatic.workspace_id.as_deref(),
            Some(initialized.workspace_id.as_str())
        );
        assert_eq!(registry.envelope().unwrap().revision, revision_before);
        let explicit_path =
            resolve_console_context(global.path(), Some(project.path().to_str().unwrap()), false)
                .unwrap();
        assert_eq!(explicit_path.workspace_id, automatic.workspace_id);
        assert!(resolve_console_context(&nested, None, true)
            .unwrap()
            .workspace_id
            .is_none());

        std::fs::write(project.path().join(".draft/project.json"), b"{}\n").unwrap();
        let fallback = resolve_console_context(&nested, None, false).unwrap();
        assert!(fallback.workspace_id.is_none());
        assert!(fallback.diagnostic.unwrap().contains("GLOBAL"));
        assert!(resolve_console_context(
            global.path(),
            Some(initialized.workspace_id.as_str()),
            false,
        )
        .is_err());
    }
}
