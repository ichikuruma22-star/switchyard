//! A std-only controlled Claude protocol fixture. No network or model calls.
use std::io::{self, BufRead, Write};

fn send(value: &str) {
    println!("{value}");
    io::stdout().flush().unwrap();
}
fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r"))
}
fn result(session: &str, interrupted: bool) {
    send(&format!(r#"{{"type":"result","session_id":{session},"uuid":"13e0dca1-1fa5-4cb7-9d93-ab86d2652f23","subtype":"{}","is_error":{interrupted},"terminal_reason":"{}","queued_turn_count":0,"result":"Fixture finished","errors":["private-diagnostic-do-not-display"]}}"#,
        if interrupted { "error_during_execution" } else { "success" }, if interrupted { "aborted_tools" } else { "completed" }));
}
fn idle(session: &str) {
    send(&format!(r#"{{"type":"system","subtype":"session_state_changed","state":"idle","sdk_host_only":true,"session_id":{session}}}"#));
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let scenario = args.get(1).map(String::as_str).unwrap_or("allow");
    let (existing, scenario) = scenario.strip_prefix("system:")
        .map_or((false, scenario), |scenario| (true, scenario));
    assert!(args.windows(2).any(|pair| pair == ["--permission-mode", "default"]));
    assert!(args.windows(2).any(|pair| pair == ["--permission-prompt-tool", "stdio"]));
    assert_eq!(std::env::var("CLAUDE_CODE_SDK_READS_SESSION_STATE").as_deref(), Ok("1"));
    if !existing {
        assert!(std::env::var_os("CLAUDE_CONFIG_DIR").is_some());
    }
    assert!(!args.iter().any(|arg| arg.contains("literal")), "prompts must travel over stdin");
    let resuming = args.iter().any(|arg| arg.starts_with("--resume="));
    assert_eq!(resuming, scenario == "resume");
    let session = args.iter().find_map(|arg| arg.strip_prefix("--session-id=").or_else(|| arg.strip_prefix("--resume="))).unwrap();
    let session = quoted(session);
    let project = std::env::current_dir().unwrap();
    let cwd = quoted(project.to_str().unwrap());
    let marker = project.join("approved.txt");
    let tool_name = if scenario == "deny_only" { "ExitPlanMode" } else { "Write" };
    let approval = format!(r#"{{"type":"control_request","request_id":"permission-fixture","request":{{"subtype":"can_use_tool","tool_name":"{tool_name}","tool_use_id":"tool-fixture","input":{{"content":"literal $(not a shell) `text`"}},"permission_suggestions":[{{"type":"addRules","behavior":"allow","destination":"userSettings"}}]}}}}"#);
    eprintln!("private-diagnostic-do-not-display");
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        if line.contains("\"subtype\":\"initialize\"") {
            let mode = if scenario == "policy_mismatch" { "bypassPermissions" } else { "default" };
            send(&format!(r#"{{"type":"control_response","response":{{"subtype":"success","request_id":"switchya-initialize","response":{{"current_permission_mode":"{mode}","session_state":"idle","account":{{"token":"private-account-do-not-display"}}}}}}}}"#));
        } else if line.contains("\"type\":\"user\"") {
            assert_ne!(scenario, "policy_mismatch", "a model turn must not follow a policy mismatch");
            assert!(line.contains("\"client_composed\":true"));
            assert!(line.contains("literal"));
            std::fs::write(project.join("received-input.json"), &line).unwrap();
            if scenario == "early_exit" { return; }
            let actual_session = if scenario == "wrong_session" { "\"wrong-session\"" } else { &session };
            let actual_cwd = if scenario == "wrong_cwd" { "\"/missing-switchya-project\"" } else { &cwd };
            send(&format!(r#"{{"type":"system","subtype":"init","cwd":{actual_cwd},"session_id":{actual_session},"model":"fixture-model","permissionMode":"default","apiKeySource":"private-account-do-not-display"}}"#));
            if scenario == "malformed" { send("not-json"); continue; }
            if scenario == "oversized" { send(&"x".repeat(256 * 1024 + 1)); continue; }
            if scenario == "thinking" {
                // Like Claude Code: each finished block is its own assistant message,
                // so the text block is first in its message but second in the stream.
                let stream = |event: &str| send(&format!(r#"{{"type":"stream_event","session_id":{session},"event":{event}}}"#));
                let assistant = |block: &str| send(&format!(r#"{{"type":"assistant","session_id":{session},"message":{{"id":"msg-fixture","content":[{block}]}}}}"#));
                stream(r#"{"type":"message_start","message":{"id":"msg-fixture"}}"#);
                stream(r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#);
                stream(r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Fixture thought"}}"#);
                stream(r#"{"type":"content_block_stop","index":0}"#);
                assistant(r#"{"type":"thinking","thinking":"Fixture thought","signature":"fixture"}"#);
                stream(r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#);
                stream(r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Fixture "}}"#);
                stream(r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"text"}}"#);
                stream(r#"{"type":"content_block_stop","index":1}"#);
                assistant(r#"{"type":"text","text":"Fixture text"}"#);
                stream(r#"{"type":"message_stop"}"#);
                result(&session, false);
                idle(&session);
                continue;
            }
            send(&format!(r#"{{"type":"stream_event","session_id":{session},"event":{{"type":"message_start","message":{{"id":"msg-fixture"}}}}}}"#));
            send(&format!(r#"{{"type":"stream_event","session_id":{session},"event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"Fixture text"}}}}}}"#));
            if scenario == "streaming" { std::thread::sleep(std::time::Duration::from_millis(500)); }
            send(&format!(r#"{{"type":"assistant","session_id":{session},"message":{{"id":"msg-fixture","content":[{{"type":"text","text":"Fixture text"}},{{"type":"tool_use","id":"tool-fixture","name":"Write","input":{{"content":"literal $(not a shell) `text`"}}}}]}}}}"#));
            if matches!(scenario, "durable" | "resume" | "delayed_idle" | "exit_without_idle") {
                result(&session, false);
                if scenario == "exit_without_idle" { return; }
                if scenario == "delayed_idle" {
                    std::thread::sleep(std::time::Duration::from_millis(350));
                    std::fs::write(project.join("background-finished.txt"), "finished").unwrap();
                }
                idle(&session);
                continue;
            }
            if scenario == "unsupported_control" {
                send(r#"{"type":"control_request","request_id":"unknown-fixture","request":{"subtype":"unknown_request"}}"#);
                continue;
            }
            send(&approval);
            if scenario == "duplicate" { send(&approval); }
            if scenario == "cancel" {
                send(r#"{"type":"control_cancel_request","request_id":"permission-fixture"}"#);
                result(&session, false);
                idle(&session);
            }
        } else if line.contains("\"subtype\":\"interrupt\"") {
            send(r#"{"type":"control_response","response":{"subtype":"success","request_id":"switchya-interrupt","response":{}}}"#);
            if scenario == "interrupt_unconfirmed" { continue; }
            send(r#"{"type":"control_cancel_request","request_id":"permission-fixture"}"#);
            result(&session, true);
            idle(&session);
        } else if line.contains("\"type\":\"control_response\"") {
            if scenario == "unsupported_control" {
                assert!(line.contains("\"subtype\":\"error\""));
            } else {
                assert!(!line.contains("updatedPermissions"), "one-time approval must never grant persistent permission rules");
                if line.contains("\"behavior\":\"allow\"") {
                    assert!(line.contains("\"updatedInput\":{\"content\":\"literal $(not a shell) `text`\"}"));
                    std::fs::write(&marker, "explicitly approved").unwrap();
                } else { assert!(line.contains("\"behavior\":\"deny\"")); }
                send(&format!(r#"{{"type":"user","session_id":{session},"message":{{"content":[{{"type":"tool_result","tool_use_id":"tool-fixture","content":[{{"type":"text","text":"Fixture tool result"}},{{"type":"image","data":"private-image-do-not-display"}}]}}]}},"tool_use_result":{{"private":"private-metadata-do-not-display"}}}}"#));
            }
            result(&session, false);
            idle(&session);
        }
    }
}
