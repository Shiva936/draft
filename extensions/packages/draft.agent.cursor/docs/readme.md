# Cursor

Runs Cursor as a Draft candidate.

## What it can and cannot do

The tool action returns **proposed mutations** and nothing else. Its response schema has no operation id, no attribution and no preconditions — Draft constructs all of those itself, observes the targets, builds the fencing preconditions and applies the plan under lease. An agent proposes; only Draft authors an authority-bearing operation.

Running it requires a `process.execute` grant on _this_ artifact. Authorizing another agent package never authorizes this one: each has its own signature, attestation, authorization and update lineage, which is why they are three packages rather than one.
