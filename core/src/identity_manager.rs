use crate::errors::DraftError;
use crate::models::{Identity, IdentitySource, RepoContext};
use crate::git_adapter::{GitAdapter, GitCliAdapter};

pub struct IdentityManager;

impl IdentityManager {
    pub fn current(ctx: &RepoContext) -> Result<Option<Identity>, DraftError> {
        let git = GitCliAdapter::new(ctx.repo_root.clone());
        let name = git.config_get("user.name")?;
        let email = git.config_get("user.email")?;

        match (name, email) {
            (Some(n), Some(e)) => Ok(Some(Identity {
                name: n,
                email: e,
                source: IdentitySource::GitConfig,
            })),
            _ => Ok(None),
        }
    }

    pub fn coauthor_trailer(identity: &Identity) -> String {
        format!("Co-authored-by: {} <{}>", identity.name, identity.email)
    }
}
