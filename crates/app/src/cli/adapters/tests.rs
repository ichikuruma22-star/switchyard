use super::*;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use clap::Parser;
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, mpsc};

fn fixture_run(state: RunState) -> AdapterRun {
    AdapterRun {
        id: "runone".into(),
        adapter_id: "codex".into(),
        command_id: uuid::Uuid::new_v4().to_string(),
        conversation_id: "runone".into(),
        continued_from: None,
        title: "Fixture".into(),
        profile_id: Some("original".into()),
        profile_name: Some("Original account".into()),
        project_path: "fixture-project".into(),
        state,
        thread_id: Some("saved-thread".into()),
        turn_id: Some("saved-turn".into()),
        model: Some("fixture".into()),
        started_at_ms: 0,
        ended_at_ms: None,
        last_seq: 0,
        first_retained_seq: 1,
        pending_approvals: vec![],
        ephemeral: false,
        history_error: None,
        permission_boundary: "Fixture requires explicit review.".into(),
    }
}

#[test]
fn completed_reply_compaction_does_not_claim_that_history_was_lost() {
    let completed = AdapterEvent {
        schema_version: 1,
        run_id: "runone".into(),
        seq: 22,
        at_ms: 0,
        kind: "item_completed".into(),
        payload: json!({"item":{"id":"message","type":"agentMessage","text":"Complete reply"}}),
    };
    // The live Claude run replaced deltas 19..21 with completion 22 while its
    // retained-history boundary stayed at 1. The full reply remains available.
    assert!(!history_was_trimmed(
        18,
        std::slice::from_ref(&completed),
        1
    ));
    // Genuine retention past the reader's cursor still needs a warning.
    assert!(history_was_trimmed(
        18,
        std::slice::from_ref(&completed),
        22
    ));
    // A batch already delivered to this reader has not been lost even if the
    // server's retention boundary advances while the response is in flight.
    assert!(!history_was_trimmed(21, &[completed], 30));
    assert!(!history_was_trimmed(18, &[], 22));
}

fn fixture_approval() -> AdapterApproval {
    AdapterApproval {
        id: "approvalone".into(),
        expected_hash: "exact-preview-hash".into(),
        method: "item/commandExecution/requestApproval".into(),
        preview: json!({"command":"write fixture.txt","cwd":"fixture-project"}),
        can_allow_once: true,
        permission_boundary: "Fixture requires explicit review.".into(),
    }
}

struct Data {
    token: String,
    run: AdapterRun,
    profiles: Value,
    project: Project,
    events: Vec<AdapterEvent>,
    starts: Vec<Value>,
    decisions: Vec<Value>,
    mutations: Vec<String>,
    interrupt_state: RunState,
    invalid_start_reply: bool,
    submission_wait: Option<(Arc<Notify>, Arc<Notify>)>,
    event_reads: Vec<u64>,
    /// Remaining run reads answered with an undecodable body, as a temporary
    /// transport failure appears to this client.
    read_failures: usize,
    run_reads: usize,
}

impl Data {
    fn new(state: RunState) -> Self {
        let run = fixture_run(state);
        Self {
            token: String::new(),
            project: Project {
                id: "projectone".into(),
                name: "Fixture".into(),
                root: run.project_path.clone(),
                created_at_ms: 0,
            },
            run,
            profiles: json!({"defaults":{"codex":"newdefault"},"profiles":[
                {"id":"original","name":"Original account","agent_id":"codex","managed":false,"refreshing":false,"account":{"auth_status":"unknown","email":"private@example.test"}},
                {"id":"newdefault","name":"Another account","agent_id":"codex","managed":false,"refreshing":false,"account":{"auth_status":"unknown"}}
            ]}),
            events: vec![],
            starts: vec![],
            decisions: vec![],
            mutations: vec![],
            interrupt_state: RunState::Interrupted,
            invalid_start_reply: false,
            submission_wait: None,
            event_reads: vec![],
            read_failures: 0,
            run_reads: 0,
        }
    }
}

struct Harness {
    remote: Remote,
    data: Arc<Mutex<Data>>,
    stopped: CancellationToken,
    server: tokio::task::JoinHandle<()>,
}

impl Harness {
    async fn new(mut data: Data) -> Self {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let launch = LaunchInfo::new(listener.local_addr().unwrap().port());
        data.token = launch.token.clone();
        let data = Arc::new(Mutex::new(data));
        let router = Router::new().fallback(handle).with_state(data.clone());
        let stopped = CancellationToken::new();
        let shutdown = stopped.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .unwrap();
        });
        Self {
            remote: Remote::new(launch).unwrap(),
            data,
            stopped,
            server,
        }
    }

    async fn stop(self) {
        self.stopped.cancel();
        self.server.await.unwrap();
    }
}

async fn handle(State(data): State<Arc<Mutex<Data>>>, request: Request) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let query = request.uri().query().unwrap_or_default().to_owned();
    assert_eq!(
        request.headers()["authorization"].to_str().unwrap(),
        format!("Bearer {}", data.lock().unwrap().token)
    );
    let body = to_bytes(request.into_body(), 256 * 1024).await.unwrap();
    let input: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    if method == Method::POST && path == "/api/adapter-runs" {
        let (run, invalid, wait) = {
            let mut data = data.lock().unwrap();
            data.mutations.push(path);
            data.starts.push(input.clone());
            data.run.command_id = input["command_id"].as_str().unwrap().into();
            (
                data.run.clone(),
                data.invalid_start_reply,
                data.submission_wait.clone(),
            )
        };
        if let Some((started, release)) = wait {
            started.notify_one();
            release.notified().await;
        }
        return if invalid {
            Response::new(Body::from("not-json"))
        } else {
            Json(json!({"run":run})).into_response()
        };
    }
    let mut data = data.lock().unwrap();
    if method == Method::POST {
        data.mutations.push(path.clone());
    }
    match (method, path.as_str()) {
        (Method::GET, "/api/account-profiles") => Json(data.profiles.clone()).into_response(),
        (Method::GET, "/api/projects") => Json(json!({"projects":[data.project]})).into_response(),
        (Method::POST, "/api/projects") => {
            data.project.root = input["path"].as_str().unwrap().into();
            Json(json!({"project":data.project})).into_response()
        }
        (Method::GET, "/api/adapter-runs") => Json(json!({"runs":[data.run]})).into_response(),
        (Method::GET, "/api/adapter-runs/runone") => {
            data.run_reads += 1;
            if data.read_failures > 0 {
                data.read_failures -= 1;
                return Response::new(Body::from("not-json"));
            }
            Json(json!({"run":data.run})).into_response()
        }
        (Method::GET, "/api/adapter-runs/runone/events") => {
            let after = url::form_urlencoded::parse(query.as_bytes())
                .find(|(k, _)| k == "after_seq")
                .unwrap()
                .1
                .parse::<u64>()
                .unwrap();
            data.event_reads.push(after);
            Json(json!({"events":data.events.iter().filter(|event|event.seq > after).take(200).collect::<Vec<_>>()})).into_response()
        }
        (Method::POST, "/api/adapter-runs/runone/decisions") => {
            data.decisions.push(input);
            data.run.pending_approvals.clear();
            data.run.state = RunState::Completed;
            Json(json!({"ok":true})).into_response()
        }
        (Method::POST, "/api/adapter-runs/runone/interrupt") => {
            data.run.state = data.interrupt_state;
            data.run.pending_approvals.clear();
            Json(json!({"ok":true})).into_response()
        }
        (Method::POST, "/api/adapter-runs/runone/recovery") => {
            data.run.state = RunState::Interrupted;
            Json(json!({"run":data.run})).into_response()
        }
        _ => (
            StatusCode::NOT_FOUND,
            Json(json!({"error":{"message":format!("Unexpected fixture route: {path}")}})),
        )
            .into_response(),
    }
}

#[test]
fn parser_keeps_continuation_bound_and_recovery_explicit() {
    assert!(
        Cli::try_parse_from([
            "switchya", "agent", "continue", "--run", "runone", "--prompt", "next"
        ])
        .is_ok()
    );
    for flag in ["--project", "--profile"] {
        assert!(
            Cli::try_parse_from([
                "switchya", "agent", "continue", "--run", "runone", "--prompt", "next", flag,
                "other"
            ])
            .is_err()
        );
    }
    assert!(Cli::try_parse_from(["switchya", "agent", "recover", "--run", "runone"]).is_err());
    assert!(
        Cli::try_parse_from([
            "switchya",
            "agent",
            "recover",
            "--run",
            "runone",
            "--reviewed"
        ])
        .is_ok()
    );
    assert!(
        Cli::try_parse_from([
            "switchya",
            "agent",
            "run",
            "--prompt",
            "hi",
            "--command-id",
            "not-a-uuid"
        ])
        .is_err()
    );
}

#[test]
fn structured_output_neutralizes_terminal_controls_without_breaking_json() {
    let value = json!({"text":"name\u{202e}\u{2066}\u{007f}\u{0085}\u{001b}[31m\nnext"});
    let formatted = format_json(&value).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&formatted).unwrap(), value);
    for character in ['\u{202e}', '\u{2066}', '\u{007f}', '\u{0085}', '\u{001b}'] {
        assert!(!formatted.contains(character));
    }
}

#[test]
fn compacted_stream_fragments_do_not_drop_the_end_or_repeat_the_final_message() {
    let event = |kind: &str, payload| AdapterEvent {
        schema_version: 1,
        run_id: "runone".into(),
        seq: 1,
        at_ms: 0,
        kind: kind.into(),
        payload,
    };
    let mut streamed = HashMap::new();
    let first = event(
        "assistant_delta",
        json!({"item_id":"reply","text":"Hello 🌏"}),
    );
    let final_reply = event(
        "item_completed",
        json!({"item":{"type":"agentMessage","id":"reply","text":"Hello 🌏. The tests passed."}}),
    );
    let mut output = assistant_output(&first, &mut streamed).unwrap();
    // The intermediate deltas were compacted before this client read them.
    output.push_str(&assistant_output(&final_reply, &mut streamed).unwrap());
    output.push_str(&assistant_output(&final_reply, &mut streamed).unwrap());
    assert_eq!(output, "Hello 🌏. The tests passed.");
    let other_reply = event(
        "item_completed",
        json!({"item":{"type":"agentMessage","id":"another","text":"A different message."}}),
    );
    assert_eq!(
        assistant_output(&other_reply, &mut streamed).unwrap(),
        "A different message."
    );
}

#[test]
fn a_corrected_final_message_is_displayed_in_full() {
    let mut streamed = HashMap::from([("reply".to_owned(), "Incomplete draft".to_owned())]);
    let event = AdapterEvent {
        schema_version: 1,
        run_id: "runone".into(),
        seq: 3,
        at_ms: 0,
        kind: "item_completed".into(),
        payload: json!({"item":{"type":"agentMessage","id":"reply","text":"Corrected answer"}}),
    };
    assert_eq!(
        assistant_output(&event, &mut streamed).unwrap(),
        "\nFinal reply:\nCorrected answer"
    );
    assert_eq!(assistant_output(&event, &mut streamed).unwrap(), "");
}

#[test]
fn a_completed_command_reads_as_status_and_bounded_unescaped_output() {
    let summary = item_summary(
        &json!({"id":"item_7","type":"commandExecution","command":"cargo test -p switchyard-app",
            "cwd":"E:\\projects\\switchyard-app","status":"completed","exitCode":0,
            "aggregatedOutput":"running 2 tests\ntest result: ok. 2 passed\n"}),
        "runone",
    )
    .unwrap();
    assert_eq!(
        summary,
        "$ cargo test -p switchyard-app\n  in E:\\projects\\switchyard-app\n  completed, exit code 0\nrunning 2 tests\ntest result: ok. 2 passed"
    );
    // A single file read must not become tens of thousands of escaped characters.
    let long = (0..5000).fold(String::new(), |mut text, line| {
        use std::fmt::Write;
        let _ = writeln!(text, "line {line} of a long file");
        text
    });
    let bounded = item_summary(
        &json!({"type":"commandExecution","command":"type big.txt","status":"failed","exitCode":1,
            "aggregatedOutput":long}),
        "runone",
    )
    .unwrap();
    assert!(bounded.len() < 4400, "summary length {}", bounded.len());
    assert!(bounded.contains("failed, exit code 1"));
    assert!(bounded.contains("line 4999 of a long file"));
    assert!(bounded.contains("switchya agent show --run runone"));
    assert!(!bounded.contains("\\n"));
}

#[test]
fn a_very_long_completed_command_keeps_its_start_status_and_output() {
    let long = format!(
        "pwsh -Command Set-Content -Path big.rs -Value @'\n{}\n'@",
        "x".repeat(60_000)
    );
    let summary = item_summary(
        &json!({"id":"item_11","type":"commandExecution","command":long,"cwd":"E:\\projects",
            "status":"completed","exitCode":0,"aggregatedOutput":"wrote big.rs\n"}),
        "runone",
    )
    .unwrap();
    assert!(summary.len() < 1200, "summary length {}", summary.len());
    assert!(summary.starts_with("$ pwsh -Command Set-Content -Path big.rs -Value @'"));
    assert!(summary.contains("… command shortened; `switchya agent show --run runone`"));
    assert!(summary.contains("  completed, exit code 0"));
    assert!(summary.ends_with("wrote big.rs"));
    let tool = item_summary(
        &json!({"id":"item_12","type":"claudeTool","name":"PowerShell","status":"completed",
            "input":{"command":"x".repeat(50_000)},"output":"done"}),
        "runone",
    )
    .unwrap();
    assert!(tool.len() < 700, "summary length {}", tool.len());
    assert!(tool.starts_with("PowerShell xxx"));
    assert!(tool.contains("… tool input shortened; `switchya agent show --run runone`"));
    assert!(tool.ends_with("done"));
}

#[test]
fn completed_items_report_file_paths_tool_status_and_unknown_activity() {
    let files = item_summary(
        &json!({"id":"item_8","type":"fileChange","status":"completed","changes":[
            {"path":"app-ui/adapters.js","kind":{"type":"update"},"diff":"@@ -1 +1 @@\n-old\n+new\n"},
            {"path":"docs/old.md","kind":{"type":"update","move_path":"docs/new.md"}},
            {"path":"notes.txt","kind":{"type":"delete"}}]}),
        "runone",
    )
    .unwrap();
    assert_eq!(
        files,
        "Edited 3 files (completed)\n  edit app-ui/adapters.js\n  edit docs/old.md → docs/new.md\n  delete notes.txt"
    );
    let tool = item_summary(
        &json!({"id":"toolone","type":"claudeTool","name":"Edit","status":"failed",
            "input":{"file_path":"crates/app/src/cli/adapters.rs","old_string":"a"},"output":"String to replace not found"}),
        "runone",
    )
    .unwrap();
    assert_eq!(
        tool,
        "Edit crates/app/src/cli/adapters.rs (failed)\nString to replace not found"
    );
    // Unknown items stay visible rather than silently disappearing.
    let unknown = item_summary(
        &json!({"id":"item_9","type":"webSearch","status":"completed","query":"switchya"}),
        "runone",
    )
    .unwrap();
    assert!(unknown.starts_with("Web search (completed), item item_9"));
    assert!(unknown.contains("switchya"));
}

#[test]
fn lifecycle_only_items_are_not_rendered_as_activity() {
    for item in [
        json!({"id":"item_1","type":"reasoning"}),
        json!({"id":"item_2","type":"userMessage","status":"completed"}),
        json!({"id":"item_3","type":"agentMessage","text":""}),
        json!({"id":"item_4","type":"fileChange","status":null,"changes":[]}),
    ] {
        assert!(lifecycle_only(&item), "{item} reports no activity");
        assert_eq!(item_summary(&item, "runone"), None);
    }
    assert!(!lifecycle_only(
        &json!({"id":"item_5","type":"reasoning","text":"Checking the event renderer"})
    ));
    assert_eq!(
        item_summary(
            &json!({"id":"item_5","type":"reasoning","text":"Checking the renderer"}),
            "runone"
        )
        .unwrap(),
        "Reasoning (status not reported), item item_5\n{\"text\":\"Checking the renderer\"}"
    );
}

#[tokio::test]
async fn continuation_uses_the_saved_project_and_account_despite_a_new_default() {
    let fixture = Harness::new(Data::new(RunState::Completed)).await;
    let command_id = uuid::Uuid::new_v4();
    let body = prepare(
        &fixture.remote,
        Selection::Continue("runone".into()),
        &MessageArgs {
            prompt: "Only this new message".into(),
            command_id: Some(command_id),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        body,
        json!({"adapter_id":"codex","project_id":"projectone","profile_id":"original","prompt":"Only this new message","command_id":command_id.to_string(),"continue_run_id":"runone"})
    );
    assert!(fixture.data.lock().unwrap().mutations.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn claude_starts_select_its_default_and_continuations_keep_its_original_account() {
    let project = tempfile::tempdir().unwrap();
    let mut data = Data::new(RunState::Completed);
    data.run.adapter_id = "claude".into();
    data.profiles["profiles"][0]["agent_id"] = json!("claude");
    data.profiles["profiles"][0]["managed"] = json!(true);
    data.profiles["profiles"][0]["account"]["auth_status"] = json!("signed_in");
    data.profiles["defaults"]["claude"] = json!("claude-default");
    data.profiles["profiles"].as_array_mut().unwrap().push(json!({"id":"claude-default","name":"Claude default","agent_id":"claude","managed":true,"refreshing":false,"account":{"auth_status":"signed_in"}}));
    let fixture = Harness::new(data).await;
    let message = MessageArgs {
        prompt: "A new message".into(),
        command_id: None,
    };
    let continued = prepare(
        &fixture.remote,
        Selection::Continue("runone".into()),
        &message,
    )
    .await
    .unwrap();
    assert_eq!(continued["adapter_id"], "claude");
    assert_eq!(continued["profile_id"], "original");
    let started = prepare(
        &fixture.remote,
        Selection::New {
            agent: "claude".into(),
            project: Some(project.path().into()),
            profile: None,
        },
        &message,
    )
    .await
    .unwrap();
    assert_eq!(started["adapter_id"], "claude");
    assert_eq!(started["profile_id"], "claude-default");
    let wrong = prepare(
        &fixture.remote,
        Selection::New {
            agent: "claude".into(),
            project: Some(project.path().into()),
            profile: Some("newdefault".into()),
        },
        &message,
    )
    .await;
    assert!(wrong.is_err());
    assert!(
        !fixture
            .data
            .lock()
            .unwrap()
            .mutations
            .iter()
            .any(|path| path.ends_with("/refresh"))
    );
    fixture.stop().await;
}

#[tokio::test]
async fn accepted_retry_keeps_its_original_managed_profile_without_an_auth_refresh() {
    let project = tempfile::tempdir().unwrap();
    let mut data = Data::new(RunState::Completed);
    data.profiles["profiles"][0]["managed"] = json!(true);
    let command_id = uuid::Uuid::parse_str(&data.run.command_id).unwrap();
    let fixture = Harness::new(data).await;
    let body = prepare(
        &fixture.remote,
        Selection::New {
            agent: "codex".into(),
            project: Some(project.path().to_owned()),
            profile: None,
        },
        &MessageArgs {
            prompt: "original prompt".into(),
            command_id: Some(command_id),
        },
    )
    .await
    .unwrap();
    assert_eq!(body["profile_id"], "original");
    assert_eq!(fixture.data.lock().unwrap().mutations, ["/api/projects"]);
    fixture.stop().await;
}

#[tokio::test]
async fn noninteractive_review_never_approves_and_reads_back_interruption() {
    let mut data = Data::new(RunState::AwaitingApproval);
    data.run.pending_approvals.push(fixture_approval());
    let fixture = Harness::new(data).await;
    let error = watch(
        &fixture.remote,
        "runone",
        &mut None,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "approval_required");
    {
        let data = fixture.data.lock().unwrap();
        assert!(data.decisions.is_empty());
        assert_eq!(data.mutations, ["/api/adapter-runs/runone/interrupt"]);
        assert_eq!(data.run.state, RunState::Interrupted);
    }
    fixture.stop().await;
}

#[tokio::test]
async fn interactive_decision_names_the_exact_approval_and_sends_its_hash_once() {
    let mut data = Data::new(RunState::AwaitingApproval);
    data.run.pending_approvals.push(fixture_approval());
    let fixture = Harness::new(data).await;
    let (sender, receiver) = mpsc::channel(3);
    sender.send(Ok("allow_once obsolete".into())).await.unwrap();
    sender
        .send(Ok("allow_once approvalone".into()))
        .await
        .unwrap();
    let mut input = Some(super::super::TerminalInput::new(receiver));
    watch(
        &fixture.remote,
        "runone",
        &mut input,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        fixture.data.lock().unwrap().decisions,
        [
            json!({"approval_id":"approvalone","expected_hash":"exact-preview-hash","decision":"allow_once"})
        ]
    );
    fixture.stop().await;
}

#[test]
fn watch_attaches_by_run_id_and_accepts_no_prompt_or_account_arguments() {
    assert!(Cli::try_parse_from(["switchya", "agent", "watch", "--run", "runone"]).is_ok());
    for flag in ["--prompt", "--project", "--profile"] {
        assert!(
            Cli::try_parse_from([
                "switchya", "agent", "watch", "--run", "runone", flag, "value"
            ])
            .is_err()
        );
    }
}

#[tokio::test]
async fn attaching_to_an_active_run_submits_no_work_and_binds_the_exact_approval() {
    let mut data = Data::new(RunState::AwaitingApproval);
    data.run.pending_approvals.push(fixture_approval());
    let fixture = Harness::new(data).await;
    let (sender, receiver) = mpsc::channel(1);
    sender
        .send(Ok("allow_once approvalone".into()))
        .await
        .unwrap();
    let mut input = Some(super::super::TerminalInput::new(receiver));
    view(
        &fixture.remote,
        "runone",
        &mut input,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    {
        let data = fixture.data.lock().unwrap();
        // Attaching reads the run and answers its exact approval. It never starts
        // or continues a turn, and it never interrupts the agent it attached to.
        assert_eq!(
            data.decisions,
            [
                json!({"approval_id":"approvalone","expected_hash":"exact-preview-hash","decision":"allow_once"})
            ]
        );
        assert!(data.starts.is_empty());
        assert_eq!(data.mutations, ["/api/adapter-runs/runone/decisions"]);
    }
    fixture.stop().await;
}

#[tokio::test]
async fn a_noninteractive_or_detached_viewer_leaves_the_run_running() {
    let mut data = Data::new(RunState::AwaitingApproval);
    data.run.pending_approvals.push(fixture_approval());
    let fixture = Harness::new(data).await;
    let pending = view(
        &fixture.remote,
        "runone",
        &mut None,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(pending.code, "approval_required");
    assert!(pending.message.contains("still pending"));
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let detached = view(&fixture.remote, "runone", &mut None, &cancelled)
        .await
        .unwrap_err();
    assert_eq!(detached.code, "detached");
    {
        let data = fixture.data.lock().unwrap();
        // Neither path approved, denied, interrupted or started anything.
        assert!(data.mutations.is_empty());
        assert!(data.decisions.is_empty() && data.starts.is_empty());
        assert_eq!(data.run.state, RunState::AwaitingApproval);
        assert_eq!(data.run.pending_approvals.len(), 1);
    }
    fixture.stop().await;
}

#[tokio::test]
async fn a_viewer_can_detach_while_its_initial_run_read_is_stalled() {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let remote = Remote::new(LaunchInfo::new(listener.local_addr().unwrap().port())).unwrap();
    let cancelled = CancellationToken::new();
    let cancel_on_read = cancelled.clone();
    let server_done = CancellationToken::new();
    let finish_server = server_done.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = BufReader::new(socket);
        let mut request_line = String::new();
        socket.read_line(&mut request_line).await.unwrap();
        // Model a reachable host that accepted the read but never answered it.
        // Keep the socket open until the test has observed detachment.
        cancel_on_read.cancel();
        finish_server.cancelled().await;
        request_line
    });
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        view(&remote, "runone", &mut None, &cancelled),
    )
    .await;
    server_done.cancel();
    let request_line = server.await.unwrap();
    assert_eq!(request_line, "GET /api/adapter-runs/runone HTTP/1.1\r\n");
    assert_eq!(
        result
            .expect("detaching must not wait for the HTTP timeout or its retries")
            .unwrap_err()
            .code,
        "detached"
    );
}

#[tokio::test]
async fn read_only_run_reads_recover_from_temporary_failures_a_bounded_number_of_times() {
    let mut data = Data::new(RunState::Running);
    data.read_failures = READ_ATTEMPTS - 1;
    let fixture = Harness::new(data).await;
    assert_eq!(
        read_run(&fixture.remote, "runone").await.unwrap().id,
        "runone"
    );
    assert_eq!(fixture.data.lock().unwrap().run_reads, READ_ATTEMPTS);
    fixture.data.lock().unwrap().read_failures = READ_ATTEMPTS + 5;
    fixture.data.lock().unwrap().run_reads = 0;
    let error = read_run(&fixture.remote, "runone").await.unwrap_err();
    assert_eq!(error.code, "invalid_response");
    {
        let data = fixture.data.lock().unwrap();
        // Retries are bounded, and no read ever became a write.
        assert_eq!(data.run_reads, READ_ATTEMPTS);
        assert!(data.mutations.is_empty() && data.starts.is_empty());
    }
    fixture.stop().await;
}

#[test]
fn deny_only_requests_cannot_be_approved_and_stale_hashes_do_not_match() {
    let mut approval = fixture_approval();
    approval.can_allow_once = false;
    assert_eq!(approval_answer("allow_once approvalone", &approval), None);
    assert_eq!(
        approval_answer("deny approvalone", &approval),
        Some(ApprovalDecision::Deny)
    );
    let mut view = fixture_run(RunState::AwaitingApproval);
    view.pending_approvals.push(approval.clone());
    assert!(same_approval(&view, &approval));
    view.pending_approvals[0].expected_hash = "changed".into();
    assert!(!same_approval(&view, &approval));
}

#[tokio::test]
async fn an_already_resolved_approval_does_not_consume_buffered_acceptance() {
    let fixture = Harness::new(Data::new(RunState::Completed)).await;
    let (sender, receiver) = mpsc::channel(1);
    sender
        .send(Ok("allow_once approvalone".into()))
        .await
        .unwrap();
    let mut input = Some(super::super::TerminalInput::new(receiver));
    assert!(
        await_approval(&fixture.remote, "runone", &fixture_approval(), &mut input)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        input
            .as_mut()
            .unwrap()
            .receiver
            .try_recv()
            .unwrap()
            .unwrap(),
        "allow_once approvalone"
    );
    assert!(fixture.data.lock().unwrap().mutations.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn waiting_for_input_observes_another_clients_completed_decision() {
    let mut data = Data::new(RunState::AwaitingApproval);
    data.run.pending_approvals.push(fixture_approval());
    let fixture = Harness::new(data).await;
    let (_sender, receiver) = mpsc::channel(1);
    let mut input = Some(super::super::TerminalInput::new(receiver));
    let approval = fixture_approval();
    let future = await_approval(&fixture.remote, "runone", &approval, &mut input);
    let change = async {
        tokio::time::sleep(Duration::from_millis(30)).await;
        let mut data = fixture.data.lock().unwrap();
        data.run.state = RunState::Completed;
        data.run.pending_approvals.clear();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(future, change)
    })
    .await
    .unwrap();
    assert!(result.unwrap().is_none());
    assert!(fixture.data.lock().unwrap().mutations.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn an_uncertain_start_is_not_resubmitted_and_retains_its_command_id() {
    let project = tempfile::tempdir().unwrap();
    let mut data = Data::new(RunState::Running);
    data.invalid_start_reply = true;
    let fixture = Harness::new(data).await;
    let command_id = uuid::Uuid::new_v4();
    let error = start(
        &fixture.remote,
        Selection::New {
            agent: "codex".into(),
            project: Some(project.path().to_owned()),
            profile: None,
        },
        MessageArgs {
            prompt: "new prompt".into(),
            command_id: Some(command_id),
        },
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "submission_unconfirmed");
    assert!(error.message.contains(&command_id.to_string()));
    assert_eq!(fixture.data.lock().unwrap().starts.len(), 1);
    assert_eq!(
        fixture.data.lock().unwrap().starts[0]["command_id"],
        command_id.to_string()
    );
    fixture.stop().await;
}

#[tokio::test]
async fn cancellation_during_submission_stops_the_accepted_run_without_replaying() {
    let project = tempfile::tempdir().unwrap();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut data = Data::new(RunState::Running);
    data.submission_wait = Some((started.clone(), release.clone()));
    let fixture = Harness::new(data).await;
    let cancelled = CancellationToken::new();
    let operation = start(
        &fixture.remote,
        Selection::New {
            agent: "codex".into(),
            project: Some(project.path().to_owned()),
            profile: None,
        },
        MessageArgs {
            prompt: "new prompt".into(),
            command_id: None,
        },
        &cancelled,
    );
    let cancel = async {
        started.notified().await;
        cancelled.cancel();
        release.notify_one();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(operation, cancel)
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap_err().code, "interrupted");
    {
        let data = fixture.data.lock().unwrap();
        assert_eq!(data.starts.len(), 1);
        assert_eq!(data.run.state, RunState::Interrupted);
        assert_eq!(
            data.mutations
                .iter()
                .filter(|path| path.ends_with("/interrupt"))
                .count(),
            1
        );
    }
    fixture.stop().await;
}

#[tokio::test]
async fn an_interrupt_acknowledgement_is_not_proof_of_a_confirmed_outcome() {
    let mut data = Data::new(RunState::Running);
    data.interrupt_state = RunState::RecoveryRequired;
    let fixture = Harness::new(data).await;
    assert_eq!(
        interrupt_and_confirm(&fixture.remote, "runone")
            .await
            .unwrap_err()
            .code,
        "recovery_required"
    );
    assert!(
        !fixture
            .data
            .lock()
            .unwrap()
            .mutations
            .iter()
            .any(|path| path.ends_with("/recovery"))
    );
    fixture.stop().await;
}

#[tokio::test]
async fn showing_saved_history_is_read_only_and_drains_more_than_one_page() {
    let mut data = Data::new(RunState::Completed);
    data.run.last_seq = 202;
    data.run.first_retained_seq = 2;
    data.events = (2..=202)
        .map(|seq| AdapterEvent {
            schema_version: 1,
            run_id: "runone".into(),
            seq,
            at_ms: 0,
            kind: "notice".into(),
            payload: json!({"message":"fixture"}),
        })
        .collect();
    let fixture = Harness::new(data).await;
    dispatch(
        &fixture.remote,
        AgentCommand::Show {
            run: "runone".into(),
        },
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(fixture.data.lock().unwrap().event_reads, [0, 201]);
    assert!(fixture.data.lock().unwrap().mutations.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn owned_host_commands_release_the_data_directory_without_starting_an_agent() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("gateway.toml");
    let data = root.path().join("state");
    let cli = Cli {
        config: Some(config),
        data_dir: Some(data.clone()),
        client_key_name: None,
        launch_info: None,
        command: None,
    };
    execute(&cli, AgentCommand::Accounts, &CancellationToken::new())
        .await
        .unwrap();
    assert!(data.join("adapter-runs.sqlite3").is_file());
    execute(&cli, AgentCommand::Sessions, &CancellationToken::new())
        .await
        .unwrap();
    let manager = switchyard_agent_adapters::AdapterManager::open(&data).unwrap();
    assert!(manager.list_runs().is_empty());
}
