use super::*;
use std::{path::PathBuf, sync::OnceLock, time::Duration};

fn fixture() -> PathBuf {
    static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
    FIXTURE
        .get_or_init(|| {
            let directory = tempfile::tempdir().unwrap().keep();
            let binary = directory.join(if cfg!(windows) {
                "claude-fixture.exe"
            } else {
                "claude-fixture"
            });
            let source =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude_child.rs");
            let output = std::process::Command::new("rustc")
                .arg("--edition=2024")
                .arg(source)
                .arg("-o")
                .arg(&binary)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            binary
        })
        .clone()
}

fn request(project: &Path, previous: Option<String>) -> StartRequest {
    StartRequest {
        adapter_id: "claude".into(),
        project_path: project.into(),
        prompt: if previous.is_some() {
            "New followup literal"
        } else {
            "First message literal"
        }
        .into(),
        command_id: uuid::Uuid::new_v4().to_string(),
        continue_run_id: previous,
        auto_approve: Default::default(),
        review_paths: Vec::new(),
        profile: Some(ProfileBinding {
            id: "claude-fixture".into(),
            name: "Claude fixture".into(),
            agent_id: "claude".into(),
            home: project.into(),
            managed: true,
        }),
    }
}

fn existing_profile() -> ProfileBinding {
    ProfileBinding {
        id: "system-claude".into(),
        name: "Existing Claude CLI".into(),
        agent_id: "claude".into(),
        home: PathBuf::new(),
        managed: false,
    }
}

fn start(manager: &AdapterManager, scenario: &str) -> (tempfile::TempDir, AdapterRun) {
    start_profile(manager, scenario, true)
}

fn start_profile(
    manager: &AdapterManager,
    scenario: &str,
    managed: bool,
) -> (tempfile::TempDir, AdapterRun) {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().canonicalize().unwrap();
    let mut request = request(&project, None);
    if !managed {
        request.profile = Some(existing_profile());
    }
    profile::validate(request.profile.as_ref().unwrap(), "claude").unwrap();
    let run = manager
        .start_process(
            request,
            project,
            fixture(),
            vec![if managed {
                scenario.into()
            } else {
                format!("system:{scenario}")
            }],
        )
        .unwrap();
    (directory, run)
}

async fn wait(
    manager: &AdapterManager,
    id: &str,
    predicate: impl Fn(&AdapterRun) -> bool,
) -> AdapterRun {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let run = manager.run(id).unwrap();
            if predicate(&run) {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "run did not reach expected state: {:?}",
            manager.events(id, 0, 200)
        )
    })
}

#[tokio::test]
async fn claude_permission_decisions_bind_exact_input_and_keep_diagnostics_private() {
    for (managed, decision) in [
        (true, ApprovalDecision::AllowOnce),
        (true, ApprovalDecision::Deny),
        (false, ApprovalDecision::AllowOnce),
        (false, ApprovalDecision::Deny),
    ] {
        let manager = AdapterManager::new();
        let (directory, run) = start_profile(&manager, "allow", managed);
        let pending = wait(&manager, &run.id, |run| {
            run.state == RunState::AwaitingApproval
        })
        .await;
        assert!(!directory.path().join("approved.txt").exists());
        let approval = &pending.pending_approvals[0];
        assert_eq!(approval.method, "claude/can_use_tool");
        assert!(approval.can_allow_once);
        assert!(
            manager
                .decide(&run.id, &approval.id, "wrong-hash", decision)
                .await
                .is_err()
        );
        manager
            .decide(&run.id, &approval.id, &approval.expected_hash, decision)
            .await
            .unwrap();
        let finished = wait(&manager, &run.id, |run| !run.state.is_active()).await;
        assert_eq!(finished.state, RunState::Completed);
        manager.shutdown().await;
        assert_eq!(
            directory.path().join("approved.txt").exists(),
            decision == ApprovalDecision::AllowOnce
        );
        let events = manager.events(&run.id, 0, 200).unwrap();
        let output = serde_json::to_string(&events).unwrap();
        assert!(!output.contains("private-"), "private CLI data was exposed");
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == "assistant_delta")
                .count(),
            0
        );
        assert_eq!(
            events
                .iter()
                .filter(
                    |event| event.payload.pointer("/item/type").and_then(Value::as_str)
                        == Some("agentMessage")
                )
                .count(),
            1
        );
        assert!(events.iter().any(|event| {
            event
                .payload
                .pointer("/item/output")
                .and_then(Value::as_str)
                == Some("Fixture tool result")
        }));
        assert!(
            finished
                .permission_boundary
                .contains("existing permission rules")
        );
    }
}

#[tokio::test]
async fn claude_policy_is_confirmed_before_sending_input() {
    let manager = AdapterManager::new();
    let (directory, run) = start(&manager, "policy_mismatch");
    let stopped = wait(&manager, &run.id, |run| !run.state.is_active()).await;
    assert_eq!(stopped.state, RunState::Failed);
    assert!(stopped.thread_id.is_none());
    assert!(!directory.path().join("received-input.json").exists());
    manager.shutdown().await;
}

#[tokio::test]
async fn claude_partial_text_uses_the_shared_transcript_contract_before_completion() {
    let manager = AdapterManager::new();
    let (_directory, run) = start(&manager, "streaming");
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(delta) = manager
                .events(&run.id, 0, 200)
                .unwrap()
                .into_iter()
                .find(|event| event.kind == "assistant_delta")
            {
                assert_eq!(delta.payload["text"], "Fixture text");
                assert_eq!(delta.payload["item_id"], "claude-root-msg-fixture-0");
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let pending = wait(&manager, &run.id, |run| {
        run.state == RunState::AwaitingApproval
    })
    .await;
    let approval = &pending.pending_approvals[0];
    manager
        .decide(
            &run.id,
            &approval.id,
            &approval.expected_hash,
            ApprovalDecision::Deny,
        )
        .await
        .unwrap();
    assert_eq!(
        wait(&manager, &run.id, |run| !run.state.is_active())
            .await
            .state,
        RunState::Completed
    );
    manager.shutdown().await;
}

#[tokio::test]
async fn claude_text_after_thinking_replaces_its_streamed_fragments() {
    let manager = AdapterManager::new();
    let (_directory, run) = start(&manager, "thinking");
    let finished = wait(&manager, &run.id, |run| !run.state.is_active()).await;
    assert_eq!(finished.state, RunState::Completed);
    let events = manager.events(&run.id, 0, 200).unwrap();
    assert!(!events.iter().any(|event| event.kind == "assistant_delta"));
    let messages: Vec<_> = events
        .iter()
        .filter(|event| {
            event.payload.pointer("/item/type").and_then(Value::as_str) == Some("agentMessage")
        })
        .collect();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].payload["item"]["id"],
        "claude-root-msg-fixture-1"
    );
    assert_eq!(messages[0].payload["item"]["text"], "Fixture text");
    manager.shutdown().await;
}

#[tokio::test]
async fn claude_protocol_faults_and_exit_without_idle_require_review() {
    for scenario in [
        "wrong_session",
        "wrong_cwd",
        "malformed",
        "oversized",
        "early_exit",
        "duplicate",
        "exit_without_idle",
    ] {
        let manager = AdapterManager::new();
        let (_directory, run) = start(&manager, scenario);
        let stopped = wait(&manager, &run.id, |run| !run.state.is_active()).await;
        assert_eq!(stopped.state, RunState::RecoveryRequired, "{scenario}");
        assert!(stopped.pending_approvals.is_empty());
        manager.shutdown().await;
    }
}

#[tokio::test]
async fn claude_waits_for_background_work_after_result() {
    let manager = AdapterManager::new();
    let (directory, run) = start(&manager, "delayed_idle");
    wait(&manager, &run.id, |run| run.turn_id.is_some()).await;
    assert!(manager.run(&run.id).unwrap().state.is_active());
    assert_eq!(
        wait(&manager, &run.id, |run| !run.state.is_active())
            .await
            .state,
        RunState::Completed
    );
    manager.shutdown().await;
    assert!(directory.path().join("background-finished.txt").exists());
}

#[tokio::test]
async fn claude_interrupt_requires_result_and_idle_not_just_acknowledgement() {
    for scenario in ["interrupt", "interrupt_unconfirmed"] {
        let manager = AdapterManager::new();
        let (directory, run) = start(&manager, scenario);
        wait(&manager, &run.id, |run| {
            run.state == RunState::AwaitingApproval
        })
        .await;
        manager.interrupt(&run.id).await.unwrap();
        let stopped = wait(&manager, &run.id, |run| !run.state.is_active()).await;
        assert_eq!(
            stopped.state,
            if scenario == "interrupt" {
                RunState::Interrupted
            } else {
                RunState::RecoveryRequired
            }
        );
        assert!(!directory.path().join("approved.txt").exists());
        manager.shutdown().await;
    }
}

#[tokio::test]
async fn claude_special_interactions_are_deny_only_and_unknown_controls_are_rejected() {
    let manager = AdapterManager::new();
    let (_directory, run) = start(&manager, "deny_only");
    let pending = wait(&manager, &run.id, |run| {
        run.state == RunState::AwaitingApproval
    })
    .await;
    let approval = &pending.pending_approvals[0];
    assert!(!approval.can_allow_once);
    assert!(
        manager
            .decide(
                &run.id,
                &approval.id,
                &approval.expected_hash,
                ApprovalDecision::AllowOnce
            )
            .await
            .is_err()
    );
    manager
        .decide(
            &run.id,
            &approval.id,
            &approval.expected_hash,
            ApprovalDecision::Deny,
        )
        .await
        .unwrap();
    assert_eq!(
        wait(&manager, &run.id, |run| !run.state.is_active())
            .await
            .state,
        RunState::Completed
    );
    manager.shutdown().await;
    for scenario in ["unsupported_control", "cancel"] {
        let manager = AdapterManager::new();
        let (_directory, run) = start(&manager, scenario);
        let finished = wait(&manager, &run.id, |run| !run.state.is_active()).await;
        assert_eq!(finished.state, RunState::Completed);
        assert!(finished.pending_approvals.is_empty());
        manager.shutdown().await;
    }
}

#[tokio::test]
async fn claude_continuation_after_restart_resumes_only_the_new_message() {
    for managed in [true, false] {
        let data = tempfile::tempdir().unwrap();
        let manager = AdapterManager::open(data.path()).unwrap();
        let (directory, first) = start_profile(&manager, "durable", managed);
        let first = wait(&manager, &first.id, |run| !run.state.is_active()).await;
        assert_eq!(first.state, RunState::Completed);
        manager.shutdown().await;
        drop(manager);
        let manager = AdapterManager::open(data.path()).unwrap();
        let project = directory.path().canonicalize().unwrap();
        let mut request = request(&project, Some(first.id.clone()));
        let original = manager.run_profile(&first.id).unwrap().unwrap();
        assert_eq!(original.managed, managed);
        if !managed {
            assert_eq!(original, existing_profile());
        }
        request.profile = Some(original);
        let mut changed = request.clone();
        changed.profile = if managed {
            Some(existing_profile())
        } else {
            self::request(&project, Some(first.id.clone())).profile
        };
        assert!(matches!(
            manager.start_process(
                changed.clone(),
                project.clone(),
                fixture(),
                vec!["resume".into()]
            ),
            Err(AdapterError::Conflict(_))
        ));
        let next = manager
            .start_process(
                request.clone(),
                project.clone(),
                fixture(),
                vec![if managed {
                    "resume".into()
                } else {
                    "system:resume".into()
                }],
            )
            .unwrap();
        assert!(matches!(
            manager.start(changed),
            Err(AdapterError::Conflict(_))
        ));
        let repeated = manager.start(request).unwrap();
        assert_eq!(repeated.id, next.id);
        let next = wait(&manager, &next.id, |run| !run.state.is_active()).await;
        assert_eq!(next.state, RunState::Completed);
        assert_eq!(next.thread_id, first.thread_id);
        assert_eq!(next.conversation_id, first.conversation_id);
        let sent = std::fs::read_to_string(project.join("received-input.json")).unwrap();
        assert!(sent.contains("New followup literal"));
        assert!(!sent.contains("First message literal"));
        manager.shutdown().await;
    }
}

#[tokio::test]
async fn claude_requires_an_explicit_valid_account() {
    let directory = tempfile::tempdir().unwrap();
    let mut request = request(&directory.path().canonicalize().unwrap(), None);
    request.profile = None;
    assert!(matches!(
        AdapterManager::new().start(request),
        Err(AdapterError::Invalid(_))
    ));
}

#[tokio::test]
async fn claude_auto_approval_covers_built_in_tools_but_never_deny_only_requests() {
    // `account_inside` keeps the fixture account's own folder in the project.
    let start_auto = |manager: &AdapterManager,
                      scenario: &str,
                      auto_approve: AutoApproval,
                      account_inside: bool| {
        let directory = tempfile::tempdir().unwrap();
        let account = tempfile::tempdir().unwrap();
        let project = directory.path().canonicalize().unwrap();
        let mut request = request(&project, None);
        request.auto_approve = AutoApprovalSource::new(move || auto_approve);
        if !account_inside {
            request.profile.as_mut().unwrap().home = account.path().to_owned();
        }
        let run = manager
            .start_process(request, project, fixture(), vec![scenario.into()])
            .unwrap();
        (directory, account, run)
    };
    let edits = AutoApproval {
        edits: true,
        commands: false,
    };
    // Write is a file edit: it is answered without a prompt.
    let manager = AdapterManager::new();
    let (directory, _account, run) = start_auto(&manager, "allow", edits, false);
    let finished = wait(&manager, &run.id, |run| !run.state.is_active()).await;
    assert_eq!(finished.state, RunState::Completed);
    manager.shutdown().await;
    assert!(directory.path().join("approved.txt").exists());
    let events = manager.events(&run.id, 0, 200).unwrap();
    assert!(events.iter().any(|e| e.kind == "approval_requested"));
    let decision = events
        .iter()
        .find(|e| e.kind == "approval_resolved")
        .unwrap();
    assert_eq!(decision.payload["decided_by"], "auto_approve");
    assert_eq!(decision.payload["decision"], "allow_once");
    assert_eq!(decision.payload["request"]["tool"], "Write");
    assert!(
        decision.payload["request"]["paths"][0]
            .as_str()
            .unwrap()
            .ends_with("approved.txt")
    );

    // Commands-only trust does not cover Write; deny-only plan changes, a write
    // to a Git hook and a write into the account's own folder always ask.
    for (scenario, auto, account_inside) in [
        (
            "allow",
            AutoApproval {
                edits: false,
                commands: true,
            },
            false,
        ),
        (
            "deny_only",
            AutoApproval {
                edits: true,
                commands: true,
            },
            false,
        ),
        ("write_hook", edits, false),
        ("allow", edits, true),
    ] {
        let manager = AdapterManager::new();
        let (directory, _account, run) = start_auto(&manager, scenario, auto, account_inside);
        let pending = wait(&manager, &run.id, |run| {
            run.state == RunState::AwaitingApproval
        })
        .await;
        let approval = &pending.pending_approvals[0];
        manager
            .decide(
                &run.id,
                &approval.id,
                &approval.expected_hash,
                ApprovalDecision::Deny,
            )
            .await
            .unwrap();
        wait(&manager, &run.id, |run| !run.state.is_active()).await;
        manager.shutdown().await;
        assert!(
            !directory.path().join("approved.txt").exists(),
            "{scenario}"
        );
    }
    assert_eq!(claude::tool_kind("Edit"), Some(types::ApprovalKind::Edit));
    assert_eq!(
        claude::tool_kind("PowerShell"),
        Some(types::ApprovalKind::Command)
    );
    assert_eq!(claude::tool_kind("WebFetch"), None);
    assert_eq!(claude::tool_kind("mcp__server__tool"), None);
}
