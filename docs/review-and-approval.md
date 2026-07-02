# Review And Approval

Draft separates review from final approval so users can inspect evidence before save.

## Review

```bash
draft review -p <ChangePack>
draft review -p <ChangePack> --comment "looks good"
draft review -p <ChangePack> --tui
```

Review records that the ChangePack entered the human review boundary. The optional TUI opens the Review Cockpit.

## Approval And Rejection

```bash
draft approve -p <ChangePack> --reason "verified locally"
draft reject -p <ChangePack> --reason "needs changes"
```

Default policy requires approval before save. High-risk changes require human approval when that policy is enabled.

## What To Check

- Changed files and patch content.
- Verification output and evidence.
- Risk findings.
- Policy blockers.
- Receipts and prior decisions.

Approval is local Draft metadata. It is not a hosted code review approval.

