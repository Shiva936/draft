# Risk Engine

`draft risk` is deterministic and local. It returns a score, band, policy decision, stable reason codes, hotspots, evidence gaps, and a receipt.

Default rules cover sensitive paths, auth/security files, payments, database migrations, dependency lockfiles, CI/CD files, container files, deleted tests, binary changes, large changes, and missing verification.

`.draft/risk.toml` can tune thresholds and path rules. `--explain` includes factor text. `--include-evidence` includes evidence summaries.
