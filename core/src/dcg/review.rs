//! `Review` — the record that someone looked.
//!
//! # Why a review is separate from a decision
//!
//! A review is the act of examining a revision; a decision is the conclusion.
//! They come apart in both directions: a reviewer can read a change and not
//! conclude anything yet, and a decision made without any recorded review is
//! precisely the thing an audit wants to notice.
//!
//! Merging them would make "under review" unrepresentable — there would be no
//! state between untouched and decided, and work in progress would look
//! identical to work nobody had opened.

use draft_dcg_contract::ids::{ActorId, ReviewId, RevisionPackId};
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::Digest;
use serde::{Deserialize, Serialize};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;
use crate::support::immutable_store::ImmutableFactStore;

/// One reviewer's examination of one exact revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    pub id: ReviewId,
    /// The exact revision examined.
    pub revision_pack: RevisionPackId,
    pub reviewer: ActorId,
    pub started_at: Timestamp,
    /// What the reviewer wrote. Opaque to Core.
    #[serde(default)]
    pub comments: Vec<ReviewComment>,
}

/// One comment left during a review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewComment {
    pub author: ActorId,
    pub body: String,
    pub written_at: Timestamp,
}

impl Review {
    pub fn digest(&self) -> DraftResult<Digest> {
        self.validate()?;
        Digest::parse(try_canonical_hash(self)?)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
    }

    pub fn validate(&self) -> DraftResult<()> {
        for comment in &self.comments {
            if comment.body.trim().is_empty() {
                return Err(DraftError::new(
                    DraftErrorKind::Validation,
                    "an empty review comment records nothing",
                ));
            }
        }
        Ok(())
    }

    pub fn covers(&self, revision: &RevisionPackId) -> bool {
        &self.revision_pack == revision
    }
}

/// Create-once storage for reviews.
pub struct ReviewStore {
    facts: ImmutableFactStore<Review>,
}

impl ReviewStore {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            facts: ImmutableFactStore::new(directory),
        }
    }

    pub fn put(&self, review: &Review) -> DraftResult<()> {
        review.validate()?;
        self.facts.put(review.id.as_str(), review)?;
        Ok(())
    }

    pub fn get(&self, id: &ReviewId) -> DraftResult<Option<Review>> {
        self.facts.get(id.as_str())
    }

    /// Every recorded review, in id order.
    pub fn list(&self) -> DraftResult<Vec<Review>> {
        let mut reviews = Vec::new();
        for id in self.facts.list_ids()? {
            if let Some(review) = self.facts.get(&id)? {
                reviews.push(review);
            }
        }
        Ok(reviews)
    }
}
