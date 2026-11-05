-- Data model from PLAN.md. Timestamps are ISO-8601 UTC text.
-- Deleting a row deletes everything under it (ON DELETE CASCADE).

CREATE TABLE projects (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL CHECK (length(trim(name)) > 0),
    repo_path   TEXT NOT NULL UNIQUE,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE tasks (
    id               INTEGER PRIMARY KEY,
    project_id       INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    title            TEXT NOT NULL CHECK (length(trim(title)) > 0),
    stage            TEXT NOT NULL DEFAULT 'backlog'
                     CHECK (stage IN ('backlog', 'research', 'plan', 'implement', 'review', 'done')),
    position         INTEGER NOT NULL,
    worktree_path    TEXT,
    linear_issue_id  TEXT,
    github_issue     TEXT,
    created_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX tasks_project ON tasks(project_id, stage, position);

CREATE TABLE sessions (
    id              INTEGER PRIMARY KEY,
    task_id         INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    agent_kind      TEXT NOT NULL CHECK (agent_kind IN ('claude', 'codex')),
    cli_session_id  TEXT,
    skill           TEXT,
    status          TEXT NOT NULL DEFAULT 'idle'
                    CHECK (status IN ('idle', 'running', 'completed', 'failed', 'interrupted')),
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    started_at      TEXT,
    ended_at        TEXT
);
CREATE INDEX sessions_task ON sessions(task_id);

CREATE TABLE events (
    id            INTEGER PRIMARY KEY,
    session_id    INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq           INTEGER NOT NULL,
    kind          TEXT NOT NULL,
    payload_json  TEXT NOT NULL,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (session_id, seq)
);

CREATE TABLE artifacts (
    id          INTEGER PRIMARY KEY,
    task_id     INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    path        TEXT NOT NULL,
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (task_id, path)
);

CREATE TABLE pull_requests (
    id            INTEGER PRIMARY KEY,
    task_id       INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    repo          TEXT NOT NULL,
    number        INTEGER NOT NULL,
    state         TEXT,
    checks_state  TEXT,
    review_state  TEXT,
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (repo, number)
);
