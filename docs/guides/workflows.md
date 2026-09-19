# Workflows

Draft is designed to make agent-scale changes reviewable before a person accepts them.

## Recommended Flow

```bash
draft pack checkpoint "before agent run"
draft task spawn "agent edit" -- <agent command>
draft status
draft pack new "agent change" --scope <res-id>
draft pack revision seal <cpk-id>
draft pack evidence run <rpk-id>
draft pack assess <rpk-id> --risk low
draft pack gates evaluate <rpk-id>
draft pack decide <rpk-id> --approve
draft promote <cpk-id> <rpk-id>
```

## Why Spawn Through Draft

`draft task spawn` records task intent, candidate links, optional ChangePack links, and instruction text. Command candidates run in an isolated workspace copy and can produce reviewable ChangePacks; manual candidates remain queued for human edits. This gives reviewers context for what the agent was asked to do and evidence about its execution before changes are reviewed.

## Reading What A ChangePack Did

Three commands answer three different questions about a sealed revision, and none of them guesses:

```bash
draft pack representation show <rpk-id>   # how the work is explained
draft pack impact <rpk-id>                # what it reaches
draft pack coverage <rpk-id>              # what has actually been proved
```

`representation` is recorded when the revision is sealed, from the same observations it was sealed over. `impact` reports only what an authorized extractor found — layout, dependency edges and name similarity produce nothing. `coverage` reports a Resource as covered only where evidence read an observation of that exact Resource; everything that merely _looks_ like coverage is rejected, because a confident "covered" for something nothing has verified is worse than no answer.

Before promoting more than one ChangePack:

```bash
draft pack conflicts <cpk-id>
draft pack compose <cpk-id> <cpk-id>
draft pack disperse <cpk-id> <cpk-id>
```

## Review Checklist For Agent ChangePacks

- Inspect every changed file.
- Read captured stdout and stderr when available.
- Run verification locally.
- Check risk findings for broad, binary, deletion, or sensitive-path changes.
- Require human approval for high-risk changes.
- Promote only after the gate is satisfied and somebody has decided.

## Failed Runs

A failed agent command can still produce useful evidence. Keep the run record, inspect the project, and decide whether to open a ChangePack or recover to a checkpoint.

## Software domain: integrating with an external history tool

Everything in this section is _software-domain_ usage, not part of Draft's model. Draft does not know what Git is; it invokes nothing implicitly. What follows is a pattern for wiring an external history tool in through hooks you configure.

Excluding an external tool's control directory from project state is likewise a _view rule_, contributed by `draft.software.project`. Adopting it is an audited observation change with a preview — not something Draft assumes.

### Pattern: Draft Verifies, Git Records

Draft gates the change; a hook you configure records it wherever you keep history:

```toml
[hooks.verify]
kind = "raw"
command = "git add -A && git commit -m \"{{message}}\""
```

Flow:

```text
draft pack new -> draft pack revision seal -> draft pack evidence run ->
draft pack gates evaluate -> draft pack decide --approve ->
draft promote -> the Baseline advances -> your hook mirrors the change into Git
```

A hook is never a promotion. What the project accepts is changed by `draft promote`, and only by an approving Decision citing a satisfied Gate over the exact revision. If the hook fails, that is a `HOOK_FAILED` refusal and the work is untouched.

### Pattern: Draft And Git Side By Side

Draft accepts its own Baseline after the gate is satisfied, while a hook mirrors the change into Git history. Draft receipts remain the verification record and Git remains the collaboration surface.

### Notes

- `.draft/` must never be committed; add it to `.gitignore`. Draft itself hard-excludes `.draft/` from ChangePacks, digests, and change candidates.
- Hook template variables such as `{{message}}`, `{{title}}`, `{{change_pack_id}}`, and `{{receipt_id}}` are documented in [Configuration](../reference/configuration.md#placeholders).
- `draft recover run evt_<id>` touches project files only; Git history is unaffected.

## Draft-only workflows

Draft works with no external history tool at all. Accepted Baselines are then the project's own record of what it agreed to — which is the ordinary case for any project that is not a software repository.

### The Draft-Only Loop

```text
draft init                            # create .draft/ and the initial Baseline
draft pack checkpoint <name>        # capture a state you can come back to
# ... edit files ...
draft pack new <intent> --scope …  # declare what the work is and may touch
draft pack revision seal <cpk-id>            # observe and seal it as a revision
draft pack evidence run <rpk-id> # run the project's checks
draft pack assess <rpk-id> --risk … # judge what the evidence means
draft pack gates evaluate <rpk-id>  # check every required condition
draft pack decide <rpk-id> --approve
draft promote <cpk-id> <rpk-id>       # the only command that advances the Baseline
```

Each promotion accepts a new Baseline. `draft activity list` and `draft baseline receipts` show the tamper-evident history, while canonical manifests, revisions and evidence are retained.

### Recovering State

- `draft recover run chk_<id>` restores a checkpoint.
- `draft recover run evt_<id>` restores the state the named Activity event recorded a checkpoint of.
- `draft recover run cpk_<id>` works while mutable staging retains a recovery snapshot; afterward, recover to a checkpoint instead.

### Maintenance And Exit

- `draft maintenance gc` prunes rebuildable caches, orphaned staging, and temporary metadata, rebuilds indexes, and validates the accepted Baseline.
- `draft maintenance remove-project` removes `.draft/` without touching project files; it refuses if unsafe pending state exists (`--force` overrides after a clear warning).

Draft-only workflows are fully offline: no remote server or registry is contacted. See [Protocol Contracts](../internals/protocol.md).
