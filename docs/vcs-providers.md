# VCS providers

A provider maps Draft's neutral model to a real version-control system (or a plain folder). Providers implement two traits in `core::vcs`:

- `VcsProvider` — `id`, `name`, `detect`, `open`, `capabilities`.
- `VcsRepository` — `current_view`, `status`, `diff`, `ignore_rules`, `conflicts`, `create_checkpoint`, `restore_checkpoint`, `prepare_finalization`, `finalize`, `undo_provider_action`.

## Capabilities
Providers declare capabilities (staging area, operation log, change ids, patch identity, local checkpoints, history rewrite, remote publish, ...). Core branches on capabilities, never on provider names. See `draft provider list`.

## Detection & selection
Each provider reports a confidence (`Exact > High > Medium > Low > None`). The registry picks the highest; equal top confidences yield a structured `ProviderAmbiguous` error. The filesystem provider matches any directory at `Low` confidence as a fallback.

## Status
| Provider | Status |
| --- | --- |
| git | complete reference provider |
| fs | experimental, limited (status scan only; no finalization) |
| jj / mercurial / pijul | experimental scaffolds (detection + capabilities) |

Experimental providers return structured `UnsupportedOperation` errors for operations they do not implement. They are **not** production-ready.
