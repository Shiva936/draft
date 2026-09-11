//! Format-level limits.
//!
//! These are properties of the *format*, not tuning knobs, which is why they
//! are frozen constants in the portable crate rather than configuration. An
//! importer that raised them would accept archives every other implementation
//! refuses, and the format would no longer mean one thing.
//!
//! Each bounds a specific resource-exhaustion shape rather than being a round
//! number chosen for comfort: a single enormous member, an archive whose
//! members are individually small but collectively enormous, and an archive
//! with a pathological number of tiny members.

/// Largest a single archive member may be, uncompressed.
pub const MAX_ENTRY_BYTES: u64 = 100 * 1024 * 1024;

/// Largest the sum of all members may be, uncompressed.
///
/// Checked *while reading*, never from the archive's own declared sizes: a
/// declared size is attacker-controlled, so trusting it is what a decompression
/// bomb relies on.
pub const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

/// Most members an archive may contain.
pub const MAX_ENTRIES: usize = 20_000;

/// Whether one member's size is within the per-entry limit.
pub fn entry_size_permitted(size: u64) -> bool {
    size <= MAX_ENTRY_BYTES
}

/// Whether a running total stays within the archive-wide limit.
///
/// Saturating, so an overflowing sum can never wrap into an acceptable value.
pub fn total_size_permitted(total_so_far: u64, next: u64) -> bool {
    total_so_far.saturating_add(next) <= MAX_TOTAL_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_per_entry_limit_is_inclusive() {
        assert!(entry_size_permitted(MAX_ENTRY_BYTES));
        assert!(!entry_size_permitted(MAX_ENTRY_BYTES + 1));
    }

    #[test]
    fn the_archive_limit_accumulates() {
        assert!(total_size_permitted(0, MAX_TOTAL_BYTES));
        assert!(!total_size_permitted(1, MAX_TOTAL_BYTES));
        assert!(total_size_permitted(MAX_TOTAL_BYTES - 10, 10));
    }

    #[test]
    fn an_overflowing_total_cannot_wrap_into_acceptance() {
        // The arithmetic an attacker would aim at: two sizes that sum past
        // u64::MAX. Saturating addition makes the answer "no", not "yes".
        assert!(!total_size_permitted(u64::MAX, u64::MAX));
        assert!(!total_size_permitted(u64::MAX - 1, 5));
    }

    #[test]
    fn one_huge_member_and_many_small_ones_are_bounded_separately() {
        // A member inside the per-entry limit can still break the archive
        // limit, which is why both exist.
        assert!(entry_size_permitted(MAX_ENTRY_BYTES));
        assert!(!total_size_permitted(MAX_TOTAL_BYTES, MAX_ENTRY_BYTES));
    }
}
