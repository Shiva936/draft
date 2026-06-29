# ADR 0005 — Git as the first reference provider

**Status:** Accepted (v0.2.0)

## Context
We need one complete, production-quality provider to validate the abstraction and preserve v0.1.0 workflows.

## Decision
`providers/git` is the complete reference implementation. All Git execution goes through a single command wrapper (`providers/git/command.rs`) using structured argument arrays (no shell strings). Git checkpoints use non-destructive `git stash create` snapshots; finalization creates a real commit; undo uses a safe `git reset --soft HEAD^`. Experimental providers (fs, jj, mercurial, pijul) ship as scaffolds with declared capabilities and structured unsupported errors.

## Consequences
- The full v0.1.0 Git flow works end to end through the provider interface.
- Non-Git providers are clearly marked experimental and never block Git.
