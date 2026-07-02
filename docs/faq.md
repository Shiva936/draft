# FAQ

## Does Draft Replace Git?

No. Draft is a local review and safety layer. It does not replace Git, Jujutsu, CI, editors, agents, deployment systems, or code hosts.

## Does Draft Create Commits Or Pull Requests?

No. Draft has no native commit, push, pull request, merge request, publish, or hosted review command. A user-owned hook can run any local shell command, but Draft treats that as opaque hook execution.

## Where Does Draft Store Data?

Draft stores local metadata under `.draft/`. Treat it as sensitive because it can contain file content, command output, evidence, receipts, and event history.

## What Is The Difference Between `draft event` And `draft event --raw`?

`draft event` renders a readable timeline from stored event records. `draft event --raw` prints the underlying JSONL event envelopes. Draft stores only the raw event stream.

## Can I Use Draft Offline?

Yes. Core CLI flows are local and do not require a network service.

## Is The Daemon Required?

No. The CLI works without `draftd`. Service crates support optional local background and live flows.

## What Should I Do Before Risky Work?

Run `draft checkpoint "before work"` so you have a clear rollback target.

