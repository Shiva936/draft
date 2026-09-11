//! Draft API compatibility.
//!
//! A manifest states the Draft API it accepts as a SemVer requirement. Taking
//! the offered version as a parameter rather than reading it from Draft is what
//! lets publishing tooling outside the Draft repository answer "would this
//! package install on Draft 0.3.4?" without linking Draft at all.

/// Whether `requirement` (a SemVer requirement such as `^0.3.4`) accepts
/// `api_version`. A malformed requirement or version is never compatible.
pub fn draft_api_compatible(requirement: &str, api_version: &str) -> bool {
    let Ok(requirement) = semver::VersionReq::parse(requirement) else {
        return false;
    };
    semver::Version::parse(api_version).is_ok_and(|version| requirement.matches(&version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_requirements_track_the_offered_api() {
        assert!(draft_api_compatible("^0.3.4", "0.3.4"));
        assert!(!draft_api_compatible("^0.3.4", "0.3.3"));
        assert!(!draft_api_compatible("^0.3.4", "0.4.0"));
        assert!(draft_api_compatible("^1.2", "1.9.0"));
    }

    #[test]
    fn malformed_input_is_never_compatible() {
        assert!(!draft_api_compatible("not a requirement", "0.3.4"));
        assert!(!draft_api_compatible("^0.3.4", "not a version"));
        assert!(!draft_api_compatible("", ""));
    }
}
