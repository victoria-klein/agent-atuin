use std::env;
use std::path::PathBuf;
use std::process::Command;

use clap::Subcommand;
use eyre::{Result, WrapErr, bail};
use serde::Serialize;

use atuin_client::{database::Database, settings::Settings};
use atuin_common::utils;
use atuin_memory::{
    Memory, MemoryCreateJson, MemoryJson,
    database::{MemoryDatabase, SqliteMemoryDb},
};

/// JSON output for memory show
#[derive(Debug, Serialize)]
pub struct MemoryShowJson {
    pub id: String,
    pub description: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub created_at: String,
    pub linked_commands: Vec<String>,
}

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Create a new memory with a description
    Create {
        /// The description of what was done
        description: String,

        /// Link the last N commands from the current session
        #[arg(long = "link-last")]
        link_last: Option<usize>,

        /// Link specific history IDs
        #[arg(long = "link")]
        link: Vec<String>,

        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },

    /// List memories
    #[command(alias = "ls")]
    List {
        /// Filter by current git repository
        #[arg(long)]
        repo: bool,

        /// Filter by current working directory
        #[arg(long)]
        cwd: bool,

        /// Filter by agent ID
        #[arg(long)]
        agent: Option<String>,

        /// Limit number of results
        #[arg(long, short)]
        limit: Option<usize>,

        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },

    /// Search memories by description
    Search {
        /// Search query
        query: String,

        /// Search memories with linked commands matching this pattern
        #[arg(long = "command")]
        command_pattern: Option<String>,

        /// Scope to current git repository
        #[arg(long)]
        repo: bool,

        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },

    /// Show details of a specific memory
    Show {
        /// Memory ID
        id: String,

        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },

    /// Link more commands to an existing memory
    Link {
        /// Memory ID
        id: String,

        /// History IDs to link
        #[arg(long)]
        history_id: Vec<String>,

        /// Link the last N commands from the current session
        #[arg(long = "last")]
        last: Option<usize>,
    },

    /// Delete a memory
    #[command(alias = "rm")]
    Delete {
        /// Memory ID
        id: String,
    },
}

/// Get the path to the memory database
fn memory_db_path(settings: &Settings) -> PathBuf {
    let data_dir = PathBuf::from(&settings.db_path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| {
            directories::ProjectDirs::from("com", "atuin", "atuin")
                .map(|d| d.data_dir().to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."))
        });

    data_dir.join("memory.db")
}

/// Get current git info (repo root, branch, commit)
fn get_git_info() -> (Option<String>, Option<String>, Option<String>) {
    let cwd = utils::get_current_dir();
    let repo_root = utils::in_git_repo(&cwd);

    if repo_root.is_none() {
        return (None, None, None);
    }

    let repo_root_str = repo_root
        .as_ref()
        .and_then(|p| p.to_str())
        .map(String::from);

    // Get current branch
    let branch = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        });

    // Get current commit
    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        });

    (repo_root_str, branch, commit)
}

impl Cmd {
    pub async fn run(&self, settings: &Settings, db: &impl Database) -> Result<()> {
        let memory_db_path = memory_db_path(settings);
        let memory_db = SqliteMemoryDb::new(&memory_db_path)
            .await
            .wrap_err("Failed to open memory database")?;

        match self {
            Self::Create {
                description,
                link_last,
                link,
                json,
            } => {
                self.handle_create(&memory_db, db, description, *link_last, link, *json)
                    .await
            }
            Self::List {
                repo,
                cwd,
                agent,
                limit,
                json,
            } => {
                self.handle_list(&memory_db, *repo, *cwd, agent.as_deref(), *limit, *json)
                    .await
            }
            Self::Search {
                query,
                command_pattern,
                repo,
                json,
            } => {
                self.handle_search(&memory_db, query, command_pattern.as_deref(), *repo, *json)
                    .await
            }
            Self::Show { id, json } => self.handle_show(&memory_db, id, *json).await,
            Self::Link {
                id,
                history_id,
                last,
            } => {
                self.handle_link(&memory_db, db, id, history_id, *last)
                    .await
            }
            Self::Delete { id } => self.handle_delete(&memory_db, id).await,
        }
    }

    async fn handle_create(
        &self,
        memory_db: &SqliteMemoryDb,
        db: &impl Database,
        description: &str,
        link_last: Option<usize>,
        link_ids: &[String],
        json: bool,
    ) -> Result<()> {
        let cwd = utils::get_current_dir();
        let (repo_root, git_branch, git_commit) = get_git_info();
        let agent_id = env::var("ATUIN_AGENT_ID").ok();

        let memory = Memory::new(
            description.to_string(),
            cwd,
            repo_root.clone(),
            git_branch.clone(),
            git_commit.clone(),
            agent_id,
        );

        memory_db.create(&memory).await?;

        let mut linked_count = 0;

        // Link specified history IDs
        for history_id in link_ids {
            memory_db.link_command(&memory.id, history_id).await?;
            linked_count += 1;
        }

        // Link last N commands from session
        if let Some(n) = link_last {
            let context = atuin_client::database::current_context().await?;
            let history = db
                .list(
                    &[atuin_client::settings::FilterMode::Session],
                    &context,
                    Some(n),
                    false,
                    false,
                )
                .await?;

            for h in history {
                memory_db.link_command(&memory.id, &h.id.0).await?;
                linked_count += 1;
            }
        }

        if json {
            let output = MemoryCreateJson {
                id: memory.id,
                description: memory.description,
                commands_linked: linked_count,
                repo: repo_root.as_ref().and_then(|r| {
                    std::path::Path::new(r)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(String::from)
                }),
                branch: git_branch,
                commit: git_commit,
            };
            println!("{}", serde_json::to_string(&output)?);
        } else {
            println!("Created memory: {}", memory.id);
            println!("  Description: {}", memory.description);
            if let Some(repo) = repo_root {
                println!("  Repo: {}", repo);
            }
            println!("  Commands linked: {}", linked_count);
        }

        Ok(())
    }

    async fn handle_list(
        &self,
        memory_db: &SqliteMemoryDb,
        repo: bool,
        cwd: bool,
        agent: Option<&str>,
        limit: Option<usize>,
        json: bool,
    ) -> Result<()> {
        let current_cwd = utils::get_current_dir();
        let (repo_root, _, _) = get_git_info();

        let repo_filter = if repo { repo_root.as_deref() } else { None };
        let cwd_filter = if cwd {
            Some(current_cwd.as_str())
        } else {
            None
        };

        let memories = memory_db
            .list(repo_filter, cwd_filter, agent, limit)
            .await?;

        if json {
            let mut output = Vec::new();
            for m in &memories {
                let count = memory_db.get_linked_command_count(&m.id).await.unwrap_or(0);
                let mut json = MemoryJson::from(m);
                json.commands_count = count;
                output.push(json);
            }
            println!("{}", serde_json::to_string(&output)?);
        } else {
            if memories.is_empty() {
                println!("No memories found.");
                return Ok(());
            }

            for m in &memories {
                let count = memory_db.get_linked_command_count(&m.id).await.unwrap_or(0);
                println!("{} ({} commands)", m.id, count);
                println!("  {}", m.description);
                if let Some(repo) = &m.repo_root {
                    println!("  Repo: {}", repo);
                }
                println!(
                    "  Created: {}",
                    m.created_at
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default()
                );
                println!();
            }
        }

        Ok(())
    }

    async fn handle_search(
        &self,
        memory_db: &SqliteMemoryDb,
        query: &str,
        command_pattern: Option<&str>,
        repo: bool,
        json: bool,
    ) -> Result<()> {
        let (repo_root, _, _) = get_git_info();
        let repo_filter = if repo { repo_root.as_deref() } else { None };

        let memories = if let Some(cmd_pattern) = command_pattern {
            memory_db
                .search_by_command(cmd_pattern, repo_filter)
                .await?
        } else {
            memory_db.search(query, repo_filter).await?
        };

        if json {
            let mut output = Vec::new();
            for m in &memories {
                let count = memory_db.get_linked_command_count(&m.id).await.unwrap_or(0);
                let mut json = MemoryJson::from(m);
                json.commands_count = count;
                output.push(json);
            }
            println!("{}", serde_json::to_string(&output)?);
        } else {
            if memories.is_empty() {
                println!("No memories found matching '{}'", query);
                return Ok(());
            }

            for m in &memories {
                let count = memory_db.get_linked_command_count(&m.id).await.unwrap_or(0);
                println!("{} ({} commands)", m.id, count);
                println!("  {}", m.description);
                println!();
            }
        }

        Ok(())
    }

    async fn handle_show(&self, memory_db: &SqliteMemoryDb, id: &str, json: bool) -> Result<()> {
        let memory = memory_db.get(id).await?;

        let Some(memory) = memory else {
            bail!("Memory not found: {}", id);
        };

        let linked_commands = memory_db.get_linked_commands(id).await?;

        if json {
            let output = MemoryShowJson {
                id: memory.id,
                description: memory.description,
                cwd: memory.cwd,
                repo: memory.repo_root.as_ref().and_then(|r| {
                    std::path::Path::new(r)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(String::from)
                }),
                branch: memory.git_branch,
                commit: memory.git_commit,
                agent_id: memory.agent_id,
                created_at: memory
                    .created_at
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
                linked_commands,
            };
            println!("{}", serde_json::to_string(&output)?);
        } else {
            println!("Memory: {}", memory.id);
            println!(
                "Created: {}{}",
                memory
                    .created_at
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
                memory
                    .agent_id
                    .as_ref()
                    .map(|a| format!(" by {}", a))
                    .unwrap_or_default()
            );

            if let Some(repo) = &memory.repo_root {
                let branch = memory.git_branch.as_deref().unwrap_or("unknown");
                let commit = memory.git_commit.as_deref().unwrap_or("unknown");
                println!("Repo: {} ({} @ {})", repo, branch, commit);
            }

            println!();
            println!("Description:");
            println!("  {}", memory.description);
            println!();

            println!("Linked commands ({}):", linked_commands.len());
            for (i, cmd_id) in linked_commands.iter().enumerate() {
                println!("  [{}] {}", i, cmd_id);
            }
        }

        Ok(())
    }

    async fn handle_link(
        &self,
        memory_db: &SqliteMemoryDb,
        db: &impl Database,
        memory_id: &str,
        history_ids: &[String],
        last: Option<usize>,
    ) -> Result<()> {
        // Verify memory exists
        let memory = memory_db.get(memory_id).await?;
        if memory.is_none() {
            bail!("Memory not found: {}", memory_id);
        }

        let mut linked_count = 0;

        // Link specified history IDs
        for history_id in history_ids {
            memory_db.link_command(memory_id, history_id).await?;
            linked_count += 1;
        }

        // Link last N commands from session
        if let Some(n) = last {
            let context = atuin_client::database::current_context().await?;
            let history = db
                .list(
                    &[atuin_client::settings::FilterMode::Session],
                    &context,
                    Some(n),
                    false,
                    false,
                )
                .await?;

            for h in history {
                memory_db.link_command(memory_id, &h.id.0).await?;
                linked_count += 1;
            }
        }

        println!("Linked {} commands to memory {}", linked_count, memory_id);
        Ok(())
    }

    async fn handle_delete(&self, memory_db: &SqliteMemoryDb, id: &str) -> Result<()> {
        // Verify memory exists
        let memory = memory_db.get(id).await?;
        if memory.is_none() {
            bail!("Memory not found: {}", id);
        }

        memory_db.delete(id).await?;
        println!("Deleted memory: {}", id);
        Ok(())
    }
}
