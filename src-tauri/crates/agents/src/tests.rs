use super::*;
use std::time::Instant;
use tokio::sync::mpsc::UnboundedReceiver;

fn manager() -> (Arc<AgentManager>, UnboundedReceiver<AgentEvent>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let m = AgentManager::new(move |ev: AgentEvent| {
        let _ = tx.send(ev);
    });
    (m, rx)
}

/// Collect events for one run until its exit event. `on_event` may react
/// (e.g. answer an approval).
async fn collect(
    rx: &mut UnboundedReceiver<AgentEvent>,
    run_id: &str,
    timeout: Duration,
    mut on_event: impl FnMut(&AgentEvent),
) -> Vec<AgentEvent> {
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let ev = tokio::time::timeout(left, rx.recv())
            .await
            .expect("timed out waiting for exit")
            .expect("sink closed");
        if ev.run_id != run_id {
            continue;
        }
        on_event(&ev);
        let done = ev.kind == "exit";
        out.push(ev);
        if done {
            return out;
        }
    }
}

fn kinds(evs: &[AgentEvent]) -> Vec<String> {
    evs.iter()
        .map(|e| match &e.event_type {
            Some(t) => format!("{}:{t}", e.kind),
            None => e.kind.to_string(),
        })
        .collect()
}

fn sh(m: &Arc<AgentManager>, script: &str, stdin_lines: Vec<StdinMsg>) -> String {
    m.spawn(Spawn {
        name: "sh",
        bin: Path::new("/bin/sh"),
        args: vec!["-c".into(), script.into()],
        env: &std::env::vars().collect(),
        cwd: "/tmp",
        stdin_lines,
        close_on_result: false,
    })
    .unwrap()
}

#[test]
fn event_types() {
    let c = json!({"type":"system","subtype":"init"});
    assert_eq!(event_type(&c).unwrap(), "system/init");
    let r = json!({"type":"control_request","request":{"subtype":"can_use_tool"}});
    assert_eq!(event_type(&r).unwrap(), "control_request/can_use_tool");
    let x = json!({"type":"item.completed","item":{"type":"agent_message"}});
    assert_eq!(event_type(&x).unwrap(), "item.completed/agent_message");
    assert_eq!(
        event_type(&json!({"type":"turn.started"})).unwrap(),
        "turn.started"
    );
    assert!(event_type(&json!([1])).is_none());
}

#[test]
fn approval_shapes() {
    let a = approval_response("r1", true, json!({"x":1}));
    assert_eq!(a.pointer("/response/response/behavior").unwrap(), "allow");
    assert_eq!(a.pointer("/response/request_id").unwrap(), "r1");
    let d = approval_response("r1", false, Value::Null);
    assert_eq!(d.pointer("/response/response/behavior").unwrap(), "deny");
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_json_stderr_and_nonzero_exit_become_events() {
    let (m, mut rx) = manager();
    let script = r#"read x; echo 'not json'; printf '{"type":"hi","got":"%s"}\n' "$x"; echo oops >&2; exit 3"#;
    let id = sh(
        &m,
        script,
        vec![StdinMsg::Line("ping".into()), StdinMsg::Close],
    );
    let evs = collect(&mut rx, &id, Duration::from_secs(10), |_| {}).await;
    let k = kinds(&evs);
    assert_eq!(k.first().unwrap(), "started");
    assert!(k.contains(&"invalid_json".to_string()), "{k:?}");
    assert!(k.contains(&"stderr".to_string()), "{k:?}");
    let hi = evs
        .iter()
        .find(|e| e.event_type.as_deref() == Some("hi"))
        .unwrap();
    assert_eq!(hi.payload["got"], "ping");
    let exit = evs.last().unwrap();
    assert_eq!(exit.payload["code"], 3);
    assert_eq!(exit.payload["success"], false);
    let seqs: Vec<u64> = evs.iter().map(|e| e.seq).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
    assert_eq!(m.active_runs(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_folder_and_empty_prompt_are_errors() {
    let (m, _rx) = manager();
    let r = m
        .start(AgentKind::Codex, "/definitely/not/here", "hi")
        .await;
    assert!(r.unwrap_err().contains("Folder does not exist"));
    let r = m.start(AgentKind::Codex, "/tmp", " ").await;
    assert!(r.unwrap_err().contains("empty"));
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_kills_the_process_group() {
    let (m, mut rx) = manager();
    // `sleep` is a grandchild holding stdout open.
    let id = sh(&m, r#"echo '{"type":"up"}'; sleep 30"#, vec![]);
    let m2 = m.clone();
    let id2 = id.clone();
    let evs = collect(&mut rx, &id, Duration::from_secs(10), move |e| {
        if e.event_type.as_deref() == Some("up") {
            m2.stop(&id2).unwrap();
        }
    })
    .await;
    assert_eq!(evs.last().unwrap().payload["stopped"], true);
    assert_eq!(m.active_runs(), 0);
    assert!(m.stop(&id).is_err());
}

fn scratch_dir(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("hl-local-e2e-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.to_string_lossy().into_owned()
}

/// Real `claude` CLI: needs it installed and logged in. Costs tokens.
/// cargo test -p hl-agents -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn e2e_claude_with_approval() {
    let (m, mut rx) = manager();
    let dir = scratch_dir("claude");
    let id = m
        .start(
            AgentKind::Claude,
            &dir,
            "Use the Write tool to create e2e.txt containing exactly: ok. Then stop.",
        )
        .await
        .unwrap();
    let m2 = m.clone();
    let id2 = id.clone();
    let mut asked = 0;
    let evs = collect(&mut rx, &id, Duration::from_secs(180), |e| {
        if e.event_type.as_deref() == Some("control_request/can_use_tool") {
            asked += 1;
            let p = &e.payload;
            m2.respond_approval(
                &id2,
                p["request_id"].as_str().unwrap(),
                true,
                p["request"]["input"].clone(),
            )
            .unwrap();
        }
    })
    .await;
    println!("{:#?}", kinds(&evs));
    assert!(asked >= 1, "no approval request seen");
    assert!(kinds(&evs).iter().any(|k| k == "event:result/success"));
    assert_eq!(evs.last().unwrap().payload["success"], true);
    let body = std::fs::read_to_string(format!("{dir}/e2e.txt")).unwrap();
    assert_eq!(body.trim(), "ok");
}

/// Real `codex` CLI: needs it installed and logged in. Costs tokens.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn e2e_codex() {
    let (m, mut rx) = manager();
    let id = m
        .start(
            AgentKind::Codex,
            &scratch_dir("codex"),
            "Reply with exactly: pong",
        )
        .await
        .unwrap();
    let evs = collect(&mut rx, &id, Duration::from_secs(180), |_| {}).await;
    println!("{:#?}", kinds(&evs));
    let msg = evs
        .iter()
        .find(|e| e.event_type.as_deref() == Some("item.completed/agent_message"))
        .expect("no agent_message");
    assert!(
        msg.payload["item"]["text"]
            .as_str()
            .unwrap()
            .contains("pong")
    );
    assert_eq!(evs.last().unwrap().payload["success"], true);
}
