# target.local

`target.local` is Draft v0.3.0’s only optional external execution hook.

## Behavior

Draft runs `target.local` only after:

1. the changepack exists;
2. required verification has passed or policy allows it;
3. required approval is present;
4. save policy allows the action;
5. the save candidate does not contain `.draft/`.

If any check fails, Draft records the failure and does not run the command.

## Configuration

```bash
draft config set target.local "printf %s {message} > .last-draft-save"
```

`{message}` is replaced with a shell-quoted rendered save message. The command runs from the workspace root.

## Receipt Capture

The save receipt records:

- command hash;
- shell;
- working directory;
- exit code;
- stdout object reference;
- stderr object reference;
- start and end times;
- failure reason when applicable.

The command text itself is not the identity of the save. The hash and captured result are the durable audit fields.

## Failure

If `target.local` exits non-zero, Draft writes a failed save receipt and emits `SaveFailed`.

## Security

Draft does not sandbox `target.local`. Configure it as carefully as any local shell command. Do not assume Draft understands what the command does.

## Reserved Keys

`target.remote` is reserved and rejected in v0.3.0.
