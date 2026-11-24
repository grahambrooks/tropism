//! Small helpers for pointing at a line of a file.

/// The 1-based line where a TOML key is declared: `key = …`, `"key" = …`, or a
/// `[section.key]` header. Evidence carries a line so a reader can jump to it;
/// the TOML parser does not keep positions, and the key is enough to find it.
pub fn line_of_key(text: &str, key: &str) -> Option<u32> {
    let quoted = format!("\"{key}\"");
    text.lines()
        .position(|line| {
            let line = line.trim_start();
            let starts = |prefix: &str| {
                line.strip_prefix(prefix).is_some_and(|rest| {
                    let rest = rest.trim_start();
                    rest.starts_with('=') || rest.starts_with('.')
                })
            };
            if starts(key) || starts(&quoted) {
                return true;
            }
            line.starts_with('[')
                && line
                    .trim_start_matches('[')
                    .trim_end()
                    .trim_end_matches(']')
                    .rsplit('.')
                    .next()
                    .is_some_and(|last| last == key || last == quoted)
        })
        .and_then(|index| u32::try_from(index + 1).ok())
}

/// The 1-based line of the first occurrence of `needle`.
pub fn line_of(text: &str, needle: &str) -> Option<u32> {
    text.lines()
        .position(|line| line.contains(needle))
        .and_then(|index| u32::try_from(index + 1).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_inline_quoted_and_header_declarations() {
        let text = "[dependencies]\nmmdx = \"1\"\nmmd = { git = \"x\" }\n\"puml\" = { git = \"y\" }\n[dependencies.scene]\n";
        assert_eq!(line_of_key(text, "mmd"), Some(3));
        assert_eq!(line_of_key(text, "puml"), Some(4));
        assert_eq!(line_of_key(text, "scene"), Some(5));
        assert_eq!(line_of_key(text, "absent"), None);
    }
}
