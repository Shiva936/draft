# Receipt Protocol

Draft v0.3.3 receipts are canonical attestations for trust-relevant operations.
Receipt IDs use `rcp_<id>`.

Required stable fields:

- `receipt_id`
- `receipt_type`
- `base_before`
- `base_after`
- `stable_head_before`
- `stable_head_after`
- `pack_digest`
- `pack_summary`
- `composition_hash`
- `workspace_hash`
- `verification_result`
- `hook_results`
- `save_mode`
- `timestamp`
- `signer`
- `event_hash`
- `previous_event_hash`

The canonical signing/hash payload excludes signature bytes and all human CLI
formatting.
