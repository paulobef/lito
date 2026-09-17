//! Lito Policy - approval rules for terminal commands.
//!
//! Every terminal command Lito is asked to run - whether it comes from an
//! `Operation::RunCommand` plan step or a reconnaissance query issued during
//! planning - is passed through [`CommandPolicy::judge`], which returns one of
//! three [`Verdict`]s: run it now, ask the user first, or refuse outright.
//!
//! This crate is deliberately UI-free and side-effect-free (it never touches
//! the filesystem beyond `PATH`/`~` resolution and `canonicalize`), so the
//! whole decision surface is unit-testable. Execution lives in `lito-exec`.
//!
//! It is also sized to host `P1.1 - Politique de chemins et confinement`: the
//! [`CommandPolicy::roots`] allow-list of directories introduced here for
//! commands is the same shape a future `PathPolicy` for `DELETE`/`MOVE`/... will
//! want, and should move alongside `CommandPolicy` rather than being duplicated.

pub mod denylist;
pub mod readonly;

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

pub use denylist::Violation;

/// How much the user has to be asked before a command runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalMode {
    /// Every command needs an explicit click, every time.
    #[default]
    Validation,
    /// Commands whose exact `(argv, root)` the user has already approved with
    /// "remember" run automatically; everything else still asks.
    AutoRestricted,
    /// Everything runs automatically - except the hard denylist, which is
    /// still enforced (see [`CommandPolicy::judge`]).
    AutoUnsafe,
}

impl ApprovalMode {
    /// Round-trips through the string form persisted in `settings`.
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalMode::Validation => "validation",
            ApprovalMode::AutoRestricted => "auto-restricted",
            ApprovalMode::AutoUnsafe => "auto-unsafe",
        }
    }

    /// Parse the persisted string form; unknown values fall back to the safe
    /// default rather than erroring.
    pub fn from_str_lenient(s: &str) -> Self {
        match s {
            "auto-restricted" => ApprovalMode::AutoRestricted,
            "auto-unsafe" => ApprovalMode::AutoUnsafe,
            _ => ApprovalMode::Validation,
        }
    }
}

/// A concrete command to be judged/run. `cwd` is expected to be already
/// `~`-expanded and canonicalized by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// `argv[0]` is the program, the rest are its arguments. For a
    /// `shell: true` spec this is still a token list, joined with spaces
    /// before being handed to `/bin/sh -c`.
    pub argv: Vec<String>,
    pub shell: bool,
    pub cwd: PathBuf,
}

impl CommandSpec {
    pub fn new(argv: Vec<String>, shell: bool, cwd: PathBuf) -> Self {
        Self { argv, shell, cwd }
    }

    /// `argv[0]`, or `""` for an (invalid) empty argv.
    pub fn program(&self) -> &str {
        self.argv.first().map(String::as_str).unwrap_or("")
    }
}

/// The outcome of [`CommandPolicy::judge`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Run without asking.
    AutoAllow,
    /// Ask the user. `rememberable` is false for `shell: true` specs - a shell
    /// string isn't analyzable, so it can never enter the allowlist.
    NeedsApproval { rememberable: bool },
    /// Hard refusal. No amount of approval or mode-switching gets past this.
    Denied(Violation),
}

/// The allowlist entry for a command: its canonical argv plus the authorized
/// root its `cwd` resolved into. `shell: true` specs never produce a key.
///
/// `canonical` is `serde_json` of the argv rather than a space-join, so
/// `["rm", "-rf a"]` and `["rm", "-rf", "a"]` don't collide.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AllowlistKey {
    pub canonical: String,
    pub root: PathBuf,
}

impl AllowlistKey {
    pub fn new(argv: &[String], root: PathBuf) -> Self {
        Self {
            canonical: serde_json::to_string(argv).unwrap_or_default(),
            root,
        }
    }
}

/// The default authorized roots (those `P1.1` specifies): `~/Desktop`,
/// `~/Documents`, `~/Downloads`, `~/Pictures`. A `cwd` must resolve inside one
/// of these in *every* mode, or the command is denied.
pub fn default_roots() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    ["Desktop", "Documents", "Downloads", "Pictures"]
        .iter()
        .map(|d| home.join(d))
        .collect()
}

/// Resolve a program name to an absolute path the way a shell would: an
/// absolute or slash-containing name is taken as-is, a bare name is looked up
/// on `PATH`. Returns `None` if a bare name isn't found on `PATH`.
pub fn resolve_program(program: &str) -> Option<PathBuf> {
    if program.is_empty() {
        return None;
    }
    let p = Path::new(program);
    if p.is_absolute() {
        return Some(p.to_path_buf());
    }
    if program.contains('/') {
        return Some(lexical_normalize(
            &std::env::current_dir().unwrap_or_default().join(p),
        ));
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// The absolute path form of a program name for display in the approval card,
/// falling back to the raw name if it can't be resolved.
pub fn resolved_program_display(program: &str) -> String {
    resolve_program(program)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| program.to_string())
}

/// The approval policy for terminal commands: a set of authorized roots, a
/// remembered-command allowlist, and the current [`ApprovalMode`].
pub struct CommandPolicy {
    roots: Vec<PathBuf>,
    allowlist: HashSet<AllowlistKey>,
    mode: ApprovalMode,
}

impl CommandPolicy {
    /// Build a policy. Roots are canonicalized where they exist so `cwd`
    /// containment comparisons line up (`cwd` is canonicalized by the caller).
    pub fn new(mode: ApprovalMode, roots: Vec<PathBuf>, allowlist: HashSet<AllowlistKey>) -> Self {
        let roots = roots
            .into_iter()
            .map(|r| r.canonicalize().unwrap_or_else(|_| lexical_normalize(&r)))
            .collect();
        Self {
            roots,
            allowlist,
            mode,
        }
    }

    /// A policy with the default roots and an empty allowlist.
    pub fn with_defaults(mode: ApprovalMode) -> Self {
        Self::new(mode, default_roots(), HashSet::new())
    }

    pub fn mode(&self) -> ApprovalMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: ApprovalMode) {
        self.mode = mode;
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// The allowlist entries, for the settings panel's revoke UI.
    pub fn allowlist(&self) -> impl Iterator<Item = &AllowlistKey> {
        self.allowlist.iter()
    }

    /// Record `key` as auto-allowed (the user clicked "Autoriser et mémoriser").
    pub fn insert_allow(&mut self, key: AllowlistKey) {
        self.allowlist.insert(key);
    }

    /// Drop one allowlist entry (the settings panel's per-row "×"). Returns
    /// whether it was present.
    pub fn remove_allow(&mut self, key: &AllowlistKey) -> bool {
        self.allowlist.remove(key)
    }

    /// The allowlist key this spec would produce if remembered, or `None` if
    /// it isn't rememberable (`shell: true`, or `cwd` outside every root).
    pub fn allowlist_key(&self, spec: &CommandSpec) -> Option<AllowlistKey> {
        if spec.shell {
            return None;
        }
        let root = self.containing_root(&spec.cwd)?;
        Some(AllowlistKey::new(&spec.argv, root.clone()))
    }

    /// The core decision. Evaluated in exactly this order:
    ///
    /// 1. Hard denylist - **before** the mode, so `auto-unsafe` can't bypass it.
    /// 2. `cwd` outside every authorized root - denied in all modes.
    /// 3. `auto-unsafe` -> [`Verdict::AutoAllow`].
    /// 4. `auto-restricted` + not a shell string + argv/root in the allowlist
    ///    -> [`Verdict::AutoAllow`].
    /// 5. otherwise -> [`Verdict::NeedsApproval`] (rememberable unless shell).
    pub fn judge(&self, spec: &CommandSpec) -> Verdict {
        if let Some(violation) = denylist::check(spec, &self.roots) {
            return Verdict::Denied(violation);
        }

        let Some(root) = self.containing_root(&spec.cwd) else {
            return Verdict::Denied(Violation::CwdOutsideRoots);
        };

        match self.mode {
            ApprovalMode::AutoUnsafe => Verdict::AutoAllow,
            ApprovalMode::AutoRestricted => {
                let allowed = !spec.shell
                    && self
                        .allowlist
                        .contains(&AllowlistKey::new(&spec.argv, root.clone()));
                if allowed {
                    Verdict::AutoAllow
                } else {
                    Verdict::NeedsApproval {
                        rememberable: !spec.shell,
                    }
                }
            }
            ApprovalMode::Validation => Verdict::NeedsApproval {
                rememberable: !spec.shell,
            },
        }
    }

    /// The authorized root that contains `cwd` (or is `cwd`), if any.
    pub fn containing_root(&self, cwd: &Path) -> Option<&PathBuf> {
        self.roots
            .iter()
            .find(|r| cwd == r.as_path() || cwd.starts_with(r))
    }
}

/// Collapse `.`/`..` components lexically, without touching the filesystem.
/// Used for both program and argument paths that may not exist yet.
pub(crate) fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Expand a leading `~` against `$HOME`; leave anything else unchanged.
pub(crate) fn expand_tilde(s: &str) -> PathBuf {
    if s == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn tmp_roots() -> (tempfile::TempDir, Vec<PathBuf>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("work");
        std::fs::create_dir_all(&root).unwrap();
        (dir, vec![root])
    }

    fn spec_in(root: &Path, argv: &[&str], shell: bool) -> CommandSpec {
        CommandSpec::new(
            argv.iter().map(|s| s.to_string()).collect(),
            shell,
            root.canonicalize().unwrap(),
        )
    }

    #[test]
    fn validation_mode_always_asks() {
        let (_d, roots) = tmp_roots();
        let policy = CommandPolicy::new(ApprovalMode::Validation, roots.clone(), HashSet::new());
        let v = policy.judge(&spec_in(&roots[0], &["ls", "-la"], false));
        assert_eq!(v, Verdict::NeedsApproval { rememberable: true });
    }

    #[test]
    fn validation_mode_shell_is_not_rememberable() {
        let (_d, roots) = tmp_roots();
        let policy = CommandPolicy::new(ApprovalMode::Validation, roots.clone(), HashSet::new());
        let v = policy.judge(&spec_in(&roots[0], &["ls | wc -l"], true));
        assert_eq!(
            v,
            Verdict::NeedsApproval {
                rememberable: false
            }
        );
    }

    #[test]
    fn auto_unsafe_allows_ordinary_commands() {
        let (_d, roots) = tmp_roots();
        let policy = CommandPolicy::new(ApprovalMode::AutoUnsafe, roots.clone(), HashSet::new());
        assert_eq!(
            policy.judge(&spec_in(&roots[0], &["rm", "note.txt"], false)),
            Verdict::AutoAllow
        );
    }

    #[test]
    fn auto_unsafe_still_denies_the_denylist() {
        let (_d, roots) = tmp_roots();
        let policy = CommandPolicy::new(ApprovalMode::AutoUnsafe, roots.clone(), HashSet::new());
        assert!(matches!(
            policy.judge(&spec_in(&roots[0], &["sudo", "ls"], false)),
            Verdict::Denied(_)
        ));
    }

    #[test]
    fn auto_restricted_matrix() {
        let (_d, roots) = tmp_roots();
        let root = roots[0].canonicalize().unwrap();
        let key = AllowlistKey::new(&["ls".to_string(), "-la".to_string()], root.clone());
        let mut allow = HashSet::new();
        allow.insert(key);
        let policy = CommandPolicy::new(ApprovalMode::AutoRestricted, roots.clone(), allow);

        // In allowlist, argv, not shell -> auto.
        assert_eq!(
            policy.judge(&spec_in(&roots[0], &["ls", "-la"], false)),
            Verdict::AutoAllow
        );
        // Same argv, but shell -> asks, not rememberable.
        assert_eq!(
            policy.judge(&spec_in(&roots[0], &["ls", "-la"], true)),
            Verdict::NeedsApproval {
                rememberable: false
            }
        );
        // Not in allowlist -> asks.
        assert_eq!(
            policy.judge(&spec_in(&roots[0], &["ls", "-l"], false)),
            Verdict::NeedsApproval { rememberable: true }
        );
    }

    #[test]
    fn cwd_outside_roots_is_denied_in_every_mode() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        let root = dir.path().join("work");
        std::fs::create_dir_all(&root).unwrap();

        for mode in [
            ApprovalMode::Validation,
            ApprovalMode::AutoRestricted,
            ApprovalMode::AutoUnsafe,
        ] {
            let policy = CommandPolicy::new(mode, vec![root.clone()], HashSet::new());
            let spec = CommandSpec::new(
                vec!["ls".to_string()],
                false,
                outside.canonicalize().unwrap(),
            );
            assert_eq!(
                policy.judge(&spec),
                Verdict::Denied(Violation::CwdOutsideRoots),
                "mode {mode:?}"
            );
        }
    }

    #[test]
    fn allowlist_key_is_none_for_shell_and_outside_roots() {
        let (_d, roots) = tmp_roots();
        let policy = CommandPolicy::new(ApprovalMode::Validation, roots.clone(), HashSet::new());
        assert!(policy
            .allowlist_key(&spec_in(&roots[0], &["ls | cat"], true))
            .is_none());
        let outside = CommandSpec::new(vec!["ls".into()], false, PathBuf::from("/nope"));
        assert!(policy.allowlist_key(&outside).is_none());
        assert!(policy
            .allowlist_key(&spec_in(&roots[0], &["ls"], false))
            .is_some());
    }

    #[test]
    fn approval_mode_string_round_trip() {
        for mode in [
            ApprovalMode::Validation,
            ApprovalMode::AutoRestricted,
            ApprovalMode::AutoUnsafe,
        ] {
            assert_eq!(ApprovalMode::from_str_lenient(mode.as_str()), mode);
        }
        assert_eq!(
            ApprovalMode::from_str_lenient("garbage"),
            ApprovalMode::Validation
        );
    }
}
