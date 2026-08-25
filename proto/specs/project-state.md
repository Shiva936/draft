# Project State Protocol

Project stability is verified at the whole-project level. Pack validity does not imply project stability. A project state is stable only after verification produces a `ProjectStateVerified` receipt.

The transmitted project-state report is a registered, self-describing contract
containing its schema version, resulting workspace hash, named check results,
and aggregate pass/fail result. In v0.3.4 it supports schema version `1`.
