# Setting Up agent-atuin for AI Agents

This guide covers how to set up and use agent-atuin for AI agents that need to execute shell commands and maintain context across sessions.

## Installation

### Quick Install

```bash
curl -sSL https://raw.githubusercontent.com/symbolicvic/agent-atuin/main/install.sh | sh
```

### Manual Installation

1. Download the appropriate binary from [GitHub Releases](https://github.com/symbolicvic/agent-atuin/releases)
2. Extract and place in your PATH (e.g., `~/.local/bin/`)
3. Run shell integration setup

## Shell Integration

Add to your shell configuration:

**Bash** (`~/.bashrc`):
```bash
[[ -f ~/.bash-preexec.sh ]] && source ~/.bash-preexec.sh
eval "$(atuin init bash)"
```

**Zsh** (`~/.zshrc`):
```zsh
eval "$(atuin init zsh)"
```

**Fish** (`~/.config/fish/config.fish`):
```fish
atuin init fish | source
```

## Agent Configuration

### Identifying Your Agent

Set the `ATUIN_AGENT_ID` environment variable to tag all commands with your agent's identifier:

```bash
export ATUIN_AGENT_ID="claude-code-1"
```

Or tag the current session:

```bash
atuin session tag --agent "claude-code-1"
```

All commands executed after setting this will be associated with the specified agent ID.

## Commands for Agents

All commands support `--json` for structured output suitable for parsing.

### Search History

```bash
# Search for commands matching a pattern
atuin search --json "git push"

# Filter by agent
atuin search --json --agent "claude-code-1" "cargo build"
```

**JSON Output:**
```json
{"id":"uuid","command":"git push origin main","exit":0,"duration":1234567890,"cwd":"/project","session":"abc","hostname":"host:user","timestamp":"2024-01-15T10:30:00Z","agent_id":"claude-code-1"}
```

### List History

```bash
# List recent commands
atuin history list --json

# List last N commands
atuin history last --json 10

# Filter by agent
atuin history list --json --agent "claude-code-1"
```

### Statistics

```bash
# Get command statistics
atuin stats --json
```

**JSON Output:**
```json
{"period":"all","total_commands":1523,"unique_commands":847,"top":[{"command":"git","count":234},{"command":"cd","count":189}]}
```

### Key-Value Store

```bash
# Store a value
atuin kv set mykey "myvalue"

# Retrieve with JSON
atuin kv get --json mykey

# List all keys
atuin kv list --json
```

### Sync Status

```bash
atuin sync --json
```

## Memory Store

The memory store allows agents to create searchable memories linked to commands they've executed.

### Create a Memory

```bash
# Create a memory and link the 5 most recent commands from history
atuin memory create "Fixed authentication bug by updating JWT validation" --link-last 5

# Create with specific command IDs
atuin memory create "Deployed version 2.0" --link <history-id-1> --link <history-id-2>

# Create as a child of another memory
atuin memory create "Sub-task: update tests" --parent <parent-memory-id> --link-last 3

# Parent can also be set via environment variable
export ATUIN_PARENT_MEMORY_ID="<parent-memory-id>"
atuin memory create "Sub-task: update docs" --link-last 2

# JSON output
atuin memory create --json "Refactored database layer" --link-last 3
```

**JSON Output:**
```json
{"id":"uuid","description":"Fixed authentication bug...","commands_linked":5,"repo":"myproject","branch":"main","commit":"abc123"}
```

### List Memories

```bash
# List all memories
atuin memory list --json

# Filter by current git repository
atuin memory list --repo --json

# Filter by current directory
atuin memory list --cwd --json

# Filter by agent
atuin memory list --agent "claude-code-1" --json

# Limit results
atuin memory list --limit 10 --json
```

**JSON Output:**
```json
[{"id":"uuid","description":"Fixed auth bug","repo":"myproject","created_at":"2024-01-15T10:30:00Z","commands_count":5}]
```

### Search Memories

```bash
# Search by description (full-text search)
atuin memory search "authentication" --json

# Search by linked command pattern
atuin memory search --command "cargo test" --json

# Scope to current repository
atuin memory search "database" --repo --json
```

### Show Memory Details

```bash
atuin memory show <memory-id> --json
```

**JSON Output:**
```json
{
  "id": "uuid",
  "description": "Fixed authentication bug by updating JWT validation",
  "cwd": "/home/user/project",
  "repo": "myproject",
  "branch": "main",
  "commit": "abc123",
  "agent_id": "claude-code-1",
  "parent_memory_id": null,
  "created_at": "2024-01-15T10:30:00Z",
  "linked_commands": ["history-id-1", "history-id-2", "history-id-3"]
}
```

### Link More Commands

```bash
# Add commands to an existing memory
atuin memory link <memory-id> --history-id <id1> --history-id <id2>

# Link last N commands from current session
atuin memory link <memory-id> --last 3
```

### Delete a Memory

```bash
atuin memory delete <memory-id>
```

### Memory Hierarchy

Memories can form parent-child trees, useful for tracking sub-tasks within a larger task.

#### List Children

```bash
# Show direct children of a memory
atuin memory children <memory-id> --json
```

#### Show Ancestors

```bash
# Trace the parent chain from a memory back to the root
atuin memory ancestors <memory-id> --json
```

#### Tree View

```bash
# Show all memory trees (roots and their descendants)
atuin memory tree

# Start from a specific root
atuin memory tree --root <memory-id>

# Limit traversal depth
atuin memory tree --depth 3

# JSON output (nested structure)
atuin memory tree --json
```

### Replay Linked Commands

Re-run the commands linked to a memory:

```bash
# Preview commands without executing
atuin memory run <memory-id> --dry-run

# Run with confirmation before each command
atuin memory run <memory-id> --interactive

# Run all, continuing past failures
atuin memory run <memory-id> --keep-going

# Run all commands in the current directory (instead of their original cwd)
atuin memory run <memory-id> --here
```

## Typical Agent Workflow

1. **Start Session**: Set agent ID
   ```bash
   export ATUIN_AGENT_ID="my-agent-session-1"
   ```

2. **Execute Commands**: Run shell commands as normal - they're automatically tracked

3. **Search Context**: Find relevant past commands
   ```bash
   atuin search --json "similar task"
   ```

4. **Create Memory**: After completing a task, create a memory
   ```bash
   atuin memory create "Completed feature X by doing Y" --link-last 10
   ```

5. **Future Reference**: Search memories for context
   ```bash
   atuin memory search "feature X" --json
   ```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `ATUIN_AGENT_ID` | Identifies the agent for command tagging |
| `ATUIN_SESSION` | Session identifier (set by shell integration or plugin) |
| `ATUIN_PARENT_MEMORY_ID` | Default parent for new memories (used by `memory create`, optional) |
| `ATUIN_LOG` | Log level (e.g., `debug`, `info`) |

## Database Locations

- History database: `~/.local/share/atuin/history.db`
- Memory database: `~/.local/share/atuin/memory.db`
- Configuration: `~/.config/atuin/config.toml`

## Tips for Agents

1. **Always use `--json`** for programmatic access to command output
2. **Set `ATUIN_AGENT_ID`** early in your session to track all commands
3. **Create memories** after completing significant tasks to build searchable context
4. **Use repository filtering** (`--repo`) when working within git repositories
5. **Link commands explicitly** when the automatic `--link-last` might include irrelevant commands
6. **Use parent-child relationships** to organize memories hierarchically (e.g., session root → task → sub-task)
7. **Use `memory run --dry-run`** to preview before replaying commands from a previous session

## Troubleshooting

### Check System Health

```bash
atuin doctor --json
```

### Verify Daemon Status

```bash
atuin daemon status --json
```

### View Store Status

```bash
atuin store status --json
```
