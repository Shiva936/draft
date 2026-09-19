#!/usr/bin/env bash
set -euo pipefail

# Keep the operational counter registry honest about what it can actually see.
#
# `Counter::observed()` returns None for a counter nothing emits, so an operator
# is never shown a `0` that means "not implemented". That promise is only worth
# anything if `EMITTED` matches reality, and a hand-maintained list drifts the
# moment somebody adds or removes a call site. So the list is checked both ways:
#
#   * a counter in EMITTED with no call site  -> it would report 0 forever
#   * a call site for a counter not in EMITTED -> its increments are invisible
#
# And now a third way. The frozen §2.57 vocabulary is complete and every name in
# it is wired to a real boundary, so a counter *missing* from EMITTED is a
# regression rather than honest incompleteness. Success therefore means 46/46:
# every frozen metric declared, named exactly, and attached to a production site.
#
# "Production" is enforced rather than assumed: a call site inside a `#[cfg(test)]`
# module does not count, or a test could satisfy this gate while the boundary it
# describes emitted nothing in a real run.

cd "$(dirname "$0")/.."
registry=core/src/support/telemetry.rs

python3 - "$registry" <<'PY'
import os, re, sys

registry = sys.argv[1]
source = open(registry).read()

def block(name):
    match = re.search(rf"pub const {name}: &\[Counter\] = &\[(.*?)\];", source, re.S)
    if not match:
        sys.exit(f"{registry}: no {name} list")
    return [m.group(1) for m in re.finditer(r"Counter::(\w+)", match.group(1))]

declared = block("ALL")
claimed = set(block("EMITTED"))

names = re.search(r"const NAMES: \[&str; ALL\.len\(\)\] = \[(.*?)\];", source, re.S)
if not names:
    sys.exit(f"{registry}: no NAMES table")
frozen = re.findall(r'"([a-z0-9_]+)"', names.group(1))

if len(declared) != len(frozen):
    sys.exit(f"{registry}: {len(declared)} counters but {len(frozen)} names")

# The frozen §2.57 vocabulary, spelled out. A rename is a breaking operational
# change, and a dashboard written against a name that quietly moved is worse
# than one written against a name that disappeared.
EXPECTED = {
    "project_control_cas_conflicts", "provider_binding_cas_conflicts",
    "change_pack_definition_cas_conflicts", "publication_control_cas_conflicts",
    "publication_registry_conflicts", "publication_outcome_conflicts",
    "publication_resolution_head_conflicts", "publication_outcome_recovery_fast_forwards",
    "publication_abandoned_before_dispatch", "publication_attempt_number_gaps",
    "publication_allocation_cas_failures", "publication_abandon_prepared_recoveries",
    "publication_journal_transition_conflicts", "publication_late_result_observations",
    "publication_route_staleness_refusals", "immutable_fact_integrity_violations",
    "publication_self_consistency_rejections", "publication_authority_linearization_refusals",
    "task_record_cas_conflicts", "process_lock_wait_seconds", "stale_lease_fence_rejections",
    "activity_append_contention", "activity_torn_tail_truncations", "activity_hard_corruptions",
    "activity_chain_verify_failures", "mutation_journal_abandoned", "mutation_journal_recovered",
    "promotion_recovery_finalizations", "promotion_change_pack_completion_recoveries",
    "promotion_inconsistent_states", "publication_indeterminate_total",
    "publication_no_effect_total", "publication_reconciliation_total",
    "publication_unsafe_retry_authorizations", "publication_inconsistent_states",
    "observation_digest_mismatches", "observation_run_digest_mismatches",
    "publication_digest_mismatches", "publication_attempt_digest_mismatches",
    "security_fact_digest_mismatches", "coverage_evidence_verify_failures",
    "semantics_contract_conflicts", "gc_objects_marked", "gc_objects_collected",
    "gc_recovery_roots_preserved", "read_model_stale_rejections",
}
if set(frozen) != EXPECTED:
    missing = sorted(EXPECTED - set(frozen))
    extra = sorted(set(frozen) - EXPECTED)
    sys.exit(
        "the frozen §2.57 vocabulary has drifted; "
        f"missing: {missing}; unexpected: {extra}"
    )

# Production call sites: everything outside a test module, the registry itself
# and the integration-test trees.
#
# Two shapes count, and both are real emission boundaries:
#
#   Counter::X.increment() / .add()   the direct site
#   .counting_conflicts_as(Counter::X)  a Store declaring which counter its own
#                                       guarded compare-exchange increments
#
# The second is not a shortcut. `RevisionedRecordStore` increments it inside the
# critical section that lost the exchange, which is the one place that can tell
# a stale caller from ordinary work — and the Store is where the counter's
# identity is known.
DIRECT = re.compile(r"Counter::(\w+)\s*\.\s*(?:increment|add)\s*\(")
DECLARED_AT_STORE = re.compile(r"counting_conflicts_as\s*\(\s*Counter::(\w+)")

emitting = set()
for root in ("core/src", "cli/src", "console", "services"):
    for directory, _, files in os.walk(root):
        if "/tests" in directory or "node_modules" in directory:
            continue
        for name in files:
            if not name.endswith(".rs"):
                continue
            path = os.path.join(directory, name)
            if os.path.abspath(path) == os.path.abspath(registry):
                continue
            text = open(path).read()
            # Everything from the first `#[cfg(test)]` on is test code. Coarse
            # and deliberately so: a counter emitted only below that marker is
            # a counter production never touches.
            marker = text.find("#[cfg(test)]")
            if marker != -1:
                text = text[:marker]
            emitting |= {m.group(1) for m in DIRECT.finditer(text)}
            emitting |= {m.group(1) for m in DECLARED_AT_STORE.finditer(text)}

# The lock-wait helper is the registry's only indirection, so its counter counts
# as emitted wherever the helper is called.
for root in ("core/src", "services", "cli/src"):
    for directory, _, files in os.walk(root):
        if "node_modules" in directory:
            continue
        for name in files:
            if name.endswith(".rs") and "record_lock_wait(" in open(
                os.path.join(directory, name)
            ).read():
                emitting.add("ProcessLockWaitMicros")

unknown = emitting - set(declared)
if unknown:
    sys.exit("counters incremented but not declared: " + ", ".join(sorted(unknown)))

stale = sorted(claimed - emitting)
if stale:
    sys.exit(
        "EMITTED claims counters no production path increments, so they would "
        "report 0 forever instead of unknown: " + ", ".join(stale)
    )

invisible = sorted(emitting - claimed)
if invisible:
    sys.exit(
        "counters are incremented but missing from EMITTED, so their values "
        "read as unknown: " + ", ".join(invisible)
    )

unwired = sorted(set(declared) - emitting)
if unwired:
    sys.exit(
        "the frozen vocabulary is fully implemented, so every counter must be "
        "attached to a production boundary; these are not: " + ", ".join(unwired)
    )

print(
    f"Telemetry: {len(declared)} frozen counters, all named exactly and all "
    f"attached to a production emission boundary."
)
PY
