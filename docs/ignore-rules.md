# Draft Ignore Rules

Draft uses `.draft/.ignore` for Draft-specific scan exclusions.

## Important Boundary

`.draft/` is always excluded. This is a hard rule implemented by the scanner and save guards, not just a line in the ignore file.

Draft ignore rules are not imported from other tools. Draft scans the workspace directly and applies only Draft rules plus the hard `.draft/` exclusion.

## Commands

```bash
draft ignore add "notes/"
draft ignore remove "notes/"
draft ignore list
```

Rules are stored as plain lines. Blank lines and comments are ignored.

## Matching

Draft supports straightforward path-prefix and file-pattern matching suitable for local scan filtering. Negated rules can re-include a path unless the path is under `.draft/`, which can never be re-included.

Use forward slashes in rules. Draft normalizes platform path separators before matching.

## Review Guidance

Keep ignore rules narrow. Ignoring broad paths can hide files from ChangePacks, verification, and rollback planning. When in doubt, leave files visible and let review or policy decide whether they belong in a ChangePack.
