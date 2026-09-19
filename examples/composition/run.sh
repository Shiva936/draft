#!/usr/bin/env sh
# shellcheck source=../lib.sh
. "$(dirname "$0")/../lib.sh"
# Several ChangePacks against one Baseline: independent, conflicting, and how
# composition refuses to guess.
fresh_project
printf 'v1\n' > app.txt
printf 'd1\n' > docs.txt
draft init

one="$(draft pack new "edit the app" --scope app.txt --json | top id)"
printf 'v2\n' > app.txt
draft pack revision seal "$one" > /dev/null

two="$(draft pack new "edit the docs" --scope docs.txt --json | top id)"
printf 'd2\n' > docs.txt
draft pack revision seal "$two" > /dev/null

three="$(draft pack new "edit the app again" --scope app.txt --json | top id)"
printf 'v3\n' > app.txt
draft pack revision seal "$three" > /dev/null

step "Disjoint work from the same Baseline is independent"
draft pack compare "$one" "$two"

step "Work over the same Resource conflicts, and the Resource is named"
draft pack compare "$one" "$three"
draft pack conflicts "$one"

step "Lineage, not proximity"
draft pack depends "$one"

step "A set composes only if every pair is independent"
draft pack compose "$one" "$two" || true
draft pack disperse "$one" "$two" "$three"
