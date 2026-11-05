use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hl_core::Db;
use hl_core::model::{AgentKind, Session, status};
use serde_json::json;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::runtime::EnvFn;
use crate::transcript::{Decision, Item};
use crate::{Runtime, SessionEvent, Sink, claude, event_type, protocol};

// ---------- pure protocol tests ----------

#[test]
fn event_types() {
    let c = json!({"type":"system","subtype":"init"});
    assert_eq!(event_type(&c).unwrap(), "system/init");
    let r = json!({"type":"control_request","request":{"subtype":"can_use_tool"}});
    assert_eq!(event_type(&r).unwrap(), "control_request/can_use_tool");
    let x = json!({"type":"item.completed","item":{"type":"agent_message"}});
    assert_eq!(event_type(&x).unwrap(), "item.completed/agent_message");
    assert!(event_type(&json!([1])).is_none());
}

#[test]
fn claude_parses_core_events() {
    let p = protocol(AgentKind::Claude);
    let a = json!({"type":"assistant","message":{"content":[
        {"type":"text","text":"hi"},
        {"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}});
    assert_eq!(
        p.parse(&a),
        vec![
            Item::AssistantText { text: "hi".into() },
            Item::ToolUse {
                id: "t1".into(),
                name: "Bash".into(),
                input: json!({"command":"ls"})
            }
        ]
    );
    let u = json!({"type":"user","message":{"content":[
        {"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"a"},{"type":"text","text":"b"}],"is_error":true}]}});
    assert_eq!(
        p.parse(&u),
        vec![Item::ToolResult {
            tool_use_id: "t1".into(),
            content: "a\nb".into(),
            is_error: true
        }]
    );
    let r = json!({"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.5,
        "usage":{"input_tokens":1,"output_tokens":2,"cache_read_input_tokens":3,"cache_creation_input_tokens":4}});
    match &p.parse(&r)[0] {
        Item::TurnEnd {
            ok,
            cost_usd,
            usage,
            ..
        } => {
            assert!(ok);
            assert_eq!(*cost_usd, Some(0.5));
            assert_eq!(usage.as_ref().unwrap().cache_creation_tokens, 4);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn claude_sanitize_drops_account() {
    let p = protocol(AgentKind::Claude);
    let v = json!({"type":"control_response","response":{"response":{"account":{"email":"a@b"},"pid":1}}});
    let s = p.sanitize(v);
    assert!(s.pointer("/response/response/account").is_none());
    assert_eq!(s.pointer("/response/response/pid").unwrap(), 1);
}

#[test]
fn session_permissions_are_always_session_scoped() {
    let req = json!({"tool_name":"Write","permission_suggestions":[
        {"type":"setMode","mode":"acceptEdits","destination":"localSettings"}]});
    let p = claude::session_permissions(&req);
    assert_eq!(p[0]["destination"], "session");
    assert_eq!(p[0]["mode"], "acceptEdits");
    let none = claude::session_permissions(&json!({"tool_name":"Bash"}));
    assert_eq!(none[0]["type"], "addRules");
    assert_eq!(none[0]["rules"][0]["toolName"], "Bash");
    assert_eq!(none[0]["destination"], "session");
}

#[test]
fn codex_args_and_parse() {
    let p = protocol(AgentKind::Codex);
    let fresh = p.args("/w", None);
    assert!(fresh.windows(2).any(|w| w == ["--cd", "/w"]));
    let resumed = p.args("/w", Some("T1"));
    assert_eq!(&resumed[..2], ["exec", "resume"]);
    assert_eq!(resumed.last().unwrap(), "-");
    assert!(resumed.contains(&"T1".to_string()));
    let tc = json!({"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":60,"output_tokens":7}});
    match &p.parse(&tc)[0] {
        Item::TurnEnd { usage, .. } => {
            let u = usage.as_ref().unwrap();
            assert_eq!(
                (u.input_tokens, u.cache_read_tokens, u.output_tokens),
                (40, 60, 7)
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        p.cli_session_id(&json!({"type":"thread.started","thread_id":"T9"})),
        Some("T9".into())
    );
}

// ---------- runtime tests with a fake claude ----------

enum Msg {
    Event(SessionEvent),
    Session(Session),
}

struct ChanSink(UnboundedSender<Msg>);

impl Sink for ChanSink {
    fn event(&self, e: &SessionEvent) {
        let _ = self.0.send(Msg::Event(e.clone()));
    }
    fn session(&self, s: &Session) {
        let _ = self.0.send(Msg::Session(s.clone()));
    }
}

struct Fx {
    db: Arc<Db>,
    rt: Arc<Runtime>,
    rx: UnboundedReceiver<Msg>,
    session: Session,
    env: Arc<EnvFn>,
    dir: PathBuf,
}

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "hl-agents-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// PATH with `claude` pointing at the fake script.
fn fake_env(dir: &std::path::Path) -> Arc<EnvFn> {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-claude");
    std::os::unix::fs::symlink(fake, bin.join("claude")).unwrap();
    let mut env: HashMap<String, String> = std::env::vars().collect();
    let path = format!(
        "{}:{}",
        bin.display(),
        env.get("PATH").cloned().unwrap_or_default()
    );
    env.insert("PATH".into(), path);
    Arc::new(move || (env.clone(), None))
}

fn fixture(name: &str) -> Fx {
    let dir = tmpdir(name);
    let db = Arc::new(Db::open(&dir.join("t.db")).unwrap());
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let p = db.create_project("p", &repo.to_string_lossy()).unwrap();
    let t = db.create_task(p.id, "t").unwrap();
    let session = db.create_session(t.id, AgentKind::Claude).unwrap();
    let env = fake_env(&dir);
    let (tx, rx) = unbounded_channel();
    let rt = Runtime::with_env(db.clone(), ChanSink(tx), env.clone()).unwrap();
    Fx {
        db,
        rt,
        rx,
        session,
        env,
        dir,
    }
}

/// Wait until a session update has this status; returns events seen.
async fn wait_status(rx: &mut UnboundedReceiver<Msg>, want: &str) -> Vec<SessionEvent> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut evs = vec![];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(Msg::Event(e))) => evs.push(e),
            Ok(Some(Msg::Session(s))) if s.status == want => return evs,
            Ok(Some(Msg::Session(_))) => {}
            _ => panic!(
                "timed out waiting for status {want}; events: {:#?}",
                kinds(&evs)
            ),
        }
    }
}

async fn wait_item(
    rx: &mut UnboundedReceiver<Msg>,
    pred: impl Fn(&Item) -> bool,
) -> (SessionEvent, Item) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(Msg::Event(e))) => {
                if let Some(i) = e.items.iter().find(|i| pred(i)).cloned() {
                    return (e, i);
                }
            }
            Ok(Some(_)) => {}
            _ => panic!("timed out waiting for item"),
        }
    }
}

fn kinds(evs: &[SessionEvent]) -> Vec<String> {
    evs.iter()
        .map(|e| format!("{}:{}", e.kind, e.event_type.clone().unwrap_or_default()))
        .collect()
}

fn texts(evs: &[SessionEvent]) -> Vec<String> {
    evs.iter()
        .flat_map(|e| &e.items)
        .filter_map(|i| match i {
            Item::AssistantText { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn multi_turn_persists_events_and_usage() {
    let mut fx = fixture("multi");
    let id = fx.session.id;
    fx.rt.send(id, "plain:hello").await.unwrap();
    let evs = wait_status(&mut fx.rx, status::IDLE).await;
    assert_eq!(texts(&evs), ["hello"]);
    assert!(matches!(&evs[0].items[0], Item::UserPrompt { text } if text == "plain:hello"));

    fx.rt.send(id, "plain:again").await.unwrap();
    wait_status(&mut fx.rx, status::IDLE).await;

    let tr = fx.rt.transcript(id).unwrap();
    assert!(tr.live);
    assert_eq!(tr.session.cli_session_id.as_deref(), Some("fake-session-1"));
    // One process served both turns.
    assert_eq!(
        tr.events
            .iter()
            .filter(|e| e.kind == "process_started")
            .count(),
        1
    );
    assert_eq!(texts(&tr.events), ["hello", "again"]);
    // Cost is cumulative within a process: 0.01 then 0.02 -> 0.02 total.
    assert!((tr.usage.cost_usd - 0.02).abs() < 1e-9, "{:?}", tr.usage);
    assert_eq!((tr.usage.turns, tr.usage.tokens.output_tokens), (2, 10));
    // seq is dense and ordered.
    let seqs: Vec<i64> = tr.events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (0..seqs.len() as i64).collect::<Vec<_>>());
    // The account in the initialize reply was never stored.
    let raw = serde_json::to_string(&fx.db.list_events(id, None).unwrap()).unwrap();
    assert!(!raw.contains("someone@example.com"));
    fx.rt.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn approvals_allow_session_and_deny() {
    let mut fx = fixture("approve");
    let id = fx.session.id;

    fx.rt.send(id, "tool").await.unwrap();
    let (_, item) = wait_item(&mut fx.rx, |i| matches!(i, Item::ApprovalRequest { .. })).await;
    let Item::ApprovalRequest { request_id, .. } = item else {
        unreachable!()
    };
    assert_eq!(
        fx.rt.transcript(id).unwrap().pending_approvals,
        std::slice::from_ref(&request_id)
    );
    fx.rt
        .respond_approval(id, &request_id, Decision::AllowSession)
        .unwrap();
    let (_, result) = wait_item(&mut fx.rx, |i| matches!(i, Item::ToolResult { .. })).await;
    let Item::ToolResult {
        content, is_error, ..
    } = result
    else {
        unreachable!()
    };
    assert!(!is_error);
    let sent: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(sent["updatedPermissions"][0]["destination"], "session");
    wait_status(&mut fx.rx, status::IDLE).await;
    // Answering twice is an error.
    assert!(
        fx.rt
            .respond_approval(id, &request_id, Decision::AllowOnce)
            .is_err()
    );

    fx.rt.send(id, "tool").await.unwrap();
    let (_, item) = wait_item(&mut fx.rx, |i| matches!(i, Item::ApprovalRequest { .. })).await;
    let Item::ApprovalRequest { request_id, .. } = item else {
        unreachable!()
    };
    fx.rt
        .respond_approval(id, &request_id, Decision::Deny)
        .unwrap();
    let (_, result) = wait_item(&mut fx.rx, |i| matches!(i, Item::ToolResult { .. })).await;
    assert!(matches!(result, Item::ToolResult { is_error: true, .. }));
    wait_status(&mut fx.rx, status::IDLE).await;

    let decisions: Vec<Decision> = fx
        .rt
        .transcript(id)
        .unwrap()
        .events
        .iter()
        .flat_map(|e| e.items.clone())
        .filter_map(|i| match i {
            Item::ApprovalResponse { decision, .. } => Some(decision),
            _ => None,
        })
        .collect();
    assert_eq!(decisions, [Decision::AllowSession, Decision::Deny]);
    fx.rt.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn interrupt_keeps_the_process_and_busy_send_is_refused() {
    let mut fx = fixture("interrupt");
    let id = fx.session.id;
    fx.rt.send(id, "hang").await.unwrap();
    wait_item(
        &mut fx.rx,
        |i| matches!(i, Item::AssistantText { text } if text == "working"),
    )
    .await;
    let busy = fx.rt.send(id, "plain:x").await.unwrap_err();
    assert!(busy.contains("already running"), "{busy}");
    fx.rt.interrupt(id).unwrap();
    let evs = wait_status(&mut fx.rx, status::IDLE).await;
    assert!(evs.iter().flat_map(|e| &e.items).any(
        |i| matches!(i, Item::TurnEnd { ok: false, subtype: Some(s), .. } if s == "error_during_execution")
    ));
    assert!(fx.rt.interrupt(id).is_err(), "no turn running now");

    fx.rt.send(id, "plain:after").await.unwrap();
    wait_status(&mut fx.rx, status::IDLE).await;
    let tr = fx.rt.transcript(id).unwrap();
    assert_eq!(
        tr.events
            .iter()
            .filter(|e| e.kind == "process_started")
            .count(),
        1
    );
    fx.rt.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_mid_turn_then_send_resumes_cli_session() {
    let mut fx = fixture("resume");
    let id = fx.session.id;
    fx.rt.send(id, "hang").await.unwrap();
    wait_item(&mut fx.rx, |i| matches!(i, Item::AssistantText { .. })).await;
    fx.rt.stop(id).unwrap();
    wait_status(&mut fx.rx, status::INTERRUPTED).await;
    assert!(!fx.rt.is_live(id));

    fx.rt.send(id, "args").await.unwrap();
    let evs = wait_status(&mut fx.rx, status::IDLE).await;
    let args = texts(&evs).pop().unwrap();
    assert!(args.contains("\"--resume\", \"fake-session-1\""), "{args}");
    fx.rt.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn crash_marks_failed() {
    let mut fx = fixture("crash");
    fx.rt.send(fx.session.id, "crash").await.unwrap();
    let evs = wait_status(&mut fx.rx, status::FAILED).await;
    assert!(evs.iter().any(|e| e.kind == "process_exit"));
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_marks_interrupted_and_rebuilds_transcript() {
    let mut fx = fixture("restart");
    let id = fx.session.id;
    fx.rt.send(id, "plain:before restart").await.unwrap();
    wait_status(&mut fx.rx, status::IDLE).await;
    fx.rt.send(id, "hang").await.unwrap();
    wait_item(
        &mut fx.rx,
        |i| matches!(i, Item::AssistantText { text } if text == "working"),
    )
    .await;
    let before = fx.rt.transcript(id).unwrap().events.len();
    assert_eq!(fx.db.get_session(id).unwrap().status, status::RUNNING);

    // Simulate an app crash: drop the runtime without stopping, reopen the
    // DB file, start a new runtime.
    drop(fx.rt);
    let db2 = Arc::new(Db::open(&fx.dir.join("t.db")).unwrap());
    let (tx, mut rx2) = unbounded_channel();
    let rt2 = Runtime::with_env(db2, ChanSink(tx), fx.env.clone()).unwrap();
    let tr = rt2.transcript(id).unwrap();
    assert_eq!(tr.session.status, status::INTERRUPTED);
    assert!(!tr.live);
    assert!(tr.events.len() >= before, "{} < {before}", tr.events.len());
    assert_eq!(texts(&tr.events)[0], "before restart");

    // And it can carry on, resuming the CLI session.
    rt2.send(id, "args").await.unwrap();
    let evs = wait_status(&mut rx2, status::IDLE).await;
    assert!(texts(&evs).pop().unwrap().contains("fake-session-1"));
    rt2.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_cli_and_folder_are_reported() {
    let fx = fixture("missing");
    // Codex session with an env that has no codex on PATH.
    let t = fx.db.get_session(fx.session.id).unwrap().task_id;
    let s = fx.db.create_session(t, AgentKind::Codex).unwrap();
    let (tx, _rx) = unbounded_channel();
    let empty: Arc<EnvFn> = Arc::new(|| {
        (
            HashMap::from([("PATH".into(), "/nonexistent".into())]),
            None,
        )
    });
    let rt = Runtime::with_env(fx.db.clone(), ChanSink(tx), empty).unwrap();
    let e = rt.send(s.id, "hi").await.unwrap_err();
    assert!(e.contains("not found on PATH"), "{e}");
    assert_eq!(fx.db.get_session(s.id).unwrap().status, status::FAILED);

    std::fs::remove_dir_all(fx.dir.join("repo")).unwrap();
    let e = fx.rt.send(fx.session.id, "plain:x").await.unwrap_err();
    assert!(e.contains("Working folder does not exist"), "{e}");
}

// ---------- real CLIs (ignored: need login, cost tokens) ----------
// cargo test -p hl-agents -- --ignored --nocapture --test-threads=1

fn real_fixture(
    name: &str,
    agent: AgentKind,
) -> (
    Arc<Db>,
    Arc<Runtime>,
    UnboundedReceiver<Msg>,
    Session,
    PathBuf,
) {
    let dir = tmpdir(name);
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let db = Arc::new(Db::open(&dir.join("t.db")).unwrap());
    let p = db.create_project("p", &repo.to_string_lossy()).unwrap();
    let t = db.create_task(p.id, "t").unwrap();
    let s = db.create_session(t.id, agent).unwrap();
    let (tx, rx) = unbounded_channel();
    let rt = Runtime::new(db.clone(), ChanSink(tx)).unwrap();
    (db, rt, rx, s, repo)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn e2e_claude_approve_interrupt_resume() {
    let (_db, rt, mut rx, s, repo) = real_fixture("e2e-claude", AgentKind::Claude);
    let file = repo.join("e2e.txt");
    rt.send(
        s.id,
        &format!(
            "Use the Write tool to create {} containing exactly: ok. Then stop.",
            file.display()
        ),
    )
    .await
    .unwrap();
    let (_, item) = wait_item(&mut rx, |i| matches!(i, Item::ApprovalRequest { .. })).await;
    let Item::ApprovalRequest { request_id, .. } = item else {
        unreachable!()
    };
    rt.respond_approval(s.id, &request_id, Decision::AllowOnce)
        .unwrap();
    wait_status(&mut rx, status::IDLE).await;
    assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), "ok");

    // Stop the process, then a follow-up must resume the same CLI session.
    rt.stop(s.id).unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    rt.send(
        s.id,
        "What file did you just create? Reply with only the file name.",
    )
    .await
    .unwrap();
    let evs = wait_status(&mut rx, status::IDLE).await;
    let reply = texts(&evs).join(" ");
    println!("resume reply: {reply}");
    assert!(reply.contains("e2e.txt"), "{reply}");
    let tr = rt.transcript(s.id).unwrap();
    assert!(tr.usage.cost_usd > 0.0);
    println!("usage: {:?}", tr.usage);
    rt.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn e2e_codex_two_turns_with_resume() {
    let (_db, rt, mut rx, s, _repo) = real_fixture("e2e-codex", AgentKind::Codex);
    rt.send(s.id, "Remember the word: pelican. Reply with exactly: ok")
        .await
        .unwrap();
    wait_status(&mut rx, status::IDLE).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    rt.send(
        s.id,
        "What word did I ask you to remember? Reply with only the word.",
    )
    .await
    .unwrap();
    let evs = wait_status(&mut rx, status::IDLE).await;
    let reply = texts(&evs).join(" ");
    println!("codex reply: {reply}");
    assert!(reply.to_lowercase().contains("pelican"), "{reply}");
    let tr = rt.transcript(s.id).unwrap();
    assert_eq!(tr.usage.turns, 2);
    rt.shutdown();
}
