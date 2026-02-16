use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

pub mod database;

/// A memory entry that links natural language descriptions to commands
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Memory {
    /// UUIDv7 identifier
    pub id: String,
    /// Natural language description of what was done
    pub description: String,
    /// Current working directory when memory was created
    pub cwd: String,
    /// Git repository root (if in a repo)
    pub repo_root: Option<String>,
    /// Git branch at creation time
    pub git_branch: Option<String>,
    /// Git commit hash at creation time
    pub git_commit: Option<String>,
    /// Which agent created this memory
    pub agent_id: Option<String>,
    /// When the memory was created
    pub created_at: OffsetDateTime,
}

/// Link between a memory and a history command
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCommand {
    pub memory_id: String,
    pub history_id: String,
}

impl Memory {
    pub fn new(
        description: String,
        cwd: String,
        repo_root: Option<String>,
        git_branch: Option<String>,
        git_commit: Option<String>,
        agent_id: Option<String>,
    ) -> Self {
        Self {
            id: atuin_common::utils::uuid_v7().as_simple().to_string(),
            description,
            cwd,
            repo_root,
            git_branch,
            git_commit,
            agent_id,
            created_at: OffsetDateTime::now_utc(),
        }
    }
}

/// JSON output format for memory list
#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryJson {
    pub id: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    pub created_at: String,
    pub commands_count: usize,
}

impl From<&Memory> for MemoryJson {
    fn from(m: &Memory) -> Self {
        Self {
            id: m.id.clone(),
            description: m.description.clone(),
            repo: m.repo_root.as_ref().and_then(|r| {
                std::path::Path::new(r)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(String::from)
            }),
            created_at: m
                .created_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
            commands_count: 0, // Will be filled in when querying
        }
    }
}

/// JSON output for memory creation result
#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryCreateJson {
    pub id: String,
    pub description: String,
    pub commands_linked: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}
