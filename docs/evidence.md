# Evidence

Evidence is durable context attached to tasks, runs, verification, and changepacks.

## Evidence Sources

Draft records evidence from:

- `draft spawn` command executions;
- `draft verify` command executions;
- generated verification summaries;
- risk and policy outputs;
- save receipts and hook results.

## Captured Data

Command evidence captures:

- command string or command hash;
- shell;
- working directory;
- start and end times;
- exit code;
- stdout object reference;
- stderr object reference;
- related changepack or run id.

Large payloads are stored as objects and referenced by hash.

## Evidence And Review

Evidence should answer: what was run, where it ran, when it ran, what it returned, and how it affected save readiness.

Reviewers should treat missing evidence as a policy concern. The default save policy requires verification before save.

## Privacy

Command output may include secrets, paths, source snippets, or machine details. Treat `.draft/` as sensitive if evidence was captured from private workspaces.
