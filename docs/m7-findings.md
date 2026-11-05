# M7 findings: GitHub

Date: 2026-11-05. gh 2.103.3. Flags were checked against `gh pr create --help`, `gh pr merge --help`, `gh pr view --help`, `gh issue view --help` and `gh auth status --help`.

## Auth

- Every GitHub call goes through `gh`, run with `GH_PROMPT_DISABLED=1`, so it never prompts.
- **gh login:** used by default. Settings shows the account from `gh auth status --hostname github.com --json hosts`: login, and where the token comes from (e.g. `keyring`).
- **Fallback token:** saved in the macOS Keychain (account `github-token`), after `gh api user` confirms it works.
  - The app passes it to gh and to agent sessions as `GH_TOKEN`, which gh prefers over its stored login.
  - Settings then shows the source as `GH_TOKEN` ("saved token"). Removing it goes back to gh's login.
- gh's auth failures (401, "Bad credentials", "gh auth login") turn into one message that says what to do. This ports the idea of upstream `classify.ts`.
- `gh` must be installed in both cases, because opening and merging PRs use it (the plan names `gh pr create` and `gh pr merge`).

## Issue import

- Accepted: `12`, `#12`, `owner/repo#12` or `https://github.com/owner/repo/issues/12`.
- A bare number means the project's own repo, from `gh repo view --json nameWithOwner` run in the repo folder.
- The task is created as follows:
  - Its title and description come from the issue.
  - Its slug is `gh-12-<title>`.
  - Each issue can be imported once per project.
- `github-issue.md` in the docs folder holds the state, labels, link, body and comments.
- The one import box tells the two apart: `#12`, `owner/repo#12` and github.com URLs go to GitHub; anything else goes to Linear.

## Opening a PR

The task needs a ready worktree. The app then:
1. Picks the base: the task's `base_ref`. If the worktree branched from a detached commit, it uses the repo's default branch (`gh repo view --json defaultBranchRef`).
2. Refuses if the worktree has uncommitted files, because a PR only carries commits. It also refuses if the branch has no commits the base lacks.
3. Pushes the branch: `git push --set-upstream origin <branch>`.
4. Links the open PR for that branch if one exists (`gh pr list --head`). Otherwise it creates one with `gh pr create --repo --head --base --title --body-file -`. Because `--head` is given, gh does not push or offer a fork.
5. Builds the body from the newest PR-description doc (the `describe-pr` skill writes one), or the task description if there is none. It adds `Closes #12` for an issue from the same repo, plus the Linear link.

## Polling and merging

- Open PRs are refreshed every 45 seconds with `gh pr view --json number,url,title,state,isDraft,mergeable,mergeStateStatus,reviewDecision,statusCheckRollup,headRefName,baseRefName`.
- `statusCheckRollup` is reduced to one word: any failure → failing, any pending → pending, all done → passing, nothing → none. CheckRuns use status and conclusion; StatusContexts use state. Skipped and neutral count as passing.
- The board card shows e.g. "PR #5 · checks passing · approved". The task view adds badges, a "Not ready" reason from `mergeStateStatus`, Refresh, and Merge.
- **Merge:** `gh pr merge <n> --repo <r> --squash|--merge|--rebase`, with no `--delete-branch`, because the task's worktree still has that branch checked out.
- When a PR is first seen as merged, the task moves to Done. That also moves a linked Linear ticket to Done, forward only.

## Tests

- Unit tests:
  - Issue reference parsing, check roll-up, PR and issue JSON, `github-issue.md`, and auth-error detection.
  - In the DB: issue tasks, the duplicate refusal, and PR upsert and open-PR listing.
- `cargo test -- --ignored gh_live` passed: real gh, read-only, against `cli/cli`. It read PR #1 and issue #1 and the auth status (login via keyring).
- Not tested automatically: push, `gh pr create` and `gh pr merge`. They change a real repo, so they are part of the hand test.
