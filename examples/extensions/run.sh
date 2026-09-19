#!/usr/bin/env sh
# shellcheck source=../lib.sh
. "$(dirname "$0")/../lib.sh"
# Install a first-party extension and see what it contributes to a Pack.
# Extension-workspace-gated: set DRAFT_EXAMPLE_EXTENSIONS to the path of the
# repository's extensions/packages directory (with a built package).
if [ -z "${DRAFT_EXAMPLE_EXTENSIONS:-}" ]; then
  echo "SKIPPED extensions: set DRAFT_EXAMPLE_EXTENSIONS to run it"
  exit 0
fi
fresh_project
printf 'line one\n' > notes.txt
draft init

step "Install a first-party package; it grants nothing by being installed"
draft extension install "$DRAFT_EXAMPLE_EXTENSIONS/draft.text.document"
draft extension list
draft extension tool list

step "Its evidence and explanations now appear on a Pack"
change_pack="$(draft pack new "edit the notes" --scope notes.txt --json | top id)"
printf 'line one\nline two\n' > notes.txt
revision_pack="$(draft pack revision seal "$change_pack" --json | top id)"
draft pack representation show "$revision_pack"
draft pack evidence run "$revision_pack"
