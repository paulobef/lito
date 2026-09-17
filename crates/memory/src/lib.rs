//! Memory Store - SQLite persistence for sessions, plans, history, and undo

use anyhow::Result;
use chrono::Utc;
use lito_protocol::PlanStep;
use rusqlite::{params, Connection, Result as SqliteResult};
use std::path::PathBuf;
use std::sync::Mutex;

/// Memory store for sessions, conversation history, plans, and undo logs.
///
/// The connection is guarded by a `Mutex` (rather than exposed as a bare
/// `rusqlite::Connection`, which is `Send` but not `Sync`) so a `&MemoryStore`
/// can be held across `.await` points in the core agent's tokio task.
pub struct MemoryStore {
    conn: Mutex<Connection>,
}

/// A row in the session list - enough to render a sidebar entry without a
/// second round-trip. `step_count` is the step count of the session's most
/// recent plan, if it has one yet.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub updated_at: i64,
    pub step_count: Option<usize>,
}

/// The most recent plan proposed in a session, used to restore the plan
/// inspector panel when switching back into that session.
#[derive(Debug, Clone)]
pub struct PlanRecord {
    pub plan_id: String,
    pub steps: Vec<PlanStep>,
    pub status: String,
    pub undo_id: Option<String>,
}

impl MemoryStore {
    /// Create a new memory store
    pub fn new(path: Option<PathBuf>) -> Result<Self> {
        let db_path = path.unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("lito")
                .join("memory.sqlite")
        });

        std::fs::create_dir_all(db_path.parent().unwrap())?;

        let conn = Connection::open(&db_path)?;

        // Initialize schema
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS plans (
                id TEXT PRIMARY KEY,
                prompt TEXT NOT NULL,
                context TEXT NOT NULL,
                steps TEXT NOT NULL, -- JSON
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                executed_at INTEGER,
                undo_id TEXT
            );

            CREATE TABLE IF NOT EXISTS undo_log (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                action TEXT NOT NULL, -- JSON
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                thinking TEXT,
                created_at INTEGER NOT NULL
            );
        "#,
        )?;

        // Columns added after their tables first shipped, so `CREATE TABLE IF
        // NOT EXISTS` above won't backfill them into a database that already
        // has the table. Add them separately and ignore the "duplicate
        // column" error on a database that already has them from the
        // statement above (or from a previous run of this code).
        for (table, column, ty) in [
            ("messages", "thinking", "TEXT"),
            ("messages", "session_id", "TEXT"),
            ("plans", "session_id", "TEXT"),
        ] {
            if let Err(e) =
                conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {ty}"), [])
            {
                if !e.to_string().contains("duplicate column name") {
                    return Err(e.into());
                }
            }
        }

        Self::backfill_default_session(&conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Pre-session-support databases have `messages`/`plans` rows with no
    /// `session_id`. If we find any and no session exists yet, group them
    /// all into one recovered session rather than losing/orphaning that
    /// history.
    fn backfill_default_session(conn: &Connection) -> Result<()> {
        let legacy_messages: i64 = conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id IS NULL",
            [],
            |row| row.get(0),
        )?;
        let legacy_plans: i64 = conn.query_row(
            "SELECT COUNT(*) FROM plans WHERE session_id IS NULL",
            [],
            |row| row.get(0),
        )?;
        let session_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;

        if (legacy_messages == 0 && legacy_plans == 0) || session_count > 0 {
            return Ok(());
        }

        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().timestamp();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![id, "Conversation précédente", now],
        )?;
        conn.execute(
            "UPDATE messages SET session_id = ?1 WHERE session_id IS NULL",
            params![id],
        )?;
        conn.execute(
            "UPDATE plans SET session_id = ?1 WHERE session_id IS NULL",
            params![id],
        )?;

        Ok(())
    }

    /// Create a new session and return its id.
    pub fn create_session(&self, title: &str) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().timestamp();
        self.conn.lock().unwrap().execute(
            "INSERT INTO sessions (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![id, title, now],
        )?;
        Ok(id)
    }

    /// Bump a session's `updated_at` to now - called whenever it sees new
    /// activity (a message, a plan execution) so the most recently active
    /// session sorts first.
    pub fn touch_session(&self, session_id: &str) -> Result<()> {
        let now = Utc::now().timestamp();
        self.conn.lock().unwrap().execute(
            "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
            params![now, session_id],
        )?;
        Ok(())
    }

    /// Overwrite a session's title (e.g. once derived from its first user
    /// message).
    pub fn set_session_title(&self, session_id: &str, title: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE sessions SET title = ?1 WHERE id = ?2",
            params![title, session_id],
        )?;
        Ok(())
    }

    /// List all sessions, most recently active first, each with its latest
    /// plan's step count (if it has a plan yet) for the sidebar subtitle.
    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let rows: Vec<(String, String, i64)> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn
                .prepare("SELECT id, title, updated_at FROM sessions ORDER BY updated_at DESC")?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<SqliteResult<Vec<_>>>()?;
            rows
        };

        rows.into_iter()
            .map(|(id, title, updated_at)| {
                let step_count = self
                    .latest_plan_for_session(&id)?
                    .map(|plan| plan.steps.len());
                Ok(SessionSummary {
                    id,
                    title,
                    updated_at,
                    step_count,
                })
            })
            .collect()
    }

    /// The most recently created plan for a session, if any - used to
    /// restore the plan inspector panel when switching back into a session.
    pub fn latest_plan_for_session(&self, session_id: &str) -> Result<Option<PlanRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, steps, status, undo_id FROM plans WHERE session_id = ?1 \
             ORDER BY created_at DESC LIMIT 1",
        )?;
        let row = match stmt.query_row(params![session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        }) {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        let (plan_id, steps_json, status, undo_id) = row;
        let steps: Vec<PlanStep> = serde_json::from_str(&steps_json)?;
        Ok(Some(PlanRecord {
            plan_id,
            steps,
            status,
            undo_id,
        }))
    }

    /// Which session a given plan belongs to, if known - used to touch the
    /// right session after executing a plan.
    pub fn get_plan_session_id(&self, plan_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT session_id FROM plans WHERE id = ?1")?;
        match stmt.query_row(params![plan_id], |row| row.get(0)) {
            Ok(id) => Ok(id),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Save a plan to memory, scoped to the session that requested it.
    pub fn save_plan(
        &self,
        session_id: &str,
        plan_id: &str,
        prompt: &str,
        context: &str,
        steps: &[PlanStep],
    ) -> Result<()> {
        let now = Utc::now().timestamp();
        let steps_json = serde_json::to_string(steps)?;

        self.conn.lock().unwrap().execute(
            "INSERT INTO plans (id, session_id, prompt, context, steps, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![plan_id, session_id, prompt, context, steps_json, "pending", now],
        )?;

        Ok(())
    }

    /// Update plan status
    pub fn update_plan_status(
        &self,
        plan_id: &str,
        status: &str,
        undo_id: Option<String>,
    ) -> Result<()> {
        let now = Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();

        if let Some(undo_id) = undo_id {
            conn.execute(
                "UPDATE plans SET status = ?1, executed_at = ?2, undo_id = ?3 WHERE id = ?4",
                params![status, now, undo_id, plan_id],
            )?;
        } else {
            conn.execute(
                "UPDATE plans SET status = ?1, executed_at = ?2 WHERE id = ?3",
                params![status, now, plan_id],
            )?;
        }

        Ok(())
    }

    /// Get a plan by ID
    pub fn get_plan(&self, plan_id: &str) -> Result<Option<Vec<PlanStep>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT steps FROM plans WHERE id = ?1")?;
        let steps_json: String = match stmt.query_row(params![plan_id], |row| row.get(0)) {
            Ok(json) => json,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        let steps: Vec<PlanStep> = serde_json::from_str(&steps_json)?;
        Ok(Some(steps))
    }

    /// Log an undo action
    pub fn log_undo(&self, session_id: &str, action: &str) -> Result<()> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().timestamp();

        self.conn.lock().unwrap().execute(
            "INSERT INTO undo_log (id, session_id, action, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, session_id, action, now],
        )?;

        Ok(())
    }

    /// Get undo actions for a session
    pub fn get_undo_actions(&self, session_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT action FROM undo_log WHERE session_id = ?1 ORDER BY created_at DESC",
        )?;

        let actions = stmt
            .query_map(params![session_id], |row| row.get(0))?
            .collect::<SqliteResult<Vec<String>>>()?;

        Ok(actions)
    }

    /// Persist one turn of the conversation (role is "user" or "assistant"),
    /// scoped to the session it happened in. `thinking` is the reasoning
    /// trace a "thinking" model emitted for this turn, if any - always
    /// `None` for user turns.
    pub fn save_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        thinking: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().timestamp();
        self.conn.lock().unwrap().execute(
            "INSERT INTO messages (session_id, role, content, thinking, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, role, content, thinking, now],
        )?;
        Ok(())
    }

    /// Fetch the most recent `limit` conversation turns for one session,
    /// oldest first, as `(role, content, thinking)` triples - ready to be
    /// mapped into whatever chat message type the caller's LLM client
    /// expects.
    pub fn get_recent_messages_for_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<(String, String, Option<String>)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT role, content, thinking FROM messages WHERE session_id = ?1 \
             ORDER BY id DESC LIMIT ?2",
        )?;
        let mut rows = stmt
            .query_map(params![session_id, limit as i64], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<SqliteResult<Vec<(String, String, Option<String>)>>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// Get a persisted setting by key
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
        match stmt.query_row(params![key], |row| row.get(0)) {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Persist a setting by key, overwriting any existing value
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lito_protocol::{Operation, PlanStep};
    use tempfile::tempdir;

    #[test]
    fn test_memory_store() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(Some(dir.path().join("test.sqlite"))).unwrap();
        let session_id = store.create_session("Nouvelle tâche").unwrap();

        let steps = vec![PlanStep {
            id: "1".to_string(),
            description: "Test step".to_string(),
            operation: Operation::Mkdir,
            path: Some("/tmp/test".into()),
            target: None,
            content: None,
            argv: None,
            shell: false,
            cwd: None,
        }];

        store
            .save_plan(&session_id, "plan1", "Test prompt", "~/Documents", &steps)
            .unwrap();
        let retrieved = store.get_plan("plan1").unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap()[0].description, "Test step");
    }

    #[test]
    fn test_message_thinking_survives_restart() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let session_id;

        {
            let store = MemoryStore::new(Some(db_path.clone())).unwrap();
            session_id = store.create_session("Nouvelle tâche").unwrap();
            store
                .save_message(&session_id, "user", "What is 12*7?", None)
                .unwrap();
            store
                .save_message(
                    &session_id,
                    "assistant",
                    "84",
                    Some("12*7 = 84, let me double check..."),
                )
                .unwrap();
        }

        // Reopen fresh, like a real app restart would.
        let store = MemoryStore::new(Some(db_path)).unwrap();
        let rows = store
            .get_recent_messages_for_session(&session_id, 20)
            .unwrap();

        assert_eq!(
            rows,
            vec![
                ("user".to_string(), "What is 12*7?".to_string(), None),
                (
                    "assistant".to_string(),
                    "84".to_string(),
                    Some("12*7 = 84, let me double check...".to_string())
                ),
            ]
        );
    }

    #[test]
    fn sessions_have_isolated_histories() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(Some(dir.path().join("test.sqlite"))).unwrap();

        let a = store.create_session("Session A").unwrap();
        let b = store.create_session("Session B").unwrap();

        store
            .save_message(&a, "user", "hello from A", None)
            .unwrap();
        store
            .save_message(&b, "user", "hello from B", None)
            .unwrap();

        let a_rows = store.get_recent_messages_for_session(&a, 20).unwrap();
        let b_rows = store.get_recent_messages_for_session(&b, 20).unwrap();

        assert_eq!(a_rows.len(), 1);
        assert_eq!(a_rows[0].1, "hello from A");
        assert_eq!(b_rows.len(), 1);
        assert_eq!(b_rows[0].1, "hello from B");
    }

    #[test]
    fn list_sessions_orders_by_most_recently_touched_and_reports_step_count() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(Some(dir.path().join("test.sqlite"))).unwrap();

        let older = store.create_session("Older").unwrap();
        let newer = store.create_session("Newer").unwrap();

        let steps = vec![PlanStep {
            id: "1".to_string(),
            description: "step".to_string(),
            operation: Operation::Mkdir,
            path: Some("/tmp/test".into()),
            target: None,
            content: None,
            argv: None,
            shell: false,
            cwd: None,
        }];
        store
            .save_plan(&older, "plan1", "prompt", "", &steps)
            .unwrap();

        // `updated_at` has second resolution, so touch `older` explicitly
        // rather than racing the clock to prove reordering works.
        store.touch_session(&older).unwrap();

        let sessions = store.list_sessions().unwrap();
        assert_eq!(sessions[0].id, older);
        assert_eq!(sessions[0].step_count, Some(1));
        assert_eq!(sessions[1].id, newer);
        assert_eq!(sessions[1].step_count, None);
    }

    #[test]
    fn legacy_rows_without_a_session_are_recovered_on_reopen() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");

        {
            // Simulate a pre-session database by writing straight to a
            // freshly-initialized store's connection, bypassing the
            // session-aware helpers.
            let store = MemoryStore::new(Some(db_path.clone())).unwrap();
            store
                .conn
                .lock()
                .unwrap()
                .execute(
                    "INSERT INTO messages (role, content, created_at) VALUES ('user', 'legacy message', 0)",
                    [],
                )
                .unwrap();
        }

        let store = MemoryStore::new(Some(db_path)).unwrap();
        let sessions = store.list_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        let rows = store
            .get_recent_messages_for_session(&sessions[0].id, 20)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "legacy message");
    }
}
