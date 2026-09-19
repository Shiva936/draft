//! The one Merkle construction every DCG root is built from.
//!
//! Frozen for v1, in full:
//!
//! * leaves are canonically encoded and sorted by the owning root's rule
//!   **before** they arrive here — ordering is the caller's contract, hashing
//!   is this module's;
//! * leaves are grouped into fixed [`MERKLE_CHUNK_SIZE`] chunks, and each chunk
//!   is hashed under the `.leaf` domain;
//! * chunk hashes are combined pairwise under the `.node` domain, with an odd
//!   node promoted unchanged to the next level;
//! * the final root binds the **leaf count** alongside the tree hash, so two
//!   different shapes can never produce one root;
//! * an empty collection has its own defined constant under the `.empty`
//!   domain, rather than a zero hash that could collide with a real one.
//!
//! Leaf and interior hashes live in different domains, which is what stops a
//! interior node's bytes from being replayed as a leaf.

use crate::digest::{domain_hash, Digest};

/// How many canonical leaf encodings are grouped into one leaf chunk.
pub const MERKLE_CHUNK_SIZE: usize = 256;

/// The root of a canonically ordered leaf sequence.
///
/// `domain` is the owning root's frozen separator; the `.leaf`, `.node` and
/// `.empty` sub-domains are derived from it here so no caller can pick them
/// inconsistently.
pub fn merkle_root(domain: &str, leaves: &[Vec<u8>]) -> Digest {
    if leaves.is_empty() {
        return empty_root(domain);
    }

    let mut level: Vec<Digest> = leaves
        .chunks(MERKLE_CHUNK_SIZE)
        .map(|chunk| {
            let fields: Vec<&[u8]> = chunk.iter().map(Vec::as_slice).collect();
            domain_hash(&format!("{domain}.leaf"), fields)
        })
        .collect();

    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        for pair in level.chunks(2) {
            match pair {
                [left, right] => next.push(domain_hash(
                    &format!("{domain}.node"),
                    [left.as_str().as_bytes(), right.as_str().as_bytes()],
                )),
                // An odd node is promoted unchanged. Safe because the leaf
                // count is bound into the root below, so a promoted node can
                // never make two different shapes agree.
                [only] => next.push(only.clone()),
                _ => unreachable!("chunks(2) yields one or two elements"),
            }
        }
        level = next;
    }

    domain_hash(
        domain,
        [
            (leaves.len() as u64).to_be_bytes().as_slice(),
            level[0].as_str().as_bytes(),
        ],
    )
}

/// The defined root of an empty collection.
pub fn empty_root(domain: &str) -> Digest {
    domain_hash(&format!("{domain}.empty"), std::iter::empty::<&[u8]>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves(count: usize) -> Vec<Vec<u8>> {
        (0..count)
            .map(|i| format!("leaf-{i}").into_bytes())
            .collect()
    }

    #[test]
    fn an_empty_collection_has_its_own_defined_root() {
        let empty = merkle_root("draft.test/v1", &[]);
        assert_eq!(empty, empty_root("draft.test/v1"));
        // Distinct from a collection holding one empty leaf.
        assert_ne!(empty, merkle_root("draft.test/v1", &[Vec::new()]));
    }

    #[test]
    fn the_root_is_deterministic() {
        assert_eq!(
            merkle_root("draft.test/v1", &leaves(1000)),
            merkle_root("draft.test/v1", &leaves(1000))
        );
    }

    #[test]
    fn changing_any_leaf_changes_the_root() {
        let base = merkle_root("draft.test/v1", &leaves(500));
        let mut altered = leaves(500);
        altered[499] = b"tampered".to_vec();
        assert_ne!(base, merkle_root("draft.test/v1", &altered));
        let mut altered = leaves(500);
        altered[0] = b"tampered".to_vec();
        assert_ne!(base, merkle_root("draft.test/v1", &altered));
    }

    #[test]
    fn reordering_leaves_changes_the_root() {
        // Ordering is the caller's contract; this proves it is enforced by the
        // hash rather than merely assumed.
        let ordered = leaves(10);
        let mut swapped = ordered.clone();
        swapped.swap(0, 9);
        assert_ne!(
            merkle_root("draft.test/v1", &ordered),
            merkle_root("draft.test/v1", &swapped)
        );
    }

    #[test]
    fn the_leaf_count_is_bound_so_promotion_cannot_alias() {
        // An odd level promotes a node unchanged. Binding the count is what
        // stops two different leaf counts reaching the same root.
        for count in [1, 2, 3, 255, 256, 257, 512, 513] {
            let root = merkle_root("draft.test/v1", &leaves(count));
            for other in [1, 2, 3, 255, 256, 257, 512, 513] {
                if other != count {
                    assert_ne!(root, merkle_root("draft.test/v1", &leaves(other)));
                }
            }
        }
    }

    #[test]
    fn chunk_boundaries_do_not_alias() {
        let below = merkle_root("draft.test/v1", &leaves(MERKLE_CHUNK_SIZE - 1));
        let at = merkle_root("draft.test/v1", &leaves(MERKLE_CHUNK_SIZE));
        let above = merkle_root("draft.test/v1", &leaves(MERKLE_CHUNK_SIZE + 1));
        assert_ne!(below, at);
        assert_ne!(at, above);
    }

    #[test]
    fn the_domain_partitions_roots_of_identical_content() {
        assert_ne!(
            merkle_root("draft.dcg.project-state-root/v1", &leaves(4)),
            merkle_root("draft.dcg.state-evidence-root/v1", &leaves(4))
        );
        assert_ne!(
            empty_root("draft.dcg.project-state-root/v1"),
            empty_root("draft.dcg.coverage-evidence-root/v1")
        );
    }
}
