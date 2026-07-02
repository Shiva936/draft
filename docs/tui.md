# Review Cockpit TUI

The TUI is the interactive Review Cockpit for Draft changepacks. It is designed for repeated review work where a user needs to inspect status, evidence, risk, policy, approvals, receipts, and rollback options without leaving Draft.

## Current Capabilities

The TUI layer renders changepack-oriented review cockpit sections and can be launched from CLI review flows. It uses the same core state as the CLI and has a testable terminal renderer.

## Intended Cockpit Panels

The cockpit exposes:

- workspace status and latest scan time;
- changepack list and selected changepack details;
- file changes and overlap indicators;
- verification and save-readiness counts;
- policy blockers;
- decisions;
- approve/reject actions;
- compare and compose actions;
- save readiness and receipt summary;
- rollback action affordance;
- service connection state.

## Daemon Relationship

The TUI must work without a daemon for static review. When `draftd` is available, the TUI can use it for live refresh, background verification, and service status.

## Safety Requirements

The TUI must not hide policy blockers. Save actions must present verification, approval, risk, and `.draft/` candidate state before execution. Any failed save receipt must be visible.
