# Verification

Verification runs local commands and records results as Draft evidence.

## Running Verification

```bash
draft verify -p <ChangePack-id-or-name>
```

Draft loads verification configuration, runs the selected checks from the workspace root, captures stdout, stderr, exit code, timing, and stores a verification receipt.

## Result States

Verification can be:

- passed;
- failed;
- skipped;
- errored.

Policy can block save when verification is missing or failed.

## Configuration

Verification profile selection is configured through Draft config and verification files. Commands are local shell commands. Draft does not infer semantics from the command name; it only records the result.

## Evidence Links

Verification results attach to the ChangePack. Save receipts include enough references to reconstruct why a ChangePack was allowed or blocked.

## Good Checks

Good verification commands are deterministic, local, and scoped to the change. They should return non-zero on failure and keep output concise enough for review.

## Failure Handling

When verification fails:

1. inspect the verification receipt;
2. inspect stdout and stderr objects if needed;
3. fix the workspace;
4. create or update the ChangePack;
5. run verification again.
