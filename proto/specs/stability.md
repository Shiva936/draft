# Stability Protocol

`stable_head` points to the latest verified stable base state. It is not a branch. It may advance only after successful project-state verification.

Stable-head metadata is compact and does not duplicate pack payloads. Submitted pack manifests, immutable revisions, evidence, and trust records remain in their canonical domain stores; only mutable staging is disposed.
