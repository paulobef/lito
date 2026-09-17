//! A small built-in allowlist of argv patterns that are notoriously
//! side-effect-free. It is used **only** by reconnaissance commands issued
//! during planning (see the agent crate): those short-circuit the approval
//! prompt even in `validation` mode, so that the common case (`ls`,
//! `git status`) doesn't freeze plan generation on a modal.
//!
//! It is *not* consulted by [`crate::CommandPolicy::judge`] - the hard
//! denylist and the `cwd`-in-root rule still apply to recon commands, this
//! only bypasses the *approval* step.
//!
//! Matching is on the whole argv pattern (program + subcommand + flags), not
//! on `argv[0]` alone.

/// `<N>` matches any integer token; `<path>` matches any single non-flag
/// token; every other pattern element must match verbatim.
const PATTERNS: &[&[&str]] = &[
    &["ls"],
    &["ls", "-l"],
    &["ls", "-a"],
    &["ls", "-la"],
    &["ls", "-al"],
    &["ls", "-lah"],
    &["ls", "<path>"],
    &["ls", "-l", "<path>"],
    &["ls", "-la", "<path>"],
    &["pwd"],
    &["git", "status"],
    &["git", "log", "--oneline", "-n", "<N>"],
    &["git", "diff", "--stat"],
    &["wc", "-l", "<path>"],
    &["file", "<path>"],
    &["du", "-sh", "<path>"],
    &["stat", "<path>"],
    &["head", "-n", "<N>", "<path>"],
    &["tail", "-n", "<N>", "<path>"],
];

/// Whether `argv` exactly matches one of the built-in read-only patterns.
pub fn is_readonly(argv: &[String]) -> bool {
    PATTERNS.iter().any(|pat| matches_pattern(pat, argv))
}

fn matches_pattern(pat: &[&str], argv: &[String]) -> bool {
    if pat.len() != argv.len() {
        return false;
    }
    pat.iter().zip(argv).all(|(p, a)| match *p {
        "<N>" => a.parse::<i64>().is_ok(),
        "<path>" => !a.starts_with('-'),
        literal => literal == a,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn accepts_known_read_only_commands() {
        assert!(is_readonly(&argv(&["ls"])));
        assert!(is_readonly(&argv(&["ls", "-la"])));
        assert!(is_readonly(&argv(&["ls", "-la", "subdir"])));
        assert!(is_readonly(&argv(&["pwd"])));
        assert!(is_readonly(&argv(&["git", "status"])));
        assert!(is_readonly(&argv(&["git", "log", "--oneline", "-n", "20"])));
        assert!(is_readonly(&argv(&["git", "diff", "--stat"])));
        assert!(is_readonly(&argv(&["wc", "-l", "notes.txt"])));
        assert!(is_readonly(&argv(&["file", "image.png"])));
        assert!(is_readonly(&argv(&["du", "-sh", "."])));
        assert!(is_readonly(&argv(&["stat", "a.txt"])));
        assert!(is_readonly(&argv(&["head", "-n", "5", "log.txt"])));
        assert!(is_readonly(&argv(&["tail", "-n", "5", "log.txt"])));
    }

    #[test]
    fn rejects_anything_not_an_exact_pattern() {
        assert!(!is_readonly(&argv(&["ls", "-la", "--color", "x"])));
        assert!(!is_readonly(&argv(&["rm", "-rf", "x"])));
        assert!(!is_readonly(&argv(&["git", "log"])));
        assert!(!is_readonly(&argv(&[
            "git",
            "log",
            "--oneline",
            "-n",
            "notanumber"
        ])));
        assert!(!is_readonly(&argv(&["cat", "file"])));
        assert!(!is_readonly(&argv(&["head", "-c", "5", "f"])));
        assert!(!is_readonly(&[]));
    }
}
