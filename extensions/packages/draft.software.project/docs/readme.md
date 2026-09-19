# Software project

Teaches Draft what a software project is.

Everything here is a convention of one domain. Draft manages projects, changes, evidence and approvals without any of it — installing this package adds the software vocabulary on top.

## What it contributes

- **View rules** — excludes another history tool's control directory, build output and installed dependencies from project state. This is the one contribution that changes _what Draft observes_, so adopting it is an audited observation transition with a preview, not a silent reinterpretation.
- **Protections** — registry credential files.
- **Risk rules** — CI, container and breadth-of-change conditions.
- **An intent vocabulary** and **task templates** — the words this domain uses.

## On excluding `.git`

Draft does not know what Git is, and does not need to. Excluding `.git/**` is a statement that an external history tool's control directory is not authored project state — true for this domain, and expressed here as a rule Draft applies rather than a special case Draft contains.
