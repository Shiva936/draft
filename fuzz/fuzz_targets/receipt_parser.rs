#![no_main]
//! Fuzz the signed-receipt envelope parser and its signing-message bytes.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(envelope) = serde_json::from_str::<draft_dcg_contract::receipt::ReceiptEnvelope>(s)
        {
            let _ = envelope.signing_message().signing_bytes();
        }
    }
});
