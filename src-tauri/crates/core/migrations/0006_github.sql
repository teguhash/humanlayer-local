-- M7: GitHub issues and pull requests. tasks.github_issue (from 0001)
-- holds "owner/repo#12".
ALTER TABLE tasks ADD COLUMN github_issue_url TEXT;
CREATE UNIQUE INDEX tasks_github_issue ON tasks(project_id, github_issue)
    WHERE github_issue IS NOT NULL;

-- pull_requests (from 0001) keeps state, checks_state and review_state;
-- these complete what the board shows.
ALTER TABLE pull_requests ADD COLUMN url TEXT;
ALTER TABLE pull_requests ADD COLUMN title TEXT;
ALTER TABLE pull_requests ADD COLUMN is_draft INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pull_requests ADD COLUMN mergeable TEXT;
ALTER TABLE pull_requests ADD COLUMN merge_state TEXT;
ALTER TABLE pull_requests ADD COLUMN head_ref TEXT;
ALTER TABLE pull_requests ADD COLUMN base_ref TEXT;
CREATE INDEX pull_requests_task ON pull_requests(task_id);
