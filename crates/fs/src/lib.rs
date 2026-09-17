//! Lito FS - File system operations with undo support

use anyhow::Result;
use lito_protocol::{FileMetadata, FsQuery, Operation, PlanStep, QueryOp, QueryResult};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::fs;
use uuid::Uuid;

/// Cap on how many entries a single LIST query returns. Folders like
/// Downloads can hold thousands of files; dumping all of them into the
/// model's context would blow the prompt budget for no benefit, so the
/// listing is capped and flagged as `truncated` instead.
pub const MAX_LIST_ENTRIES: usize = 500;

/// File system operations with undo logging
pub struct FsOps {
    undo_log: Vec<UndoEntry>,
}

#[derive(Debug, Clone)]
struct UndoEntry {
    /// Groups entries executed together so a whole plan run can be undone at once.
    session_id: String,
    operation: UndoOperation,
}

#[derive(Debug, Clone)]
enum UndoOperation {
    CreateDir {
        path: PathBuf,
    },
    Move {
        from: PathBuf,
        to: PathBuf,
    },
    Copy {
        to: PathBuf,
    },
    Delete {
        path: PathBuf,
        backup: PathBuf,
    },
    Symlink {
        link: PathBuf,
    },
    WriteFile {
        path: PathBuf,
        previous: Option<PathBuf>,
    },
}

impl FsOps {
    pub fn new() -> Self {
        Self {
            undo_log: Vec::new(),
        }
    }

    /// Execute a single plan step, logging an undo entry under `session_id`.
    ///
    /// `step.path`/`step.target` are expanded for a leading `~` before use -
    /// the model has no way to know the real home directory, so it sometimes
    /// emits paths like `~/Downloads/foo.txt` verbatim, which the filesystem
    /// layer would otherwise treat as a literal (nonexistent) `~` directory
    /// relative to the process's cwd.
    pub async fn execute_step(&mut self, step: &PlanStep, session_id: &str) -> Result<()> {
        let path = step.path.as_deref().map(expand_tilde);
        let target = step.target.as_deref().map(expand_tilde);

        match step.operation {
            Operation::Mkdir => {
                let path = require_path(&path, "MKDIR")?;
                self.create_dir_all(path, session_id).await
            }
            Operation::Move => {
                let (from, to) = require_pair(&path, &target, "MOVE")?;
                self.move_file(from, to, session_id).await
            }
            Operation::Copy => {
                let (from, to) = require_pair(&path, &target, "COPY")?;
                self.copy_file(from, to, session_id).await
            }
            Operation::Delete => {
                let path = require_path(&path, "DELETE")?;
                self.delete(path, session_id).await
            }
            Operation::Rename => {
                let (from, to) = require_pair(&path, &target, "RENAME")?;
                self.move_file(from, to, session_id).await
            }
            Operation::Symlink => {
                let (target, link) = require_pair(&path, &target, "SYMLINK")?;
                self.create_symlink(target, link, session_id).await
            }
            Operation::WriteFile => {
                let path = require_path(&path, "WRITE_FILE")?;
                let content = step
                    .content
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("WRITE_FILE step requires `content`"))?;
                self.write_file(path, content, session_id).await
            }
            // Deliberate: RUN_COMMAND is neither a file operation nor a
            // reversible one, so it never goes through `FsOps`. The command
            // runner in `app::dispatch_step` intercepts it before this point;
            // reaching here means a wiring bug, so fail loudly rather than
            // silently no-op.
            Operation::RunCommand => Err(anyhow::anyhow!(
                "RUN_COMMAND is executed by the command runner, not by FsOps"
            )),
        }
    }

    /// Answer a read-only LIST/STAT query. Never mutates the filesystem, so -
    /// unlike `execute_step` - this doesn't need `&mut self` or a
    /// `session_id`; there's nothing to undo.
    pub async fn run_query(&self, query: &FsQuery) -> Result<QueryResult> {
        let path = expand_tilde(&query.path);
        let entries = match query.query {
            QueryOp::List => self.list_dir(&path).await?,
            QueryOp::Stat => vec![self.stat(&path).await?],
        };
        let truncated = query.query == QueryOp::List && entries.len() >= MAX_LIST_ENTRIES;
        Ok(QueryResult {
            query: query.query,
            path: query.path.clone(),
            entries,
            truncated,
        })
    }

    /// List the direct children of a directory (not recursive), capped at
    /// `MAX_LIST_ENTRIES`.
    pub async fn list_dir(&self, path: &Path) -> Result<Vec<FileMetadata>> {
        let mut entries = Vec::new();
        let mut read_dir = fs::read_dir(path).await?;
        while entries.len() < MAX_LIST_ENTRIES {
            let Some(entry) = read_dir.next_entry().await? else {
                break;
            };
            entries.push(metadata_for(entry.path()).await?);
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    /// Metadata for exactly the given path.
    pub async fn stat(&self, path: &Path) -> Result<FileMetadata> {
        metadata_for(path.to_path_buf()).await
    }

    /// Create directory and all parents
    pub async fn create_dir_all(&mut self, path: &Path, session_id: &str) -> Result<()> {
        fs::create_dir_all(path).await?;
        self.log_undo(
            session_id,
            UndoOperation::CreateDir {
                path: path.to_path_buf(),
            },
        );
        Ok(())
    }

    /// Move/rename a file or directory
    pub async fn move_file(&mut self, from: &Path, to: &Path, session_id: &str) -> Result<()> {
        fs::rename(from, to).await?;
        self.log_undo(
            session_id,
            UndoOperation::Move {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
            },
        );
        Ok(())
    }

    /// Copy a file
    pub async fn copy_file(&mut self, from: &Path, to: &Path, session_id: &str) -> Result<()> {
        fs::copy(from, to).await?;
        self.log_undo(
            session_id,
            UndoOperation::Copy {
                to: to.to_path_buf(),
            },
        );
        Ok(())
    }

    /// Delete a file or directory (with backup for undo)
    pub async fn delete(&mut self, path: &Path, session_id: &str) -> Result<()> {
        let backup = path.with_extension(format!("bak.{}", Uuid::new_v4()));
        fs::rename(path, &backup).await?;
        self.log_undo(
            session_id,
            UndoOperation::Delete {
                path: path.to_path_buf(),
                backup,
            },
        );
        Ok(())
    }

    /// Write literal text content to a new (or existing) file. If a file
    /// already exists at `path`, it's backed up first so undo can restore it;
    /// otherwise undo just removes the file this created.
    pub async fn write_file(&mut self, path: &Path, content: &str, session_id: &str) -> Result<()> {
        let previous = if fs::try_exists(path).await? {
            let backup = path.with_extension(format!("bak.{}", Uuid::new_v4()));
            fs::rename(path, &backup).await?;
            Some(backup)
        } else {
            None
        };
        fs::write(path, content).await?;
        self.log_undo(
            session_id,
            UndoOperation::WriteFile {
                path: path.to_path_buf(),
                previous,
            },
        );
        Ok(())
    }

    /// Create a symbolic link
    pub async fn create_symlink(
        &mut self,
        target: &Path,
        link: &Path,
        session_id: &str,
    ) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(target, link)?;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::symlink_file;
            symlink_file(target, link)?;
        }
        self.log_undo(
            session_id,
            UndoOperation::Symlink {
                link: link.to_path_buf(),
            },
        );
        Ok(())
    }

    /// Undo every operation recorded under `session_id`, most recent first.
    pub async fn undo(&mut self, session_id: &str) -> Result<()> {
        let entries: Vec<_> = self
            .undo_log
            .iter()
            .filter(|e| e.session_id == session_id)
            .cloned()
            .collect();

        for entry in entries.into_iter().rev() {
            self.reverse_operation(&entry.operation).await?;
        }
        self.undo_log.retain(|e| e.session_id != session_id);
        Ok(())
    }

    async fn reverse_operation(&self, op: &UndoOperation) -> Result<()> {
        match op {
            UndoOperation::CreateDir { path } => {
                if path.exists() {
                    fs::remove_dir_all(path).await?;
                }
            }
            UndoOperation::Move { from, to } => {
                if to.exists() {
                    fs::rename(to, from).await?;
                }
            }
            UndoOperation::Copy { to } => {
                if to.exists() {
                    fs::remove_file(to).await?;
                }
            }
            UndoOperation::Delete { path, backup } => {
                if backup.exists() {
                    fs::rename(backup, path).await?;
                }
            }
            UndoOperation::Symlink { link } => {
                if link.exists() {
                    fs::remove_file(link).await?;
                }
            }
            UndoOperation::WriteFile { path, previous } => {
                if path.exists() {
                    fs::remove_file(path).await?;
                }
                if let Some(backup) = previous {
                    if backup.exists() {
                        fs::rename(backup, path).await?;
                    }
                }
            }
        }
        Ok(())
    }

    fn log_undo(&mut self, session_id: &str, operation: UndoOperation) {
        self.undo_log.push(UndoEntry {
            session_id: session_id.to_string(),
            operation,
        });
    }
}

impl Default for FsOps {
    fn default() -> Self {
        Self::new()
    }
}

/// Expand a leading `~` to the user's home directory. Leaves the path
/// unchanged if it doesn't start with `~`, or if the home directory can't be
/// determined (in which case the original, almost-certainly-still-wrong path
/// is kept so the resulting error still names the path the model requested).
fn expand_tilde(path: &Path) -> PathBuf {
    let Ok(rest) = path.strip_prefix("~") else {
        return path.to_path_buf();
    };
    match dirs::home_dir() {
        Some(home) => home.join(rest),
        None => path.to_path_buf(),
    }
}

/// Build a `FileMetadata` for `path`. `created`/`modified` are best-effort:
/// `SystemTime::created`/`modified` return an error on filesystems that
/// don't track them, in which case the field is just left `None` rather than
/// failing the whole query.
async fn metadata_for(path: PathBuf) -> Result<FileMetadata> {
    let meta = fs::metadata(&path).await?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(FileMetadata {
        name,
        path,
        is_dir: meta.is_dir(),
        size: meta.len(),
        created: meta.created().ok().map(system_time_to_rfc3339),
        modified: meta.modified().ok().map(system_time_to_rfc3339),
    })
}

fn system_time_to_rfc3339(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339()
}

fn require_path<'a>(path: &'a Option<PathBuf>, op: &str) -> Result<&'a Path> {
    path.as_deref()
        .ok_or_else(|| anyhow::anyhow!("{op} step requires `path`"))
}

fn require_pair<'a>(
    path: &'a Option<PathBuf>,
    target: &'a Option<PathBuf>,
    op: &str,
) -> Result<(&'a Path, &'a Path)> {
    let path = require_path(path, op)?;
    let target = target
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("{op} step requires `target`"))?;
    Ok((path, target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_leading_tilde() {
        let home = dirs::home_dir().expect("test environment must have a home dir");
        assert_eq!(
            expand_tilde(Path::new("~/Downloads/foo.txt")),
            home.join("Downloads/foo.txt")
        );
        assert_eq!(expand_tilde(Path::new("~")), home);
    }

    #[test]
    fn leaves_non_tilde_paths_unchanged() {
        assert_eq!(
            expand_tilde(Path::new("/Users/x/Downloads/foo.txt")),
            Path::new("/Users/x/Downloads/foo.txt")
        );
        assert_eq!(
            expand_tilde(Path::new("relative/path.txt")),
            Path::new("relative/path.txt")
        );
    }

    #[tokio::test]
    async fn lists_direct_children_only() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), b"a").await.unwrap();
        fs::write(dir.path().join("b.txt"), b"bb").await.unwrap();
        fs::create_dir(dir.path().join("sub")).await.unwrap();
        fs::write(dir.path().join("sub/c.txt"), b"ccc")
            .await
            .unwrap();

        let ops = FsOps::new();
        let entries = ops.list_dir(dir.path()).await.unwrap();

        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "b.txt", "sub"]);
        let sub = entries.iter().find(|e| e.name == "sub").unwrap();
        assert!(sub.is_dir);
        let a = entries.iter().find(|e| e.name == "a.txt").unwrap();
        assert!(!a.is_dir);
        assert_eq!(a.size, 1);
    }

    #[tokio::test]
    async fn stat_reports_metadata_for_exact_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hello.txt");
        fs::write(&file, b"hello").await.unwrap();

        let ops = FsOps::new();
        let meta = ops.stat(&file).await.unwrap();

        assert_eq!(meta.name, "hello.txt");
        assert_eq!(meta.size, 5);
        assert!(!meta.is_dir);
    }

    #[tokio::test]
    async fn run_query_dispatches_list_and_stat() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("only.txt"), b"x").await.unwrap();

        let ops = FsOps::new();

        let list_result = ops
            .run_query(&FsQuery {
                query: QueryOp::List,
                path: dir.path().to_path_buf(),
            })
            .await
            .unwrap();
        assert_eq!(list_result.entries.len(), 1);
        assert!(!list_result.truncated);

        let stat_result = ops
            .run_query(&FsQuery {
                query: QueryOp::Stat,
                path: dir.path().join("only.txt"),
            })
            .await
            .unwrap();
        assert_eq!(stat_result.entries.len(), 1);
        assert_eq!(stat_result.entries[0].name, "only.txt");
    }
}
