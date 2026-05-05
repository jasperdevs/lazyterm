use std::path::Path;

use lazyterm_core::{SessionId, SessionSummary};
use rusqlite::{params, Connection};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SessionStoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("session payload for {id} failed to deserialize: {source}")]
    CorruptSession {
        id: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("session json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, SessionStoreError>;

pub struct SessionStore {
    connection: Connection,
}

impl SessionStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path)?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_memory() -> Result<Self> {
        let connection = Connection::open_in_memory()?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn save(&self, summary: &SessionSummary) -> Result<()> {
        let payload = serde_json::to_string(summary)?;
        self.connection.execute(
            "insert into sessions (id, payload) values (?1, ?2)
             on conflict(id) do update set payload = excluded.payload",
            params![summary.id.as_str(), payload],
        )?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<SessionSummary>> {
        let mut statement = self
            .connection
            .prepare("select id, payload from sessions order by id asc")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut sessions = Vec::new();
        for row in rows {
            let (id, payload) = row?;
            sessions.push(parse_session_summary(id, payload)?);
        }
        Ok(sessions)
    }

    pub fn delete(&self, id: &SessionId) -> Result<()> {
        self.connection
            .execute("delete from sessions where id = ?1", params![id.as_str()])?;
        Ok(())
    }

    fn migrate(&self) -> Result<()> {
        self.connection.execute_batch(
            "create table if not exists sessions (
                id text primary key not null,
                payload text not null
            );",
        )?;
        Ok(())
    }
}

fn parse_session_summary(id: String, payload: String) -> Result<SessionSummary> {
    serde_json::from_str(&payload)
        .map_err(|source| SessionStoreError::CorruptSession { id, source })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use lazyterm_core::{AgentKind, SessionId, SessionStatus, SessionSummary, WorkspaceRef};

    use super::*;

    #[test]
    fn round_trips_sessions() {
        let store = SessionStore::open_memory().expect("store opens");
        let session = SessionSummary {
            id: SessionId::new("test-session"),
            title: "Test session".into(),
            agent: AgentKind::Codex,
            status: SessionStatus::Running,
            workspace: WorkspaceRef {
                cwd: PathBuf::from("."),
                git_branch: Some("main".into()),
            },
            command: "codex".into(),
            last_activity: "testing".into(),
            notification: None,
        };

        store.save(&session).expect("session saves");

        assert_eq!(store.list().expect("sessions list"), vec![session]);
    }

    #[test]
    fn save_overwrites_existing_session() {
        let store = SessionStore::open_memory().expect("store opens");
        let session = SessionSummary {
            id: SessionId::new("test-session"),
            title: "First title".into(),
            agent: AgentKind::Codex,
            status: SessionStatus::Running,
            workspace: WorkspaceRef {
                cwd: PathBuf::from("."),
                git_branch: Some("main".into()),
            },
            command: "codex".into(),
            last_activity: "first".into(),
            notification: None,
        };
        let updated = SessionSummary {
            title: "Updated title".into(),
            status: SessionStatus::Done,
            last_activity: "second".into(),
            ..session.clone()
        };

        store.save(&session).expect("initial session saves");
        store.save(&updated).expect("updated session saves");

        assert_eq!(store.list().expect("sessions list"), vec![updated]);
    }

    #[test]
    fn list_reports_corrupt_row_with_session_id() {
        let store = SessionStore::open_memory().expect("store opens");
        store
            .connection
            .execute(
                "insert into sessions (id, payload) values (?1, ?2)",
                params!["broken-session", "{not valid json}"],
            )
            .expect("row inserts");

        let error = store.list().expect_err("list should fail");

        match error {
            SessionStoreError::CorruptSession { id, source } => {
                assert_eq!(id, "broken-session");
                assert!(source.is_syntax());
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}
