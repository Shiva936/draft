# Finalization

Finalization converts reviewed Draft changes into a provider-native object. It replaces the v0.1.0 "commit" model internally; `draft commit` is the compatibility command (ADR 0002).

## Flow
```
draft commit
  → core::App rescans changes, applies latest review decisions
  → gather risk + latest verification + conflicts
  → create a pre-finalization checkpoint (if supported)
  → policy gates (review/risk/verification/conflict)
  → provider.prepare_finalization → provider.finalize
  → write a receipt + append FinalizationPlanned/FinalizationCompleted
```

## Policy (`.draft/config.toml`, `[finalization]`)
- `require_review` — all included changes must be approved
- `require_verification` / `allow_unverified`
- `block_on_high_risk` / `allow_high_risk_with_confirmation`

Blocked gates produce a structured error (CLI exit code 2) explaining what failed and how to proceed. Defaults preserve v0.1.0 ergonomics (review/verification not strictly required; high risk blocks unless confirmed).
