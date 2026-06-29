# Identity

Draft tracks **who or what** performed an action via `ActorRef` (human / agent / service / unknown).

Resolution order: workspace `.draft/identity.json` → user-global `~/.config/draft/identity.toml` → environment (`$USER`) → `Unknown` (never crashes). Operations and receipts embed the actor. Service actions are attributed to a `service` actor.

Providers own their own committer identity (e.g. Git's `user.name`/`user.email`); Draft identity records who ran Draft and is the extension point for richer provider-identity mapping later.
