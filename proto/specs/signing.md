# Signing Protocol

Draft signs canonical receipt payloads with the active local actor key when a
signing identity is available. Verification re-derives canonical bytes, resolves
the public key, checks revocation, and validates event-chain linkage.
