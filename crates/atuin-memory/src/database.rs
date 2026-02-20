use std::path::Path;
use std::str::FromStr;

use async_trait::async_trait;
use eyre::Result;
use fs_err as fs;
use sqlx::{
    Row,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow},
};
use time::OffsetDateTime;

use crate::Memory;

/// Database operations for the memory store
#[async_trait]
pub trait MemoryDatabase: Send + Sync + 'static {
    /// Create a new memory
    async fn create(&self, memory: &Memory) -> Result<()>;

    /// Get a memory by ID
    async fn get(&self, id: &str) -> Result<Option<Memory>>;

    /// Check if a memory exists
    async fn exists(&self, id: &str) -> Result<bool>;

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

    /// Get all children of a memory
    async fn get_children(&self, parent_id: &str) -> Result<Vec<Memory>>;

    /// Get all ancestors of a memory (parent chain to root)
    async fn get_ancestors(&self, id: &str) -> Result<Vec<Memory>>;

    /// Get a tree of memories starting from root_id (or all roots if None)
    async fn get_tree(
        &self,
        root_id: Option<&str>,
        max_depth: Option<usize>,
    ) -> Result<Vec<Memory>>;

    /// Get all root memories (memories with no parent)
    async fn get_roots(&self, limit: Option<usize>) -> Result<Vec<Memory>>;
}

/// SQLite implementation of the memory database
#[derive(Debug, Clone)]
pub struct SqliteMemoryDb {
    pool: SqlitePool,
}

impl SqliteMemoryDb {
    pub async fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let path_str = path.as_os_str().to_str().unwrap();

        // Only create directories for file-based databases, not in-memory
        if !path_str.starts_with("sqlite::") && !path.exists() {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
        }

        let opts = SqliteConnectOptions::from_str(path_str)?
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true)
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
                parent_memory_id TEXT,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (parent_memory_id) REFERENCES memories(id) ON DELETE SET NULL
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
            CREATE INDEX IF NOT EXISTS idx_memories_parent ON memories(parent_memory_id);
            "#,
        )
        .execute(pool)
        .await?;

        // Migration: Add parent_memory_id column if it doesn't exist (for existing databases)
        // SQLite doesn't support IF NOT EXISTS for ALTER TABLE, so we check first
        let column_exists: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM pragma_table_info('memories') WHERE name = 'parent_memory_id'",
        )
        .fetch_one(pool)
        .await?;

        if column_exists.0 == 0 {
            sqlx::query("ALTER TABLE memories ADD COLUMN parent_memory_id TEXT")
                .execute(pool)
                .await?;
        }

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
            parent_memory_id: row.get("parent_memory_id"),
            created_at: OffsetDateTime::from_unix_timestamp_nanos(created_at_nanos as i128)
                .unwrap_or_else(|_| OffsetDateTime::now_utc()),
        }
    }
}

#[async_trait]
impl MemoryDatabase for SqliteMemoryDb {
    async fn create(&self, memory: &Memory) -> Result<()> {
        // Validate parent exists if specified
        if let Some(ref parent_id) = memory.parent_memory_id {
            if !self.exists(parent_id).await? {
                return Err(eyre::eyre!("Parent memory not found: {}", parent_id));
            }
        }

        sqlx::query(
            r#"
            INSERT INTO memories (id, description, cwd, repo_root, git_branch, git_commit, agent_id, parent_memory_id, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
        )
        .bind(&memory.id)
        .bind(&memory.description)
        .bind(&memory.cwd)
        .bind(&memory.repo_root)
        .bind(&memory.git_branch)
        .bind(&memory.git_commit)
        .bind(&memory.agent_id)
        .bind(&memory.parent_memory_id)
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

    async fn exists(&self, id: &str) -> Result<bool> {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM memories WHERE id = ?1")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;

        Ok(count.0 > 0)
    }

    async fn list(
        &self,
        repo_root: Option<&str>,
        cwd: Option<&str>,
        agent_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<Memory>> {
        let mut query = String::from("SELECT * FROM memories WHERE 1=1");
        let mut param_num = 1;

        if repo_root.is_some() {
            query.push_str(&format!(" AND repo_root = ?{}", param_num));
            param_num += 1;
        }
        if cwd.is_some() {
            query.push_str(&format!(" AND cwd = ?{}", param_num));
            param_num += 1;
        }
        if agent_id.is_some() {
            query.push_str(&format!(" AND agent_id = ?{}", param_num));
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

    async fn get_children(&self, parent_id: &str) -> Result<Vec<Memory>> {
        let results = sqlx::query(
            "SELECT * FROM memories WHERE parent_memory_id = ?1 ORDER BY created_at DESC",
        )
        .bind(parent_id)
        .map(Self::row_to_memory)
        .fetch_all(&self.pool)
        .await?;

        Ok(results)
    }

    async fn get_ancestors(&self, id: &str) -> Result<Vec<Memory>> {
        let results = sqlx::query(
            r#"
            WITH RECURSIVE ancestors AS (
                SELECT * FROM memories WHERE id = ?1
                UNION ALL
                SELECT m.* FROM memories m
                INNER JOIN ancestors a ON m.id = a.parent_memory_id
            )
            SELECT * FROM ancestors WHERE id != ?1 ORDER BY created_at DESC
            "#,
        )
        .bind(id)
        .map(Self::row_to_memory)
        .fetch_all(&self.pool)
        .await?;

        Ok(results)
    }

    async fn get_tree(
        &self,
        root_id: Option<&str>,
        max_depth: Option<usize>,
    ) -> Result<Vec<Memory>> {
        let depth = max_depth.unwrap_or(10) as i64;

        let results = if let Some(root) = root_id {
            sqlx::query(
                r#"
                WITH RECURSIVE tree AS (
                    SELECT *, 0 as depth FROM memories WHERE id = ?1
                    UNION ALL
                    SELECT m.*, t.depth + 1 FROM memories m
                    INNER JOIN tree t ON m.parent_memory_id = t.id
                    WHERE t.depth < ?2
                )
                SELECT id, description, cwd, repo_root, git_branch, git_commit, agent_id, parent_memory_id, created_at
                FROM tree ORDER BY depth, created_at DESC
                "#,
            )
            .bind(root)
            .bind(depth)
            .map(Self::row_to_memory)
            .fetch_all(&self.pool)
            .await?
        } else {
            // Get all trees starting from roots
            sqlx::query(
                r#"
                WITH RECURSIVE tree AS (
                    SELECT *, 0 as depth FROM memories WHERE parent_memory_id IS NULL
                    UNION ALL
                    SELECT m.*, t.depth + 1 FROM memories m
                    INNER JOIN tree t ON m.parent_memory_id = t.id
                    WHERE t.depth < ?1
                )
                SELECT id, description, cwd, repo_root, git_branch, git_commit, agent_id, parent_memory_id, created_at
                FROM tree ORDER BY depth, created_at DESC
                "#,
            )
            .bind(depth)
            .map(Self::row_to_memory)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(results)
    }

    async fn get_roots(&self, limit: Option<usize>) -> Result<Vec<Memory>> {
        let mut query = String::from(
            "SELECT * FROM memories WHERE parent_memory_id IS NULL ORDER BY created_at DESC",
        );

        if let Some(limit) = limit {
            query.push_str(&format!(" LIMIT {}", limit));
        }

        let results = sqlx::query(&query)
            .map(Self::row_to_memory)
            .fetch_all(&self.pool)
            .await?;

        Ok(results)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use time::OffsetDateTime;

    /// Create a test memory with minimal fields
    fn test_memory(description: &str) -> Memory {
        Memory {
            id: atuin_common::utils::uuid_v7().as_simple().to_string(),
            description: description.to_string(),
            cwd: "/home/test".to_string(),
            repo_root: None,
            git_branch: None,
            git_commit: None,
            agent_id: None,
            parent_memory_id: None,
            created_at: OffsetDateTime::now_utc(),
        }
    }

    /// Create a test memory with a parent
    fn test_memory_with_parent(description: &str, parent_id: &str) -> Memory {
        Memory {
            id: atuin_common::utils::uuid_v7().as_simple().to_string(),
            description: description.to_string(),
            cwd: "/home/test".to_string(),
            repo_root: None,
            git_branch: None,
            git_commit: None,
            agent_id: None,
            parent_memory_id: Some(parent_id.to_string()),
            created_at: OffsetDateTime::now_utc(),
        }
    }

    /// Create a test memory with all fields populated
    fn test_memory_full(
        description: &str,
        cwd: &str,
        repo_root: Option<&str>,
        agent_id: Option<&str>,
    ) -> Memory {
        Memory {
            id: atuin_common::utils::uuid_v7().as_simple().to_string(),
            description: description.to_string(),
            cwd: cwd.to_string(),
            repo_root: repo_root.map(String::from),
            git_branch: Some("main".to_string()),
            git_commit: Some("abc123".to_string()),
            agent_id: agent_id.map(String::from),
            parent_memory_id: None,
            created_at: OffsetDateTime::now_utc(),
        }
    }

    /// Create an in-memory test database
    async fn test_db() -> SqliteMemoryDb {
        SqliteMemoryDb::new("sqlite::memory:").await.unwrap()
    }

    // ==================== Phase 1: Basic CRUD Tests ====================

    #[tokio::test]
    async fn test_create_and_get() {
        let db = test_db().await;
        let memory = test_memory("Test memory description");

        db.create(&memory).await.unwrap();

        let retrieved = db.get(&memory.id).await.unwrap().unwrap();
        assert_eq!(retrieved.id, memory.id);
        assert_eq!(retrieved.description, memory.description);
        assert_eq!(retrieved.cwd, memory.cwd);
        assert_eq!(retrieved.repo_root, memory.repo_root);
        assert_eq!(retrieved.git_branch, memory.git_branch);
        assert_eq!(retrieved.git_commit, memory.git_commit);
        assert_eq!(retrieved.agent_id, memory.agent_id);
    }

    #[tokio::test]
    async fn test_get_nonexistent() {
        let db = test_db().await;

        let result = db.get("nonexistent-id").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_delete() {
        let db = test_db().await;
        let memory = test_memory("Memory to delete");

        db.create(&memory).await.unwrap();
        assert!(db.get(&memory.id).await.unwrap().is_some());

        db.delete(&memory.id).await.unwrap();
        assert!(db.get(&memory.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_list_empty() {
        let db = test_db().await;

        let results = db.list(None, None, None, None).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_list_all() {
        let db = test_db().await;

        let memory1 = test_memory("First memory");
        let memory2 = test_memory("Second memory");
        let memory3 = test_memory("Third memory");

        db.create(&memory1).await.unwrap();
        // Small delay to ensure different created_at times
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        db.create(&memory2).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        db.create(&memory3).await.unwrap();

        let results = db.list(None, None, None, None).await.unwrap();
        assert_eq!(results.len(), 3);
        // Should be ordered by created_at DESC (newest first)
        assert_eq!(results[0].id, memory3.id);
        assert_eq!(results[1].id, memory2.id);
        assert_eq!(results[2].id, memory1.id);
    }

    // ==================== Phase 2: Filtering Tests ====================

    #[tokio::test]
    async fn test_list_with_limit() {
        let db = test_db().await;

        for i in 0..5 {
            let memory = test_memory(&format!("Memory {}", i));
            db.create(&memory).await.unwrap();
        }

        let results = db.list(None, None, None, Some(3)).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn test_list_filter_by_repo_root() {
        let db = test_db().await;

        let memory1 = test_memory_full("Memory in repo A", "/home/test", Some("/repo/a"), None);
        let memory2 = test_memory_full("Memory in repo B", "/home/test", Some("/repo/b"), None);
        let memory3 = test_memory_full(
            "Memory in repo A again",
            "/home/test",
            Some("/repo/a"),
            None,
        );

        db.create(&memory1).await.unwrap();
        db.create(&memory2).await.unwrap();
        db.create(&memory3).await.unwrap();

        let results = db.list(Some("/repo/a"), None, None, None).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(
            results
                .iter()
                .all(|m| m.repo_root == Some("/repo/a".to_string()))
        );
    }

    #[tokio::test]
    async fn test_list_filter_by_cwd() {
        let db = test_db().await;

        let memory1 = test_memory_full("Memory in dir A", "/dir/a", None, None);
        let memory2 = test_memory_full("Memory in dir B", "/dir/b", None, None);
        let memory3 = test_memory_full("Memory in dir A again", "/dir/a", None, None);

        db.create(&memory1).await.unwrap();
        db.create(&memory2).await.unwrap();
        db.create(&memory3).await.unwrap();

        let results = db.list(None, Some("/dir/a"), None, None).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|m| m.cwd == "/dir/a"));
    }

    #[tokio::test]
    async fn test_list_filter_by_agent_id() {
        let db = test_db().await;

        let memory1 = test_memory_full("Memory by agent A", "/home/test", None, Some("agent-a"));
        let memory2 = test_memory_full("Memory by agent B", "/home/test", None, Some("agent-b"));
        let memory3 = test_memory_full(
            "Memory by agent A again",
            "/home/test",
            None,
            Some("agent-a"),
        );

        db.create(&memory1).await.unwrap();
        db.create(&memory2).await.unwrap();
        db.create(&memory3).await.unwrap();

        let results = db.list(None, None, Some("agent-a"), None).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(
            results
                .iter()
                .all(|m| m.agent_id == Some("agent-a".to_string()))
        );
    }

    #[tokio::test]
    async fn test_list_filter_combined() {
        // This test verifies that combined filters work correctly
        // (tests the parameter numbering fix)
        let db = test_db().await;

        let memory1 = test_memory_full("Match all", "/dir/a", Some("/repo/x"), Some("agent-1"));
        let memory2 = test_memory_full("Wrong repo", "/dir/a", Some("/repo/y"), Some("agent-1"));
        let memory3 = test_memory_full("Wrong cwd", "/dir/b", Some("/repo/x"), Some("agent-1"));
        let memory4 = test_memory_full("Wrong agent", "/dir/a", Some("/repo/x"), Some("agent-2"));
        let memory5 = test_memory_full(
            "Match all again",
            "/dir/a",
            Some("/repo/x"),
            Some("agent-1"),
        );

        db.create(&memory1).await.unwrap();
        db.create(&memory2).await.unwrap();
        db.create(&memory3).await.unwrap();
        db.create(&memory4).await.unwrap();
        db.create(&memory5).await.unwrap();

        let results = db
            .list(Some("/repo/x"), Some("/dir/a"), Some("agent-1"), None)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|m| {
            m.repo_root == Some("/repo/x".to_string())
                && m.cwd == "/dir/a"
                && m.agent_id == Some("agent-1".to_string())
        }));
    }

    // ==================== Phase 3: FTS Search Tests ====================

    #[tokio::test]
    async fn test_search_basic() {
        let db = test_db().await;

        let memory1 = test_memory("Fixed the authentication bug in login");
        let memory2 = test_memory("Added new dashboard feature");
        let memory3 = test_memory("Refactored authentication module");

        db.create(&memory1).await.unwrap();
        db.create(&memory2).await.unwrap();
        db.create(&memory3).await.unwrap();

        let results = db.search("authentication", None).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(
            results
                .iter()
                .all(|m| m.description.contains("authentication"))
        );
    }

    #[tokio::test]
    async fn test_search_no_results() {
        let db = test_db().await;

        let memory = test_memory("Fixed a bug");
        db.create(&memory).await.unwrap();

        let results = db.search("nonexistent", None).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_search_with_repo_filter() {
        let db = test_db().await;

        let memory1 =
            test_memory_full("Fixed auth bug", "/home/test", Some("/repo/frontend"), None);
        let memory2 = test_memory_full(
            "Fixed auth issue",
            "/home/test",
            Some("/repo/backend"),
            None,
        );
        let memory3 = test_memory_full(
            "Fixed auth problem",
            "/home/test",
            Some("/repo/frontend"),
            None,
        );

        db.create(&memory1).await.unwrap();
        db.create(&memory2).await.unwrap();
        db.create(&memory3).await.unwrap();

        let results = db.search("auth", Some("/repo/frontend")).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(
            results
                .iter()
                .all(|m| m.repo_root == Some("/repo/frontend".to_string()))
        );
    }

    // ==================== Phase 4: Command Linking Tests ====================

    #[tokio::test]
    async fn test_link_command() {
        let db = test_db().await;
        let memory = test_memory("Test memory");

        db.create(&memory).await.unwrap();
        db.link_command(&memory.id, "history-123").await.unwrap();

        let commands = db.get_linked_commands(&memory.id).await.unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0], "history-123");
    }

    #[tokio::test]
    async fn test_link_multiple_commands() {
        let db = test_db().await;
        let memory = test_memory("Test memory");

        db.create(&memory).await.unwrap();
        db.link_command(&memory.id, "history-1").await.unwrap();
        db.link_command(&memory.id, "history-2").await.unwrap();
        db.link_command(&memory.id, "history-3").await.unwrap();

        let commands = db.get_linked_commands(&memory.id).await.unwrap();
        assert_eq!(commands.len(), 3);
    }

    #[tokio::test]
    async fn test_link_command_duplicate() {
        let db = test_db().await;
        let memory = test_memory("Test memory");

        db.create(&memory).await.unwrap();
        db.link_command(&memory.id, "history-123").await.unwrap();
        // Should not error on duplicate (INSERT OR IGNORE)
        db.link_command(&memory.id, "history-123").await.unwrap();

        let commands = db.get_linked_commands(&memory.id).await.unwrap();
        assert_eq!(commands.len(), 1);
    }

    #[tokio::test]
    async fn test_get_linked_command_count() {
        let db = test_db().await;
        let memory = test_memory("Test memory");

        db.create(&memory).await.unwrap();
        assert_eq!(db.get_linked_command_count(&memory.id).await.unwrap(), 0);

        db.link_command(&memory.id, "history-1").await.unwrap();
        db.link_command(&memory.id, "history-2").await.unwrap();
        assert_eq!(db.get_linked_command_count(&memory.id).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_delete_cascades_to_commands() {
        // This test verifies that foreign_keys(true) is enabled
        let db = test_db().await;
        let memory = test_memory("Test memory");

        db.create(&memory).await.unwrap();
        db.link_command(&memory.id, "history-1").await.unwrap();
        db.link_command(&memory.id, "history-2").await.unwrap();

        // Delete the memory
        db.delete(&memory.id).await.unwrap();

        // Commands should be cascaded deleted
        let commands = db.get_linked_commands(&memory.id).await.unwrap();
        assert!(commands.is_empty());
    }

    // ==================== Phase 5: Edge Cases Tests ====================

    #[tokio::test]
    async fn test_special_characters_in_description() {
        let db = test_db().await;

        let memory =
            test_memory("Fixed bug with 'quotes' and \"double quotes\" and special chars: <>&;");
        db.create(&memory).await.unwrap();

        let retrieved = db.get(&memory.id).await.unwrap().unwrap();
        assert_eq!(retrieved.description, memory.description);

        // Also test FTS search with special chars
        let results = db.search("quotes", None).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_memory_with_all_fields() {
        let db = test_db().await;

        let memory = Memory {
            id: atuin_common::utils::uuid_v7().as_simple().to_string(),
            description: "Full memory with all fields".to_string(),
            cwd: "/home/user/project".to_string(),
            repo_root: Some("/home/user/project".to_string()),
            git_branch: Some("feature/test".to_string()),
            git_commit: Some("abc123def456".to_string()),
            agent_id: Some("claude-code-v1".to_string()),
            parent_memory_id: None,
            created_at: OffsetDateTime::now_utc(),
        };

        db.create(&memory).await.unwrap();

        let retrieved = db.get(&memory.id).await.unwrap().unwrap();
        assert_eq!(retrieved.id, memory.id);
        assert_eq!(retrieved.description, memory.description);
        assert_eq!(retrieved.cwd, memory.cwd);
        assert_eq!(retrieved.repo_root, memory.repo_root);
        assert_eq!(retrieved.git_branch, memory.git_branch);
        assert_eq!(retrieved.git_commit, memory.git_commit);
        assert_eq!(retrieved.agent_id, memory.agent_id);
        assert_eq!(retrieved.parent_memory_id, memory.parent_memory_id);
    }

    // ==================== Phase 6: Parent-Child Relationship Tests ====================

    #[tokio::test]
    async fn test_create_with_parent() {
        let db = test_db().await;

        // Create parent memory
        let parent = test_memory("Parent task");
        db.create(&parent).await.unwrap();

        // Create child memory
        let child = test_memory_with_parent("Child task", &parent.id);
        db.create(&child).await.unwrap();

        // Verify child has parent_memory_id set
        let retrieved = db.get(&child.id).await.unwrap().unwrap();
        assert_eq!(retrieved.parent_memory_id, Some(parent.id.clone()));
    }

    #[tokio::test]
    async fn test_create_with_invalid_parent_fails() {
        let db = test_db().await;

        // Try to create a child with non-existent parent
        let child = test_memory_with_parent("Orphan task", "nonexistent-parent-id");
        let result = db.create(&child).await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Parent memory not found")
        );
    }

    #[tokio::test]
    async fn test_get_children() {
        let db = test_db().await;

        // Create parent
        let parent = test_memory("Parent task");
        db.create(&parent).await.unwrap();

        // Create children
        let child1 = test_memory_with_parent("Child 1", &parent.id);
        let child2 = test_memory_with_parent("Child 2", &parent.id);
        let child3 = test_memory_with_parent("Child 3", &parent.id);

        db.create(&child1).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        db.create(&child2).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        db.create(&child3).await.unwrap();

        // Get children
        let children = db.get_children(&parent.id).await.unwrap();
        assert_eq!(children.len(), 3);

        // Should be ordered by created_at DESC (newest first)
        assert_eq!(children[0].id, child3.id);
        assert_eq!(children[1].id, child2.id);
        assert_eq!(children[2].id, child1.id);
    }

    #[tokio::test]
    async fn test_get_children_empty() {
        let db = test_db().await;

        let memory = test_memory("Memory without children");
        db.create(&memory).await.unwrap();

        let children = db.get_children(&memory.id).await.unwrap();
        assert!(children.is_empty());
    }

    #[tokio::test]
    async fn test_get_ancestors() {
        let db = test_db().await;

        // Create a chain: grandparent -> parent -> child
        let grandparent = test_memory("Grandparent");
        db.create(&grandparent).await.unwrap();

        let parent = test_memory_with_parent("Parent", &grandparent.id);
        db.create(&parent).await.unwrap();

        let child = test_memory_with_parent("Child", &parent.id);
        db.create(&child).await.unwrap();

        // Get ancestors of child
        let ancestors = db.get_ancestors(&child.id).await.unwrap();
        assert_eq!(ancestors.len(), 2);

        // Should contain parent and grandparent (order by created_at DESC)
        let ancestor_ids: Vec<&str> = ancestors.iter().map(|m| m.id.as_str()).collect();
        assert!(ancestor_ids.contains(&parent.id.as_str()));
        assert!(ancestor_ids.contains(&grandparent.id.as_str()));
    }

    #[tokio::test]
    async fn test_get_ancestors_root_has_none() {
        let db = test_db().await;

        let root = test_memory("Root memory");
        db.create(&root).await.unwrap();

        let ancestors = db.get_ancestors(&root.id).await.unwrap();
        assert!(ancestors.is_empty());
    }

    #[tokio::test]
    async fn test_get_tree() {
        let db = test_db().await;

        // Create a tree structure:
        // root1
        //   ├── child1
        //   │   └── grandchild1
        //   └── child2
        // root2
        //   └── child3

        let root1 = test_memory("Root 1");
        db.create(&root1).await.unwrap();

        let root2 = test_memory("Root 2");
        db.create(&root2).await.unwrap();

        let child1 = test_memory_with_parent("Child 1", &root1.id);
        db.create(&child1).await.unwrap();

        let child2 = test_memory_with_parent("Child 2", &root1.id);
        db.create(&child2).await.unwrap();

        let child3 = test_memory_with_parent("Child 3", &root2.id);
        db.create(&child3).await.unwrap();

        let grandchild1 = test_memory_with_parent("Grandchild 1", &child1.id);
        db.create(&grandchild1).await.unwrap();

        // Get full tree
        let tree = db.get_tree(None, None).await.unwrap();
        assert_eq!(tree.len(), 6);

        // Get tree from root1 only
        let tree_from_root1 = db.get_tree(Some(&root1.id), None).await.unwrap();
        assert_eq!(tree_from_root1.len(), 4); // root1, child1, child2, grandchild1

        // Get tree with limited depth
        let shallow_tree = db.get_tree(Some(&root1.id), Some(1)).await.unwrap();
        assert_eq!(shallow_tree.len(), 3); // root1, child1, child2 (no grandchild due to depth limit)
    }

    #[tokio::test]
    async fn test_get_roots() {
        let db = test_db().await;

        // Create some roots and some children
        let root1 = test_memory("Root 1");
        let root2 = test_memory("Root 2");
        let root3 = test_memory("Root 3");

        db.create(&root1).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        db.create(&root2).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        db.create(&root3).await.unwrap();

        let child = test_memory_with_parent("Child", &root1.id);
        db.create(&child).await.unwrap();

        // Get all roots
        let roots = db.get_roots(None).await.unwrap();
        assert_eq!(roots.len(), 3);

        // Get limited roots
        let limited_roots = db.get_roots(Some(2)).await.unwrap();
        assert_eq!(limited_roots.len(), 2);
    }

    #[tokio::test]
    async fn test_delete_parent_orphans_children() {
        let db = test_db().await;

        // Create parent and child
        let parent = test_memory("Parent");
        db.create(&parent).await.unwrap();

        let child = test_memory_with_parent("Child", &parent.id);
        db.create(&child).await.unwrap();

        // Verify child has parent
        let retrieved_child = db.get(&child.id).await.unwrap().unwrap();
        assert_eq!(retrieved_child.parent_memory_id, Some(parent.id.clone()));

        // Delete parent
        db.delete(&parent.id).await.unwrap();

        // Verify child still exists but parent_memory_id is now NULL
        let orphaned_child = db.get(&child.id).await.unwrap().unwrap();
        assert!(orphaned_child.parent_memory_id.is_none());
    }

    #[tokio::test]
    async fn test_exists() {
        let db = test_db().await;

        let memory = test_memory("Test memory");
        db.create(&memory).await.unwrap();

        assert!(db.exists(&memory.id).await.unwrap());
        assert!(!db.exists("nonexistent-id").await.unwrap());
    }
}
