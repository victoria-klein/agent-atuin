-- Add agent_id column to track which agent executed each command
alter table history add column agent_id text;
create index idx_history_agent_id on history(agent_id);
