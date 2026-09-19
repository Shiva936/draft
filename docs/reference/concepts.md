# Concepts

Draft is organized around the Draft Change Graph. See [The Draft Change Graph](dcg.md) for the model itself; this page is the working vocabulary.

## Project

A Draft project is a directory with a `.draft/` store, identified by a `prj_` id. Draft metadata is private project state and is always excluded from change candidates.

A **ChangePack workspace** (`wsp_`) is a different thing with a similar name: the mutable staging area one ChangePack's edits accumulate in before they are sealed into a revision. The project is where the graph lives; a ChangePack workspace is where one piece of work is in progress.

## Checkpoints

A checkpoint records a snapshot of the project's observed content, plus a recovery anchor for each resource captured under the same live fencing. Later ChangePacks compare against a snapshot.

Create a checkpoint before risky edits, long agent sessions, large refactors, or local experiments:

```bash
draft pack checkpoint "before agent run"
```

Draft returns a `chk_` id and records a `CheckpointCreated` Activity event. A checkpoint is a Draft snapshot, not a VCS commit, and either the snapshot or the event that recorded it is a recovery target:

```bash
draft recover run chk_<id>
draft recover run evt_<id>
```

Recovery restores project files toward the snapshot and protects `.draft/`. It deletes, so a plan names what would be removed before anything runs.

## Packs

Draft's primary work concept is a **Pack**.

A **ChangePack** (`cpk_`) is a project-local governable work lineage. A **RevisionPack** (`rpk_`) is an immutable exact revision of a ChangePack.

Evidence, assessments, reviews, gates and decisions bind local work to an exact RevisionPack.

**Promotion is the only operation that changes accepted Draft state**, by creating a new Baseline.

**Publication** is a separate, optional external effect performed from a Baseline.

Those are the only two members of the Pack family. A ChangePack's definition says what it is for and what it may touch; each RevisionPack is sealed against it and never changes afterwards — sealing identical content again converges on the same `rpk_`, and different content can never reuse one.

### Purpose And Lifecycle

A ChangePack lets a person or agent collect a delta, seal it as an exact revision, attach evidence, record what it was judged to mean, and — once a Decision cites a satisfied Gate — promote it into an accepted Baseline with a signed receipt.

1. Create a checkpoint.
2. Edit the workspace.
3. Create a ChangePack from the working tree.
4. Attach evidence through task execution or verification.
5. Run risk and policy checks.
6. Review, comment, and decide.
7. Promote, once an approving Decision cites a satisfied Gate.
8. Use receipts and Activity for audit, and checkpoints for recovery.

### Create And Inspect

```bash
draft pack checkpoint "before work"
# edit files
draft pack new "parser cleanup" --scope src/parser.rs
draft pack revision seal <cpk-id>
draft pack list
```

ChangePack IDs use the `cpk_` prefix; a sealed revision uses `rpk_`.

The review flow is:

```bash
draft pack evidence run <rpk-id>
draft pack assess <rpk-id> --risk low
draft pack review <rpk-id> --comment "read it"
draft pack gates evaluate <rpk-id>
draft pack decide <rpk-id> --approve
draft promote <cpk-id> <rpk-id>
```

### Lifecycle

```bash
draft pack abandon <cpk-id>
draft pack reopen <cpk-id>
```

A ChangePack's lifecycle — active, completed, abandoned — is a separate question from how far through review any one of its revisions has got. Abandoning stops future work and retains everything already recorded; there is no delete.

### Manifest And Patch Data

A ChangePack manifest includes:

- schema version;
- ChangePack ID and name;
- status and task ID;
- base snapshot ID;
- change-set, evidence, verification, risk, decision, and receipt references;
- source ChangePack IDs for composed ChangePacks;
- actor and timestamps.

The change set records, for each resource, the states it moved between and the neutral aspects of that move — `added`, `removed`, `content_changed`, `metadata_changed`, `relocated`, `form_changed`, `attributes_changed`. Anything Draft could not determine is recorded separately as a derivation gap, never folded in with the changes. Draft always rejects `.draft/` in change candidates. A ChangePack's lifecycle is `Active → Completed | Abandoned`, with `Abandoned → Active` on reopen; how far through review any one of its revisions has got is a separate question. A promotion disposes mutable staging after durable finalization while retaining the immutable ChangePack and its history.

### Review Guidance

Reviewers should inspect:

- files changed;
- binary or deletion risk;
- evidence and verification output;
- policy blockers;
- comments and prior decisions;
- the promotion receipt after completion.

ChangePacks are local Draft records, not commits, pull requests, hosted reviews, or merge requests. Hooks may call external tools, but Draft does not model those tools natively and a hook is never a promotion.

## Candidates, Tasks, And Executions

Candidates and tasks connect user intent, local execution profiles, and ChangePack provenance. Candidates are named execution profiles; they do not represent people, permissions, or hosted roles.

### Candidates

```bash
draft pack candidate add cli-helper --kind command -- "cargo test"
draft pack candidate list
draft pack candidate show cli-helper
draft pack candidate remove cli-helper
```

### Tasks

A task is a local record with an ID, title, optional description, actor, risk profile, optional linked issue text, status, and creation time. Tasks can link instructions, candidate profiles, ChangePacks, and evidence.

```bash
draft task spawn "agent edit" -c cli-helper -- "update parser error handling"
draft task list
draft task
```

A stored task can be spawned without repeating its instruction. An inline task requires text after `--`. Tasks can be linked to ChangePacks with `-p <cpk-id>` or through later review context.

### Executions

An execution records an opaque candidate command with its shell, working directory, status, timestamps, stdout and stderr object references, exit code, and linked task or ChangePack. Command candidates run in an isolated copy of the project and can produce a ChangePack against the accepted Baseline. Manual candidates queue a record for human work instead.

```bash
draft task spawn "agent edit" -c <candidate-name> -- <instruction>
```

An execution may produce file changes, while the ChangePack captures the final delta. Keeping both lets reviewers see the result and the process that produced it. Acceptability is still decided through ChangePack review, verification, risk, policy, and approval.

## Evidence

Evidence is durable context attached to tasks, executions, verification, and ChangePacks.

### Sources And Captured Data

Draft records evidence from:

- task and candidate execution provenance;
- `draft pack evidence run` command executions and summaries;
- risk and policy outputs;
- promotion receipts and hook results.

Command evidence captures the command string or hash, shell, working directory, start and end times, exit code, stdout and stderr object references, and the related ChangePack or execution ID. Large payloads are stored as objects and referenced by hash.

Evidence should answer what was run, where and when it ran, what it returned, and how it affected the gate. Reviewers should treat missing evidence as a policy concern; the default policy requires verification.

Command output may include secrets, paths, source snippets, or machine details. Treat `.draft/` as sensitive when evidence was captured from private workspaces.

### What The Evidence Actually Speaks For

Evidence existing is not the same as a Resource being proved. `draft pack coverage` reports each touched Resource as `direct`, `indirect` or `uncovered`, and is deliberately hard to satisfy: **direct** means evidence read an observation of that exact Resource, and **indirect** means somebody asserted it — a declared coverage relationship or a producer attestation — followed exactly one hop, so every indirect answer traces to a single assertion a person can read and disagree with.

Same directory, imported by, adjacent in the graph, reachable from something tested and named similarly are each rejected. Every one of them would produce a confident `covered` for a Resource nothing has ever verified, and the distance between "related to something tested" and "tested" is the distance between a passing review and an outage.

## Representations

A representation is the derived explanation of one sealed revision: what changed, where, and what a reader has to weigh. It is recorded when the revision is sealed, from the same observations the revision was sealed over, and binds that exact revision — two revisions can carry the same material change while differing in everything a reviewer weighed.

Each touched Resource names the strategy that explains it. Where a contributed presentation claims the Resource it is named; otherwise the neutral rendering applies, which always exists and no extension contributes. The neutral rendering says exactly what Core can justify — which Resource changed, between which two authoritative state digests, and a whole-Resource conflict claim, because Draft cannot say where inside a Resource the work landed.

It does not diff content, parse a payload or interpret a domain. Doing any of that would make Core the semantic authority for every kind of Resource, which is the coupling the contribution model exists to avoid.

## Comparing And Composing ChangePacks

Draft's model for reasoning about two ChangePacks at once. `draft pack compare` answers it for a pair; `draft pack conflicts` answers it for one ChangePack against every other; `draft pack compose` and `draft pack disperse` answer it for a set.

Comparison reports the resources each ChangePack touches, the resources both touch, a compatibility summary, and how the two interfere where they do.

Whether two changes to one resource are composable depends on what is installed. With no comparison capability, Draft knows _that_ both ChangePacks changed the resource and cannot establish that their changes are separable — so it refuses, and says so. With a comparison installed, each change claims a region of a contributed coordinate space or a key of a contributed key space, and claims that do not overlap are provably independent.

Three relations are reported, and they are not synonyms. `Independent` means Draft established separability. `Conflicting` means the claims collide. `Indeterminate` means Draft cannot relate them at all — two incomparable coordinate spaces, or a claim on one side and silence on the other. Indeterminate fails closed: an unprovable separation is not a separation.

Composition is over sealed _revisions_, not ChangePacks: a ChangePack is an intention that can be resealed, and composing intentions would let a reseal change what a composition claimed without the composition moving. It holds only when every pair is independent **and** every member was sealed from the same Baseline — disjoint Resource sets prove nothing across different starting points.

Draft refuses interfering revisions instead of guessing how to merge them. `disperse` is the inverse and is not a mutation: it reports which members already stand alone, and names the relation holding each one that does not.

## Activity And Receipts

Draft stores framed, hash-chained records in `.draft/events/events.log`, the sole authoritative Activity file. `draft activity list` renders a human-readable timeline and `--raw` prints each logical record. See [Storage And Events](../internals/storage-and-events.md).

A receipt attests exactly one of three things: a Promotion that accepted a Baseline, a publication attempt's primary outcome, or an authorized resolution of one. Local acts that are already immutable facts in their own stores are deliberately not receipted — two records of one act, with no rule for which is authoritative, is worse than one.

## Hooks

A hook is a user-configured shell command. Draft records hook execution but does not treat its contents as native Git, hosting, deployment, or remote behavior. See [Configuration](configuration.md#hooks).

## Remaining

Nothing in the model is left without a surface in v0.3.4. Where a question is deliberately answered more weakly than a reader might expect — indirect proof coverage, the neutral representation's whole-Resource claim — the command says so rather than the surface being absent.
