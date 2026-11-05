//! Linear GraphQL API with a personal API key (`Authorization: <key>`).
//! The key is only ever sent in that header; it is never logged or put in
//! an error message.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Error, Result};

pub const ENDPOINT: &str = "https://api.linear.app/graphql";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct State {
    pub id: String,
    pub name: String,
    /// triage | backlog | unstarted | started | completed | canceled
    #[serde(rename = "type")]
    pub kind: String,
    pub position: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Comment {
    pub body: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    pub author: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Issue {
    /// Linear's internal id (used for mutations).
    pub id: String,
    /// e.g. ENG-123
    pub identifier: String,
    pub title: String,
    pub description: String,
    pub url: String,
    pub branch: Option<String>,
    pub updated_at: String,
    pub state: State,
    /// The team's workflow states.
    pub states: Vec<State>,
    pub assignee: Option<String>,
    pub comments: Vec<Comment>,
    /// "ENG-100 Parent title"
    pub parent: Option<String>,
}

const ISSUE_FIELDS: &str = "
fragment IssueFields on Issue {
  id identifier title description url branchName updatedAt
  state { id name type position }
  team { states { nodes { id name type position } } }
  assignee { name }
  parent { identifier title }
  comments(first: 50) { nodes { body createdAt user { name } } }
}";

pub struct Client {
    http: reqwest::Client,
    key: String,
    endpoint: String,
}

impl Client {
    pub fn new(key: &str) -> Self {
        Self::with_endpoint(key, ENDPOINT)
    }

    /// For tests against a local server.
    pub fn with_endpoint(key: &str, endpoint: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("humanlayer-local")
            .build()
            .expect("http client");
        Self {
            http,
            key: key.trim().to_string(),
            endpoint: endpoint.to_string(),
        }
    }

    /// Run a query. Returns `data`, plus GraphQL errors as (path, message).
    async fn raw(&self, query: &str, vars: Value) -> Result<(Value, Vec<(String, String)>)> {
        let res = self
            .http
            .post(&self.endpoint)
            .header("Authorization", &self.key)
            .json(&json!({ "query": query, "variables": vars }))
            .send()
            .await
            .map_err(|e| Error::Network(e.without_url().to_string()))?;
        let status = res.status();
        let body: Value = res
            .json()
            .await
            .map_err(|e| Error::Network(e.without_url().to_string()))?;
        let errors: Vec<(String, String)> = body["errors"]
            .as_array()
            .map(|es| {
                es.iter()
                    .map(|e| {
                        let path = e["path"][0].as_str().unwrap_or_default().to_string();
                        let msg = e["message"].as_str().unwrap_or("unknown error").to_string();
                        let code = e["extensions"]["code"].as_str().unwrap_or_default();
                        (
                            path,
                            if code.is_empty() {
                                msg
                            } else {
                                format!("{code}: {msg}")
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let auth_failed = status == reqwest::StatusCode::UNAUTHORIZED
            || errors
                .iter()
                .any(|(_, m)| m.starts_with("AUTHENTICATION_ERROR"));
        if auth_failed {
            return Err(Error::Auth);
        }
        if !status.is_success() && body["data"].is_null() {
            let msg = errors
                .first()
                .map(|(_, m)| m.clone())
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(Error::Api(msg));
        }
        Ok((body["data"].clone(), errors))
    }

    /// Like `raw`, but any GraphQL error is an error.
    async fn query(&self, query: &str, vars: Value) -> Result<Value> {
        let (data, errors) = self.raw(query, vars).await?;
        match errors.into_iter().next() {
            Some((_, m)) => Err(Error::Api(m)),
            None => Ok(data),
        }
    }

    /// Who the key belongs to: "Name (Organization)".
    pub async fn viewer(&self) -> Result<String> {
        let d = self
            .query("query { viewer { name organization { name } } }", json!({}))
            .await?;
        Ok(format!(
            "{} ({})",
            d["viewer"]["name"].as_str().unwrap_or("?"),
            d["viewer"]["organization"]["name"].as_str().unwrap_or("?")
        ))
    }

    /// One issue by identifier (ENG-123) or id.
    pub async fn issue(&self, id: &str) -> Result<Issue> {
        let mut found = self.issues(&[id.to_string()]).await?;
        found
            .pop()
            .flatten()
            .ok_or_else(|| Error::NotFound(id.into()))
    }

    /// Several issues in one request. `None` where an issue was not found.
    pub async fn issues(&self, ids: &[String]) -> Result<Vec<Option<Issue>>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let vars: Vec<String> = (0..ids.len()).map(|i| format!("$v{i}: String!")).collect();
        let fields: Vec<String> = (0..ids.len())
            .map(|i| format!("i{i}: issue(id: $v{i}) {{ ...IssueFields }}"))
            .collect();
        let query = format!(
            "query({}) {{ {} }}\n{ISSUE_FIELDS}",
            vars.join(", "),
            fields.join(" ")
        );
        let mut v = serde_json::Map::new();
        for (i, id) in ids.iter().enumerate() {
            v.insert(format!("v{i}"), id.to_uppercase().into());
        }
        let (data, errors) = self.raw(&query, Value::Object(v)).await?;
        let mut out = vec![];
        for i in 0..ids.len() {
            let key = format!("i{i}");
            match &data[&key] {
                Value::Null => {
                    // Not found is reported as an error on this alias.
                    let err = errors.iter().find(|(p, _)| *p == key);
                    if let Some((_, m)) = err
                        && !is_not_found(m)
                    {
                        return Err(Error::Api(m.clone()));
                    }
                    out.push(None);
                }
                raw => out.push(Some(parse_issue(raw)?)),
            }
        }
        Ok(out)
    }

    pub async fn set_state(&self, issue_id: &str, state_id: &str) -> Result<()> {
        let d = self
            .query(
                "mutation($id: String!, $s: String!) { issueUpdate(id: $id, input: { stateId: $s }) { success } }",
                json!({ "id": issue_id, "s": state_id }),
            )
            .await?;
        ok(&d["issueUpdate"])
    }

    /// Post a comment; returns its id.
    pub async fn create_comment(&self, issue_id: &str, body: &str) -> Result<String> {
        let d = self
            .query(
                "mutation($id: String!, $b: String!) { commentCreate(input: { issueId: $id, body: $b }) { success comment { id } } }",
                json!({ "id": issue_id, "b": body }),
            )
            .await?;
        ok(&d["commentCreate"])?;
        d["commentCreate"]["comment"]["id"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| Error::Api("comment id missing".into()))
    }

    /// Ok(false) if the comment no longer exists.
    pub async fn update_comment(&self, comment_id: &str, body: &str) -> Result<bool> {
        let (d, errors) = self
            .raw(
                "mutation($id: String!, $b: String!) { commentUpdate(id: $id, input: { body: $b }) { success } }",
                json!({ "id": comment_id, "b": body }),
            )
            .await?;
        if let Some((_, m)) = errors.first() {
            return if is_not_found(m) {
                Ok(false)
            } else {
                Err(Error::Api(m.clone()))
            };
        }
        ok(&d["commentUpdate"]).map(|_| true)
    }

    /// Download a file the ticket links to. Linear's own uploads need the
    /// key; other hosts get no Authorization header.
    pub async fn download(&self, url: &str) -> Result<(Vec<u8>, Option<String>)> {
        let mut req = self.http.get(url);
        if is_linear_upload(url) {
            req = req.header("Authorization", &self.key);
        }
        let res = req
            .send()
            .await
            .map_err(|e| Error::Network(e.without_url().to_string()))?;
        if !res.status().is_success() {
            return Err(Error::Api(format!(
                "download failed: HTTP {}",
                res.status()
            )));
        }
        let ctype = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let bytes = res
            .bytes()
            .await
            .map_err(|e| Error::Network(e.without_url().to_string()))?;
        Ok((bytes.to_vec(), ctype))
    }
}

pub fn is_linear_upload(url: &str) -> bool {
    url.starts_with("https://uploads.linear.app/")
}

fn is_not_found(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("not found") || m.contains("entity not found") || m.contains("could not find")
}

fn ok(v: &Value) -> Result<()> {
    if v["success"].as_bool() == Some(true) {
        Ok(())
    } else {
        Err(Error::Api(
            "Linear reported the change as unsuccessful".into(),
        ))
    }
}

fn parse_issue(v: &Value) -> Result<Issue> {
    let state = |s: &Value| -> Result<State> {
        serde_json::from_value(s.clone()).map_err(|e| Error::Api(format!("bad state: {e}")))
    };
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    Ok(Issue {
        id: s("id"),
        identifier: s("identifier"),
        title: s("title"),
        description: s("description"),
        url: s("url"),
        branch: v["branchName"]
            .as_str()
            .filter(|b| !b.is_empty())
            .map(String::from),
        updated_at: s("updatedAt"),
        state: state(&v["state"])?,
        states: v["team"]["states"]["nodes"]
            .as_array()
            .map(|xs| xs.iter().map(state).collect::<Result<Vec<_>>>())
            .transpose()?
            .unwrap_or_default(),
        assignee: v["assignee"]["name"].as_str().map(String::from),
        comments: v["comments"]["nodes"]
            .as_array()
            .map(|xs| {
                xs.iter()
                    .map(|c| Comment {
                        body: c["body"].as_str().unwrap_or_default().into(),
                        created_at: c["createdAt"].as_str().unwrap_or_default().into(),
                        author: c["user"]["name"].as_str().map(String::from),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        parent: v["parent"]["identifier"]
            .as_str()
            .map(|id| format!("{id} {}", v["parent"]["title"].as_str().unwrap_or_default())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Serve one canned response per connection; return the requests seen.
    async fn serve(responses: Vec<(u16, Value)>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/graphql", l.local_addr().unwrap());
        let h = tokio::spawn(async move {
            let mut seen = vec![];
            for (code, body) in responses {
                let (mut s, _) = l.accept().await.unwrap();
                let mut buf = vec![0u8; 65536];
                let mut req = String::new();
                // Read headers and body (Content-Length) of one request.
                loop {
                    let n = s.read(&mut buf).await.unwrap();
                    req.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if let Some(i) = req.find("\r\n\r\n") {
                        let len = req[..i]
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if req.len() >= i + 4 + len {
                            break;
                        }
                    }
                }
                seen.push(req);
                let b = body.to_string();
                let resp = format!(
                    "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{b}",
                    b.len()
                );
                s.write_all(resp.as_bytes()).await.unwrap();
            }
            seen
        });
        (url, h)
    }

    fn issue_json(identifier: &str) -> Value {
        json!({
            "id": "uuid-1", "identifier": identifier, "title": "Fix login",
            "description": "It breaks", "url": "https://linear.app/x/issue/ENG-1/fix-login",
            "branchName": "eng-1-fix-login", "updatedAt": "2026-11-05T10:00:00.000Z",
            "state": {"id": "s1", "name": "Todo", "type": "unstarted", "position": 1.0},
            "team": {"states": {"nodes": [
                {"id": "s1", "name": "Todo", "type": "unstarted", "position": 1.0},
                {"id": "s2", "name": "In Progress", "type": "started", "position": 2.0}
            ]}},
            "assignee": null, "parent": null,
            "comments": {"nodes": [{"body": "hi", "createdAt": "2026-11-05T09:00:00.000Z", "user": {"name": "Ann"}}]}
        })
    }

    #[tokio::test(flavor = "current_thread")]
    async fn issues_batch_auth_and_not_found() {
        let (url, h) = serve(vec![
            (
                200,
                json!({"data": {"i0": issue_json("ENG-1"), "i1": null},
                       "errors": [{"message": "Entity not found: Issue", "path": ["i1"]}]}),
            ),
            (
                400,
                json!({"errors": [{"message": "Authentication required, not authenticated",
                       "extensions": {"code": "AUTHENTICATION_ERROR"}}]}),
            ),
        ])
        .await;
        let c = Client::with_endpoint("lin_api_test", &url);
        let got = c.issues(&["eng-1".into(), "ENG-404".into()]).await.unwrap();
        let i = got[0].as_ref().unwrap();
        assert_eq!(
            (i.identifier.as_str(), i.state.name.as_str()),
            ("ENG-1", "Todo")
        );
        assert_eq!(i.states.len(), 2);
        assert_eq!(i.comments[0].author.as_deref(), Some("Ann"));
        assert!(got[1].is_none());
        assert!(matches!(c.viewer().await, Err(Error::Auth)));
        let reqs = h.await.unwrap();
        // The key goes in the header as is; identifiers are upper-cased.
        assert!(
            reqs[0]
                .to_lowercase()
                .contains("authorization: lin_api_test")
        );
        assert!(reqs[0].contains("\"v0\":\"ENG-1\""));
    }
}
