//! Terminal access to the app's existing Codex adapter API. No second execution
//! backend: account binding, continuation, recovery and decisions stay in the host.
use super::{
    Cli, Input, ProjectResponse, Remote, interactive_input, line, options, segment, terminal_text,
    write_stderr, write_stdout,
};
use crate::{AppError, LaunchInfo, Result, start_host};
use axum::http::StatusCode;
use clap::{Args, Subcommand};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use switchyard_agent::Project;
use switchyard_agent_adapters::{
    AdapterApproval, AdapterEvent, AdapterRun, ApprovalDecision, RunState,
};
use tokio_util::sync::CancellationToken;

#[derive(Subcommand)]
pub(super) enum AgentCommand {
    /// List saved Codex runs, conversation IDs and their bound accounts.
    #[command(alias = "runs")]
    Sessions,
    /// Read one saved run and its retained activity without starting work.
    Show {
        #[arg(long)]
        run: String,
    },
    /// List cached Codex profile IDs; listing does not inspect account credentials.
    Accounts,
    /// Start a new Codex conversation using its installed CLI configuration.
    Run {
        #[arg(long)]
        project: Option<PathBuf>,
        #[arg(long)]
        profile: Option<String>,
        #[command(flatten)]
        message: MessageArgs,
    },
    /// Send a new message to the latest finished run of a saved conversation.
    Continue {
        #[arg(long)]
        run: String,
        #[command(flatten)]
        message: MessageArgs,
    },
    /// Acknowledge manual review of uncertain effects; never replay old work.
    Recover {
        #[arg(long)]
        run: String,
        /// Confirm you inspected the project and the uncertain run's activity.
        #[arg(long, required = true)]
        reviewed: bool,
    },
    /// Request interruption and read back the run's actual terminal state.
    Interrupt {
        #[arg(long)]
        run: String,
    },
}

#[derive(Args)]
pub(super) struct MessageArgs {
    #[arg(long)]
    prompt: String,
    /// Reuse only with the exact same request to reconcile an uncertain start.
    #[arg(long)]
    command_id: Option<uuid::Uuid>,
}

#[derive(Deserialize)]
struct Runs {
    runs: Vec<AdapterRun>,
}

#[derive(Deserialize)]
struct RunResponse {
    run: AdapterRun,
}

#[derive(Deserialize)]
struct Events {
    events: Vec<AdapterEvent>,
}

#[derive(Deserialize)]
struct Projects {
    projects: Vec<Project>,
}

#[derive(Deserialize)]
struct Profiles {
    profiles: Vec<Profile>,
    defaults: ProfileDefaults,
}

#[derive(Deserialize)]
struct ProfileDefaults {
    codex: Option<String>,
}

#[derive(Deserialize)]
struct Profile {
    id: String,
    name: String,
    agent_id: String,
    managed: bool,
    refreshing: bool,
    account: AccountState,
}

#[derive(Deserialize)]
struct AccountState {
    auth_status: String,
}

enum Selection {
    New {
        project: Option<PathBuf>,
        profile: Option<String>,
    },
    Continue(String),
}

pub(super) async fn execute(
    cli: &Cli,
    command: AgentCommand,
    cancelled: &CancellationToken,
) -> Result<()> {
    if let Some(path) = &cli.launch_info {
        if cli.config.is_some() || cli.data_dir.is_some() || cli.client_key_name.is_some() {
            return Err(AppError::invalid(
                "--launch-info uses the active host's configuration; omit local configuration flags.",
            ));
        }
        let remote = Remote::new(LaunchInfo::read(path)?)?;
        return dispatch(&remote, command, cancelled).await;
    }
    // An owned host uses exactly the app's durable stores and account manager.
    // It is always shut down, including on input/output errors or cancellation.
    let host = start_host(options(cli)?).await?;
    let result = match Remote::new(host.launch.clone()) {
        Ok(remote) => dispatch(&remote, command, cancelled).await,
        Err(error) => Err(error),
    };
    let shutdown = host.shutdown().await;
    result.and(shutdown)
}

async fn dispatch(
    remote: &Remote,
    command: AgentCommand,
    cancelled: &CancellationToken,
) -> Result<()> {
    match command {
        AgentCommand::Sessions => {
            let runs: Runs = remote.request(Method::GET, "adapter-runs", None).await?;
            print_json(&runs.runs)
        }
        AgentCommand::Show { run } => {
            let view = read_run(remote, &run).await?;
            let mut after = 0;
            let mut events = Vec::new();
            // Read the snapshot's retained window. A busy run cannot make this
            // read-only command follow new events indefinitely.
            while after < view.last_seq {
                let batch = read_events(remote, &run, after).await?;
                if batch.is_empty() {
                    break;
                }
                for event in batch {
                    if event.run_id != run || event.seq <= after {
                        return Err(AppError::local(
                            "The host returned inconsistent agent activity.",
                        ));
                    }
                    after = event.seq;
                    if event.seq <= view.last_seq {
                        events.push(event);
                    }
                }
            }
            print_json(&json!({"run":view,"events":events}))
        }
        AgentCommand::Accounts => {
            let profiles: Profiles = remote
                .request(Method::GET, "account-profiles", None)
                .await?;
            let views: Vec<_> = profiles.profiles.into_iter()
                .filter(|profile| profile.agent_id == "codex")
                .map(|profile| json!({
                    "id":profile.id,"name":profile.name,"managed":profile.managed,
                    "selected":profiles.defaults.codex.as_deref() == Some(profile.id.as_str()),
                    "cached_auth_status":profile.account.auth_status,"refreshing":profile.refreshing
                })).collect();
            print_json(&views)
        }
        AgentCommand::Run {
            project,
            profile,
            message,
        } => {
            start(
                remote,
                Selection::New { project, profile },
                message,
                cancelled,
            )
            .await
        }
        AgentCommand::Continue { run, message } => {
            start(remote, Selection::Continue(run), message, cancelled).await
        }
        AgentCommand::Recover { run, reviewed } => {
            if !reviewed {
                return Err(AppError::invalid(
                    "Inspect the project and saved activity first, then use --reviewed.",
                ));
            }
            let response: RunResponse = remote
                .request(
                    Method::POST,
                    &format!("adapter-runs/{}/recovery", segment(&run)),
                    Some(json!({})),
                )
                .await?;
            if response.run.history_error.is_some() || response.run.state != RunState::Interrupted {
                return Err(AppError::local(
                    "The recovery review could not be saved. Inspect the run before continuing.",
                ));
            }
            write_stderr(
                "Recovery review recorded. The previous outcome remains unknown; nothing was replayed.\n",
            )?;
            print_json(&response.run)
        }
        AgentCommand::Interrupt { run } => {
            let view = interrupt_and_confirm(remote, &run).await?;
            print_json(&view)
        }
    }
}

fn print_json(value: &impl Serialize) -> Result<()> {
    write_stdout(&format!("{}\n", format_json(value)?))
}

fn format_json(value: &impl Serialize) -> Result<String> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|_| AppError::local("Could not format agent information."))?;
    // Keep redirected output valid JSON while neutralizing terminal controls
    // and bidirectional overrides that JSON need not escape by default.
    Ok(text.chars().fold(String::new(), |mut output, character| {
        if (character.is_control() && !matches!(character, '\n' | '\t'))
            || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            use std::fmt::Write;
            let _ = write!(output, "\\u{:04x}", character as u32);
        } else {
            output.push(character);
        }
        output
    }))
}

async fn read_run(remote: &Remote, id: &str) -> Result<AdapterRun> {
    Ok(remote
        .request::<RunResponse>(Method::GET, &format!("adapter-runs/{}", segment(id)), None)
        .await?
        .run)
}

async fn read_events(remote: &Remote, id: &str, after: u64) -> Result<Vec<AdapterEvent>> {
    Ok(remote
        .request::<Events>(
            Method::GET,
            &format!(
                "adapter-runs/{}/events?after_seq={after}&limit=200",
                segment(id)
            ),
            None,
        )
        .await?
        .events)
}

async fn prepare(remote: &Remote, selection: Selection, message: &MessageArgs) -> Result<Value> {
    if message.prompt.trim().is_empty()
        || message.prompt.len() > 64 * 1024
        || message.prompt.contains('\0')
    {
        return Err(AppError::invalid(
            "Prompt must contain text and be at most 64 KiB without NUL characters.",
        ));
    }
    let (project, requested_profile, previous) = match selection {
        Selection::New { project, profile } => {
            let path = project
                .map_or_else(std::env::current_dir, Ok)
                .map_err(|_| AppError::local("Could not determine the project directory."))?
                .canonicalize()
                .map_err(|_| AppError::invalid("Project path must be an accessible directory."))?;
            let path = path
                .to_str()
                .ok_or_else(|| AppError::invalid("Project path must contain valid Unicode."))?;
            let project: ProjectResponse = remote
                .request(Method::POST, "projects", Some(json!({"path":path})))
                .await?;
            (project.project, profile, None)
        }
        Selection::Continue(id) => {
            let previous = read_run(remote, &id).await?;
            let projects: Projects = remote.request(Method::GET, "projects", None).await?;
            let project = projects
                .projects
                .into_iter()
                .find(|p| p.root == previous.project_path)
                .ok_or_else(|| {
                    AppError::invalid(
                        "The conversation's project is no longer registered in this workspace.",
                    )
                })?;
            let profile = previous.profile_id.ok_or_else(|| {
                AppError::invalid("This conversation has no saved account binding.")
            })?;
            (project, Some(profile), Some(id))
        }
    };
    let profiles: Profiles = remote
        .request(Method::GET, "account-profiles", None)
        .await?;
    let command_id = message
        .command_id
        .unwrap_or_else(uuid::Uuid::new_v4)
        .to_string();
    let runs: Runs = remote.request(Method::GET, "adapter-runs", None).await?;
    let accepted = runs.runs.iter().find(|run| run.command_id == command_id);
    let profile_id = requested_profile
        .or_else(|| accepted.and_then(|run| run.profile_id.clone()))
        .or(profiles.defaults.codex)
        .ok_or_else(|| {
            AppError::invalid(
                "Select a Codex account with --profile, or choose a default in the app.",
            )
        })?;
    let profile = profiles
        .profiles
        .iter()
        .find(|p| p.id == profile_id && p.agent_id == "codex")
        .ok_or_else(|| AppError::invalid("The selected Codex account profile is not available."))?;
    // The host performs the authoritative complete-request idempotency check.
    // An accepted retry must not be made dependent on a new auth inspection.
    if profile.managed && profile.account.auth_status != "signed_in" && accepted.is_none() {
        remote
            .request::<Value>(
                Method::POST,
                &format!("account-profiles/{}/refresh", segment(&profile_id)),
                Some(json!({})),
            )
            .await?;
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let profiles: Profiles = remote.request(Method::GET, "account-profiles", None).await?;
                let profile = profiles.profiles.iter().find(|p| p.id == profile_id)
                    .ok_or_else(|| AppError::invalid("The selected account profile is no longer available."))?;
                if !profile.refreshing {
                    return if profile.account.auth_status == "signed_in" { Ok(()) } else {
                        Err(AppError::invalid("Complete this managed profile's sign-in in the app before starting a run."))
                    };
                }
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        }).await.map_err(|_| AppError::local("The account status check did not finish; no run was submitted."))??;
    }
    Ok(
        json!({"adapter_id":"codex","project_id":project.id,"profile_id":profile_id,
        "prompt":message.prompt,"command_id":command_id,"continue_run_id":previous}),
    )
}

async fn start(
    remote: &Remote,
    selection: Selection,
    message: MessageArgs,
    cancelled: &CancellationToken,
) -> Result<()> {
    let body = tokio::select! {
        biased;
        () = cancelled.cancelled() => return Err(cancelled_before_start()),
        result = prepare(remote, selection, &message) => result?,
    };
    if cancelled.is_cancelled() {
        return Err(cancelled_before_start());
    }
    write_stderr(&format!("Command: {}\n", body["command_id"]))?;
    // Once dispatched, await the bounded response even if Ctrl-C arrives. A
    // known accepted run can then be interrupted; an uncertain POST is never retried.
    let response = remote
        .request::<RunResponse>(Method::POST, "adapter-runs", Some(body.clone()))
        .await;
    let run = match response {
        Ok(response) => response.run,
        Err(error) if matches!(error.code, "host_unreachable" | "invalid_response") => {
            return Err(AppError::new(
                StatusCode::BAD_GATEWAY,
                "submission_unconfirmed",
                format!(
                    "Start outcome is unknown for command {}. Read `switchya agent sessions` and inspect the matching run before submitting again. No automatic retry was made.",
                    body["command_id"],
                ),
            ));
        }
        Err(error) => return Err(error),
    };
    watch(remote, &run.id, &mut interactive_input(), cancelled).await
}

fn cancelled_before_start() -> AppError {
    AppError::new(
        StatusCode::CONFLICT,
        "interrupted",
        "Cancelled before a model run was submitted.",
    )
}

async fn watch(
    remote: &Remote,
    id: &str,
    input: &mut Input,
    cancelled: &CancellationToken,
) -> Result<()> {
    let result = tokio::select! {
        biased;
        () = cancelled.cancelled() => Err(AppError::new(StatusCode::CONFLICT, "interrupted", "Interruption requested.")),
        result = async {
            let run = read_run(remote, id).await?;
            write_stderr(&format!("Run: {}\nConversation: {}\nAccount: {}\n{}\n",
                terminal_text(&run.id), terminal_text(&run.conversation_id),
                terminal_text(run.profile_name.as_deref().unwrap_or("Unknown")), terminal_text(&run.permission_boundary)))?;
            drive(remote, id, input).await
        } => result,
    };
    if result.is_err() {
        match interrupt_and_confirm(remote, id).await {
            Ok(run) => {
                let _ = write_stderr(&format!(
                    "Run stopped in {:?}. Recorded effects remain.\n",
                    run.state
                ));
            }
            Err(error) => {
                let _ = write_stderr(&format!(
                    "Stop not confirmed: {} Inspect run {} before continuing.\n",
                    terminal_text(&error.message),
                    terminal_text(id)
                ));
            }
        }
    }
    result
}

async fn drive(remote: &Remote, id: &str, input: &mut Input) -> Result<()> {
    let mut after = 0;
    let mut streamed = HashMap::new();
    loop {
        let events = read_events(remote, id, after).await?;
        for event in &events {
            if event.run_id != id || event.seq <= after {
                return Err(AppError::local(
                    "The host returned inconsistent agent activity.",
                ));
            }
            if event.seq > after.saturating_add(1) {
                write_stderr("\nSome earlier activity was compacted or is no longer retained.\n")?;
            }
            after = event.seq;
            render_event(event, &mut streamed)?;
        }
        if events.len() == 200 {
            continue;
        }
        let view = read_run(remote, id).await?;
        if !view.state.is_active() {
            // The final state can arrive between the event and state reads.
            if view.last_seq > after {
                continue;
            }
            if view.history_error.is_some() {
                return Err(AppError::local(
                    "Part of this run's history could not be saved. Inspect the project before continuing.",
                ));
            }
            return if view.state == RunState::Completed {
                write_stderr("\nCompleted. Review changes and observed checks separately.\n")
            } else {
                Err(AppError::new(
                    StatusCode::CONFLICT,
                    "run_stopped",
                    format!(
                        "Run stopped in {:?}; inspect its saved activity and the project before continuing.",
                        view.state
                    ),
                ))
            };
        }
        if let Some(approval) = view.pending_approvals.first() {
            write_stderr(&format!(
                "\nApproval {}\n{}\n{}\n",
                terminal_text(&approval.id),
                terminal_text(&approval.permission_boundary),
                terminal_text(&serde_json::to_string_pretty(&approval.preview).unwrap_or_default())
            ))?;
            if input.is_none() {
                return Err(AppError::new(
                    StatusCode::CONFLICT,
                    "approval_required",
                    "This operation requires interactive review. It was not approved; interruption is being requested.",
                ));
            }
            if let Some(decision) = await_approval(remote, id, approval, input).await? {
                let result = remote.request::<Value>(Method::POST, &format!("adapter-runs/{}/decisions", segment(id)),
                    Some(json!({"approval_id":approval.id,"expected_hash":approval.expected_hash,"decision":decision}))).await;
                match result {
                    Ok(_) => {}
                    Err(error) if error.status == StatusCode::CONFLICT => {
                        write_stderr(
                            "\nThis approval changed in another client. Refreshing the run.\n",
                        )?;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
}

fn assistant_output(
    event: &AdapterEvent,
    streamed: &mut HashMap<String, String>,
) -> Option<String> {
    if event.kind == "assistant_delta" {
        let text = event.payload["text"].as_str()?;
        if let Some(id) = event.payload["item_id"].as_str() {
            streamed.entry(id.into()).or_default().push_str(text);
        }
        return Some(text.into());
    }
    let item = &event.payload["item"];
    if event.kind != "item_completed" || item["type"] != "agentMessage" {
        return None;
    }
    let text = item["text"].as_str()?;
    let id = item["id"].as_str();
    let output = match id.and_then(|id| streamed.get(id)) {
        // Saved history removes the fragments once the complete message arrives.
        // Some fragments may never have reached this polling client. Emit the
        // missing suffix rather than suppressing the entire final message.
        Some(delivered) => text
            .strip_prefix(delivered.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("\nFinal reply:\n{text}")),
        None => text.into(),
    };
    if let Some(id) = id {
        streamed.insert(id.into(), text.into());
    }
    Some(output)
}

fn render_event(event: &AdapterEvent, streamed: &mut HashMap<String, String>) -> Result<()> {
    if let Some(text) = assistant_output(event, streamed) {
        return write_stdout(&terminal_text(&text));
    }
    match event.kind.as_str() {
        "item_completed" => {
            let item = &event.payload["item"];
            if item["type"] != "agentMessage" {
                write_stderr(&format!(
                    "\n{}\n",
                    terminal_text(&serde_json::to_string_pretty(item).unwrap_or_default())
                ))?;
            }
        }
        "notice" | "adapter_error" => {
            if let Some(message) = event.payload["message"].as_str() {
                write_stderr(&format!("\n{}\n", terminal_text(message)))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn same_approval(view: &AdapterRun, approval: &AdapterApproval) -> bool {
    view.state.is_active()
        && view.pending_approvals.iter().any(|p| {
            p.id == approval.id
                && p.expected_hash == approval.expected_hash
                && p.can_allow_once == approval.can_allow_once
        })
}

fn approval_answer(answer: &str, approval: &AdapterApproval) -> Option<ApprovalDecision> {
    if answer.trim() == format!("deny {}", approval.id) {
        Some(ApprovalDecision::Deny)
    } else if approval.can_allow_once && answer.trim() == format!("allow_once {}", approval.id) {
        Some(ApprovalDecision::AllowOnce)
    } else {
        None
    }
}

async fn await_approval(
    remote: &Remote,
    id: &str,
    approval: &AdapterApproval,
    input: &mut Input,
) -> Result<Option<ApprovalDecision>> {
    let prompt = if approval.can_allow_once {
        format!(
            "Type allow_once {} or deny {}: ",
            terminal_text(&approval.id),
            terminal_text(&approval.id)
        )
    } else {
        format!(
            "This request can only be denied. Type deny {}: ",
            terminal_text(&approval.id)
        )
    };
    write_stderr(&prompt)?;
    if !same_approval(&read_run(remote, id).await?, approval) {
        return Ok(None);
    }
    let refresh = tokio::time::sleep(Duration::from_millis(200));
    tokio::pin!(refresh);
    loop {
        tokio::select! {
            biased;
            _ = &mut refresh => {
                let current = read_run(remote, id).await?;
                refresh.as_mut().reset(tokio::time::Instant::now() + Duration::from_millis(200));
                if !same_approval(&current, approval) { return Ok(None); }
            }
            answer = line(input) => {
                let answer = answer?.ok_or_else(|| AppError::new(StatusCode::CONFLICT, "approval_required", "Input ended without a decision. The operation was not approved."))?;
                if let Some(decision) = approval_answer(&answer, approval) { return Ok(Some(decision)); }
                write_stderr("The response must name this exact approval and a permitted decision.\n")?;
                write_stderr(&prompt)?;
            }
        }
    }
}

async fn interrupt_and_confirm(remote: &Remote, id: &str) -> Result<AdapterRun> {
    tokio::time::timeout(Duration::from_secs(8), async {
        let mut view = read_run(remote, id).await?;
        if view.state.is_active() {
            let result = remote.request::<Value>(Method::POST,
                &format!("adapter-runs/{}/interrupt", segment(id)), Some(json!({}))).await;
            if let Err(error) = result && error.status != StatusCode::CONFLICT { return Err(error); }
            loop {
                view = read_run(remote, id).await?;
                if !view.state.is_active() { break; }
                tokio::time::sleep(Duration::from_millis(120)).await;
            }
        }
        if view.state == RunState::RecoveryRequired {
            return Err(AppError::new(StatusCode::CONFLICT, "recovery_required", "Codex did not confirm the outcome. Review the project and acknowledge recovery explicitly."));
        }
        Ok(view)
    }).await.map_err(|_| AppError::new(StatusCode::CONFLICT, "interrupt_unconfirmed", "The host has not confirmed interruption."))?
}

#[cfg(test)]
mod tests;
