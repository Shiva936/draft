#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
core="$root_dir/core/src"
# The Core module layering.
#
# `allowed_for` is the target architecture. `owed_for` names the edges that do
# not hold yet, each with the stage that removes it.
#
# Listing them is deliberate. The alternative — widening `allowed_for` until the
# check passes — makes a violated layering indistinguishable from a satisfied
# one, and the debt invisible. Here a known edge is tolerated by name, a new one
# fails, and removing a debt entry is how a stage proves it finished.
domains=(support contracts project installation dcg activity trust extension execution recovery \
         receipt task provenance authority evidence gate promotion publication \
         read_model app)

module_pattern='app|activity|authority|contracts|dcg|evidence|execution|extension|gate|installation|project|promotion|provenance|publication|read_model|recovery|receipt|support|task|trust'

allowed_for() {
  case "$1" in
    support) echo "support" ;;
    contracts) echo "contracts support" ;;
    # `project` owns where a project is stored. It must reach neither
    # `extension` nor `trust`: the platform's base cannot depend on what
    # happens to be installed, or on trust evaluation that sits above it.
    project) echo "project contracts support" ;;
    # The installation lifecycle manages the program, never a project: it
    # reaches `project` only for the global store's ownership marker (purge),
    # and nothing from dcg, promotion, publication, receipt or activity.
    installation) echo "installation project contracts support" ;;
    dcg) echo "dcg project contracts support" ;;
    activity) echo "activity dcg project contracts support" ;;
    trust) echo "trust dcg project contracts support" ;;
    extension) echo "extension dcg project contracts support" ;;
    execution) echo "execution extension dcg project contracts support" ;;
    recovery) echo "recovery execution dcg project contracts support" ;;
    receipt) echo "receipt trust dcg project contracts support" ;;
    task) echo "task dcg project extension contracts support" ;;
    # Derivation provenance names the producer of a derived artifact, so it
    # reaches extension identity; it does not reach the graph itself.
    provenance) echo "provenance extension contracts support" ;;
    # Authority evaluates grants against project security state. It reaches
    # `project` for that state and nothing above it: an authority decision
    # must not depend on what happens to be installed.
    authority) echo "authority project contracts support" ;;
    # Evidence and Assessments are facts about the graph, bound to exact
    # revisions and observations. They read the graph, the execution that
    # produced them, the provenance naming who produced it, and the extension
    # identity that names a producer — and nothing above those.
    evidence) echo "evidence execution provenance extension dcg project contracts support" ;;
    # A gate reads evidence and assessments and decides. It sits above them
    # and below anything that acts on the answer.
    gate) echo "gate evidence authority provenance extension dcg project contracts support" ;;
    # Promotion commits an accepted Baseline. It reads gates, evidence and
    # the graph, and reaches receipt to issue the promotion receipt.
    promotion) echo "promotion receipt gate evidence dcg project contracts support" ;;
    # Publication delivers a promoted Baseline outside Draft. It reaches
    # `promotion` for what was accepted, `receipt` for the external-effect
    # receipt, `authority` and `trust` for the authority the effect happens
    # under, and `execution` for the lease. It does not reach `extension`: what
    # is installed must not decide whether an external effect is permitted.
    publication) echo "publication promotion receipt authority trust execution gate evidence dcg project contracts support" ;;
    read_model) echo "read_model $(printf '%s ' "${domains[@]}" | sed 's/app//')" ;;
    app) echo "${domains[*]}" ;;
  esac
}

# Edges that do not hold yet, as `module:dependency` entries.
#
# Empty, and it stays that way. §2.3's table is the layering, and an entry here
# is a layering violation tolerated by name — useful only while a stage is
# mid-flight, never as a permanent exception. The check below fails on a stale
# entry too, so paying a debt off means deleting its line.
owed_reverse_edges=()

is_owed() {
  local edge="$1"
  local entry
  for entry in "${owed_reverse_edges[@]}"; do
    [ "$entry" = "$edge" ] && return 0
  done
  return 1
}

unused_debt=("${owed_reverse_edges[@]}")
for domain in "${domains[@]}"; do
  [ -d "$core/$domain" ] || continue
  allowed=" $(allowed_for "$domain") "
  while IFS= read -r dependency; do
    [ -z "$dependency" ] && continue
    [ "$dependency" = "$domain" ] && continue
    if [[ "$allowed" != *" $dependency "* ]]; then
      if is_owed "$domain:$dependency"; then
        # Known and named; drop it from the unused list so a debt that has
        # actually been paid off cannot linger here pretending to be needed.
        unused_debt=("${unused_debt[@]/$domain:$dependency}")
        continue
      fi
      echo "Forbidden core dependency: $domain -> $dependency" >&2
      echo "Either remove it, or — if it is genuinely transitional — add it to \
owed_reverse_edges with the stage that removes it." >&2
      exit 1
    fi
  done < <(rg -o "crate::($module_pattern)" \
    "$core/$domain" --glob '*.rs' 2>/dev/null | sed 's/.*crate:://' | sort -u)
done

# A debt entry that no longer matches anything is a layering that has been
# fixed; leaving it listed would hide the next regression behind it.
for entry in "${unused_debt[@]}"; do
  [ -z "$entry" ] && continue
  echo "owed_reverse_edges lists '$entry', which no longer exists. Remove it: a \
stale entry would silently permit that edge coming back." >&2
  exit 1
done

if [ -d "$core/presentation" ] || rg -q 'mod presentation|crate::presentation' "$core"; then
  echo "draft-core must not contain a presentation namespace." >&2
  exit 1
fi
if rg -n 'serde\((alias|untagged)|serde\([^)]*alias' "$core"; then
  echo "Compatibility serde readers are forbidden in canonical v1." >&2
  exit 1
fi
if rg -n '(^|/)(design|theme)\.rs$|pub (struct|enum) [A-Za-z]*(Theme|Color|Spacing|Typography)' "$core"; then
  echo "UI design tokens must not live in draft-core." >&2
  exit 1
fi
if rg -n 'use (draft_agui|draft_tui)|crate::(console|tui)' \
  "$core"/{support,contracts,extension,project,installation,dcg,activity,task,provenance,authority,evidence,gate,promotion,trust,execution,read_model}; then
  echo "Core domains must not import UI or service implementations." >&2
  exit 1
fi
if rg -n '^pub struct [A-Za-z0-9]*(Record|Manifest|Envelope|Store)\b' "$core/app"; then
  echo "Persisted domain models and stores must not be defined under app/." >&2
  exit 1
fi
if rg -n 'serde\(deny_unknown_fields\)|^(pub(\(crate\))? )?struct (ObjectStore|ObjectPack|WorkspaceEvents|WorkspaceEventLog|Scanner|Snapshotter|IgnoreMatcher|ReviewFile|ActionReceiptDraft|VerificationConfig|VerifyFile|RiskConfig)\b|^(pub(\(crate\))? )?enum RiskLevel\b' "$core/app" --glob '*.rs'; then
  echo "Authoritative contracts, storage implementations, and domain scanners must not be defined under app/." >&2
  exit 1
fi

# One implementation per semantic operation. The filesystem observer is Core,
# not an extension, but it gets no private path: everything above it reaches it
# through `ResourceSource`, exactly as it reaches a contributed adapter. A second
# enumeration or restore implementation living above the port is the failure this
# catches — two implementations of one operation will eventually disagree, and
# the receipt would still claim they had not.
if rg -n 'Scanner::new|\.scanner\b' "$core" --glob '*.rs' | rg -v '^[^:]*dcg/(snapshot|filesystem_source)\.rs:'; then
  echo "The filesystem scanner is adapter-private; observe through ResourceSource instead." >&2
  exit 1
fi
if rg -n 'fn apply_restore_state|fn restore_[a-z_]*_from_anchor' "$core" --glob '*.rs' \
  | rg -v '^[^:]*dcg/filesystem_source\.rs:'; then
  echo "Anchor material becomes state only inside the owning adapter's restore." >&2
  exit 1
fi

# One appender, and one converter.
#
# Lower layers persist domain audit facts carrying a preallocated event id;
# exactly one place turns a fact into an Activity event and appends it. An
# append that happens *before* its mutation commits records something that may
# not have happened, and one that happens after, outside a transaction, is lost
# on the crash in between — so this is the rule that lets the append be the
# drain step of a durable transaction rather than a side effect somebody
# remembered.
#
# `ActivityLog` is the write handle. Everywhere else takes `ActivityReader`, so
# a read path cannot append by accident. The files below are the only ones that
# may hold a writer, and each is here for a stated reason:
#
#   activity/            owns the implementation
#   app/activity.rs      the single converter and the only normal appender
#   app/startup.rs       recovery, which drains what a crash left owed
#   app/promotion.rs     resolves the project's log to hand to the converter
#   app/maintenance.rs   verifies the chain before collecting
#   read_model/          reads and verifies; never appends in production
activity_writers='core/src/activity/|core/src/app/activity\.rs|core/src/app/startup\.rs|core/src/app/promotion\.rs|core/src/app/maintenance\.rs|core/src/read_model/'
if rg -n --glob '*.rs' 'ActivityLog' "$core" | rg -v "$activity_writers"; then
  echo "Only core::activity owns the Activity write handle; everything else reads \
through ActivityReader." >&2
  exit 1
fi
# The payload shape is built in exactly one place, so one kind of fact cannot
# reach the ledger under two different shapes.
if rg -n --glob '*.rs' 'fn payload_of' "$core" | rg -v 'core/src/app/activity\.rs'; then
  echo "The domain-fact-to-Activity-payload conversion belongs to app/activity.rs alone." >&2
  exit 1
fi

echo "Core dependency allowlist and ownership checks passed."
