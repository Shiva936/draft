#![no_main]
//! Fuzz the Publication family's self-consistency validation.
//!
//! A digest proves bytes are unchanged. It proves nothing about whether the
//! derived fields inside them agree with their own canonical inputs, which is
//! why §2.46 recomputes every one of them on parse. That recomputation is the
//! code an attacker reaches by handing Draft a well-formed, self-consistently
//! corrupted object, so it is fuzzed on the parse path rather than assumed
//! total.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(publication) =
        serde_json::from_str::<draft_dcg_contract::publication::Publication>(text)
    {
        // Both must be total: `validate` recomputes the request and
        // idempotency keys, and `digest` refuses to hash an incoherent object.
        let _ = publication.validate();
        let _ = publication.digest();
    }
    if let Ok(attempt) =
        serde_json::from_str::<draft_dcg_contract::publication::PublicationAttempt>(text)
    {
        let _ = attempt.digest();
        let _ = attempt.reference();
    }
    if let Ok(outcome) =
        serde_json::from_str::<draft_dcg_contract::publication::PublicationOutcome>(text)
    {
        let _ = outcome.digest();
    }
    if let Ok(authorization) = serde_json::from_str::<
        draft_dcg_contract::publication::PublicationRetryAuthorization,
    >(text)
    {
        let _ = authorization.validate();
        let _ = authorization.digest();
    }
});
