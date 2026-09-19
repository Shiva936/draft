#![no_main]
//! Fuzz the derived explanation of a revision.
//!
//! A representation is contributed data that Core stores, bounds and reads two
//! structured things out of — conflict claims and review units — without ever
//! interpreting the payload. That makes its parser the one place a hostile or
//! merely broken producer reaches Core's own logic, so it is fuzzed rather than
//! trusted.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(bundle) = serde_json::from_str::<
            draft_core::evidence::representation::RevisionPackRepresentationBundle,
        >(text)
        {
            // Sealing recomputes the bundle's own identity, and the claim
            // algebra reads the conflict claims. Neither may panic on anything
            // that parsed.
            let sealed = bundle.clone().seal();
            let _ = sealed.metrics();
            for representation in &bundle.representations {
                let _ = draft_core::dcg::representation::reconcile(
                    &representation.conflict_claims,
                    &representation.conflict_claims,
                );
            }
        }
    }
});
