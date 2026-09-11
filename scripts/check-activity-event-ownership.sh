#!/usr/bin/env bash
set -euo pipefail

# Prove every Activity event has a durable owner.
#
# An event nobody owns is an event nobody writes durably: it gets appended at
# some convenient moment, which is either before the fact it describes is
# committed (recording something that may not have happened) or after, outside
# any transaction (lost on the crash in between).
#
# The mapping itself is a total match in Rust, so the compiler already refuses a
# new event that skips the question. What this gate adds is that the *vocabulary*
# has not drifted from the frozen list, and that the specific ownership
# distinctions the architecture depends on are still made.

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
module="$root_dir/core/src/activity/event.rs"
failed=0

if [ ! -f "$module" ]; then
  echo "core/src/activity/event.rs is missing." >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# 1. The vocabulary is exactly the frozen list.
# ---------------------------------------------------------------------------

if ! python3 - "$module" <<'VOCABULARY_PY'
import re
import sys

FROZEN = """
ProjectCreated BaselineInitialized ProjectClosed
WorkspaceCreated CheckpointCreated
ProviderSemanticDefinitionAdded ProviderOperationalProfileAdded
ProviderBindingAdded ProviderBindingRetargeted ProviderBindingUnbound ProviderBindingRebound
TaskCreated TaskUpdated TaskClosed TaskReopened
ChangeCreated ChangeDefinitionAmended ChangeCompleted ChangeAbandoned ChangeReopened
AuthorityGranted AuthorityRevoked SecurityStateUpdated PolicyUpdated
OperationPlanned OperationExecuted OperationRefused OperationReplanned
ResourceObserved CoverageRecorded RelationDerived StateBearingDeclared
ScopeResolved RevisionSealed
EvidenceProduced AssessmentProduced
ReviewSubmitted DecisionRecorded GateEvaluated GateWaived
LeaseAcquired LeaseReleased LeaseRefused
PromotionPrepared PromotionCommitted PromotionFinalized
PromotionRefused PromotionAbandoned PromotionInconsistent BaselinePromoted
PublicationRequested PublicationDispatchCommitted
PublicationSucceeded PublicationFailed PublicationNoEffect PublicationIndeterminate
PublicationAbandonedBeforeDispatch
PublicationResolved PublicationRetryAuthorized PublicationInconsistent
ReceiptIssued RecoveryPerformed
ExtensionInstalled ExtensionAuthorized ExtensionRevoked
MaintenanceStarted MaintenanceCompleted MaintenanceFailed
""".split()

source = open(sys.argv[1]).read()

# The enum body, so the ALL list and the match arms do not confuse the scan.
body = re.search(r"pub enum EventKind \{(.*?)\n\}", source, re.DOTALL)
if not body:
    raise SystemExit("could not find the EventKind enum")
declared = re.findall(r"^\s{4}([A-Z][A-Za-z]*),", body.group(1), re.MULTILINE)

missing = [name for name in FROZEN if name not in declared]
extra = [name for name in declared if name not in FROZEN]
if missing or extra:
    print("The Activity event vocabulary has drifted from the frozen list.")
    if missing:
        print(f"  missing: {', '.join(missing)}")
    if extra:
        print(f"  unfrozen: {', '.join(extra)}")
    raise SystemExit(1)

if len(declared) != len(FROZEN):
    raise SystemExit(f"expected {len(FROZEN)} events, found {len(declared)}")
VOCABULARY_PY
then
  failed=1
fi

# ---------------------------------------------------------------------------
# 2. There is no `PublicationAttempted`, and no alias reintroducing it.
#
#    The durable Dispatching boundary happens before the external call, so an
#    event named "attempted" there could outlive a crash in which nothing was
#    ever sent — a permanent record making a claim that is false.
#
#    Asserting its absence requires naming it, so assertions do not count.
# ---------------------------------------------------------------------------

if grep -rn --include='*.rs' 'PublicationAttempted' "$root_dir/core/src" "$root_dir/cli/src" \
  "$root_dir/services" 2>/dev/null | grep -vE ':[0-9]+:\s*(//|\*)' | grep -v 'assert'; then
  echo "PublicationAttempted has been reintroduced; the dispatch boundary is committed \
before the external call, so that name would claim something untrue." >&2
  failed=1
fi

# ---------------------------------------------------------------------------
# 3. The mapping is total, and the distinctions the architecture depends on are
#    still made.
#
#    A generic "the Publication journal" would hide that resolution and
#    retry-authorization facts are made durable by their own transactions
#    rather than by a later attempt journal — exactly the confusion that would
#    let one of them be lost. The arms are parsed rather than grepped, because
#    an arm may name several events and a grep cannot tell which owns what.
# ---------------------------------------------------------------------------

if ! python3 - "$module" <<'OWNERSHIP_PY'
import re
import sys

REQUIRED = {
    "PublicationResolved": "ResolutionMutationJournal",
    "PublicationRetryAuthorized": "RetryAuthorizationJournal",
    "PublicationRequested": "PublicationCreationJournal",
    "PublicationDispatchCommitted": "PublicationAttemptJournal",
    "ChangeCompleted": "PromotionJournal",
    "ChangeAbandoned": "RecordMutationJournal",
    # Immutable definitions are independent facts; folding them into the
    # binding's mutation would leave one created but never recorded.
    "ProviderSemanticDefinitionAdded": "FactCreationJournal",
    "ProviderOperationalProfileAdded": "FactCreationJournal",
    "ProviderBindingRetargeted": "RecordMutationJournal",
    "ProjectCreated": "InitializationJournal",
}

source = open(sys.argv[1]).read()
body = re.search(r"pub fn ownership\(self\) -> EventOwnership \{(.*?)\n    \}", source, re.DOTALL)
if not body:
    raise SystemExit("could not find the ownership mapping")
body = body.group(1)

if re.search(r"^\s*_\s*=>", body, re.MULTILINE):
    raise SystemExit(
        "the ownership mapping has a catch-all arm; it must stay total so the compiler "
        "refuses a new event that names no owner"
    )

# Each arm is one or more `Self::Name` patterns, then `=> (Fact::X, Journal::Y)`.
owner_of = {}
for arm in re.finditer(
    r"((?:\s*Self::[A-Za-z]+\s*\|?)+)=>\s*\{?\s*\(Fact::([A-Za-z]+),\s*Journal::([A-Za-z]+)\)",
    body,
):
    events = re.findall(r"Self::([A-Za-z]+)", arm.group(1))
    for event in events:
        owner_of[event] = (arm.group(2), arm.group(3))

problems = []
for event, mechanism in REQUIRED.items():
    actual = owner_of.get(event)
    if actual is None:
        problems.append(f"{event} has no parsed ownership arm")
    elif actual[1] != mechanism:
        problems.append(f"{event} is owned by {actual[1]}, expected {mechanism}")

if problems:
    print("Activity event ownership has drifted:")
    for problem in problems:
        print(f"  - {problem}")
    raise SystemExit(1)

# Every event in the enum must appear in the mapping.
enum_body = re.search(r"pub enum EventKind \{(.*?)\n\}", source, re.DOTALL).group(1)
declared = set(re.findall(r"^\s{4}([A-Z][A-Za-z]*),", enum_body, re.MULTILINE))
unowned = sorted(declared - set(owner_of))
if unowned:
    print("These events have no ownership arm: " + ", ".join(unowned))
    raise SystemExit(1)
OWNERSHIP_PY
then
  failed=1
fi

if [ "$failed" -ne 0 ]; then
  exit 1
fi
echo "Every Activity event has a named AuditFact owner and journal mechanism."
