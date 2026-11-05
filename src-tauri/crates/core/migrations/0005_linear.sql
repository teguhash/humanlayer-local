-- M6: tasks linked to Linear tickets. linear_issue_id (from 0001) holds the
-- ticket identifier, e.g. ENG-123.
ALTER TABLE tasks ADD COLUMN linear_url TEXT;
-- Workflow state name and ticket updatedAt, as last synced.
ALTER TABLE tasks ADD COLUMN linear_state TEXT;
ALTER TABLE tasks ADD COLUMN linear_updated_at TEXT;
-- Title and description as last imported. A local edit makes them differ,
-- which stops the sync from overwriting that field.
ALTER TABLE tasks ADD COLUMN linear_synced_title TEXT;
ALTER TABLE tasks ADD COLUMN linear_synced_description TEXT;
CREATE UNIQUE INDEX tasks_linear ON tasks(project_id, linear_issue_id)
    WHERE linear_issue_id IS NOT NULL;

-- Docs posted to the ticket as comments, so a repost updates the comment.
CREATE TABLE linear_posts (
    task_id     INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    path        TEXT NOT NULL,
    comment_id  TEXT NOT NULL,
    posted_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (task_id, path)
);
