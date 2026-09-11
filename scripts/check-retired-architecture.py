#!/usr/bin/env python3
"""Retired architecture must stay retired.

Enforces *architecture, not English vocabulary* (§10). It does not care that
the word "pack" appears in prose; it cares that a retired symbol, identifier
prefix or storage path is still reachable as code.

Two lists, deliberately:

  RETIRED  symbols that are gone. Any occurrence in code is a regression.
  OWED     symbols a later stage retires. Each names its owning stage, and a
           *stale* entry — one whose symbol has already disappeared — fails
           too. Without that, a satisfied retirement and a violated one would
           look identical from here, which is the failure mode this whole
           two-list shape exists to prevent.

Comments are excluded: explaining why something was retired is the
documentation we want, not the thing forbidden. A code line that must
legitimately name a retired symbol — a negative test proving it is rejected —
carries `retired-architecture-ok: <reason>` on itself or the line above.
"""
import re
import subprocess
import sys

ROOTS = ["core/src", "core/tests", "core/benches", "cli/src", "cli/tests", "services",
         "console/src", "console/application/src", "console/tui/src", "console/web/src",
         "sdk", "fuzz/fuzz_targets"]

# Documentation describes the architecture, so a retired *name* surviving in a
# guide is the architecture surviving in the one place a reader trusts most.
# The CHANGELOG is excluded deliberately: explaining what was retired is the
# documentation we want, not the thing forbidden.
DOC_ROOTS = ["docs", "proto/specs", "README.md", "ROADMAP.md", "RELEASE_NOTES.md",
             "CONTRIBUTING.md", "SUPPORT.md"]
COMMENT = re.compile(r"^\s*(//|/\*|\*)")
MARKER = "retired-architecture-ok"

RETIRED = [
    (r"pck_", "the retired Change id prefix (now chg_)"),
    (r"\bPackId\b", "the retired Change id type"),
    (r"\bPackStore\b", "the retired Change content store"),
    (r"\bPackManifest\b", "the retired Change manifest"),
    (r"\bPackRevision\b", "the retired revision record"),
    (r"\bPackLockfile\b", "the retired Change lockfile"),
    (r"\bPackWorkspace\b", "the retired Change workspace"),
    (r"\bPackLifecycle\b", "the retired revision state"),
    (r"\bObjectPackIndex\b", "the retired object segment index"),
    (r"\bObjectPackEntry\b", "the retired object segment entry"),
    (r"\bFileGuard\b", "the retired advisory lock (ProcessFileLock replaces it)"),
    (r"\bEditSession\b", "the retired edit session (Workspace replaces it)"),
    (r"\bEditCommitResult\b", "the retired edit commit result"),
    (r"LedgerIdentity::Workspace", "the retired project-directory-as-workspace ledger"),
    (r"\.draft/editor/", "the retired editor path (now .draft/workspaces/)"),
    (r"\.draft/packs/", "the retired Change content path (now .draft/changes/)"),
    (r"\.draft/objects/packs/", "the retired object segment path (now objects/segments/)"),
    (r"completeness_proof", "the omitted v1 coverage field (an undefined proof is not a proof)"),
    (r"\bstable_head\b", "the retired accepted-state chain (a Baseline replaces it)"),
    (r"\bStableHead\b", "the retired accepted-state record (BaselineRecord replaces it)"),
    # The prose spelling too. The identifier patterns above never caught the
    # label a person actually reads, so `draft maintenance gc` printed "Stable
    # head valid" over a field named `accepted_baseline_valid` — the retired
    # model surviving in the one place it is most visible.
    (r"[Ss]table [Hh]ead", "the retired accepted-state vocabulary in user-facing text (say Baseline)"),
    (r"PublicationAttempted", "the misleading event name (PublicationDispatchCommitted replaces it)"),
    (r"events\.jsonl", "the retired Activity filename (events.log is the sole authoritative file)"),
    (r"\bTrustLedger\b", "the retired ledger facade (Activity + receipt issuance replace it)"),
    (r"\bEventLog\b", "the retired JSONL event ledger (ActivityLog replaces it)"),
    (r"\bWorkspaceEventLog\b", "the retired project event-log handle (ProjectActivity replaces it)"),
    (r"\bReceiptStore\b", "the retired receipt store (ReceiptEnvelopeStore replaces it)"),
    (r"\bReceiptRecord\b", "the retired receipt record (ReceiptEnvelope replaces it)"),
    (r"\bActionReceiptDraft\b", "the retired local-action receipt (v1 receipts attest promotions and publications)"),
    (r"events/event\.log", "the retired Activity filename (events/events.log is the sole authoritative file)"),
    (r"audit\.jsonl", "the retired global audit filename (the framed Activity format replaces it)"),
    (r"trust::event", "the retired event module (core::activity owns the ledger)"),
    (r"trust::ledger", "the retired ledger module (core::activity and core::receipt own it)"),
    (r"\bStableGraphIndex\b", "the retired index name (ChangeGraphIndex replaces it)"),
    (r"stable-graph\.json", "the retired index filename (index/change-graph.json replaces it)"),
    (r"\bDraftpackHeader\b", "the retired Change-oriented pack header (DraftpackEnvelope replaces it)"),
    (r"draftpack-provenance", "the retired pack provenance member (the signed manifest carries it)"),
]

# The subset of RETIRED that is a *name*, not an implementation detail, and so
# is equally wrong in prose. Deliberately narrower than RETIRED: documentation
# may legitimately say "pack" in the ordinary English sense.
RETIRED_IN_DOCS = [
    (r"pck_", "the retired Change id prefix (now chg_)"),
    (r"\bstable_head\b", "the retired accepted-state chain (a Baseline replaces it)"),
    (r"\bStableHead\b", "the retired accepted-state record (BaselineRecord replaces it)"),
    (r"[Ss]table [Hh]ead", "the retired accepted-state vocabulary (say Baseline)"),
    (r"\bPackWorkspace\b|\bPackManifest\b|\bPackLifecycle\b|\bPackRevision\b|\bPackStore\b|\bPackId\b",
     "a retired Pack type"),
    (r"\bEditSession\b", "the retired edit session (a Workspace replaces it)"),
    (r"\.draft/editor/", "the retired editor path (now .draft/workspaces/)"),
    (r"\.draft/packs/", "the retired Change content path (now .draft/changes/)"),
    (r"\.draft/objects/packs/", "the retired object segment path (now objects/segments/)"),
    (r"events\.jsonl|events/event\.log", "the retired Activity filename (events/events.log)"),
    (r"audit\.jsonl", "the retired global audit filename"),
    (r"PublicationAttempted", "the misleading event name (PublicationDispatchCommitted)"),
    (r"completeness_proof", "the omitted v1 coverage field"),
    (r"draft maintenance prune", "the removed command"),
    (r"draft change delete", "the removed command (abandon/reopen replace it)"),
    (r"PackCreated|PackVerified|PackApproved|PackSubmitted", "a retired receipt event type"),
    (r"[Ss]table graph index|stable-graph\.json", "the retired index name (index/change-graph.json)"),
]

OWED = [
    # SnapshotId still mints chk_. Not, as previously recorded, because a
    # snapshot is an obs_ Observation waiting to be unified — it is not. It
    # names the artifact rollback reads back, and the DCG acceptance path
    # derives its own run_ id and never reads this one. The family retires with
    # the checkpoint/rollback mechanism it serves, whose DCG replacement is
    # Baseline lineage.
    (r"SnapshotId, \"chk_\"", "retiring the checkpoint/rollback mechanism"),
]

_files = {}


def _lines(path):
    if path not in _files:
        try:
            _files[path] = open(path, encoding="utf-8").read().splitlines()
        except OSError:
            _files[path] = []
    return _files[path]


def hits(pattern, roots=None):
    try:
        result = subprocess.run(
            ["rg", "--no-heading", "-n", pattern, *(roots or ROOTS),
             "--glob", "!**/node_modules/**", "--glob", "!**/dist/**"],
            capture_output=True, text=True)
    except FileNotFoundError:
        sys.exit("ripgrep (rg) is required")

    found = []
    for line in result.stdout.splitlines():
        parts = line.split(":", 2)
        if len(parts) < 3:
            continue
        path, number, text = parts[0], int(parts[1]), parts[2]
        if COMMENT.match(text) or MARKER in text:
            continue
        # Prose wraps, so a marker cannot always sit on the immediately
        # preceding line the way it can in code. Look back a short window
        # instead: a paragraph that says "there is deliberately no X" is the
        # documentation this gate exists to produce, not a violation.
        window = _lines(path)[max(0, number - 4):number - 1]
        if any(MARKER in line for line in window):
            continue
        found.append(f"{path}:{number}:{text}")
    return found


def main():
    failed = False

    for pattern, description in RETIRED:
        found = hits(pattern)
        if found:
            failed = True
            print(f"Retired architecture is still reachable as code: {description}",
                  file=sys.stderr)
            for line in found:
                print(f"  {line}", file=sys.stderr)

    for pattern, description in RETIRED_IN_DOCS:
        found = hits(pattern, DOC_ROOTS)
        if found:
            failed = True
            print(f"Retired architecture is still documented as current: {description}",
                  file=sys.stderr)
            for line in found:
                print(f"  {line}", file=sys.stderr)

    for pattern, owner in OWED:
        if not hits(pattern):
            failed = True
            print(f"Stale debt entry: {pattern} is already gone but is still listed as "
                  f"owed by {owner}.", file=sys.stderr)
            print("  Move it from OWED to RETIRED in scripts/check-retired-architecture.py.",
                  file=sys.stderr)

    if failed:
        return 1
    print("Retired architecture is retired; owed retirements are accurately owed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
