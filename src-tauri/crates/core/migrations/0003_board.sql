-- M4: board and RPI flow.
-- slug: names the task's docs folder (.humanlayer/tasks/<slug>) and its
-- worktree branch. Unique per project; filled in by the app for old rows.
ALTER TABLE tasks ADD COLUMN slug TEXT;
ALTER TABLE tasks ADD COLUMN description TEXT NOT NULL DEFAULT '';
CREATE UNIQUE INDEX tasks_project_slug ON tasks(project_id, slug);
-- Sessions are listed per task; artifacts are replaced per task on scan.
CREATE INDEX artifacts_task ON artifacts(task_id);
