//! Which repository a URL names.
//!
//! The same repository is spelled many ways — `git@github.com:o/n.git`,
//! `https://github.com/o/n`, `ssh://git@github.com/o/n`, `git+https://…#v1`,
//! `github:o/n` — and a link is only a link if both ends agree on the spelling.
//! Everything reduces to `host/owner/name`, with the host and owner lower-cased
//! (GitHub treats both case-insensitively) and a trailing `.git` dropped.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct RepoId {
    pub host: String,
    pub owner: String,
    pub name: String,
}

impl RepoId {
    /// `github.com/owner/name`, the one spelling used in the report.
    pub fn display(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.name)
    }

    pub fn is_owned_by(&self, owner: &str) -> bool {
        self.owner.eq_ignore_ascii_case(owner)
    }

    fn key(&self) -> (String, String, String) {
        (
            self.host.clone(),
            self.owner.clone(),
            self.name.to_ascii_lowercase(),
        )
    }

    pub fn same_repo(&self, other: &RepoId) -> bool {
        self.key() == other.key()
    }
}

/// Parses a git remote or dependency URL. `None` for anything that does not name
/// a host, an owner and a repository.
pub fn parse(url: &str) -> Option<RepoId> {
    let mut rest = url.trim();
    rest = rest.split(['#', '?']).next()?;
    for prefix in ["git+", "git://"] {
        if let Some(stripped) = rest.strip_prefix(prefix) {
            rest = stripped;
        }
    }
    if let Some(shorthand) = rest.strip_prefix("github:") {
        return from_parts("github.com", shorthand);
    }
    let (host, path) = if let Some((_, after)) = rest.split_once("://") {
        // `https://host/o/n`, `ssh://git@host/o/n`, `ssh://git@host:22/o/n`
        let (authority, path) = after.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        let host = host.split(':').next()?;
        (host, path)
    } else if let Some((user_host, path)) = rest.split_once(':') {
        // scp-like: `git@github.com:o/n.git`
        if user_host.contains('/') {
            return None;
        }
        (user_host.rsplit('@').next()?, path)
    } else {
        // `github.com/o/n`, as a Go module path spells it.
        let (host, path) = rest.split_once('/')?;
        if !host.contains('.') {
            return None;
        }
        (host, path)
    };
    from_parts(host, path)
}

fn from_parts(host: &str, path: &str) -> Option<RepoId> {
    let mut segments = path.trim_matches('/').split('/');
    let owner = segments.next().filter(|s| !s.is_empty())?;
    let name = segments.next().filter(|s| !s.is_empty())?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    let valid = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    if !valid(owner) || !valid(name) || host.is_empty() {
        return None;
    }
    Some(RepoId {
        host: host.to_ascii_lowercase(),
        owner: owner.to_ascii_lowercase(),
        name: name.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_and_https_spellings_are_the_same_repository() {
        let spellings = [
            "git@github.com:grahambrooks/mmd.git",
            "https://github.com/grahambrooks/mmd",
            "https://github.com/grahambrooks/mmd.git",
            "https://github.com/GrahamBrooks/mmd/",
            "ssh://git@github.com/grahambrooks/mmd.git",
            "ssh://git@github.com:22/grahambrooks/mmd",
            "git+https://github.com/grahambrooks/mmd.git#v2026.11.3",
            "git+ssh://git@github.com/grahambrooks/mmd.git",
            "github:grahambrooks/mmd",
            "github.com/grahambrooks/mmd",
            "https://github.com/grahambrooks/mmd?tag=v1#abc",
        ];
        let expected = parse(spellings[0]).unwrap();
        assert_eq!(expected.display(), "github.com/grahambrooks/mmd");
        for spelling in spellings {
            let id = parse(spelling).unwrap_or_else(|| panic!("{spelling}"));
            assert!(id.same_repo(&expected), "{spelling} → {id:?}");
        }
    }

    #[test]
    fn a_go_module_path_names_its_repository() {
        let id = parse("github.com/grahambrooks/imprint/v2").unwrap();
        assert_eq!(id.name, "imprint");
    }

    #[test]
    fn non_urls_name_nothing() {
        for text in [
            "1.2.3",
            "../mmd",
            "local",
            "",
            "https://github.com/onlyowner",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
    }
}
