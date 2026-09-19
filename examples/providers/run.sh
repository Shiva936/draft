#!/usr/bin/env sh
# shellcheck source=../lib.sh
. "$(dirname "$0")/../lib.sh"
# Provider definition, operational profile and binding; unbind and rebind.
# Provider-gated: set DRAFT_EXAMPLE_PROVIDER_DIR (see ../publication).
# Never put a secret in these files — reference an environment variable.
if [ -z "${DRAFT_EXAMPLE_PROVIDER_DIR:-}" ]; then
  echo "SKIPPED providers: set DRAFT_EXAMPLE_PROVIDER_DIR to run it"
  exit 0
fi
provider="$DRAFT_EXAMPLE_PROVIDER_DIR"
fresh_project
draft init

step "Bind: an immutable definition and profile behind a mutable pointer"
binding="$(draft project provider bind files --semantics "$provider/semantics.json" --definition "$provider/definition.json" --profile "$provider/profile.json" --json | top id)"
draft project provider list
draft project provider show "$binding"

step "Unbind stops routing; rebind restores it. Neither rewrites history"
draft project provider unbind "$binding"
draft project provider rebind "$binding"
