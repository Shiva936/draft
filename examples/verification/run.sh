#!/usr/bin/env sh
# shellcheck source=../lib.sh
. "$(dirname "$0")/../lib.sh"
# Verification evidence about an exact RevisionPack, from a check the project
# declares itself, plus an explicit local hook.
fresh_project
printf 'v1\n' > app.txt
draft init
change_pack="$(draft pack new "edit the app" --scope app.txt --json | top id)"
printf 'v2\n' > app.txt
revision_pack="$(draft pack revision seal "$change_pack" --json | top id)"

step "With nothing able to check it, evidence is honestly 'unavailable'"
draft pack evidence run "$revision_pack"

step "Declare a project check, and run verification again"
declare_passing_check
draft pack evidence run "$revision_pack"
draft pack evidence list "$revision_pack"

step "A hook is an explicit local command; running it is recorded, never a promotion"
draft config hook set verify "true"
draft config hook run verify
