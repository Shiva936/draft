# Review model

`draft review` opens a review session over the current change groups and records decisions per change: **approved / rejected / deferred / needs-changes**.

- Sessions persist under `.draft/reviews/review_<id>.json` and link decisions to the acting `ActorRef`.
- Starting a review appends `ReviewStarted`; each decision appends `ReviewDecisionRecorded`.
- Finalization can require approval via `[finalization].require_review`.

`draft review --yes` approves all groups non-interactively (useful for agents and CI). Without `--yes` and attached to a terminal, the TUI launches.
