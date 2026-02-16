use std::path::Path;

use async_trait::async_trait;
use eyre::Result;
use fs_err as fs;
use sqlx::{
    Row,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow},
};
use time::OffsetDateTime;

use crate::{Memory, MemoryCommand};

/// Database operations for the memory store
#[async_trait]
pub trait MemoryDatabase: Send + Sync + 'static {
    /// Create a new memory
    async fn create(&self, memory: &Memory) -> Result<()>;

    /// Get a memory by ID
    async fn get(&self, id: &str) -> Result<Option<Memory>>;

    /// List all memories, optionally filtered
    async fn list(
        &self,
        repo_root: Option<&str>,
        cwd: Option<&str>,
        agent_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<Memory>>;

    /// Search memories by description (FTS)
    async fn search(&self, query: &str, repo_root: Option<&str>) -> Result<Vec<Memory>>;

    /// Search memories by linked command pattern
    async fn search_by_command(
        &self,
        command_pattern: &str,
        repo_root: Option<&str>,
    ) -> Result<Vec<Memory>>;

    /// Link a command to a memory
    async fn link_command(&self, memory_id: &str, history_id: &str) -> Result<()>;

    /// Get all commands linked to a memory
    async fn get_linked_commands(&self, memory_id: &str) -> Result<Vec<String>>;

    /// Get the count of linked commands for a memory
    async fn get_linked_command_count(&self, memory_id: &str) -> Result<usize>;

    /// Delete a memory and its command links
    async fn delete(&self, id: &str) -> Result<()>;
}

/// SQLite implementation of the memory database
#[derive(Debug, Clone)]
pub struct SqliteMemoryDb {
    pool: SqlitePool,
}

impl SqliteMemoryDb {
    pub async fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        if !path.exists() {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
        }

        let opts = SqliteConnectOptions::new()
            .filename(path)
            .journal_mode(SqliteJournalMode::Wal)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;

        Self::setup_db(&pool).await?;

        Ok(Self { pool })
    }

    async fn setup_db(pool: &SqlitePool) -> Result<()> {
        // Create tables
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                description TEXT NOT NULL,
                cwd TEXT NOT NULL,
                repo_root TEXT,
                git_branch TEXT,
                git_commit TEXT,
                agent_id TEXT,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS memory_commands (
                memory_id TEXT NOT NULL,
                history_id TEXT NOT NULL,
                PRIMARY KEY (memory_id, history_id),
                FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_memories_repo ON memories(repo_root);
            CREATE INDEX IF NOT EXISTS idx_memories_cwd ON memories(cwd);
            CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at);
            CREATE INDEX IF NOT EXISTS idx_memories_agent ON memories(agent_id);
            "#,
        )
        .execute(pool)
        .await?;

        // Create FTS virtual table if it doesn't exist
        sqlx::query(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                description,
                content=memories,
                content_rowid=rowid
            );
            "#,
        )
        .execute(pool)
        .await?;

        // Create triggers to keep FTS in sync
        sqlx::query(
            r#"
            CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, description) VALUES (new.rowid, new.description);
            END;
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, description) VALUES ('delete', old.rowid, old.description);
            END;
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, description) VALUES ('delete', old.rowid, old.description);
                INSERT INTO memories_fts(rowid, description) VALUES (new.rowid, new.description);
            END;
            "#,
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    fn row_to_memory(row: SqliteRow) -> Memory {
        let created_at_nanos: i64 = row.get("created_at");

        Memory {
            id: row.get("id"),
            description: row.get("description"),
            cwd: row.get("cwd"),
            repo_root: row.get("repo_root"),
            git_branch: row.get("git_branch"),
            git_commit: row.get("git_commit"),
            agent_id: row.get("agent_id"),
            created_at: OffsetDateTime::from_unix_timestamp_nanos(created_at_nanos as i128)
                .unwrap_or_else(|_| OffsetDateTime::now_utc()),
        }
    }
}

#[async_trait]
impl MemoryDatabase for SqliteMemoryDb {
    async fn create(&self, memory: &Memory) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO memories (id, description, cwd, repo_root, git_branch, git_commit, agent_id, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
        )
        .bind(&memory.id)
        .bind(&memory.description)
        .bind(&memory.cwd)
        .bind(&memory.repo_root)
        .bind(&memory.git_branch)
        .bind(&memory.git_commit)
        .bind(&memory.agent_id)
        .bind(memory.created_at.unix_timestamp_nanos() as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<Memory>> {
        let result = sqlx::query("SELECT * FROM memories WHERE id = ?1")
            .bind(id)
            .map(Self::row_to_memory)
            .fetch_optional(&self.pool)
            .await?;

        Ok(result)
    }

    async fn list(
        &self,
        repo_root: Option<&str>,
        cwd: Option<&str>,
        agent_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<Memory>> {
        let mut query = String::from("SELECT * FROM memories WHERE 1=1");

        if repo_root.is_some() {
            query.push_str(" AND repo_root = ?1");
        }
        if cwd.is_some() {
            query.push_str(" AND cwd = ?2");
        }
        if agent_id.is_some() {
            query.push_str(" AND agent_id = ?3");
        }

        query.push_str(" ORDER BY created_at DESC");

        if let Some(limit) = limit {
            query.push_str(&format!(" LIMIT {}", limit));
        }

        let mut q = sqlx::query(&query);

        if let Some(repo) = repo_root {
            q = q.bind(repo);
        }
        if let Some(cwd) = cwd {
            q = q.bind(cwd);
        }
        if let Some(agent) = agent_id {
            q = q.bind(agent);
        }

        let results = q.map(Self::row_to_memory).fetch_all(&self.pool).await?;
        Ok(results)
    }

    async fn search(&self, query: &str, repo_root: Option<&str>) -> Result<Vec<Memory>> {
        let sql = if repo_root.is_some() {
            r#"
            SELECT m.* FROM memories m
            JOIN memories_fts fts ON m.rowid = fts.rowid
            WHERE memories_fts MATCH ?1 AND m.repo_root = ?2
            ORDER BY rank
            "#
        } else {
            r#"
            SELECT m.* FROM memories m
            JOIN memories_fts fts ON m.rowid = fts.rowid
            WHERE memories_fts MATCH ?1
            ORDER BY rank
            "#
        };

        let mut q = sqlx::query(sql).bind(query);

        if let Some(repo) = repo_root {
            q = q.bind(repo);
        }

        let results = q.map(Self::row_to_memory).fetch_all(&self.pool).await?;
        Ok(results)
    }

    async fn search_by_command(
        &self,
        command_pattern: &str,
        repo_root: Option<&str>,
    ) -> Result<Vec<Memory>> {
        // This would need to join with the history table
        // For now, just return memories that have linked commands matching the pattern
        let sql = if repo_root.is_some() {
            r#"
            SELECT DISTINCT m.* FROM memories m
            JOIN memory_commands mc ON m.id = mc.memory_id
            WHERE mc.history_id LIKE ?1 AND m.repo_root = ?2
            ORDER BY m.created_at DESC
            "#
        } else {
            r#"
            SELECT DISTINCT m.* FROM memories m
            JOIN memory_commands mc ON m.id = mc.memory_id
            WHERE mc.history_id LIKE ?1
            ORDER BY m.created_at DESC
            "#
        };

        let pattern = format!("%{}%", command_pattern);
        let mut q = sqlx::query(sql).bind(&pattern);

        if let Some(repo) = repo_root {
            q = q.bind(repo);
        }

        let results = q.map(Self::row_to_memory).fetch_all(&self.pool).await?;
        Ok(results)
    }

    async fn link_command(&self, memory_id: &str, history_id: &str) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO memory_commands (memory_id, history_id) VALUES (?1, ?2)",
        )
        .bind(memory_id)
        .bind(history_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_linked_commands(&self, memory_id: &str) -> Result<Vec<String>> {
        let results: Vec<(String,)> =
            sqlx::query_as("SELECT history_id FROM memory_commands WHERE memory_id = ?1")
                .bind(memory_id)
                .fetch_all(&self.pool)
                .await?;

        Ok(results.into_iter().map(|(id,)| id).collect())
    }

    async fn get_linked_command_count(&self, memory_id: &str) -> Result<usize> {
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM memory_commands WHERE memory_id = ?1")
                .bind(memory_id)
                .fetch_one(&self.pool)
                .await?;

        Ok(count.0 as usize)
    }

    async fn delete(&self, id: &str) -> Result<()> {
        // Foreign key cascade will delete memory_commands entries
        sqlx::query("DELETE FROM memories WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}
