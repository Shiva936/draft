#!/usr/bin/env sh
# shellcheck source=../lib.sh
. "$(dirname "$0")/../lib.sh"
# One ChangePack touching several Resources: its scope, what it reaches, and
# what has actually been proved about the exact RevisionPack.
fresh_project
printf 'a\n' > a.txt
printf 'b\n' > b.txt
printf 'c\n' > c.txt
draft init

step "Declare a scope of three Resources, one of them new"
change_pack="$(draft pack new "touch several resources" --scope a.txt b.txt new.txt --json | top id)"

step "Edit two of them and add the new one"
printf 'a2\n' > a.txt
printf 'b2\n' > b.txt
printf 'new\n' > new.txt
revision_pack="$(draft pack revision seal "$change_pack" --json | top id)"

step "The resolved scope, and what this exact RevisionPack touched"
draft pack scope "$change_pack"
draft pack revision show "$change_pack" "$revision_pack"

step "What it reaches, and what evidence covers — nothing is inferred"
draft pack impact "$revision_pack"
draft pack coverage "$revision_pack"

step "Everything recorded about the ChangePack at once"
draft pack inspect "$change_pack"
