#!/usr/bin/env sh
# shellcheck source=../lib.sh
. "$(dirname "$0")/../lib.sh"
# An approved RevisionPack becomes accepted state only through Promotion,
# which creates a new Baseline and issues the Promotion receipt.
fresh_project
printf 'v1\n' > app.txt
draft init
declare_passing_check
change_pack="$(draft pack new "edit the app" --scope app.txt --json | top id)"
printf 'v2\n' > app.txt
revision_pack="$(draft pack revision seal "$change_pack" --json | top id)"
set -- $(govern "$revision_pack")
decision="$1"
gate="$2"

step "Before: the accepted Baseline"
draft baseline show

step "Promote the exact RevisionPack the decision approved"
draft promote "$change_pack" "$revision_pack" --gate "$gate" --decision "$decision"

step "After: a new Baseline, its receipt, and the ChangePack completed"
draft baseline show
draft baseline receipts
draft pack show "$change_pack"
draft activity list
