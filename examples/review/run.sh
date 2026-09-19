#!/usr/bin/env sh
# shellcheck source=../lib.sh
. "$(dirname "$0")/../lib.sh"
# Evidence → representation → assessment → review → gates → decision, each a
# separate immutable fact bound to one exact RevisionPack. None of these local
# acts issues a receipt: receipts attest a Promotion or a Publication.
fresh_project
printf 'v1\n' > app.txt
draft init
declare_passing_check
change_pack="$(draft pack new "edit the app" --scope app.txt --json | top id)"
printf 'v2\n' > app.txt
revision_pack="$(draft pack revision seal "$change_pack" --json | top id)"

step "Evidence, and the derived explanation of what the RevisionPack did"
draft pack evidence run "$revision_pack"
draft pack representation list
draft pack representation show "$revision_pack"

step "A judgement of risk, and a record that somebody looked"
draft pack assess "$revision_pack" --risk low --rationale "small, reviewed by hand"
draft pack review "$revision_pack" --comment "reads well"

step "The gate, then the decision that cites it"
gate="$(draft pack gates evaluate "$revision_pack" --json | top id)"
draft pack gates list "$change_pack" "$revision_pack"
draft pack decide "$revision_pack" --approve --gate "$gate"

step "Deciding authorizes; nothing was accepted, and no receipt exists yet"
draft pack receipts "$change_pack"
