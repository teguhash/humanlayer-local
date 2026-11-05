-- M3: per-task git worktrees.
-- worktree_status: NULL (none), creating, setting_up, ready, setup_failed, failed.
-- branch/base_ref/base_commit stay after the worktree is removed, so a
-- restored task can check the same branch out again.
ALTER TABLE tasks ADD COLUMN branch TEXT;
ALTER TABLE tasks ADD COLUMN base_ref TEXT;
ALTER TABLE tasks ADD COLUMN base_commit TEXT;
ALTER TABLE tasks ADD COLUMN worktree_status TEXT
    CHECK (worktree_status IN ('creating', 'setting_up', 'ready', 'setup_failed', 'failed'));
ALTER TABLE tasks ADD COLUMN worktree_log TEXT;
ALTER TABLE tasks ADD COLUMN archived_at TEXT;

-- The folder a session's CLI runs in, fixed at its first turn. Claude keys
-- resumable sessions by folder, so it must not move later.
ALTER TABLE sessions ADD COLUMN cwd TEXT;
