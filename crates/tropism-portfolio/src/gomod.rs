//! `go.mod`, read for its module path, its requirements and its replacements.
//!
//! Since Go 1.17 a main module's `go.mod` lists every module in its build list,
//! marked `// indirect` when nothing imports it directly, so the version written
//! there is the one minimal version selection chose. That makes the `require`
//! line both the pin and the resolution; `go.sum` adds hashes and no edges.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoMod {
    pub module: Option<String>,
    pub requires: Vec<Require>,
    pub replaces: Vec<Replace>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Require {
    pub path: String,
    pub version: String,
    pub indirect: bool,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replace {
    pub path: String,
    /// A local directory (`../lib`) or another module path.
    pub target: String,
    pub target_version: Option<String>,
    pub line: u32,
}

impl Replace {
    pub fn is_local(&self) -> bool {
        self.target.starts_with("./")
            || self.target.starts_with("../")
            || self.target.starts_with('/')
    }
}

pub fn parse(text: &str) -> GoMod {
    let mut out = GoMod::default();
    let mut block: Option<&str> = None;
    for (index, raw) in text.lines().enumerate() {
        let line_no = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let (code, comment) = match raw.split_once("//") {
            Some((code, comment)) => (code.trim(), comment.trim()),
            None => (raw.trim(), ""),
        };
        if code.is_empty() {
            continue;
        }
        if let Some(open) = block {
            if code == ")" {
                block = None;
                continue;
            }
            directive(&mut out, open, code, comment, line_no);
            continue;
        }
        let (verb, rest) = code.split_once(char::is_whitespace).unwrap_or((code, ""));
        let rest = rest.trim();
        if rest == "(" {
            block = Some(verb);
            continue;
        }
        if verb == "module" {
            out.module = Some(unquote(rest).to_owned());
            continue;
        }
        directive(&mut out, verb, rest, comment, line_no);
    }
    out
}

fn unquote(text: &str) -> &str {
    text.trim().trim_matches('"').trim_matches('`')
}

fn directive(out: &mut GoMod, verb: &str, body: &str, comment: &str, line: u32) {
    match verb {
        "require" => {
            let mut fields = body.split_whitespace();
            if let (Some(path), Some(version)) = (fields.next(), fields.next()) {
                out.requires.push(Require {
                    path: unquote(path).to_owned(),
                    version: version.to_owned(),
                    indirect: comment.split(';').any(|c| c.trim() == "indirect"),
                    line,
                });
            }
        }
        "replace" => {
            let Some((left, right)) = body.split_once("=>") else {
                return;
            };
            let Some(path) = left.split_whitespace().next() else {
                return;
            };
            let mut right = right.split_whitespace();
            let Some(target) = right.next() else {
                return;
            };
            out.replaces.push(Replace {
                path: unquote(path).to_owned(),
                target: unquote(target).to_owned(),
                target_version: right.next().map(str::to_owned),
                line,
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_and_replaces_in_both_forms() {
        let text = "module github.com/grahambrooks/bsdoc\n\ngo 1.25\n\nrequire github.com/grahambrooks/imprint v1.2.0\n\nrequire (\n\tgithub.com/spf13/cobra v1.8.0\n\tgithub.com/grahambrooks/fingerprint v0.0.0-20261101120000-abcdef123456 // indirect\n)\n\nreplace github.com/grahambrooks/imprint => ../imprint\nreplace (\n\tgithub.com/x/y v1.0.0 => github.com/x/z v1.1.0\n)\n";
        let parsed = parse(text);
        assert_eq!(
            parsed.module.as_deref(),
            Some("github.com/grahambrooks/bsdoc")
        );
        assert_eq!(parsed.requires.len(), 3);
        assert_eq!(parsed.requires[0].line, 5);
        assert!(!parsed.requires[0].indirect);
        assert!(parsed.requires[2].indirect);
        assert_eq!(parsed.replaces.len(), 2);
        assert!(parsed.replaces[0].is_local());
        assert_eq!(parsed.replaces[1].target, "github.com/x/z");
        assert_eq!(parsed.replaces[1].target_version.as_deref(), Some("v1.1.0"));
    }
}
