//! Lito Protocol - Shared types used across the UI, agent, fs, and memory crates.
//!
//! `Operation`/`PlanStep`/`ErrorCode`/`PlanId` are the real shared vocabulary; the UI and
//! core communicate via `UiToCoreMsg`/`CoreToUiMsg` (in `crates/ui`), not by serializing
//! these types.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use strum::{Display, EnumString};
use thiserror::Error;
use uuid::Uuid;

/// Operations that can be performed on the filesystem
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Operation {
    Mkdir,
    Move,
    Copy,
    Delete,
    Rename,
    Symlink,
    WriteFile,
    /// Run a terminal command. Unlike every other operation this one has no
    /// inverse and its effect can't be analyzed ahead of time, so it goes
    /// through `lito_policy::CommandPolicy` + an approval round-trip rather
    /// than `FsOps` (see `app::dispatch_step`).
    RunCommand,
}

/// A single step in an execution plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub description: String,
    pub operation: Operation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<PathBuf>,
    /// Text content for WRITE_FILE steps - the literal bytes to write to `path`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// RUN_COMMAND only: the command as an argv vector (`argv[0]` is the
    /// program). Added after `PlanStep` first shipped, hence `Option` - a
    /// pre-existing persisted plan row simply has no `argv`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// RUN_COMMAND only: run the command through `/bin/sh -c` instead of
    /// executing `argv` directly. Opt-in, per step, and surfaced visually - a
    /// shell string is never analyzable and so never enters the allowlist.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shell: bool,
    /// RUN_COMMAND only: the working directory the command runs in. Must
    /// resolve inside an authorized root in every approval mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

/// A read-only filesystem query the agent can issue mid-planning to gather
/// information it needs before it can commit to a correct plan - e.g. the
/// exact file names in a folder, or a file's size/creation date. Unlike
/// `Operation`, queries never mutate the filesystem, so `fs::FsOps` answers
/// them without going through the undo log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum QueryOp {
    /// Direct children of a directory (not recursive).
    List,
    /// Metadata for exactly the path given.
    Stat,
}

/// A query request, as emitted by the model: `{"query": "LIST", "path": "..."}`
/// or `{"query": "STAT", "path": "..."}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsQuery {
    pub query: QueryOp,
    pub path: PathBuf,
}

/// Metadata about a single filesystem entry, as returned by a LIST or STAT
/// query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    /// RFC 3339 timestamp, if the underlying filesystem tracks a creation
    /// time separately from last-modified (most do on macOS/APFS; not all
    /// filesystems do).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
}

/// The answer to an `FsQuery`. LIST returns one entry per direct child; STAT
/// returns exactly one entry, for the queried path itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub query: QueryOp,
    pub path: PathBuf,
    pub entries: Vec<FileMetadata>,
    /// True if `entries` was capped before every child of the directory was
    /// included (see `fs::MAX_LIST_ENTRIES`) - lets the model know the
    /// listing is partial rather than assuming it saw everything.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

/// Error codes returned by the core
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, EnumString, Error)]
#[strum(serialize_all = "snake_case")]
pub enum ErrorCode {
    #[error("Ollama service is offline")]
    OllamaOffline,
    #[error("Plan generation failed")]
    PlanFailed,
    #[error("Filesystem error")]
    FsError,
    #[error("Validation failed")]
    ValidationFailed,
    #[error("Permission denied")]
    PermissionDenied,
    #[error("Internal server error")]
    Internal,
}

/// Unique identifiers for plans and sessions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlanId(pub Uuid);

impl PlanId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for PlanId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for PlanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
