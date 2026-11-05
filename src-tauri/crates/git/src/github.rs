//! GitHub through the `gh` CLI. Auth is gh's own login, or a token the app
//! passes as `GH_TOKEN` (gh prefers it over its stored login). Flags were
//! checked against gh 2.103.3.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Env, Error, Result};

/// Run gh in `dir` with `stdin`. Returns trimmed stdout.
fn gh(dir: &Path, args: &[&str], env: &Env, stdin: Option<&str>) -> Result<String> {
    let bin = hl_core::shell_env::which("gh", env).ok_or_else(|| {
        Error::Invalid(
            "`gh` (GitHub CLI) not found on PATH. Install it with `brew install gh`.".into(),
        )
    })?;
    let mut child = Command::new(bin)
        .args(args)
        .current_dir(dir)
        .env_clear()
        .envs(env)
        // Never prompt; never open a pager or browser.
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Git(format!("gh: {e}")))?;
    if let Some(text) = stdin {
        let mut i = child.stdin.take().unwrap();
        i.write_all(text.as_bytes())
            .map_err(|e| Error::Git(format!("gh: {e}")))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| Error::Git(format!("gh: {e}")))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(Error::Git(if is_auth_failure(&stderr) {
        format!(
            "GitHub sign-in needed: run `gh auth login`, or add a token in Settings. ({stderr})"
        )
    } else if stderr.is_empty() {
        format!("`gh {}` failed", args.first().unwrap_or(&""))
    } else {
        stderr
    }))
}

/// gh's messages for bad or missing credentials (ported idea from upstream
/// `classify.ts`).
pub fn is_auth_failure(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("gh auth login")
        || m.contains("bad credentials")
        || m.contains("requires authentication")
        || m.contains("http 401")
}

fn json(dir: &Path, args: &[&str], env: &Env) -> Result<Value> {
    let out = gh(dir, args, env, None)?;
    serde_json::from_str(&out).map_err(|e| Error::Git(format!("gh returned bad JSON: {e}")))
}

/// Who gh is signed in as on github.com, with which credential source
/// (e.g. "keyring", "GH_TOKEN"). None when not signed in.
pub fn auth_status(env: &Env) -> Result<Option<(String, String)>> {
    let dir = std::env::temp_dir();
    // Exit 1 when not logged in; the JSON is still printed.
    let v = match json(
        &dir,
        &[
            "auth",
            "status",
            "--hostname",
            "github.com",
            "--json",
            "hosts",
        ],
        env,
    ) {
        Ok(v) => v,
        Err(Error::Git(e)) if e.contains("not logged") || is_auth_failure(&e) => return Ok(None),
        Err(e) => return Err(e),
    };
    let ok = v["hosts"]["github.com"].as_array().and_then(|accts| {
        accts
            .iter()
            .find(|a| a["active"] == true && a["state"] == "success")
            .map(|a| {
                (
                    a["login"].as_str().unwrap_or("?").to_string(),
                    a["tokenSource"].as_str().unwrap_or("?").to_string(),
                )
            })
    });
    Ok(ok)
}

/// Check a token by asking who it belongs to.
pub fn token_login(token: &str, env: &Env) -> Result<String> {
    let mut env = env.clone();
    env.insert("GH_TOKEN".into(), token.into());
    let v = json(&std::env::temp_dir(), &["api", "user"], &env)?;
    v["login"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| Error::Git("GitHub did not return a login for this token".into()))
}

/// `owner/name` of the GitHub repo that `dir`'s remotes point at.
pub fn repo_name(dir: &Path, env: &Env) -> Result<String> {
    let v = json(dir, &["repo", "view", "--json", "nameWithOwner"], env)?;
    v["nameWithOwner"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| Error::Git("gh did not report a repository".into()))
}

/// The repo's default branch name.
pub fn default_branch(dir: &Path, repo: &str, env: &Env) -> Result<String> {
    let v = json(
        dir,
        &["repo", "view", repo, "--json", "defaultBranchRef"],
        env,
    )?;
    v["defaultBranchRef"]["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| Error::Git(format!("{repo} has no default branch")))
}

/// An issue reference: `12`, `#12`, `owner/repo#12` or an issue URL.
/// Returns (repo, number); repo is None when it is the project's own.
pub fn parse_issue_ref(input: &str) -> Result<(Option<String>, u64)> {
    let s = input.trim();
    let bad = || {
        Error::Invalid(format!(
            "“{s}” is not an issue number, owner/repo#123 or GitHub issue URL."
        ))
    };
    if let Some(rest) = s
        .strip_prefix("https://github.com/")
        .or_else(|| s.strip_prefix("http://github.com/"))
    {
        let parts: Vec<&str> = rest.split(['/', '?', '#']).collect();
        if parts.len() >= 4 && parts[2] == "issues" {
            let n = parts[3].parse().map_err(|_| bad())?;
            return Ok((Some(format!("{}/{}", parts[0], parts[1])), n));
        }
        return Err(bad());
    }
    let (repo, num) = match s.rsplit_once('#') {
        Some((r, n)) if !r.is_empty() => (Some(r), n),
        Some((_, n)) => (None, n),
        None => (None, s),
    };
    if let Some(r) = repo {
        let ok = r
            .split_once('/')
            .is_some_and(|(o, n)| !o.is_empty() && !n.is_empty() && !n.contains('/'));
        if !ok {
            return Err(bad());
        }
    }
    let n: u64 = num.parse().map_err(|_| bad())?;
    if n == 0 {
        return Err(bad());
    }
    Ok((repo.map(String::from), n))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Issue {
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub url: String,
    pub state: String,
    pub labels: Vec<String>,
    /// (author, date, body)
    pub comments: Vec<(String, String, String)>,
}

pub fn issue(dir: &Path, repo: &str, number: u64, env: &Env) -> Result<Issue> {
    let n = number.to_string();
    let v = json(
        dir,
        &[
            "issue",
            "view",
            &n,
            "--repo",
            repo,
            "--json",
            "number,title,body,url,state,labels,comments",
        ],
        env,
    )?;
    Ok(parse_issue(repo, &v))
}

fn parse_issue(repo: &str, v: &Value) -> Issue {
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    Issue {
        repo: repo.into(),
        number: v["number"].as_u64().unwrap_or_default(),
        title: s("title"),
        body: s("body").replace("\r\n", "\n"),
        url: s("url"),
        state: s("state"),
        labels: v["labels"]
            .as_array()
            .map(|ls| {
                ls.iter()
                    .filter_map(|l| l["name"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        comments: v["comments"]
            .as_array()
            .map(|cs| {
                cs.iter()
                    .map(|c| {
                        (
                            c["author"]["login"]
                                .as_str()
                                .unwrap_or("someone")
                                .to_string(),
                            c["createdAt"].as_str().unwrap_or_default().to_string(),
                            c["body"].as_str().unwrap_or_default().replace("\r\n", "\n"),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// The issue as Markdown for the docs folder.
pub fn issue_md(i: &Issue) -> String {
    let mut l = vec![
        format!("# [{}#{}] {}", i.repo, i.number, i.title),
        String::new(),
    ];
    l.push(format!("- State: {}", i.state.to_lowercase()));
    if !i.labels.is_empty() {
        l.push(format!("- Labels: {}", i.labels.join(", ")));
    }
    l.push(format!("- GitHub: {}", i.url));
    if !i.body.trim().is_empty() {
        l.extend([
            String::new(),
            "## Description".into(),
            String::new(),
            i.body.trim().into(),
        ]);
    }
    if !i.comments.is_empty() {
        l.extend([String::new(), "## Comments".into()]);
        for (who, when, body) in &i.comments {
            l.extend([
                String::new(),
                format!(
                    "**{who}** ({})",
                    when.get(..16).unwrap_or(when).replace('T', " ")
                ),
                String::new(),
                body.trim().into(),
            ]);
        }
    }
    l.push(String::new());
    l.join("\n")
}

/// A pull request's state, summarized for the board.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Pr {
    pub repo: String,
    pub number: u64,
    pub url: String,
    pub title: String,
    /// OPEN | CLOSED | MERGED
    pub state: String,
    pub is_draft: bool,
    /// MERGEABLE | CONFLICTING | UNKNOWN
    pub mergeable: String,
    /// CLEAN | BLOCKED | BEHIND | DIRTY | UNSTABLE | HAS_HOOKS | DRAFT | UNKNOWN
    pub merge_state: String,
    /// APPROVED | CHANGES_REQUESTED | REVIEW_REQUIRED | "" (none needed)
    pub review: String,
    /// passing | failing | pending | none
    pub checks: String,
    pub head: String,
    pub base: String,
}

const PR_FIELDS: &str = "number,url,title,state,isDraft,mergeable,mergeStateStatus,reviewDecision,statusCheckRollup,headRefName,baseRefName";

pub fn pr(dir: &Path, repo: &str, number: u64, env: &Env) -> Result<Pr> {
    let n = number.to_string();
    let v = json(
        dir,
        &["pr", "view", &n, "--repo", repo, "--json", PR_FIELDS],
        env,
    )?;
    Ok(parse_pr(repo, &v))
}

pub fn parse_pr(repo: &str, v: &Value) -> Pr {
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    Pr {
        repo: repo.into(),
        number: v["number"].as_u64().unwrap_or_default(),
        url: s("url"),
        title: s("title"),
        state: s("state"),
        is_draft: v["isDraft"].as_bool().unwrap_or(false),
        mergeable: s("mergeable"),
        merge_state: s("mergeStateStatus"),
        review: s("reviewDecision"),
        checks: checks_summary(&v["statusCheckRollup"]).into(),
        head: s("headRefName"),
        base: s("baseRefName"),
    }
}

/// Roll CheckRuns (status + conclusion) and StatusContexts (state) up into
/// one word. Skipped and neutral runs count as passing.
pub fn checks_summary(rollup: &Value) -> &'static str {
    let Some(items) = rollup.as_array().filter(|a| !a.is_empty()) else {
        return "none";
    };
    let mut pending = false;
    for c in items {
        let outcome = if c["__typename"] == "StatusContext" {
            c["state"].as_str().unwrap_or("PENDING")
        } else if c["status"].as_str().is_some_and(|s| s != "COMPLETED") {
            "PENDING"
        } else {
            c["conclusion"].as_str().unwrap_or("PENDING")
        };
        match outcome {
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => {}
            "PENDING" | "EXPECTED" | "QUEUED" | "IN_PROGRESS" | "WAITING" | "REQUESTED" | "" => {
                pending = true
            }
            _ => return "failing",
        }
    }
    if pending { "pending" } else { "passing" }
}

/// Push `branch` from the worktree to `remote` and set it as upstream.
pub fn push(wt: &Path, remote: &str, branch: &str, env: &Env) -> Result<()> {
    crate::git(wt, &["push", "--set-upstream", remote, branch], env).map(|_| ())
}

/// Commits on HEAD that `base` does not have.
pub fn commits_ahead(wt: &Path, base: &str, env: &Env) -> Result<u64> {
    let n = crate::git(wt, &["rev-list", "--count", &format!("{base}..HEAD")], env)?;
    n.parse()
        .map_err(|_| Error::Git(format!("unexpected rev-list output: {n}")))
}

pub struct NewPr<'a> {
    pub repo: &'a str,
    pub head: &'a str,
    pub base: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub draft: bool,
}

/// Open a PR for `head` into `base`. With `--head` set, gh does not try to
/// push or fork (the app pushed already). Returns the PR number.
pub fn create_pr(dir: &Path, p: &NewPr, env: &Env) -> Result<u64> {
    let mut args = vec![
        "pr",
        "create",
        "--repo",
        p.repo,
        "--head",
        p.head,
        "--base",
        p.base,
        "--title",
        p.title,
        "--body-file",
        "-",
    ];
    if p.draft {
        args.push("--draft");
    }
    let out = gh(dir, &args, env, Some(p.body))?;
    // gh prints the PR URL.
    out.lines()
        .rev()
        .find_map(|l| {
            l.trim()
                .rsplit_once("/pull/")
                .and_then(|(_, n)| n.parse().ok())
        })
        .ok_or_else(|| Error::Git(format!("gh did not print a PR URL: {out}")))
}

/// The open PR for `head`, if one exists already.
pub fn find_pr(dir: &Path, repo: &str, head: &str, env: &Env) -> Result<Option<u64>> {
    let v = json(
        dir,
        &[
            "pr", "list", "--repo", repo, "--head", head, "--state", "open", "--json", "number",
        ],
        env,
    )?;
    Ok(v.as_array()
        .and_then(|a| a.first())
        .and_then(|p| p["number"].as_u64()))
}

/// Merge with `method` (merge | squash | rebase). The branch is kept: the
/// task's worktree may still have it checked out.
pub fn merge_pr(dir: &Path, repo: &str, number: u64, method: &str, env: &Env) -> Result<()> {
    let flag = match method {
        "merge" => "--merge",
        "squash" => "--squash",
        "rebase" => "--rebase",
        _ => return Err(Error::Invalid(format!("Unknown merge method {method}."))),
    };
    let n = number.to_string();
    gh(dir, &["pr", "merge", &n, "--repo", repo, flag], env, None).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn issue_refs() {
        assert_eq!(parse_issue_ref("12").unwrap(), (None, 12));
        assert_eq!(parse_issue_ref(" #7 ").unwrap(), (None, 7));
        assert_eq!(
            parse_issue_ref("cli/cli#3").unwrap(),
            (Some("cli/cli".into()), 3)
        );
        assert_eq!(
            parse_issue_ref("https://github.com/cli/cli/issues/14?x=1").unwrap(),
            (Some("cli/cli".into()), 14)
        );
        for bad in [
            "",
            "#",
            "abc",
            "0",
            "a#1",
            "a/b/c#1",
            "https://github.com/cli/cli/pull/3",
        ] {
            assert!(parse_issue_ref(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn checks_rollup() {
        assert_eq!(checks_summary(&json!([])), "none");
        let run = |status: &str, conclusion: &str| json!({"__typename":"CheckRun","status":status,"conclusion":conclusion});
        assert_eq!(
            checks_summary(&json!([
                run("COMPLETED", "SUCCESS"),
                run("COMPLETED", "SKIPPED")
            ])),
            "passing"
        );
        assert_eq!(
            checks_summary(&json!([
                run("COMPLETED", "SUCCESS"),
                run("IN_PROGRESS", "")
            ])),
            "pending"
        );
        assert_eq!(
            checks_summary(&json!([
                run("IN_PROGRESS", ""),
                run("COMPLETED", "FAILURE")
            ])),
            "failing"
        );
        let ctx = |s: &str| json!({"__typename":"StatusContext","state":s});
        assert_eq!(
            checks_summary(&json!([ctx("SUCCESS"), ctx("PENDING")])),
            "pending"
        );
        assert_eq!(checks_summary(&json!([ctx("ERROR")])), "failing");
    }

    #[test]
    fn parse_pr_and_issue() {
        let v = json!({"baseRefName":"main","headRefName":"t/x","isDraft":false,"mergeStateStatus":"BLOCKED",
            "mergeable":"MERGEABLE","number":5,"reviewDecision":"REVIEW_REQUIRED","state":"OPEN",
            "statusCheckRollup":[],"title":"T","url":"https://github.com/o/r/pull/5"});
        let p = parse_pr("o/r", &v);
        assert_eq!(
            (p.number, p.checks.as_str(), p.review.as_str()),
            (5, "none", "REVIEW_REQUIRED")
        );
        let i = parse_issue(
            "o/r",
            &json!({"number":3,"title":"Bug","body":"a\r\nb","url":"u","state":"OPEN",
                    "labels":[{"name":"bug"}],"comments":[{"author":{"login":"ann"},"createdAt":"2026-11-05T09:30:00Z","body":"me too"}]}),
        );
        let md = issue_md(&i);
        assert!(md.starts_with("# [o/r#3] Bug\n"));
        assert!(md.contains("- Labels: bug\n"));
        assert!(md.contains("a\nb"));
        assert!(md.contains("**ann** (2026-11-05 09:30)\n\nme too"));
    }

    #[test]
    fn auth_messages() {
        assert!(is_auth_failure("gh: HTTP 401: Bad credentials"));
        assert!(is_auth_failure(
            "To get started with GitHub CLI, please run:  gh auth login"
        ));
        assert!(!is_auth_failure("could not find pull request"));
    }

    /// Real gh, read-only, public repo. `cargo test -- --ignored gh_live`
    #[test]
    #[ignore]
    fn gh_live() {
        let env: Env = std::env::vars().collect();
        let dir = std::env::temp_dir();
        let p = pr(&dir, "cli/cli", 1, &env).unwrap();
        assert_eq!(p.state, "MERGED");
        let i = issue(&dir, "cli/cli", 1, &env).unwrap();
        assert!(!i.title.is_empty());
        println!("auth: {:?}", auth_status(&env).unwrap());
    }
}
