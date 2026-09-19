# Signing Protocol

Draft signs the canonical `ReceiptSigningMessage` — the receipt payload and the signer binding together, framed under a frozen domain separator — with the active local actor key. The framing is what stops those bytes doubling as any other domain-separated hash input in Draft.

Verification re-derives the signed bytes from the stored envelope, resolves the public key, checks revocation, and reports the signature and the two trust questions separately. What cannot be determined reads `unknown`, never `valid`.
