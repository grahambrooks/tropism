//! Release tags and the versions they name.
//!
//! A *stable version tag* is `v` (optional) followed by exactly three numeric
//! components: `v2026.11.12` (release-kit's CalVer) and `v1.4.0` (SemVer, as Go
//! modules require) both qualify; `v1.4.0-rc.1`, `nightly` and `sub/v1.2.0` do not.
//! It is the same test sustain applies, so the two tools agree on what "newest"
//! means.

use std::cmp::Ordering;

/// Three numeric components, compared numerically: `2026.11.12` > `2026.11.9`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Version(pub u64, pub u64, pub u64);

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.0, self.1, self.2).cmp(&(other.0, other.1, other.2))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The version a stable tag names, or `None` for anything else.
pub fn stable_tag(tag: &str) -> Option<Version> {
    parse(tag.strip_prefix('v').unwrap_or(tag))
}

/// A bare `1.2.3`, with an optional leading `v`. Pre-release and build suffixes
/// are rejected: a pre-release is not a release its users would be told to take.
pub fn parse(text: &str) -> Option<Version> {
    let text = text.strip_prefix('v').unwrap_or(text);
    let mut parts = text.split('.');
    let mut next = || -> Option<u64> {
        let part = parts.next()?;
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let version = Version(next()?, next()?, next()?);
    if parts.next().is_some() {
        return None;
    }
    Some(version)
}

/// The commit a Go pseudo-version names: `v0.0.0-20261101120000-abcdef123456`
/// ends in a 12-character abbreviated hash.
pub fn go_pseudo_commit(version: &str) -> Option<&str> {
    let (_, tail) = version.rsplit_once('-')?;
    let hash = tail.split('+').next()?;
    (hash.len() == 12 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hash)
}

/// A full or abbreviated commit hash, as it appears in a `rev` pin.
pub fn looks_like_commit(text: &str) -> bool {
    (7..=40).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The newest stable tag among `tags`.
pub fn newest<'a, I>(tags: I) -> Option<(&'a str, Version)>
where
    I: IntoIterator<Item = &'a str>,
{
    tags.into_iter()
        .filter_map(|tag| Some((tag, stable_tag(tag)?)))
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
}

/// How many stable tags name a version newer than `current`.
pub fn newer_than<'a, I>(tags: I, current: Version) -> u32
where
    I: IntoIterator<Item = &'a str>,
{
    let mut versions: Vec<Version> = tags
        .into_iter()
        .filter_map(stable_tag)
        .filter(|version| *version > current)
        .collect();
    // `v1.0.0` and `1.0.0` on the same commit are one release, not two.
    versions.sort();
    versions.dedup();
    u32::try_from(versions.len()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calver_compares_numerically_not_as_text() {
        assert!(stable_tag("v2026.11.12") > stable_tag("v2026.11.9"));
        assert_eq!(
            newest(["v2026.11.9", "v2026.11.12", "v2026.10.25"]).map(|(t, _)| t),
            Some("v2026.11.12")
        );
    }

    #[test]
    fn only_three_numeric_components_are_a_release() {
        for tag in [
            "v1.2",
            "v1.2.3.4",
            "v1.2.3-rc.1",
            "nightly",
            "sub/v1.2.3",
            "v1.x.3",
        ] {
            assert_eq!(stable_tag(tag), None, "{tag}");
        }
        assert_eq!(stable_tag("1.2.3"), Some(Version(1, 2, 3)));
    }

    #[test]
    fn counts_releases_newer_than_the_pin() {
        let tags = [
            "v2026.10.25",
            "v2026.11.3",
            "v2026.11.12",
            "junk",
            "2026.11.12",
        ];
        assert_eq!(newer_than(tags, Version(2026, 10, 25)), 2);
        assert_eq!(newer_than(tags, Version(2026, 11, 12)), 0);
    }

    #[test]
    fn go_pseudo_versions_name_a_commit() {
        assert_eq!(
            go_pseudo_commit("v0.0.0-20261101120000-abcdef123456"),
            Some("abcdef123456")
        );
        assert_eq!(go_pseudo_commit("v1.2.3"), None);
        assert_eq!(go_pseudo_commit("v1.2.3-rc.1"), None);
    }
}
