//! Claude Code's bidirectional stream-json protocol. Credentials and CLI
//! diagnostics stay in the owned process; only conversation data is retained.
use crate::{
    AdapterApproval, AdapterError, ApprovalDecision, Control, Result, RunState, StoredRun,
    hash_value,
    process_tree::{self, ProcessTree},
    protocol::{
        HANDSHAKE_TIMEOUT, INTERRUPT_TIMEOUT, Packet, RunInput, TURN_TIMEOUT, discard_stderr, fail,
        read_stdout, valid_id, write,
    },
    trust::{self, Trust},
    types::{ApprovalKind, CLAUDE_PERMISSION_BOUNDARY},
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    process::{ChildStdin, Command},
    sync::mpsc,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

const INITIALIZE: &str = "switchya-initialize";
const INTERRUPT: &str = "switchya-interrupt";

struct Pending {
    wire_id: String,
    input: Value,
    approval: AdapterApproval,
}

pub(crate) async fn run(
    executable: PathBuf,
    prefix_args: Vec<String>,
    input: RunInput,
    state: Arc<Mutex<StoredRun>>,
    mut control: mpsc::Receiver<Control>,
    shutdown: CancellationToken,
) {
    let RunInput {
        project,
        profile,
        prompt,
        resume_thread,
        ephemeral,
        trust,
    } = input;
    let session = resume_thread
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    if uuid::Uuid::parse_str(&session).is_err() {
        fail(
            &state,
            "The saved Claude conversation identifier is invalid. No message was sent.",
            false,
        );
        return;
    }
    let mut command = Command::new(executable);
    command.args(prefix_args).args([
        "--print",
        "--verbose",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--permission-prompt-tool",
        "stdio",
        "--permission-mode",
        "default",
        "--include-partial-messages",
    ]);
    command.arg(format!(
        "--{}={session}",
        if resume_thread.is_some() {
            "resume"
        } else {
            "session-id"
        }
    ));
    if ephemeral {
        command.arg("--no-session-persistence");
    }
    crate::profile::configure(&mut command, profile.as_ref());
    // Results can precede background tasks. This enables the authoritative idle
    // notification used by the official SDK before ending its input stream.
    command
        .env("CLAUDE_CODE_SDK_READS_SESSION_STATE", "1")
        .current_dir(&project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    process_tree::configure(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            fail(&state, "The Claude Code process could not start.", false);
            return;
        }
    };
    let mut tree = match ProcessTree::attach(&child) {
        Ok(tree) => tree,
        Err(_) => {
            let _ = child.kill().await;
            fail(
                &state,
                "Could not establish local process cleanup; Claude Code was stopped.",
                false,
            );
            return;
        }
    };
    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        let _ = tree.terminate();
        let _ = child.kill().await;
        fail(
            &state,
            "Claude Code's structured transport is unavailable.",
            false,
        );
        return;
    };
    let (packets, mut incoming) = mpsc::channel(16);
    let stdout_task = tokio::spawn(read_stdout(stdout, packets.clone()));
    let stderr_task = tokio::spawn(discard_stderr(stderr, packets));
    let mut deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut requested = false;
    let mut initialized = false;
    let mut interrupting = false;
    let mut completion = None;
    let mut confirmed_idle = None;
    let mut pending: BTreeMap<String, Pending> = BTreeMap::new();
    let mut seen_requests = HashSet::new();
    let mut transcript = Transcript::default();
    if write(&mut stdin, &json!({"type":"control_request","request_id":INITIALIZE,"request":{"subtype":"initialize","hooks":null}})).await.is_err() {
        fail(&state, "Could not initialize Claude Code's transport.", false);
    } else {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    fail(&state, "The local host shut down. This run will not be replayed.", requested);
                    break;
                }
                _ = tokio::time::sleep_until(deadline) => {
                    fail(&state, if interrupting { "Claude Code did not confirm interruption within five seconds. Review any tool activity before continuing." } else { "Claude Code exceeded the adapter time limit. Its process tree was stopped." }, requested);
                    break;
                }
                next = control.recv() => match next {
                    None => { fail(&state, "Adapter control closed.", requested); break; }
                    Some(Control::Decide { approval_id, expected_hash, decision, reply }) => {
                        let result = decide(&mut stdin, &state, &mut pending, &approval_id, &expected_hash, decision, interrupting).await;
                        let failed = matches!(result, Err(AdapterError::Protocol(_)));
                        let _ = reply.send(result);
                        if failed { fail(&state, "Approval delivery was uncertain. Review the project before continuing; nothing will be replayed.", true); break; }
                    }
                    Some(Control::Interrupt { reply }) => {
                        if interrupting { let _ = reply.send(Ok(())); continue; }
                        if !requested {
                            state.lock().state(RunState::Interrupted);
                            let _ = reply.send(Ok(()));
                            break;
                        }
                        interrupting = true;
                        state.lock().state(RunState::Interrupting);
                        let result = write(&mut stdin, &json!({"type":"control_request","request_id":INTERRUPT,"request":{"subtype":"interrupt"}})).await;
                        let failed = result.is_err();
                        let _ = reply.send(result);
                        if failed { fail(&state, "Interruption delivery failed. Review the project before continuing.", true); break; }
                        deadline = Instant::now() + INTERRUPT_TIMEOUT;
                    }
                },
                packet = incoming.recv() => {
                    let message = match packet {
                        Some(Packet::Message(message)) => message,
                        Some(Packet::Fault(reason)) => { fail(&state, reason, requested); break; }
                        Some(Packet::End) | None => { fail(&state, "Claude Code exited without confirming completion and an idle session. No work will be replayed.", requested); break; }
                    };
                    match message.get("type").and_then(Value::as_str) {
                        Some("control_response") => {
                            let response = &message["response"];
                            match response.get("request_id").and_then(Value::as_str) {
                                Some(INITIALIZE) if !requested => {
                                    if response["subtype"] != "success" || response["response"]["current_permission_mode"] != "default" || response["response"].get("session_state").is_none() {
                                        fail(&state, "Claude Code did not confirm default permission mode and session-state support. Update or check the CLI in its own terminal. No message was sent.", false);
                                        break;
                                    }
                                    // Persist intent before writing input: a host crash after this
                                    // point must never look like a message was safely unsent.
                                    {
                                        let mut run = state.lock();
                                        run.view.thread_id = Some(session.clone());
                                        run.event("notice", json!({"message":"Claude Code confirmed default permission mode. Existing CLI permission rules, hooks and integrations still apply. Only your new message is sent."}));
                                    }
                                    if state.lock().view.history_error.is_some() {
                                        fail(&state, "The conversation could not be saved before starting. No message was sent.", false);
                                        break;
                                    }
                                    requested = true;
                                    if write(&mut stdin, &json!({"type":"user","message":{"role":"user","content":prompt},"parent_tool_use_id":null,"session_id":"default","client_composed":true})).await.is_err() {
                                        fail(&state, "Message delivery to Claude Code was uncertain. Nothing will be replayed.", true);
                                        break;
                                    }
                                    deadline = Instant::now() + TURN_TIMEOUT;
                                }
                                Some(INTERRUPT) if response["subtype"] != "success" => {
                                    fail(&state, "Claude Code rejected the interruption request. Review the project before continuing.", requested);
                                    break;
                                }
                                _ => {}
                            }
                        }
                        Some("control_request") => {
                            if !initialized {
                                fail(&state, "Claude Code requested permission before confirming the conversation.", requested);
                                break;
                            }
                            if let Err(error) = request(&mut stdin, &state, &mut pending, &mut seen_requests, &message, interrupting, &trust).await {
                                fail(&state, &error.to_string(), requested);
                                break;
                            }
                        }
                        Some("control_cancel_request") => {
                            if let Some(wire_id) = message.get("request_id").and_then(Value::as_str) {
                                let ids: Vec<_> = pending.iter().filter(|(_, request)| request.wire_id == wire_id).map(|(id, _)| id.clone()).collect();
                                for id in ids { resolve(&state, &mut pending, &id, "cancelled_by_cli"); }
                            }
                        }
                        Some("system" | "assistant" | "user" | "stream_event" | "result") if requested => {
                            if message.get("session_id").and_then(Value::as_str) != Some(&session) {
                                fail(&state, "Claude Code returned data for a different conversation. The process was stopped.", true);
                                break;
                            }
                            if message["type"] == "system" && message["subtype"] == "init" {
                                let cwd = message.get("cwd").and_then(Value::as_str).and_then(|cwd| std::fs::canonicalize(cwd).ok());
                                if initialized || cwd.as_ref() != Some(&project) || message["permissionMode"] != "default" {
                                    fail(&state, "Claude Code did not confirm the requested project and default permission mode. Review the project before continuing.", true);
                                    break;
                                }
                                initialized = true;
                                let mut run = state.lock();
                                run.view.model = message.get("model").and_then(Value::as_str).map(|value| value.chars().filter(|c| !c.is_control()).take(200).collect());
                                if !interrupting { run.state(RunState::Running); }
                                continue;
                            }
                            if message["type"] == "system" && message["subtype"] == "session_state_changed" {
                                if message["state"] == "idle" && let Some(finished) = completion {
                                    // An interrupt acknowledgement alone is not completion.
                                    confirmed_idle = Some(finished);
                                    break;
                                }
                                continue;
                            }
                            if !initialized {
                                fail(&state, "Claude Code produced conversation data before confirming its project.", true);
                                break;
                            }
                            if message["type"] == "result" {
                                let Some(turn) = message.get("uuid").and_then(Value::as_str).filter(|id| uuid::Uuid::parse_str(id).is_ok()) else {
                                    fail(&state, "Claude Code returned an invalid completion identifier.", true); break;
                                };
                                let terminal = match (message.get("terminal_reason").and_then(Value::as_str), message.get("is_error").and_then(Value::as_bool)) {
                                    (Some("aborted_tools"), _) => RunState::Interrupted,
                                    (Some("completed"), Some(false)) if message["subtype"] == "success" => RunState::Completed,
                                    (_, Some(true)) => RunState::Failed,
                                    _ => { fail(&state, "Claude Code returned an unrecognized completion result. Review the project before continuing.", true); break; }
                                };
                                let mut run = state.lock();
                                run.view.turn_id = Some(turn.into());
                                if terminal == RunState::Failed {
                                    run.event("adapter_error", json!({"message":"Claude Code reported that the turn failed. Check the selected account and CLI configuration in its own terminal."}));
                                }
                                if !transcript.has_text && let Some(text) = message.get("result").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                                    run.event("item_completed", json!({"item":{"type":"agentMessage","id":format!("claude-result-{turn}"),"text":text}}));
                                }
                                run.persist(None, &[]);
                                completion = if message.get("queued_turn_count").and_then(Value::as_u64) == Some(0) { Some(terminal) } else { None };
                            } else if let Err(error) = transcript.accept(&state, &message) {
                                fail(&state, &error.to_string(), true);
                                break;
                            }
                        }
                        _ => {} // Account, auth, diagnostics and unknown envelopes are not retained.
                    }
                }
            }
        }
    }
    drop(stdin);
    // Let the CLI flush its saved session and exit after the confirmed idle
    // notification. Terminating immediately can lose a just-finished turn.
    let graceful_exit = if confirmed_idle.is_some() {
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .ok()
            .and_then(|result| result.ok())
    } else {
        None
    };
    let termination = tree.terminate();
    let reaped = graceful_exit.is_some()
        || matches!(
            tokio::time::timeout(Duration::from_secs(5), child.wait()).await,
            Ok(Ok(_))
        );
    let cleaned = tree.cleanup_confirmed(&termination, reaped);
    stdout_task.abort();
    stderr_task.abort();
    let _ = stdout_task.await;
    let _ = stderr_task.await;
    if !cleaned {
        fail(
            &state,
            "Process cleanup could not be confirmed. Inspect Claude Code processes before starting another run.",
            true,
        );
    } else if let Some(finished) = confirmed_idle {
        if graceful_exit.is_none_or(|status| finished == RunState::Completed && !status.success()) {
            fail(
                &state,
                "Claude Code reported an idle session but did not exit cleanly. Its saved conversation may be incomplete; review the project before continuing.",
                true,
            );
        } else {
            let mut run = state.lock();
            run.event("turn_completed", json!({"status":finished}));
            run.state(finished);
        }
    }
}

async fn request(
    stdin: &mut ChildStdin,
    state: &Arc<Mutex<StoredRun>>,
    pending: &mut BTreeMap<String, Pending>,
    seen: &mut HashSet<String>,
    message: &Value,
    interrupting: bool,
    trust: &Trust,
) -> Result<()> {
    let wire_id = message
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|id| valid_id(id))
        .ok_or_else(|| {
            AdapterError::Protocol("Invalid Claude permission request identifier.".into())
        })?;
    let request = &message["request"];
    if request["subtype"] != "can_use_tool" {
        return write(stdin, &json!({"type":"control_response","response":{"subtype":"error","request_id":wire_id,"error":"This control request is not supported by Switchya."}})).await;
    }
    if !seen.insert(wire_id.into()) || seen.len() > 4096 || pending.len() >= 16 {
        return Err(AdapterError::Protocol(
            "Claude Code exceeded the permission limit or repeated a request identifier.".into(),
        ));
    }
    let name = request
        .get("tool_name")
        .and_then(Value::as_str)
        .filter(|name| valid_id(name))
        .ok_or_else(|| {
            AdapterError::Protocol("Claude Code returned an invalid tool name.".into())
        })?;
    let input = request
        .get("input")
        .filter(|input| input.is_object())
        .ok_or_else(|| {
            AdapterError::Protocol("Claude Code returned an invalid tool input.".into())
        })?;
    if !request
        .get("tool_use_id")
        .and_then(Value::as_str)
        .is_some_and(valid_id)
    {
        return Err(AdapterError::Protocol(
            "Claude Code returned an invalid tool-use identifier.".into(),
        ));
    }
    if interrupting {
        return permission_response(
            stdin,
            wire_id,
            json!({"behavior":"deny","message":"The user interrupted this turn."}),
        )
        .await;
    }
    let preview = json!({"method":"claude/can_use_tool","request_id":wire_id,"request":request});
    let approval = AdapterApproval {
        id: format!("approval_{}", uuid::Uuid::new_v4().simple()),
        expected_hash: hash_value(&preview),
        method: "claude/can_use_tool".into(),
        preview,
        // These need structured answers or a permission-mode change, neither
        // of which is equivalent to approving the displayed input once.
        can_allow_once: !matches!(name, "AskUserQuestion" | "ExitPlanMode" | "EnterPlanMode"),
        permission_boundary: CLAUDE_PERMISSION_BOUNDARY.into(),
    };
    let targets = trust::claude_targets(name, input);
    if approval.can_allow_once
        && let Some(policy) = trust.approves(tool_kind(name), targets.as_deref())
    {
        // Journal the exact request before answering it, as for a human decision.
        state
            .lock()
            .event("approval_requested", json!({"approval":approval}));
        permission_response(
            stdin,
            wire_id,
            json!({"behavior":"allow","updatedInput":input}),
        )
        .await?;
        state.lock().event(
            "approval_resolved",
            json!({"approval_id":approval.id,"decision":"allow_once","decided_by":"auto_approve",
                "policy":policy,"request":trust::request_summary(&approval)}),
        );
        return Ok(());
    }
    pending.insert(
        approval.id.clone(),
        Pending {
            wire_id: wire_id.into(),
            input: input.clone(),
            approval: approval.clone(),
        },
    );
    let mut run = state.lock();
    run.view.pending_approvals.push(approval.clone());
    run.event("approval_requested", json!({"approval":approval}));
    run.state(RunState::AwaitingApproval);
    Ok(())
}

/// Only Claude Code's built-in file and shell tools have a known category. MCP,
/// web, agent and other tools always ask, whatever the approval setting.
pub(crate) fn tool_kind(name: &str) -> Option<ApprovalKind> {
    match name {
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => Some(ApprovalKind::Edit),
        "Bash" | "PowerShell" => Some(ApprovalKind::Command),
        _ => None,
    }
}

async fn permission_response(stdin: &mut ChildStdin, wire_id: &str, response: Value) -> Result<()> {
    write(stdin, &json!({"type":"control_response","response":{"subtype":"success","request_id":wire_id,"response":response}})).await
}

async fn decide(
    stdin: &mut ChildStdin,
    state: &Arc<Mutex<StoredRun>>,
    pending: &mut BTreeMap<String, Pending>,
    id: &str,
    expected_hash: &str,
    decision: ApprovalDecision,
    interrupting: bool,
) -> Result<()> {
    if interrupting {
        return Err(AdapterError::Conflict(
            "The run is interrupting; approval is no longer available.".into(),
        ));
    }
    let request = pending
        .get(id)
        .ok_or_else(|| AdapterError::Conflict("This approval is no longer pending.".into()))?;
    if request.approval.expected_hash != expected_hash {
        return Err(AdapterError::Conflict(
            "The approval preview changed. Refresh it before deciding.".into(),
        ));
    }
    if decision == ApprovalDecision::AllowOnce && !request.approval.can_allow_once {
        return Err(AdapterError::Invalid("This request needs an interaction that Switchya cannot approve once. Deny it and ask Claude to continue in ordinary chat.".into()));
    }
    let response = if decision == ApprovalDecision::AllowOnce {
        json!({"behavior":"allow","updatedInput":request.input})
    } else {
        json!({"behavior":"deny","message":"The user denied this request in Switchya. Continue in ordinary chat if clarification is needed; do not change permission mode."})
    };
    permission_response(stdin, &request.wire_id, response).await?;
    resolve(
        state,
        pending,
        id,
        if decision == ApprovalDecision::AllowOnce {
            "allow_once"
        } else {
            "deny"
        },
    );
    Ok(())
}

fn resolve(
    state: &Arc<Mutex<StoredRun>>,
    pending: &mut BTreeMap<String, Pending>,
    id: &str,
    decision: &str,
) {
    let summary = pending
        .remove(id)
        .map(|request| trust::request_summary(&request.approval));
    let mut run = state.lock();
    run.view
        .pending_approvals
        .retain(|approval| approval.id != id);
    run.event(
        "approval_resolved",
        json!({"approval_id":id,"decision":decision,"request":summary}),
    );
    if pending.is_empty() && run.view.state == RunState::AwaitingApproval {
        run.state(RunState::Running);
    }
}

#[derive(Default)]
struct Transcript {
    streams: BTreeMap<String, String>,
    /// Stream indices of started text blocks not yet completed, per parent and
    /// message. Claude Code sends each finished block as its own assistant
    /// message, so a block's position there is not its stream index.
    texts: BTreeMap<String, (String, VecDeque<u64>)>,
    tools: BTreeMap<String, Value>,
    has_text: bool,
}

impl Transcript {
    fn accept(&mut self, state: &Arc<Mutex<StoredRun>>, message: &Value) -> Result<()> {
        let parent = message
            .get("parent_tool_use_id")
            .and_then(Value::as_str)
            .unwrap_or("root");
        match message.get("type").and_then(Value::as_str) {
            Some("stream_event") => {
                let event = &message["event"];
                match event.get("type").and_then(Value::as_str) {
                    Some("message_start") => {
                        if let Some(id) = event
                            .pointer("/message/id")
                            .and_then(Value::as_str)
                            .filter(|id| valid_id(id))
                        {
                            if self.streams.len() >= 64 && !self.streams.contains_key(parent) {
                                return Err(AdapterError::Limit(
                                    "Too many concurrent Claude output streams.".into(),
                                ));
                            }
                            self.streams.insert(parent.into(), id.into());
                        }
                    }
                    Some("content_block_start")
                        if event.pointer("/content_block/type").and_then(Value::as_str)
                            == Some("text") =>
                    {
                        if let (Some(id), Some(index)) = (
                            self.streams.get(parent),
                            event.get("index").and_then(Value::as_u64),
                        ) && (self.texts.len() < 64 || self.texts.contains_key(parent))
                        {
                            let started = self.texts.entry(parent.into()).or_default();
                            if started.0 != *id {
                                *started = (id.clone(), VecDeque::new());
                            }
                            if started.1.len() < 256 {
                                started.1.push_back(index);
                            }
                        }
                    }
                    Some("content_block_delta")
                        if event.pointer("/delta/type").and_then(Value::as_str)
                            == Some("text_delta") =>
                    {
                        if let (Some(id), Some(index), Some(text)) = (
                            self.streams.get(parent),
                            event.get("index").and_then(Value::as_u64),
                            event.pointer("/delta/text").and_then(Value::as_str),
                        ) {
                            state.lock().event("assistant_delta", json!({"item_id":format!("claude-{parent}-{id}-{index}"),"text":text}));
                        }
                    }
                    Some("message_stop") => {
                        self.streams.remove(parent);
                    }
                    _ => {}
                }
            }
            Some("assistant") => {
                let Some(id) = message
                    .pointer("/message/id")
                    .and_then(Value::as_str)
                    .filter(|id| valid_id(id))
                else {
                    return Ok(());
                };
                if let Some(blocks) = message
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                {
                    for (position, block) in blocks.iter().enumerate() {
                        if block["type"] == "text"
                            && let Some(text) = block.get("text").and_then(Value::as_str)
                        {
                            // Use the streamed block's index so the completed item
                            // replaces its fragments. Without a recorded start, the
                            // message holds its whole content in stream order.
                            let index = self.completed_text(parent, id).unwrap_or(position as u64);
                            self.has_text |= !text.is_empty();
                            state.lock().event("item_completed", json!({"item":{"type":"agentMessage","id":format!("claude-{parent}-{id}-{index}"),"text":text}}));
                        } else if block["type"] == "tool_use"
                            && let (Some(id), Some(name), Some(input)) = (
                                block
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .filter(|id| valid_id(id)),
                                block
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .filter(|name| valid_id(name)),
                                block.get("input").filter(|input| input.is_object()),
                            )
                        {
                            if self.tools.len() >= 256 {
                                return Err(AdapterError::Limit(
                                    "Too many unfinished Claude tools.".into(),
                                ));
                            }
                            let item = json!({"type":"claudeTool","id":id,"name":name,"input":input,"status":"inProgress"});
                            self.tools.insert(id.into(), item.clone());
                            state.lock().event("item_started", json!({"item":item}));
                        }
                    }
                }
            }
            Some("user") => {
                if let Some(blocks) = message
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                {
                    for block in blocks.iter().filter(|block| block["type"] == "tool_result") {
                        if let Some(id) = block.get("tool_use_id").and_then(Value::as_str)
                            && let Some(mut item) = self.tools.remove(id)
                        {
                            item["status"] = json!(if block["is_error"] == true {
                                "failed"
                            } else {
                                "completed"
                            });
                            item["output"] = json!(text_content(&block["content"]));
                            state.lock().event("item_completed", json!({"item":item}));
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The stream index of this message's next started text block, if any.
    fn completed_text(&mut self, parent: &str, id: &str) -> Option<u64> {
        let (message, started) = self.texts.get_mut(parent)?;
        if message != id {
            return None;
        }
        let index = started.pop_front();
        if started.is_empty() {
            self.texts.remove(parent);
        }
        index
    }
}

fn text_content(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.into();
    }
    content
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
