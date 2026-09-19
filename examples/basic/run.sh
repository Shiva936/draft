#!/usr/bin/env sh
# shellcheck source=../lib.sh
. "$(dirname "$0")/../lib.sh"
# A ChangePack is a governable work lineage; a RevisionPack is one immutable,
# exact revision of it.
fresh_project
printf 'hello\n' > app.txt

step "Initialize the project (its first Baseline accepts app.txt)"
draft init

step "Open a ChangePack: what it is for and what it may touch"
change_pack="$(draft pack new "greet the world" --scope app.txt --json | top id)"
echo "ChangePack: $change_pack"

step "Do the work, then seal it as a RevisionPack"
printf 'hello, world\n' > app.txt
revision_pack="$(draft pack revision seal "$change_pack" --json | top id)"
echo "RevisionPack: $revision_pack"

step "Sealing the same state again converges on the same RevisionPack"
again="$(draft pack revision seal "$change_pack" --json | top id)"
[ "$again" = "$revision_pack" ] || { echo "expected $revision_pack, got $again" >&2; exit 1; }

step "Inspect the ChangePack and its RevisionPacks"
draft pack show "$change_pack"
draft pack revision list "$change_pack"
draft pack revision show "$change_pack" "$revision_pack"
