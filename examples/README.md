# Examples

Runnable walkthroughs, smallest first. Each one creates a temporary project with its own global store, so running an example never touches your projects or your Draft settings. None of them installs, updates or uninstalls Draft.

The vocabulary: a **ChangePack** (`cpk_`) is a governable work lineage; a **RevisionPack** (`rpk_`) is one immutable, exact revision of it. Decisions authorize, Promotion accepts into a Baseline, and Publication delivers a Baseline elsewhere.

| Example | Class | What it shows |
| --- | --- | --- |
| [Basic](basic/README.md) | local | Open a ChangePack, edit, and seal a RevisionPack. |
| [Multi-resource](multi-resource/README.md) | local | One ChangePack whose scope names several Resources, one of them new. |
| [Verification](verification/README.md) | local | Verification evidence about an exact RevisionPack. |
| [Review](review/README.md) | local | Evidence → representation → assessment → review → gate → decision, each an immutable fact bound to one exact RevisionPack. |
| [Composition](composition/README.md) | local | Three ChangePacks against one Baseline. |
| [Promotion](promotion/README.md) | local | An approved RevisionPack becomes accepted state only through Promotion, which creates a new Baseline, completes the ChangePack and issues the Promotion receipt. |
| [Recovery](recovery/README.md) | local | Checkpoint, make a mess, then plan, preview and restore. |
| [End to end](end-to-end/README.md) | local | The complete local lifecycle in five milestones: open and seal, govern, promote to a Baseline, read Activity and the Promotion receipt, then start the Console and stop it cleanly. |
| [Publication](publication/README.md) | provider-gated | Baseline → Publication → provider. |
| [Providers](providers/README.md) | provider-gated | A binding is a mutable pointer to an immutable semantic definition and operational profile. |
| [Extensions](extensions/README.md) | extension-gated | Install a first-party package from the repository's `extensions/packages` and see its contributions on a Pack. |

```sh
DRAFT_BIN=target/debug/draft sh examples/basic/run.sh
```

**Classes.** _Local_ examples need only the binary. _Provider-gated_ and _extension-gated_ examples print `SKIPPED` unless their environment variable is set (see each README).

**How they are checked.** `cli/tests/examples_e2e.rs` runs every local example against the built binary with `HOME`, `XDG_RUNTIME_DIR` and `DRAFT_GLOBAL_HOME` isolated, requires every example directory to be classified, reports gated examples as skipped, and fails if any example invokes a `draft` command path that is not in `cli/tests/golden/cli-surface.txt`.

[`reference/config.toml`](reference/config.toml) is an annotated project configuration, not a runnable example.
