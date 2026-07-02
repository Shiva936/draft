# Support

Draft is pre-1.0 open-source software. Support is community-oriented and best-effort unless a separate agreement says otherwise.

## Where To Start

- Read [README.md](README.md) for the project overview and quick start.
- Read [docs/README.md](docs/README.md) for topic documentation.
- Check [docs/faq.md](docs/faq.md) for common questions.
- Use [SECURITY.md](SECURITY.md) for suspected vulnerabilities.

## Good Issue Reports

Include:

- Draft version or commit;
- operating system and shell;
- exact command run;
- relevant config snippets with secrets removed;
- expected behavior;
- actual behavior and error output;
- whether `.draft/`, receipts, rollback, hooks, or event verification were involved.

## Boundaries

Draft v0.3.1 is local-first. It does not provide hosted collaboration, remote synchronization, pull requests, deployment, or native VCS operations. Hook commands may call external tools, but those commands are user-owned shell behavior.

