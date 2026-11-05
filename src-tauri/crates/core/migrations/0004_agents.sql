-- M5: which agent CLI runs a task's stage sessions.
-- A project has a default; a task may override it (NULL = use the default).
ALTER TABLE projects ADD COLUMN default_agent TEXT NOT NULL DEFAULT 'claude'
    CHECK (default_agent IN ('claude', 'codex'));
ALTER TABLE tasks ADD COLUMN agent TEXT CHECK (agent IN ('claude', 'codex'));
