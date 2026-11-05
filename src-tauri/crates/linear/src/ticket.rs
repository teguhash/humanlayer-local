//! Ticket references, images and the `ticket.md` text agents read.

use std::collections::HashMap;

use crate::{Error, Issue, Result};

/// "ENG-123", "eng-123" or a Linear issue URL
/// (`https://linear.app/<org>/issue/ENG-123/<slug>`) -> "ENG-123".
pub fn parse_ref(input: &str) -> Result<String> {
    let s = input.trim();
    let candidate = match s.find("/issue/") {
        Some(i) => s[i + "/issue/".len()..]
            .split(['/', '?', '#'])
            .next()
            .unwrap_or(""),
        None => s,
    };
    let valid = candidate.split_once('-').is_some_and(|(team, num)| {
        !team.is_empty()
            && team.chars().all(|c| c.is_ascii_alphanumeric())
            && team.starts_with(|c: char| c.is_ascii_alphabetic())
            && !num.is_empty()
            && num.chars().all(|c| c.is_ascii_digit())
    });
    if valid {
        Ok(candidate.to_uppercase())
    } else {
        Err(Error::Invalid(format!(
            "“{s}” is not a Linear ticket ID (like ENG-123) or issue URL."
        )))
    }
}

/// Image URLs in Markdown (`![alt](url)`), in order, without duplicates.
pub fn image_urls(markdown: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut rest = markdown;
    while let Some(i) = rest.find("![") {
        rest = &rest[i + 2..];
        let Some(close) = rest.find("](") else { break };
        let after = &rest[close + 2..];
        let Some(end) = after.find(')') else { break };
        // `url "title"` -> url
        let url = after[..end].split_whitespace().next().unwrap_or("");
        if url.starts_with("https://") && !out.iter().any(|u| u == url) {
            out.push(url.to_string());
        }
        rest = &after[end..];
    }
    out
}

/// Replace image URLs with local paths.
pub fn rewrite_images(markdown: &str, local: &HashMap<String, String>) -> String {
    let mut s = markdown.to_string();
    for (url, path) in local {
        s = s.replace(&format!("]({url}"), &format!("]({path}"));
    }
    s
}

/// A file name for the n-th image of a ticket.
pub fn image_file_name(
    identifier: &str,
    n: usize,
    url: &str,
    content_type: Option<&str>,
) -> String {
    let from_type = content_type.and_then(|t| match t.split(';').next()?.trim() {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/svg+xml" => Some("svg"),
        _ => None,
    });
    let from_url = url
        .split(['?', '#'])
        .next()
        .and_then(|p| p.rsplit('/').next())
        .and_then(|f| f.rsplit_once('.'))
        .map(|(_, e)| e.to_lowercase())
        .filter(|e| ["png", "jpg", "jpeg", "gif", "webp", "svg"].contains(&e.as_str()));
    let ext = from_type
        .map(String::from)
        .or(from_url)
        .unwrap_or_else(|| "png".into());
    format!("img-{}-{n}.{ext}", identifier.to_lowercase())
}

/// The ticket as Markdown for the docs folder (`ticket.md`). Image links
/// point at local copies where `local` has them.
pub fn ticket_md(issue: &Issue, local: &HashMap<String, String>) -> String {
    let mut l = vec![
        format!("# [{}] {}", issue.identifier, issue.title),
        String::new(),
    ];
    l.push(format!("- Status: {}", issue.state.name));
    if let Some(a) = &issue.assignee {
        l.push(format!("- Assignee: {a}"));
    }
    if let Some(b) = &issue.branch {
        l.push(format!("- Branch: {b}"));
    }
    if let Some(p) = &issue.parent {
        l.push(format!("- Parent: {p}"));
    }
    l.push(format!("- Linear: {}", issue.url));
    if !issue.description.trim().is_empty() {
        l.push(String::new());
        l.push("## Description".into());
        l.push(String::new());
        l.push(rewrite_images(issue.description.trim(), local));
    }
    if !issue.comments.is_empty() {
        l.push(String::new());
        l.push("## Comments".into());
        let mut cs = issue.comments.clone();
        cs.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        for c in cs {
            l.push(String::new());
            l.push(format!(
                "**{}** ({})",
                c.author.as_deref().unwrap_or("Someone"),
                c.created_at
                    .get(..16)
                    .unwrap_or(&c.created_at)
                    .replace('T', " ")
            ));
            l.push(String::new());
            l.push(rewrite_images(c.body.trim(), local));
        }
    }
    l.push(String::new());
    l.join("\n")
}

/// All image URLs in the description and comments.
pub fn issue_images(issue: &Issue) -> Vec<String> {
    let mut all = image_urls(&issue.description);
    for c in &issue.comments {
        for u in image_urls(&c.body) {
            if !all.contains(&u) {
                all.push(u);
            }
        }
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Comment, State};

    #[test]
    fn refs() {
        assert_eq!(parse_ref(" eng-123 ").unwrap(), "ENG-123");
        assert_eq!(
            parse_ref("https://linear.app/acme/issue/ENG-9/fix-login?x=1").unwrap(),
            "ENG-9"
        );
        assert_eq!(parse_ref("A1B-7").unwrap(), "A1B-7");
        for bad in [
            "",
            "ENG",
            "ENG-",
            "-12",
            "ENG-12a",
            "1AB-2",
            "https://linear.app/acme/team/ENG",
        ] {
            assert!(parse_ref(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn images_and_markdown() {
        let md = "a ![x](https://uploads.linear.app/a/b.png) b ![y](https://e.com/c.jpg \"t\") ![z](https://uploads.linear.app/a/b.png) ![l](local.png)";
        assert_eq!(
            image_urls(md),
            ["https://uploads.linear.app/a/b.png", "https://e.com/c.jpg"]
        );
        assert_eq!(
            image_file_name("ENG-1", 2, "https://x/y/abc", Some("image/jpeg")),
            "img-eng-1-2.jpg"
        );
        assert_eq!(
            image_file_name("ENG-1", 1, "https://x/y/a.GIF?s=1", None),
            "img-eng-1-1.gif"
        );
        let issue = Issue {
            id: "u".into(),
            identifier: "ENG-1".into(),
            title: "Fix login".into(),
            description: "See ![s](https://uploads.linear.app/a/b.png)".into(),
            url: "https://linear.app/x/issue/ENG-1".into(),
            branch: None,
            updated_at: String::new(),
            state: State {
                id: "s".into(),
                name: "Todo".into(),
                kind: "unstarted".into(),
                position: 1.0,
            },
            states: vec![],
            assignee: Some("Ann".into()),
            comments: vec![Comment {
                body: "later".into(),
                created_at: "2026-11-05T09:30:00.000Z".into(),
                author: None,
            }],
            parent: None,
        };
        let local = HashMap::from([(
            "https://uploads.linear.app/a/b.png".to_string(),
            "img-eng-1-1.png".to_string(),
        )]);
        let md = ticket_md(&issue, &local);
        assert!(md.starts_with("# [ENG-1] Fix login\n"));
        assert!(md.contains("- Status: Todo\n- Assignee: Ann\n"));
        assert!(md.contains("See ![s](img-eng-1-1.png)"));
        assert!(md.contains("**Someone** (2026-11-05 09:30)\n\nlater"));
        assert_eq!(issue_images(&issue).len(), 1);
    }
}
