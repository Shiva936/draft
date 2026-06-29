# Core concepts

- **Workspace** — a Draft-managed directory (`.draft/`), bound to one provider.
- **Provider** — a backend that maps Draft's neutral model to a real VCS (or a plain folder). Providers declare **capabilities**; Draft branches on capabilities, never on provider names.
- **Draft change** — a reviewable unit grouped from a provider-neutral diff. Not necessarily a commit.
- **Operation** — an append-only record of a meaningful Draft action.
- **Risk / verification / review** — pre-finalization gates.
- **Checkpoint** — a provider-delegated snapshot for safety/undo.
- **Finalization** — converting reviewed changes into a provider-native object. `draft commit` is the compatibility command for finalization.
- **Receipt** — a durable record mapping Draft IDs → provider object IDs, with risk/verification/finalization summaries and an undo hint.
- **Actor** — who/what performed an action (human, agent, service, unknown).
