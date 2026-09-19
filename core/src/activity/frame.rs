//! The physical frame an Activity record is stored in.
//!
//! # Logical record versus physical frame
//!
//! These are deliberately separate, and the distinction is load-bearing:
//!
//! * the **logical record** is the canonical `LedgerRecord`. The chain hash,
//!   the `ActivityEventId` and payload identity are computed over *its*
//!   canonical bytes and nothing else;
//! * the **physical frame** wraps those bytes with a magic marker, a length, a
//!   checksum and a terminator.
//!
//! The framing bytes are not part of event identity. They exist for exactly one
//! job: telling a **torn write** from **corruption**. Getting that distinction
//! right is the difference between recovering cleanly from a crash and quietly
//! rewriting history.
//!
//! ```text
//! magic (8) | format (1) | length (4, BE) | canonical record bytes | checksum (8) | terminator (1)
//! ```
//!
//! # Why the file is not `.jsonl`
//!
//! Because a frame is not a JSON line. A `.jsonl` suffix would promise
//! line-oriented JSON to every tool that met the file, and every one of them
//! would be wrong. `events/events.log` is the sole authoritative path; there is
//! no compatibility reader.
//!
//! # The recovery rule
//!
//! ```text
//! SAFE to truncate — ONLY when the FINAL frame is PHYSICALLY INCOMPLETE:
//!     a partial header; a declared length running past EOF; partial record
//!     bytes; a partial checksum; a missing terminator
//!
//! HARD corruption — NEVER truncated, not even as the last frame:
//!     a physically COMPLETE final frame whose checksum fails, OR any broken
//!     frame that is not the last one
//! ```
//!
//! The asymmetry is the point. A partial frame is a write that did not finish,
//! so nothing was ever committed and discarding it loses nothing. A complete
//! frame with a bad checksum is a record that *was* written and has since been
//! damaged — truncating it would silently delete history to make the file
//! parse, which is precisely the failure a ledger exists to prevent.

use sha2::{Digest as _, Sha256};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// Marks the start of a frame.
pub const FRAME_MAGIC: &[u8; 8] = b"DRFTEVT\x00";
/// The frame layout this build writes and accepts.
pub const FRAME_FORMAT: u8 = 1;
/// Bytes of checksum carried per frame.
pub const CHECKSUM_LENGTH: usize = 8;
/// Ends a frame.
pub const FRAME_TERMINATOR: u8 = b'\n';
/// Largest single record accepted, so a corrupt length cannot request an
/// enormous allocation before the checksum has a chance to fail.
pub const MAX_RECORD_LENGTH: u32 = 16 * 1024 * 1024;

const HEADER_LENGTH: usize = FRAME_MAGIC.len() + 1 + 4;

/// Wrap canonical record bytes in a frame.
pub fn encode_frame(record: &[u8]) -> DraftResult<Vec<u8>> {
    if record.len() as u64 > MAX_RECORD_LENGTH as u64 {
        return Err(DraftError::new(
            DraftErrorKind::Validation,
            format!(
                "activity record is {} bytes, over the {MAX_RECORD_LENGTH} byte frame limit",
                record.len()
            ),
        ));
    }
    let mut frame = Vec::with_capacity(HEADER_LENGTH + record.len() + CHECKSUM_LENGTH + 1);
    frame.extend_from_slice(FRAME_MAGIC);
    frame.push(FRAME_FORMAT);
    frame.extend_from_slice(&(record.len() as u32).to_be_bytes());
    frame.extend_from_slice(record);
    frame.extend_from_slice(&checksum(record));
    frame.push(FRAME_TERMINATOR);
    Ok(frame)
}

fn checksum(record: &[u8]) -> [u8; CHECKSUM_LENGTH] {
    // Detects physical damage. Integrity against a deliberate rewrite is the
    // chain hash's job, over the logical record — not this.
    let digest = Sha256::digest(record);
    let mut truncated = [0u8; CHECKSUM_LENGTH];
    truncated.copy_from_slice(&digest[..CHECKSUM_LENGTH]);
    truncated
}

/// What scanning a log found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameScan {
    /// Every frame parsed and verified.
    Complete { records: Vec<Vec<u8>> },
    /// The final frame is physically incomplete: a write that did not finish.
    ///
    /// Truncating to `valid_length` is safe, because nothing beyond it was ever
    /// committed.
    TornTail {
        records: Vec<Vec<u8>>,
        valid_length: u64,
        reason: String,
    },
    /// A complete frame failed verification, or a frame before the last is
    /// broken. Never truncated; this needs recovery, not repair.
    HardCorruption { offset: u64, reason: String },
}

impl FrameScan {
    /// The records that were verified, whatever else was found.
    pub fn records(&self) -> &[Vec<u8>] {
        match self {
            Self::Complete { records } | Self::TornTail { records, .. } => records,
            Self::HardCorruption { .. } => &[],
        }
    }
}

/// Parse every frame in `bytes`, classifying whatever stops the scan.
pub fn scan(bytes: &[u8]) -> FrameScan {
    let mut records = Vec::new();
    let mut offset = 0usize;

    loop {
        if offset == bytes.len() {
            return FrameScan::Complete { records };
        }
        let remaining = &bytes[offset..];

        // A partial header is a torn write: the frame never got far enough to
        // declare what it was.
        if remaining.len() < HEADER_LENGTH {
            return torn(records, offset, "the final frame has a partial header");
        }
        if &remaining[..FRAME_MAGIC.len()] != FRAME_MAGIC {
            // A wrong magic where a frame should start is not a short write —
            // the bytes are there and they are wrong.
            return FrameScan::HardCorruption {
                offset: offset as u64,
                reason: "frame magic is missing or damaged".into(),
            };
        }
        let format = remaining[FRAME_MAGIC.len()];
        if format != FRAME_FORMAT {
            return FrameScan::HardCorruption {
                offset: offset as u64,
                reason: format!("frame declares unsupported format {format}"),
            };
        }
        let length_at = FRAME_MAGIC.len() + 1;
        let declared = u32::from_be_bytes([
            remaining[length_at],
            remaining[length_at + 1],
            remaining[length_at + 2],
            remaining[length_at + 3],
        ]);
        if declared > MAX_RECORD_LENGTH {
            return FrameScan::HardCorruption {
                offset: offset as u64,
                reason: format!("frame declares {declared} bytes, over the frame limit"),
            };
        }

        let record_at = offset + HEADER_LENGTH;
        let checksum_at = record_at + declared as usize;
        let terminator_at = checksum_at + CHECKSUM_LENGTH;
        let frame_end = terminator_at + 1;

        // A length that runs past what was written is the classic torn tail.
        if frame_end > bytes.len() {
            return torn(
                records,
                offset,
                "the final frame is shorter than its declared length",
            );
        }

        let record = &bytes[record_at..checksum_at];
        let stored = &bytes[checksum_at..terminator_at];
        if stored != checksum(record) {
            // Physically complete and wrong. Truncating here would delete a
            // record that really was committed.
            return FrameScan::HardCorruption {
                offset: offset as u64,
                reason: "a complete frame failed its checksum".into(),
            };
        }
        if bytes[terminator_at] != FRAME_TERMINATOR {
            return FrameScan::HardCorruption {
                offset: offset as u64,
                reason: "a complete frame has a damaged terminator".into(),
            };
        }

        records.push(record.to_vec());
        offset = frame_end;
    }
}

fn torn(records: Vec<Vec<u8>>, offset: usize, reason: &str) -> FrameScan {
    FrameScan::TornTail {
        records,
        valid_length: offset as u64,
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_of(records: &[&[u8]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for record in records {
            bytes.extend_from_slice(&encode_frame(record).unwrap());
        }
        bytes
    }

    fn records(scan: &FrameScan) -> Vec<Vec<u8>> {
        scan.records().to_vec()
    }

    #[test]
    fn an_empty_log_scans_clean() {
        assert_eq!(scan(&[]), FrameScan::Complete { records: vec![] });
    }

    #[test]
    fn well_formed_frames_round_trip_in_order() {
        let bytes = log_of(&[b"first", b"second", b"third"]);
        let scan = scan(&bytes);
        assert!(matches!(scan, FrameScan::Complete { .. }));
        assert_eq!(
            records(&scan),
            vec![b"first".to_vec(), b"second".to_vec(), b"third".to_vec()]
        );
    }

    #[test]
    fn an_empty_record_is_a_valid_frame() {
        // Distinct from no frame at all, so a zero-length payload must not be
        // mistaken for a torn write.
        let scan = scan(&log_of(&[b""]));
        assert_eq!(records(&scan), vec![Vec::<u8>::new()]);
    }

    #[test]
    fn every_truncation_point_of_the_last_frame_is_a_torn_tail() {
        // The exhaustive version of the rule: whatever byte the crash landed
        // on, an unfinished final frame is always safe to discard, and the
        // frames before it always survive.
        let complete = log_of(&[b"committed", b"in-flight"]);
        let first_frame_end = encode_frame(b"committed").unwrap().len();

        for cut in (first_frame_end + 1)..complete.len() {
            match scan(&complete[..cut]) {
                FrameScan::TornTail {
                    records,
                    valid_length,
                    ..
                } => {
                    assert_eq!(
                        records,
                        vec![b"committed".to_vec()],
                        "the committed frame must survive a cut at {cut}"
                    );
                    assert_eq!(
                        valid_length, first_frame_end as u64,
                        "truncation must land on the last complete frame"
                    );
                }
                other => panic!("cut at {cut} classified as {other:?}"),
            }
        }
    }

    #[test]
    fn a_torn_first_frame_leaves_no_records_and_truncates_to_nothing() {
        let bytes = log_of(&[b"only"]);
        let scan = scan(&bytes[..bytes.len() - 1]);
        match scan {
            FrameScan::TornTail {
                records,
                valid_length,
                ..
            } => {
                assert!(records.is_empty());
                assert_eq!(valid_length, 0);
            }
            other => panic!("expected a torn tail, got {other:?}"),
        }
    }

    #[test]
    fn a_complete_final_frame_with_a_bad_checksum_is_never_truncated() {
        // The case the whole classification exists for. These bytes were
        // committed; truncating them to make the file parse would delete
        // history to hide damage.
        let mut bytes = log_of(&[b"committed", b"damaged"]);
        let last = bytes.len();
        // Flip a byte inside the final record, leaving the frame complete.
        bytes[last - CHECKSUM_LENGTH - 2] ^= 0xff;

        match scan(&bytes) {
            FrameScan::HardCorruption { reason, .. } => {
                assert!(reason.contains("checksum"), "{reason}");
            }
            other => panic!("a damaged complete frame must not be truncatable: {other:?}"),
        }
    }

    #[test]
    fn a_damaged_earlier_frame_is_hard_corruption_even_with_a_clean_tail() {
        // Only the *final* frame can ever be a torn write. Damage in the middle
        // cannot be truncated away without discarding everything after it.
        let mut bytes = log_of(&[b"damaged", b"later", b"latest"]);
        bytes[HEADER_LENGTH + 1] ^= 0xff;
        match scan(&bytes) {
            FrameScan::HardCorruption { offset, reason } => {
                assert_eq!(offset, 0);
                assert!(reason.contains("checksum"), "{reason}");
            }
            other => panic!("expected hard corruption, got {other:?}"),
        }
    }

    #[test]
    fn a_damaged_magic_is_corruption_rather_than_a_short_write() {
        // The bytes are present and wrong, which is a different fact from the
        // bytes never having been written.
        let mut bytes = log_of(&[b"first", b"second"]);
        let second = encode_frame(b"first").unwrap().len();
        bytes[second] ^= 0xff;
        match scan(&bytes) {
            FrameScan::HardCorruption { offset, reason } => {
                assert_eq!(offset, second as u64);
                assert!(reason.contains("magic"), "{reason}");
            }
            other => panic!("expected hard corruption, got {other:?}"),
        }
    }

    #[test]
    fn a_damaged_terminator_is_corruption_not_a_torn_tail() {
        // Every other byte of the frame arrived, so the write did finish; the
        // last byte was then damaged.
        let mut bytes = log_of(&[b"record"]);
        let last = bytes.len() - 1;
        bytes[last] = b'X';
        assert!(matches!(scan(&bytes), FrameScan::HardCorruption { .. }));
    }

    #[test]
    fn an_unsupported_format_marker_is_refused_rather_than_guessed_at() {
        let mut bytes = log_of(&[b"record"]);
        bytes[FRAME_MAGIC.len()] = 9;
        match scan(&bytes) {
            FrameScan::HardCorruption { reason, .. } => {
                assert!(reason.contains("format"), "{reason}")
            }
            other => panic!("expected hard corruption, got {other:?}"),
        }
    }

    #[test]
    fn an_absurd_declared_length_is_refused_before_it_is_allocated() {
        // A corrupt length field must not become an allocation request.
        let mut bytes = log_of(&[b"record"]);
        let length_at = FRAME_MAGIC.len() + 1;
        bytes[length_at..length_at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        match scan(&bytes) {
            FrameScan::HardCorruption { reason, .. } => {
                assert!(reason.contains("frame limit"), "{reason}")
            }
            other => panic!("expected hard corruption, got {other:?}"),
        }
    }

    #[test]
    fn a_record_over_the_frame_limit_is_refused_at_write_time() {
        let oversized = vec![0u8; MAX_RECORD_LENGTH as usize + 1];
        assert!(encode_frame(&oversized).is_err());
    }

    #[test]
    fn truncating_to_the_reported_length_yields_a_clean_log() {
        // What recovery actually does, end to end: truncate, then rescan.
        let complete = log_of(&[b"one", b"two"]);
        let torn_bytes = &complete[..complete.len() - 3];
        let FrameScan::TornTail { valid_length, .. } = scan(torn_bytes) else {
            panic!("expected a torn tail");
        };
        let repaired = &torn_bytes[..valid_length as usize];
        assert_eq!(
            scan(repaired),
            FrameScan::Complete {
                records: vec![b"one".to_vec()]
            }
        );
    }

    #[test]
    fn the_frame_carries_no_part_of_the_records_identity() {
        // Two frames of the same record are byte-identical, and the record
        // bytes come back exactly as written — the wrapper adds nothing the
        // chain hash could accidentally depend on.
        assert_eq!(
            encode_frame(b"record").unwrap(),
            encode_frame(b"record").unwrap()
        );
        let scan = scan(&log_of(&[b"record"]));
        assert_eq!(records(&scan), vec![b"record".to_vec()]);
    }
}
