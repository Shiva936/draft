#!/usr/bin/env sh
# shellcheck source=../lib.sh
. "$(dirname "$0")/../lib.sh"
# Baseline → Publication → provider. Provider-gated: set
# DRAFT_EXAMPLE_PROVIDER_DIR to a directory holding semantics.json,
# definition.json and profile.json for the filesystem provider. Credentials,
# where a provider needs any, come from environment variables only.
if [ -z "${DRAFT_EXAMPLE_PROVIDER_DIR:-}" ]; then
  echo "SKIPPED publication: set DRAFT_EXAMPLE_PROVIDER_DIR to run it"
  exit 0
fi
provider="$DRAFT_EXAMPLE_PROVIDER_DIR"
fresh_project
printf 'v1\n' > app.txt
draft init
declare_passing_check
change_pack="$(draft pack new "edit the app" --scope app.txt --json | top id)"
printf 'v2\n' > app.txt
revision_pack="$(draft pack revision seal "$change_pack" --json | top id)"
set -- $(govern "$revision_pack")
draft promote "$change_pack" "$revision_pack" --gate "$2" --decision "$1"

step "Bind the provider, grant publication authority, publish the Baseline"
draft project provider bind files --semantics "$provider/semantics.json" --definition "$provider/definition.json" --profile "$provider/profile.json"
draft authority grant --capability draft.publish/v1
draft baseline publish run
draft baseline publish list
draft baseline publications
