#!/usr/bin/env sh
# shellcheck source=../lib.sh
. "$(dirname "$0")/../lib.sh"
# Recovery restores a recorded state and proves it. It is not a rollback of
# accepted state: the accepted Baseline only ever moves by Promotion.
fresh_project
printf 'hello\n' > app.txt
draft init

step "Checkpoint, then make a mess"
checkpoint="$(draft pack checkpoint "before experimenting" --json | top snapshot_id)"
printf 'broken\n' > app.txt
printf 'stray\n' > stray.txt

step "Plan, preview, then restore"
draft recover plan "$checkpoint"
draft recover dry-run "$checkpoint"
draft recover run "$checkpoint"

step "The file is back and the stray one is gone"
cat app.txt
[ ! -e stray.txt ] || { echo "stray.txt survived" >&2; exit 1; }
draft baseline show
