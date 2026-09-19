#!/usr/bin/env bash
set -euo pipefail

# Assert the frozen lock partial order, and that the checker enforcing it
# implements the right rule.
#
# The rule is about what is HELD:
#
#     on acquiring lock X: every correctness lock currently held has order < X
#
# and deliberately NOT "an operation's acquisitions increase over its lifetime".
# The distinction decides whether correct code passes. This is legal, because
# nothing higher is still held when the second phase begins:
#
#     acquire 6 -> acquire 9 -> release 9 -> release 6 -> acquire 6 -> acquire 7
#
# A chronological-monotonicity check would reject that, and it is the ordinary
# shape of publication completion. So this gate verifies the ranks, and defers
# the behaviour to the runtime enforcement, whose tests exercise exactly the
# sequences that separate the two readings.

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
module="$root_dir/core/src/support/lock_order.rs"
failed=0

if [ ! -f "$module" ]; then
  echo "core/src/support/lock_order.rs is missing." >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# 1. The ranks are exactly the frozen table, in order, with no gaps.
# ---------------------------------------------------------------------------

if ! python3 - "$module" <<'RANKS_PY'
import re
import sys

FROZEN = [
    ("TrustReadFence", 1),
    ("ProjectControlLease", 2),
    ("PublicationLease", 3),
    ("ProjectControlStore", 4),
    ("ProviderBindingStore", 5),
    ("PublicationJournalStore", 6),
    ("PublicationControlStore", 7),
    ("DomainRecordStore", 8),
    ("PublicationAuxiliary", 9),
    ("ActivityLedger", 10),
]

source = open(sys.argv[1]).read()
declared = re.findall(r"^\s*([A-Z][A-Za-z]*)\s*=\s*(\d+)\s*,", source, re.MULTILINE)
declared = [(name, int(rank)) for name, rank in declared]

if declared != FROZEN:
    print("The lock order does not match the frozen table.")
    print(f"  frozen:   {FROZEN}")
    print(f"  declared: {declared}")
    raise SystemExit(1)
RANKS_PY
then
  failed=1
fi

# ---------------------------------------------------------------------------
# 2. The checker compares against the HELD set, not against a previous
#    acquisition. A `last`/`max`-style comparison would be the monotonicity
#    reading, and would reject legal phase boundaries.
# ---------------------------------------------------------------------------

if ! grep -q 'held.iter().find(|entry| entry.rank() > order.rank())' "$module"; then
  echo "The lock-order check no longer compares against every currently-held lock." >&2
  failed=1
fi
if grep -nE '\bheld\.(last|iter\(\)\.max)\b' "$module"; then
  echo "The lock-order check looks like chronological monotonicity rather than the \
held-lock rule; the legal sequence 6 -> 9 -> release -> 6 -> 7 must pass." >&2
  failed=1
fi

# ---------------------------------------------------------------------------
# 3. Ranks 8 and 9 hold several distinct records, so at most one of each may be
#    held: two records of equal rank have no order between them.
# ---------------------------------------------------------------------------

if ! grep -q 'Self::DomainRecordStore | Self::PublicationAuxiliary' "$module"; then
  echo "Ranks 8 and 9 are no longer exclusive within their rank." >&2
  failed=1
fi

# ---------------------------------------------------------------------------
# 4. The order is checked before the lock is taken, so a reverse acquisition is
#    reported as an ordering fault rather than surfacing as a timeout.
# ---------------------------------------------------------------------------

lock_module="$root_dir/core/src/support/process_lock.rs"
if ! grep -q 'let order = order.map(lock_order::enter).transpose()?;' "$lock_module"; then
  echo "process_lock no longer checks the order before acquiring." >&2
  failed=1
fi

# ---------------------------------------------------------------------------
# 5. The sequences that separate the two readings of the rule stay tested.
# ---------------------------------------------------------------------------

for behaviour in \
  release_and_reacquire_across_a_phase_boundary_is_legal \
  a_lease_may_not_be_held_while_taking_the_trust_fence \
  a_group_nine_lock_may_not_reach_back_to_publication_control \
  only_one_lock_of_an_exclusive_rank_is_held_at_a_time; do
  if ! grep -q "fn $behaviour" "$module"; then
    echo "The lock-order test '$behaviour' has been removed; it is what distinguishes \
the held-lock rule from chronological monotonicity." >&2
    failed=1
  fi
done

if [ "$failed" -ne 0 ]; then
  exit 1
fi
echo "Lock order checks passed."
