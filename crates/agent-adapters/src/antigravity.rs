//! Antigravity (`agy`) print-mode stream-json protocol, verified against
//! agy 1.3.5. Runs are read-only: `--mode plan` is requested for every run,
//! so this adapter never receives approval requests and never applies edits.
//! Sign-in is not inspected (the CLI exposes no account-status command);
//! runs surface the CLI's own authentication failures.
use crate::{
    AdapterError, Control, RunState, StoredRun,
    process_tree::{self, ProcessTree},
    protocol::{RunInput, TURN_TIMEOUT, fail},
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

/// `agy --print` takes the prompt as a command-line argument; keep it well
/// inside the Windows command-line limit. Longer work can be split across
/// conversation turns.
const MAX_PROMPT_BYTES: usize = 24_000;
const MAX_LINE_BYTES: usize = 1024 * 1024;
const CONVERSATION_CHANGED: &str = "Antigravity reported a different conversation than the saved one. Review the project before continuing; nothing will be replayed.";

enum Packet {
    Line(String),
    Fault(&'static str),
    End,
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
        ephemeral: _,
        // Plan mode never asks to edit or run anything.
        trust: _,
    } = input;
    if prompt.len() > MAX_PROMPT_BYTES {
        fail(
            &state,
            "Antigravity tasks are limited to 24,000 bytes per message in this integration. Split the task across conversation turns.",
            false,
        );
        return;
    }
    if let Some(thread) = &resume_thread
        && uuid::Uuid::parse_str(thread).is_err()
    {
        fail(
            &state,
            "The saved Antigravity conversation identifier is invalid. No message was sent.",
            false,
        );
        return;
    }
    let mut command = Command::new(executable);
    command.args(prefix_args).arg("--print").arg(&prompt).args([
        "--output-format",
        "stream-json",
        "--mode",
        "plan",
    ]);
    if let Some(thread) = &resume_thread {
        command.arg("--conversation").arg(thread);
    }
    crate::profile::configure(&mut command, profile.as_ref());
    command
        .current_dir(&project)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    process_tree::configure(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            fail(&state, "The Antigravity process could not start.", false);
            return;
        }
    };
    let mut tree = match ProcessTree::attach(&child) {
        Ok(tree) => tree,
        Err(_) => {
            let _ = child.kill().await;
            fail(
                &state,
                "Could not establish local process cleanup; Antigravity was stopped.",
                false,
            );
            return;
        }
    };
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = tree.terminate();
        let _ = child.kill().await;
        fail(
            &state,
            "Antigravity's structured output is unavailable.",
            false,
        );
        return;
    };
    let (sender, mut incoming) = mpsc::channel::<Packet>(32);
    let stdout_task = tokio::spawn(read_lines(stdout, sender.clone()));
    let stderr_task = tokio::spawn(discard_stderr(stderr, sender));
    {
        let mut run = state.lock();
        run.state(RunState::Running);
        run.event(
            "notice",
            json!({"message":"Antigravity runs in plan mode: it can inspect the project and answer, but cannot apply edits or run commands that change files. Continuing sends only your new message."}),
        );
    }
    let deadline = tokio::time::Instant::now() + TURN_TIMEOUT;
    let mut texts: BTreeMap<u64, String> = BTreeMap::new();
    let mut emitted_text = false;
    let mut finished = false;
    let mut interrupted = false;
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                fail(&state, "The local host shut down. This run will not be replayed.", true);
                break;
            }
            _ = tokio::time::sleep_until(deadline) => {
                fail(&state, "Antigravity exceeded the adapter time limit. Its process tree was stopped.", true);
                break;
            }
            next = control.recv() => match next {
                None => { fail(&state, "Adapter control closed.", true); break; }
                Some(Control::Decide { reply, .. }) => {
                    let _ = reply.send(Err(AdapterError::Invalid(
                        "Antigravity plan-mode runs have no approval requests.".into(),
                    )));
                }
                Some(Control::Interrupt { reply }) => {
                    let _ = reply.send(Ok(()));
                    interrupted = true;
                    state.lock().state(RunState::Interrupted);
                    break;
                }
            },
            packet = incoming.recv() => {
                let line = match packet {
                    Some(Packet::Line(line)) => line,
                    Some(Packet::Fault(reason)) => { fail(&state, reason, true); break; }
                    Some(Packet::End) | None => {
                        fail(&state, "Antigravity exited without confirming completion. No work will be replayed.", true);
                        break;
                    }
                };
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                match message.get("event").and_then(Value::as_str) {
                    Some("init") => {
                        if attach(&state, &message, resume_thread.as_deref()) {
                            continue;
                        }
                        fail(&state, CONVERSATION_CHANGED, true);
                        break;
                    }
                    Some("step_update") => {
                        if let Some(step) = message.get("step_update")
                            && step_event(&state, step, &mut texts)
                        {
                            emitted_text = true;
                        }
                    }
                    Some("result") => {
                        let result = message.get("result").cloned().unwrap_or(Value::Null);
                        if !attach(&state, &result, resume_thread.as_deref()) {
                            fail(&state, CONVERSATION_CHANGED, true);
                            break;
                        }
                        finish(&state, &result, emitted_text);
                        finished = true;
                        break;
                    }
                    Some("error") => {
                        let text = message.get("error").and_then(|e| e.get("message")).and_then(Value::as_str)
                            .unwrap_or("Antigravity reported an error.");
                        fail(&state, &truncate(text, 500), true);
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
    let _ = tree.terminate();
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    stdout_task.abort();
    stderr_task.abort();
    if interrupted && !finished {
        // State was already set to Interrupted when the interrupt was accepted.
    }
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Record the conversation agy reports in `value`. A run stays in one
/// conversation, and a continued run must stay in the saved one. Returns
/// false when agy reports a different conversation.
fn attach(state: &Arc<Mutex<StoredRun>>, value: &Value, resume_thread: Option<&str>) -> bool {
    let Some(id) = value
        .get("conversation_id")
        .and_then(Value::as_str)
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
    else {
        return true;
    };
    let mut run = state.lock();
    if run
        .view
        .thread_id
        .as_deref()
        .or(resume_thread)
        .is_some_and(|expected| expected != id)
    {
        return false;
    }
    run.view.thread_id = Some(id.to_owned());
    true
}

fn step_event(
    state: &Arc<Mutex<StoredRun>>,
    step: &Value,
    texts: &mut BTreeMap<u64, String>,
) -> bool {
    let index = step.get("step_index").and_then(Value::as_u64).unwrap_or(0);
    let status = step.get("state").and_then(Value::as_str).unwrap_or("");
    let mut emitted = false;
    match step.get("step_type").and_then(Value::as_str) {
        Some("agent_response") => {
            let delta = step.get("text_delta").and_then(Value::as_str).unwrap_or("");
            if !delta.is_empty() {
                texts.entry(index).or_default().push_str(delta);
                state.lock().event(
                    "assistant_delta",
                    json!({"item_id":format!("antigravity-{index}"),"text":delta}),
                );
            }
            if status == "DONE" {
                let text = texts.remove(&index).unwrap_or_default();
                if !text.trim().is_empty() {
                    state.lock().event(
                        "item_completed",
                        json!({"item":{"type":"agentMessage","id":format!("antigravity-{index}"),"text":text}}),
                    );
                    emitted = true;
                }
            }
        }
        Some("tool") => {
            let info = step.get("tool_info").cloned().unwrap_or(Value::Null);
            let item = json!({
                "type": "claudeTool",
                "id": format!("antigravity-tool-{index}"),
                "name": step.get("tool_name").and_then(Value::as_str).unwrap_or("tool"),
                "input": info.get("parameters").cloned().unwrap_or_else(|| json!({})),
                "status": if status == "DONE" { "completed" } else { "inProgress" },
            });
            let mut item = item;
            if let Some(output) = info.get("output").and_then(Value::as_str) {
                item["output"] = json!(truncate(output, 60_000));
            }
            state.lock().event(
                if status == "DONE" {
                    "item_completed"
                } else {
                    "item_started"
                },
                json!({"item":item}),
            );
        }
        _ => {}
    }
    emitted
}

fn finish(state: &Arc<Mutex<StoredRun>>, result: &Value, emitted_text: bool) {
    let status = result
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if status == "SUCCESS" {
        if !emitted_text
            && let Some(text) = result.get("response").and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            state.lock().event(
                "item_completed",
                json!({"item":{"type":"agentMessage","id":"antigravity-result","text":text}}),
            );
        }
        let mut run = state.lock();
        // agy reports no turn identifier. Record the confirmed turn so the
        // saved conversation can be continued; the state change saves it.
        run.view.turn_id = Some(uuid::Uuid::new_v4().to_string());
        run.event("turn_completed", json!({"status":"completed"}));
        run.state(RunState::Completed);
    } else {
        let message = result
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("Antigravity reported that the turn did not succeed. Check the selected account and CLI configuration in its own terminal.");
        fail(state, &truncate(message, 500), true);
    }
}

async fn read_lines(stdout: tokio::process::ChildStdout, sender: mpsc::Sender<Packet>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                let _ = sender.send(Packet::End).await;
                return;
            }
            Ok(_) => {
                if line.len() > MAX_LINE_BYTES {
                    let _ = sender
                        .send(Packet::Fault(
                            "Antigravity produced an oversized output line.",
                        ))
                        .await;
                    return;
                }
                let _ = sender.send(Packet::Line(line)).await;
            }
            Err(_) => {
                let _ = sender
                    .send(Packet::Fault("Antigravity's output could not be read."))
                    .await;
                return;
            }
        }
    }
}

async fn discard_stderr(stderr: tokio::process::ChildStderr, sender: mpsc::Sender<Packet>) {
    let mut reader = BufReader::new(stderr);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                if line.len() > MAX_LINE_BYTES {
                    let _ = sender
                        .send(Packet::Fault("Antigravity produced oversized diagnostics."))
                        .await;
                    return;
                }
            }
        }
    }
}
