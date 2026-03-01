use std::env;
use std::path::PathBuf;
use std::process::Command;

use clap::Subcommand;
use directories::ProjectDirs;
use eyre::{Result, WrapErr, bail};
use serde::Serialize;

use atuin_client::{database::Database, settings::Settings};
use atuin_common::utils;
use atuin_memory::{
    Memory, MemoryCreateJson, MemoryJson, MemoryTreeNode,
    database::{MemoryDatabase, SqliteMemoryDb},
};

/// JSON output for a linked command with full details
#[derive(Debug, Serialize)]
pub struct LinkedCommandJson {
    pub history_id: String,
    pub command: String,
    pub cwd: String,
    pub exit: i64,
    pub duration: i64,
    pub timestamp: String,
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_memory_id: Option<String>,
    pub created_at: String,
    pub linked_commands: Vec<LinkedCommandJson>,
}

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Create a new memory with a description
    Create {
        /// The description of what was done
        description: String,

        /// Link the last N commands from history
        #[arg(long = "link-last")]
        link_last: Option<usize>,

        /// Link specific history IDs
        #[arg(long = "link")]
        link: Vec<String>,

        /// Parent memory ID (can also be set via ATUIN_PARENT_MEMORY_ID env var)
        #[arg(long = "parent")]
        parent: Option<String>,

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

        /// Link the last N commands from history
        #[arg(long = "last")]
        last: Option<usize>,
    },

    /// Delete a memory
    #[command(alias = "rm")]
    Delete {
        /// Memory ID
        id: String,
    },

    /// Show children of a memory
    Children {
        /// Memory ID
        id: String,

        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },

    /// Show ancestors (parent chain to root)
    Ancestors {
        /// Memory ID
        id: String,

        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },

    /// Show memory hierarchy as a tree
    Tree {
        /// Root memory ID (shows all roots if not specified)
        #[arg(long)]
        root: Option<String>,

        /// Maximum depth to traverse
        #[arg(long, default_value = "10")]
        depth: usize,

        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },

    /// Re-run the commands linked to a memory
    #[command(alias = "replay")]
    Run {
        /// Memory ID
        id: String,

        /// Preview commands without executing
        #[arg(long)]
        dry_run: bool,

        /// Confirm each command before running
        #[arg(long, short)]
        interactive: bool,

        /// Continue running even if a command fails
        #[arg(long)]
        keep_going: bool,

        /// Run all commands in current directory instead of their original cwd
        #[arg(long)]
        here: bool,
    },
}

/// Get the path to the memory database
fn memory_db_path(settings: &Settings) -> PathBuf {
    let data_dir = PathBuf::from(&settings.db_path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| {
            ProjectDirs::from("com", "atuin", "atuin")
                .map(|d: ProjectDirs| d.data_dir().to_path_buf())
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
                parent,
                json,
            } => {
                self.handle_create(
                    &memory_db,
                    db,
                    description,
                    *link_last,
                    link,
                    parent.as_deref(),
                    *json,
                )
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
            Self::Show { id, json } => self.handle_show(&memory_db, db, id, *json).await,
            Self::Link {
                id,
                history_id,
                last,
            } => {
                self.handle_link(&memory_db, db, id, history_id, *last)
                    .await
            }
            Self::Delete { id } => self.handle_delete(&memory_db, id).await,
            Self::Children { id, json } => self.handle_children(&memory_db, id, *json).await,
            Self::Ancestors { id, json } => self.handle_ancestors(&memory_db, id, *json).await,
            Self::Tree { root, depth, json } => {
                self.handle_tree(&memory_db, root.as_deref(), *depth, *json)
                    .await
            }
            Self::Run {
                id,
                dry_run,
                interactive,
                keep_going,
                here,
            } => {
                self.handle_run(
                    &memory_db,
                    db,
                    id,
                    *dry_run,
                    *interactive,
                    *keep_going,
                    *here,
                )
                .await
            }
        }
    }

    async fn handle_create(
        &self,
        memory_db: &SqliteMemoryDb,
        db: &impl Database,
        description: &str,
        link_last: Option<usize>,
        link_ids: &[String],
        parent: Option<&str>,
        json: bool,
    ) -> Result<()> {
        let cwd = utils::get_current_dir();
        let (repo_root, git_branch, git_commit) = get_git_info();
        let agent_id = env::var("ATUIN_AGENT_ID").ok();

        // Get parent from CLI flag or environment variable
        let parent_memory_id = parent
            .map(String::from)
            .or_else(|| env::var("ATUIN_PARENT_MEMORY_ID").ok());

        let memory = Memory::new(
            description.to_string(),
            cwd,
            repo_root.clone(),
            git_branch.clone(),
            git_commit.clone(),
            agent_id,
            parent_memory_id.clone(),
        );

        memory_db.create(&memory).await?;

        let mut linked_count = 0;

        // Link specified history IDs
        for history_id in link_ids {
            memory_db.link_command(&memory.id, history_id).await?;
            linked_count += 1;
        }

        // Link last N commands from history (global, not session-scoped)
        if let Some(n) = link_last {
            let context = atuin_client::database::current_context().await?;
            let history = db
                .list(
                    &[atuin_client::settings::FilterMode::Global],
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
                parent_memory_id,
            };
            println!("{}", serde_json::to_string(&output)?);
        } else {
            println!("Created memory: {}", memory.id);
            println!("  Description: {}", memory.description);
            if let Some(repo) = repo_root {
                println!("  Repo: {}", repo);
            }
            if let Some(parent_id) = &memory.parent_memory_id {
                println!("  Parent: {}", parent_id);
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

    async fn handle_show(
        &self,
        memory_db: &SqliteMemoryDb,
        db: &impl Database,
        id: &str,
        json: bool,
    ) -> Result<()> {
        let memory = memory_db.get(id).await?;

        let Some(memory) = memory else {
            bail!("Memory not found: {}", id);
        };

        let linked_history_ids = memory_db.get_linked_commands(id).await?;

        // Fetch full command details from history database
        let mut linked_commands = Vec::new();
        for history_id in &linked_history_ids {
            if let Ok(Some(history)) = db.load(history_id).await {
                linked_commands.push(LinkedCommandJson {
                    history_id: history.id.0.clone(),
                    command: history.command,
                    cwd: history.cwd,
                    exit: history.exit,
                    duration: history.duration,
                    timestamp: history
                        .timestamp
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default(),
                });
            }
        }

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
                parent_memory_id: memory.parent_memory_id,
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

            if let Some(parent_id) = &memory.parent_memory_id {
                println!("Parent: {}", parent_id);
            }

            println!();
            println!("Description:");
            println!("  {}", memory.description);
            println!();

            println!("Linked commands ({}):", linked_commands.len());
            for (i, cmd) in linked_commands.iter().enumerate() {
                let exit_indicator = if cmd.exit == 0 { "✓" } else { "✗" };
                println!("  [{}] {} {}", i, exit_indicator, cmd.command);
                println!("      {} ({}ms, exit {})", cmd.cwd, cmd.duration, cmd.exit);
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

    async fn handle_children(
        &self,
        memory_db: &SqliteMemoryDb,
        id: &str,
        json: bool,
    ) -> Result<()> {
        // Verify memory exists
        if !memory_db.exists(id).await? {
            bail!("Memory not found: {}", id);
        }

        let children = memory_db.get_children(id).await?;

        if json {
            let mut output = Vec::new();
            for m in &children {
                let count = memory_db.get_linked_command_count(&m.id).await.unwrap_or(0);
                let mut json = MemoryJson::from(m);
                json.commands_count = count;
                output.push(json);
            }
            println!("{}", serde_json::to_string(&output)?);
        } else {
            if children.is_empty() {
                println!("No children found for memory: {}", id);
                return Ok(());
            }

            println!("Children of {}:", id);
            for m in &children {
                let count = memory_db.get_linked_command_count(&m.id).await.unwrap_or(0);
                println!("  {} ({} commands)", m.id, count);
                println!("    {}", m.description);
            }
        }

        Ok(())
    }

    async fn handle_ancestors(
        &self,
        memory_db: &SqliteMemoryDb,
        id: &str,
        json: bool,
    ) -> Result<()> {
        // Verify memory exists
        if !memory_db.exists(id).await? {
            bail!("Memory not found: {}", id);
        }

        let ancestors = memory_db.get_ancestors(id).await?;

        if json {
            let mut output = Vec::new();
            for m in &ancestors {
                let count = memory_db.get_linked_command_count(&m.id).await.unwrap_or(0);
                let mut json = MemoryJson::from(m);
                json.commands_count = count;
                output.push(json);
            }
            println!("{}", serde_json::to_string(&output)?);
        } else {
            if ancestors.is_empty() {
                println!(
                    "No ancestors found for memory: {} (this is a root memory)",
                    id
                );
                return Ok(());
            }

            println!("Ancestors of {} (nearest to root):", id);
            for (i, m) in ancestors.iter().enumerate() {
                let indent = "  ".repeat(i + 1);
                let count = memory_db.get_linked_command_count(&m.id).await.unwrap_or(0);
                println!("{}{} ({} commands)", indent, m.id, count);
                println!("{}  {}", indent, m.description);
            }
        }

        Ok(())
    }

    async fn handle_tree(
        &self,
        memory_db: &SqliteMemoryDb,
        root: Option<&str>,
        depth: usize,
        json: bool,
    ) -> Result<()> {
        // If root specified, verify it exists
        if let Some(root_id) = root {
            if !memory_db.exists(root_id).await? {
                bail!("Memory not found: {}", root_id);
            }
        }

        let memories = memory_db.get_tree(root, Some(depth)).await?;

        if json {
            // Build tree structure for JSON output
            let tree = self.build_tree_json(memory_db, &memories).await?;
            println!("{}", serde_json::to_string(&tree)?);
        } else {
            if memories.is_empty() {
                println!("No memories found.");
                return Ok(());
            }

            self.print_tree(memory_db, &memories).await?;
        }

        Ok(())
    }

    async fn build_tree_json(
        &self,
        memory_db: &SqliteMemoryDb,
        memories: &[Memory],
    ) -> Result<Vec<MemoryTreeNode>> {
        use std::collections::HashMap;

        // Build a map of parent_id -> children
        let mut children_map: HashMap<Option<String>, Vec<&Memory>> = HashMap::new();
        for m in memories {
            children_map
                .entry(m.parent_memory_id.clone())
                .or_default()
                .push(m);
        }

        // Recursively build tree nodes
        async fn build_node(
            memory_db: &SqliteMemoryDb,
            memory: &Memory,
            children_map: &HashMap<Option<String>, Vec<&Memory>>,
        ) -> Result<MemoryTreeNode> {
            let count = memory_db
                .get_linked_command_count(&memory.id)
                .await
                .unwrap_or(0);
            let mut json = MemoryJson::from(memory);
            json.commands_count = count;

            let children = if let Some(child_memories) = children_map.get(&Some(memory.id.clone()))
            {
                let mut child_nodes = Vec::new();
                for child in child_memories {
                    child_nodes.push(Box::pin(build_node(memory_db, child, children_map)).await?);
                }
                child_nodes
            } else {
                Vec::new()
            };

            Ok(MemoryTreeNode {
                memory: json,
                children,
            })
        }

        // Find roots and build tree from them
        let roots = children_map.get(&None).cloned().unwrap_or_default();
        let mut result = Vec::new();
        for root in roots {
            result.push(build_node(memory_db, root, &children_map).await?);
        }

        Ok(result)
    }

    async fn print_tree(&self, memory_db: &SqliteMemoryDb, memories: &[Memory]) -> Result<()> {
        use std::collections::HashMap;

        // Build a map of parent_id -> children
        let mut children_map: HashMap<Option<String>, Vec<&Memory>> = HashMap::new();
        for m in memories {
            children_map
                .entry(m.parent_memory_id.clone())
                .or_default()
                .push(m);
        }

        // Print tree recursively
        async fn print_node(
            memory_db: &SqliteMemoryDb,
            memory: &Memory,
            children_map: &HashMap<Option<String>, Vec<&Memory>>,
            prefix: &str,
            is_last: bool,
        ) -> Result<()> {
            let connector = if is_last { "└── " } else { "├── " };
            let count = memory_db
                .get_linked_command_count(&memory.id)
                .await
                .unwrap_or(0);

            println!("{}{}{} ({} commands)", prefix, connector, memory.id, count);

            // Print description with proper indentation
            let child_prefix = if is_last {
                format!("{}    ", prefix)
            } else {
                format!("{}│   ", prefix)
            };
            println!("{}    {}", child_prefix.trim_end(), memory.description);

            // Print children
            if let Some(children) = children_map.get(&Some(memory.id.clone())) {
                let child_prefix = if is_last {
                    format!("{}    ", prefix)
                } else {
                    format!("{}│   ", prefix)
                };

                for (i, child) in children.iter().enumerate() {
                    let is_last_child = i == children.len() - 1;
                    Box::pin(print_node(
                        memory_db,
                        child,
                        children_map,
                        &child_prefix,
                        is_last_child,
                    ))
                    .await?;
                }
            }

            Ok(())
        }

        // Print from roots
        let roots = children_map.get(&None).cloned().unwrap_or_default();
        for (i, root) in roots.iter().enumerate() {
            let is_last = i == roots.len() - 1;
            print_node(memory_db, root, &children_map, "", is_last).await?;
        }

        Ok(())
    }

    async fn handle_run(
        &self,
        memory_db: &SqliteMemoryDb,
        db: &impl Database,
        id: &str,
        dry_run: bool,
        interactive: bool,
        keep_going: bool,
        here: bool,
    ) -> Result<()> {
        let memory = memory_db.get(id).await?;

        let Some(memory) = memory else {
            bail!("Memory not found: {}", id);
        };

        let linked_history_ids = memory_db.get_linked_commands(id).await?;

        if linked_history_ids.is_empty() {
            println!("No commands linked to this memory.");
            return Ok(());
        }

        // Fetch full command details and sort by timestamp
        let mut commands = Vec::new();
        for history_id in &linked_history_ids {
            if let Ok(Some(history)) = db.load(history_id).await {
                commands.push(history);
            }
        }

        if commands.is_empty() {
            println!("No command history found for linked IDs (commands may have been deleted).");
            return Ok(());
        }

        // Sort by timestamp (oldest first for replay order)
        commands.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

        println!("Memory: {}", memory.description);
        println!("Commands to run: {}", commands.len());
        println!();

        let current_dir = utils::get_current_dir();

        for (i, cmd) in commands.iter().enumerate() {
            let run_dir = if here {
                current_dir.clone()
            } else {
                cmd.cwd.clone()
            };

            println!("[{}/{}] {}", i + 1, commands.len(), cmd.command);
            if !here {
                println!("      cwd: {}", run_dir);
            }

            if dry_run {
                println!("      (dry run - skipped)");
                println!();
                continue;
            }

            if interactive {
                print!("      Run this command? [Y/n/q] ");
                std::io::Write::flush(&mut std::io::stdout())?;

                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                let input = input.trim().to_lowercase();

                if input == "q" {
                    println!("Aborted.");
                    return Ok(());
                }
                if input == "n" {
                    println!("      (skipped)");
                    println!();
                    continue;
                }
            }

            // Execute the command
            let status = Command::new("sh")
                .arg("-c")
                .arg(&cmd.command)
                .current_dir(&run_dir)
                .status();

            match status {
                Ok(status) => {
                    if status.success() {
                        println!("      ✓ exit 0");
                    } else {
                        let code = status.code().unwrap_or(-1);
                        println!("      ✗ exit {}", code);

                        if !keep_going {
                            bail!(
                                "Command failed with exit code {}. Use --keep-going to continue on errors.",
                                code
                            );
                        }
                    }
                }
                Err(e) => {
                    println!("      ✗ failed to execute: {}", e);
                    if !keep_going {
                        bail!("Failed to execute command: {}", e);
                    }
                }
            }

            println!();
        }

        println!("Done. Ran {} commands.", commands.len());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atuin_memory::{
        Memory,
        database::{MemoryDatabase, SqliteMemoryDb},
    };
    use time::OffsetDateTime;

    // ==================== JSON Output Struct Tests ====================

    #[test]
    fn test_linked_command_json_serialization() {
        let cmd = LinkedCommandJson {
            history_id: "hist-123".into(),
            command: "cargo build".into(),
            cwd: "/home/user/project".into(),
            exit: 0,
            duration: 5000,
            timestamp: "2024-06-15T12:30:00Z".into(),
        };
        let value: serde_json::Value = serde_json::to_value(&cmd).unwrap();
        let obj = value.as_object().unwrap();

        assert_eq!(obj["history_id"], "hist-123");
        assert_eq!(obj["command"], "cargo build");
        assert_eq!(obj["cwd"], "/home/user/project");
        assert_eq!(obj["exit"], 0);
        assert_eq!(obj["duration"], 5000);
        assert_eq!(obj["timestamp"], "2024-06-15T12:30:00Z");
    }

    #[test]
    fn test_show_json_serialization() {
        let show = MemoryShowJson {
            id: "mem-1".into(),
            description: "test memory".into(),
            cwd: "/tmp".into(),
            repo: Some("my-repo".into()),
            branch: Some("main".into()),
            commit: Some("abc123".into()),
            agent_id: Some("claude".into()),
            parent_memory_id: Some("parent-1".into()),
            created_at: "2024-01-01T00:00:00Z".into(),
            linked_commands: vec![],
        };
        let value: serde_json::Value = serde_json::to_value(&show).unwrap();
        let obj = value.as_object().unwrap();

        assert_eq!(obj["id"], "mem-1");
        assert_eq!(obj["description"], "test memory");
        assert_eq!(obj["repo"], "my-repo");
        assert_eq!(obj["branch"], "main");
        assert_eq!(obj["commit"], "abc123");
        assert_eq!(obj["agent_id"], "claude");
        assert_eq!(obj["parent_memory_id"], "parent-1");
        assert!(obj["linked_commands"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_show_json_skip_none_fields() {
        let show = MemoryShowJson {
            id: "mem-2".into(),
            description: "minimal".into(),
            cwd: "/tmp".into(),
            repo: None,
            branch: None,
            commit: None,
            agent_id: None,
            parent_memory_id: None,
            created_at: "2024-01-01T00:00:00Z".into(),
            linked_commands: vec![],
        };
        let serialized = serde_json::to_string(&show).unwrap();

        assert!(!serialized.contains("\"repo\""));
        assert!(!serialized.contains("\"branch\""));
        assert!(!serialized.contains("\"commit\""));
        assert!(!serialized.contains("\"agent_id\""));
        assert!(!serialized.contains("\"parent_memory_id\""));

        // Required fields should be present
        assert!(serialized.contains("\"id\""));
        assert!(serialized.contains("\"description\""));
        assert!(serialized.contains("\"cwd\""));
        assert!(serialized.contains("\"created_at\""));
    }

    #[test]
    fn test_show_json_with_linked_commands() {
        let show = MemoryShowJson {
            id: "mem-3".into(),
            description: "with commands".into(),
            cwd: "/tmp".into(),
            repo: None,
            branch: None,
            commit: None,
            agent_id: None,
            parent_memory_id: None,
            created_at: "2024-01-01T00:00:00Z".into(),
            linked_commands: vec![
                LinkedCommandJson {
                    history_id: "h1".into(),
                    command: "ls".into(),
                    cwd: "/tmp".into(),
                    exit: 0,
                    duration: 10,
                    timestamp: "2024-01-01T00:00:01Z".into(),
                },
                LinkedCommandJson {
                    history_id: "h2".into(),
                    command: "pwd".into(),
                    cwd: "/tmp".into(),
                    exit: 0,
                    duration: 5,
                    timestamp: "2024-01-01T00:00:02Z".into(),
                },
            ],
        };
        let value: serde_json::Value = serde_json::to_value(&show).unwrap();
        let cmds = value["linked_commands"].as_array().unwrap();

        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0]["command"], "ls");
        assert_eq!(cmds[1]["command"], "pwd");
    }

    // ==================== build_tree_json Tests ====================

    /// Helper to create an in-memory test database
    async fn test_db() -> SqliteMemoryDb {
        SqliteMemoryDb::new("sqlite::memory:").await.unwrap()
    }

    /// Helper to create a Memory with specified fields
    fn make_memory(description: &str, parent: Option<&str>) -> Memory {
        Memory {
            id: atuin_common::utils::uuid_v7().as_simple().to_string(),
            description: description.to_string(),
            cwd: "/tmp".to_string(),
            repo_root: None,
            git_branch: None,
            git_commit: None,
            agent_id: None,
            parent_memory_id: parent.map(String::from),
            created_at: OffsetDateTime::now_utc(),
        }
    }

    /// Helper Cmd instance for calling build_tree_json
    fn tree_cmd() -> Cmd {
        Cmd::Tree {
            root: None,
            depth: 10,
            json: true,
        }
    }

    #[tokio::test]
    async fn test_build_tree_json_empty() {
        let db = test_db().await;
        let cmd = tree_cmd();

        let result = cmd.build_tree_json(&db, &[]).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_build_tree_json_single_root() {
        let db = test_db().await;
        let cmd = tree_cmd();

        let root = make_memory("single root", None);
        db.create(&root).await.unwrap();
        // Link a command so we can verify count
        db.link_command(&root.id, "hist-1").await.unwrap();

        let result = cmd.build_tree_json(&db, &[root.clone()]).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].memory.id, root.id);
        assert_eq!(result[0].memory.description, "single root");
        assert_eq!(result[0].memory.commands_count, 1);
        assert!(result[0].children.is_empty());
    }

    #[tokio::test]
    async fn test_build_tree_json_parent_child() {
        let db = test_db().await;
        let cmd = tree_cmd();

        let root = make_memory("root", None);
        db.create(&root).await.unwrap();

        let child1 = make_memory("child 1", Some(&root.id));
        db.create(&child1).await.unwrap();

        let child2 = make_memory("child 2", Some(&root.id));
        db.create(&child2).await.unwrap();

        let memories = vec![root.clone(), child1.clone(), child2.clone()];
        let result = cmd.build_tree_json(&db, &memories).await.unwrap();

        assert_eq!(result.len(), 1, "should have one root");
        assert_eq!(result[0].memory.id, root.id);
        assert_eq!(result[0].children.len(), 2, "root should have 2 children");

        let child_ids: Vec<&str> = result[0]
            .children
            .iter()
            .map(|c| c.memory.id.as_str())
            .collect();
        assert!(child_ids.contains(&child1.id.as_str()));
        assert!(child_ids.contains(&child2.id.as_str()));
    }

    #[tokio::test]
    async fn test_build_tree_json_deep_nesting() {
        let db = test_db().await;
        let cmd = tree_cmd();

        let root = make_memory("root", None);
        db.create(&root).await.unwrap();

        let child = make_memory("child", Some(&root.id));
        db.create(&child).await.unwrap();

        let grandchild = make_memory("grandchild", Some(&child.id));
        db.create(&grandchild).await.unwrap();

        let memories = vec![root.clone(), child.clone(), grandchild.clone()];
        let result = cmd.build_tree_json(&db, &memories).await.unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].memory.id, root.id);
        assert_eq!(result[0].children.len(), 1);
        assert_eq!(result[0].children[0].memory.id, child.id);
        assert_eq!(result[0].children[0].children.len(), 1);
        assert_eq!(result[0].children[0].children[0].memory.id, grandchild.id);
        assert!(result[0].children[0].children[0].children.is_empty());
    }

    #[tokio::test]
    async fn test_build_tree_json_multiple_roots() {
        let db = test_db().await;
        let cmd = tree_cmd();

        let root1 = make_memory("root 1", None);
        db.create(&root1).await.unwrap();

        let root2 = make_memory("root 2", None);
        db.create(&root2).await.unwrap();

        let memories = vec![root1.clone(), root2.clone()];
        let result = cmd.build_tree_json(&db, &memories).await.unwrap();

        assert_eq!(result.len(), 2, "should have two roots");
        let root_ids: Vec<&str> = result.iter().map(|r| r.memory.id.as_str()).collect();
        assert!(root_ids.contains(&root1.id.as_str()));
        assert!(root_ids.contains(&root2.id.as_str()));
    }
}
