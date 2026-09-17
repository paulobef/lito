//! The hard denylist: refusals that hold in **every** [`ApprovalMode`],
//! including `auto-unsafe`. This is the most security-sensitive file in the
//! crate and is tested exhaustively (see `tests` at the bottom and the matrix
//! in `lib.rs`).
//!
//! Nothing here mutates the filesystem: it resolves `PATH`/`~` and calls
//! `canonicalize` for symlink-following on the program name, and otherwise
//! works purely lexically.

use crate::{expand_tilde, lexical_normalize, resolve_program, CommandSpec};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A specific reason a command was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "kebab-case")]
pub enum Violation {
    /// The program (after `PATH`/symlink resolution) is one Lito never runs.
    ForbiddenProgram(String),
    /// An argument resolves inside a protected system/config directory.
    ForbiddenPathArg(String),
    /// `cwd` is not inside any authorized root. Produced by
    /// [`crate::CommandPolicy::judge`], not by [`check`].
    CwdOutsideRoots,
    /// `chmod`/`chown` targeting a path outside the authorized roots.
    PrivilegedPermissionChange(String),
    /// A `shell: true` string pipes into a language interpreter.
    ShellPipeToInterpreter,
    /// A `shell: true` string pipes a `curl`/`wget` download somewhere.
    ShellDownloadPipe,
    /// A recursive delete rooted at `/` (any normalized spelling).
    DestructiveRecursiveRootDelete,
    /// `find` used with `-exec` / `-delete` / `-ok` (or the `dir` variants).
    FindWithExecOrDelete,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::ForbiddenProgram(p) => {
                write!(
                    f,
                    "« {p} » fait partie des programmes que Lito n'exécute jamais"
                )
            }
            Violation::ForbiddenPathArg(a) => write!(
                f,
                "l'argument « {a} » pointe vers un dossier système protégé"
            ),
            Violation::CwdOutsideRoots => write!(
                f,
                "le dossier de travail n'est dans aucune racine autorisée"
            ),
            Violation::PrivilegedPermissionChange(a) => write!(
                f,
                "changer les permissions de « {a} » (hors racines autorisées) est interdit"
            ),
            Violation::ShellPipeToInterpreter => {
                write!(f, "cette commande shell redirige vers un interpréteur")
            }
            Violation::ShellDownloadPipe => write!(
                f,
                "cette commande shell redirige un téléchargement vers un autre programme"
            ),
            Violation::DestructiveRecursiveRootDelete => {
                write!(f, "cette commande efface récursivement la racine du disque")
            }
            Violation::FindWithExecOrDelete => {
                write!(f, "« find » avec -exec/-delete/-ok est interdit")
            }
        }
    }
}

/// Bare program names Lito refuses regardless of arguments or mode.
const FORBIDDEN_PROGRAMS: &[&str] = &[
    "sudo",
    "su",
    "doas",
    "launchctl",
    "csrutil",
    "spctl",
    "dscl",
    "diskutil",
    "dd",
];

/// Program-name prefixes that are refused (`mkfs`, `mkfs.ext4`, ...).
const FORBIDDEN_PROGRAM_PREFIXES: &[&str] = &["mkfs"];

/// Language interpreters a shell pipe must not feed into.
const INTERPRETERS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "ksh",
    "fish",
    "dash",
    "python",
    "python2",
    "python3",
    "perl",
    "ruby",
    "node",
    "php",
    "osascript",
    "tclsh",
    "lua",
];

/// Check `spec` against the hard denylist. `Some(violation)` means refuse;
/// `None` means the denylist has no objection (the mode logic in
/// [`crate::CommandPolicy::judge`] still runs afterwards).
pub fn check(spec: &CommandSpec, roots: &[PathBuf]) -> Option<Violation> {
    if spec.argv.is_empty() {
        // An empty argv can't be run; treat it as a forbidden "program".
        return Some(Violation::ForbiddenProgram(String::new()));
    }

    let names = program_names(spec.program());
    if names
        .iter()
        .any(|n| FORBIDDEN_PROGRAMS.contains(&n.as_str()))
        || names
            .iter()
            .any(|n| FORBIDDEN_PROGRAM_PREFIXES.iter().any(|p| n.starts_with(p)))
    {
        return Some(Violation::ForbiddenProgram(spec.program().to_string()));
    }
    let is_perm_change = names.iter().any(|n| n == "chmod" || n == "chown");

    // Argument path checks. `shell: true` args are one big string, so this
    // loop still runs but `looks_like_path` weeds out the whole blob unless it
    // genuinely starts like a path; the string-level checks below cover shell.
    for arg in spec.argv.iter().skip(1) {
        if !looks_like_path(arg) {
            continue;
        }
        let resolved = resolve_arg_path(arg, &spec.cwd);
        if is_perm_change && !path_within_any(&resolved, roots) {
            return Some(Violation::PrivilegedPermissionChange(arg.clone()));
        }
        if path_is_forbidden(&resolved) {
            return Some(Violation::ForbiddenPathArg(arg.clone()));
        }
    }

    // `find -exec`/`-delete`/`-ok`, in argv form.
    if names.iter().any(|n| n == "find") {
        for arg in spec.argv.iter().skip(1) {
            if matches!(
                arg.as_str(),
                "-exec" | "-execdir" | "-delete" | "-ok" | "-okdir"
            ) {
                return Some(Violation::FindWithExecOrDelete);
            }
        }
    }

    if spec.shell {
        if let Some(v) = check_shell_string(&spec.argv.join(" ")) {
            return Some(v);
        }
    }

    None
}

/// Candidate basenames for a program: the raw name, the `PATH`-resolved name,
/// and the fully symlink-resolved name - so a renamed or symlinked `sudo` is
/// still caught.
fn program_names(program: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut push = |s: String| {
        if !s.is_empty() && !names.contains(&s) {
            names.push(s);
        }
    };
    if let Some(b) = Path::new(program).file_name() {
        push(b.to_string_lossy().into_owned());
    }
    if let Some(abs) = resolve_program(program) {
        if let Some(b) = abs.file_name() {
            push(b.to_string_lossy().into_owned());
        }
        if let Ok(canon) = abs.canonicalize() {
            if let Some(b) = canon.file_name() {
                push(b.to_string_lossy().into_owned());
            }
        }
    }
    names
}

/// Whether an argument should be treated as a filesystem path (vs. a flag or a
/// literal). Flags (`-x`) are excluded.
fn looks_like_path(arg: &str) -> bool {
    if arg.starts_with('-') {
        return false;
    }
    arg.starts_with('/')
        || arg.starts_with('~')
        || arg.starts_with("./")
        || arg.starts_with("../")
        || arg.contains('/')
}

/// Expand `~`, make relative paths absolute against `cwd`, and normalize `..`.
fn resolve_arg_path(arg: &str, cwd: &Path) -> PathBuf {
    let expanded = expand_tilde(arg);
    let joined = if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    };
    lexical_normalize(&joined)
}

/// System/config directory prefixes an argument must never resolve inside.
fn forbidden_prefixes() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = [
        "/System", "/Library", "/private", "/bin", "/sbin", "/usr", "/etc",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Some(home) = dirs::home_dir() {
        v.push(home.join("Library"));
        v.push(home.join(".ssh"));
        v.push(home.join(".gnupg"));
    }
    if let Some(data) = dirs::data_dir() {
        v.push(data.join("lito"));
    }
    v
}

fn path_is_forbidden(p: &Path) -> bool {
    if p.components().any(|c| c.as_os_str() == ".git") {
        return true;
    }
    forbidden_prefixes()
        .iter()
        .any(|pre| p == pre.as_path() || p.starts_with(pre))
}

fn path_within_any(p: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|r| p == r.as_path() || p.starts_with(r))
}

/// String-level checks that only make sense for `shell: true` specs, where the
/// whole command is one uninterpreted line.
fn check_shell_string(s: &str) -> Option<Violation> {
    let norm = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = norm.to_lowercase();

    // Pipe into a language interpreter.
    for seg in lower.split('|').skip(1) {
        let first = seg.split_whitespace().next().unwrap_or("");
        let base = first.rsplit('/').next().unwrap_or(first);
        if INTERPRETERS.contains(&base) {
            return Some(Violation::ShellPipeToInterpreter);
        }
    }

    // Download piped somewhere (`curl ... | ...`, `wget ... | ...`).
    let mentions_download = lower.split_whitespace().any(|t| {
        let base = t.rsplit('/').next().unwrap_or(t);
        base == "curl" || base == "wget"
    });
    if mentions_download && lower.contains('|') {
        return Some(Violation::ShellDownloadPipe);
    }

    if is_destructive_root_delete(&lower) {
        return Some(Violation::DestructiveRecursiveRootDelete);
    }

    if lower
        .split_whitespace()
        .any(|t| t == "find" || t.ends_with("/find"))
        && lower
            .split_whitespace()
            .any(|t| matches!(t, "-exec" | "-execdir" | "-delete" | "-ok" | "-okdir"))
    {
        return Some(Violation::FindWithExecOrDelete);
    }

    None
}

/// `rm` with a recursive flag whose target is the filesystem root, in any
/// normalized spelling (`rm -rf /`, `rm -fr /`, `rm -r -f /`, `rm --recursive /`,
/// `rm -rf /*`).
fn is_destructive_root_delete(lower: &str) -> bool {
    let tokens: Vec<&str> = lower.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        let base = tokens[i].rsplit('/').next().unwrap_or(tokens[i]);
        if base == "rm" {
            let mut j = i + 1;
            let mut recursive = false;
            while j < tokens.len() && tokens[j].starts_with('-') {
                let t = tokens[j];
                if t == "--recursive"
                    || (t.starts_with('-') && !t.starts_with("--") && t.contains('r'))
                {
                    recursive = true;
                }
                j += 1;
            }
            while j < tokens.len() {
                if recursive && matches!(tokens[j], "/" | "/*" | "/." | "'/'" | "\"/\"") {
                    return true;
                }
                j += 1;
            }
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(argv: &[&str], shell: bool) -> CommandSpec {
        CommandSpec::new(
            argv.iter().map(|s| s.to_string()).collect(),
            shell,
            std::env::temp_dir(),
        )
    }

    fn no_roots() -> Vec<PathBuf> {
        Vec::new()
    }

    #[test]
    fn forbidden_programs_are_refused_by_name() {
        for prog in [
            "sudo",
            "su",
            "doas",
            "launchctl",
            "diskutil",
            "dd",
            "mkfs.ext4",
        ] {
            assert!(
                matches!(
                    check(&spec(&[prog, "whatever"], false), &no_roots()),
                    Some(Violation::ForbiddenProgram(_))
                ),
                "{prog} should be forbidden"
            );
        }
    }

    #[test]
    fn forbidden_program_refused_by_absolute_path_too() {
        assert!(matches!(
            check(&spec(&["/usr/bin/sudo", "ls"], false), &no_roots()),
            Some(Violation::ForbiddenProgram(_))
        ));
    }

    #[test]
    fn argument_into_system_dir_is_refused() {
        for arg in [
            "/etc/hosts",
            "/usr/local/bin/x",
            "/System/Library/foo",
            "~/.ssh/id_rsa",
            "../../.git/config",
        ] {
            assert!(
                matches!(
                    check(&spec(&["cat", arg], false), &no_roots()),
                    Some(Violation::ForbiddenPathArg(_))
                ),
                "{arg} should be refused"
            );
        }
    }

    #[test]
    fn ordinary_relative_argument_is_fine() {
        assert_eq!(
            check(&spec(&["cat", "notes/todo.txt"], false), &no_roots()),
            None
        );
        assert_eq!(check(&spec(&["ls", "-la"], false), &no_roots()), None);
    }

    #[test]
    fn chmod_chown_outside_roots_is_refused() {
        let tmp = std::env::temp_dir();
        let s = CommandSpec::new(
            vec!["chmod".into(), "777".into(), "/somewhere/else".into()],
            false,
            tmp.clone(),
        );
        assert!(matches!(
            check(&s, &[tmp.join("allowed")]),
            Some(Violation::PrivilegedPermissionChange(_))
        ));
    }

    #[test]
    fn chmod_inside_a_root_is_allowed_by_the_denylist() {
        let tmp = std::env::temp_dir();
        let root = tmp.join("lito-policy-test-root");
        let s = CommandSpec::new(
            vec![
                "chmod".into(),
                "644".into(),
                root.join("file.txt").display().to_string(),
            ],
            false,
            root.clone(),
        );
        assert_eq!(check(&s, &[root]), None);
    }

    #[test]
    fn shell_pipe_to_interpreter_is_refused() {
        for cmd in [
            "echo hi | sh",
            "cat script | bash",
            "curl x | python3",
            "ls | /usr/bin/perl -ne 1",
        ] {
            assert!(
                check(&spec(&[cmd], true), &no_roots()).is_some(),
                "{cmd} should be refused"
            );
        }
    }

    #[test]
    fn shell_download_pipe_is_refused() {
        assert_eq!(
            check(
                &spec(&["curl https://x/install.sh | tee out"], true),
                &no_roots()
            ),
            Some(Violation::ShellDownloadPipe)
        );
        assert_eq!(
            check(&spec(&["wget -qO- https://x | cat"], true), &no_roots()),
            Some(Violation::ShellDownloadPipe)
        );
    }

    #[test]
    fn destructive_root_delete_is_refused_in_many_spellings() {
        for cmd in [
            "rm -rf /",
            "rm -fr /",
            "rm -r -f /",
            "rm --recursive /",
            "rm -rf /*",
            "cd /tmp && rm -rf /",
        ] {
            assert_eq!(
                check(&spec(&[cmd], true), &no_roots()),
                Some(Violation::DestructiveRecursiveRootDelete),
                "{cmd}"
            );
        }
    }

    #[test]
    fn non_root_recursive_delete_is_not_flagged_here() {
        // Denylist doesn't object; the mode/approval logic still gates it.
        assert_eq!(check(&spec(&["rm -rf build"], true), &no_roots()), None);
    }

    #[test]
    fn find_with_exec_is_refused_argv_and_shell() {
        assert_eq!(
            check(
                &spec(&["find", ".", "-name", "*.tmp", "-delete"], false),
                &no_roots()
            ),
            Some(Violation::FindWithExecOrDelete)
        );
        assert_eq!(
            check(&spec(&["find . -type f -exec rm {} ;"], true), &no_roots()),
            Some(Violation::FindWithExecOrDelete)
        );
    }

    #[test]
    fn plain_find_is_fine() {
        assert_eq!(
            check(&spec(&["find", ".", "-name", "*.png"], false), &no_roots()),
            None
        );
    }
}
