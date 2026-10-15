//! Terminal access to the app's existing agent adapter API. No second execution
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
    /// List saved agent runs, conversation IDs and their bound accounts.
    #[command(alias = "runs")]
    Sessions,
    /// Read one saved run and its retained activity without starting work.
    Show {
        #[arg(long)]
        run: String,
    },
    /// List cached account profile IDs; listing does not inspect credentials.
    Accounts,
    /// Create a managed account profile for Codex, Claude Code or Antigravity.
    Add {
        #[arg(long, default_value = "codex", value_parser = ["codex", "claude", "antigravity"])]
        agent: String,
        #[arg(long)]
        name: String,
    },
    /// Remove a managed account profile and delete its local sign-in data.
    Remove {
        #[arg(long)]
        profile: String,
    },
    /// Set an account profile as the default for its agent's future runs.
    Use {
        #[arg(long)]
        profile: String,
    },
    /// Sign in to a managed account profile through its official CLI flow.
    Login {
        #[arg(long)]
        profile: String,
        /// Codex: browser or device. Claude: claudeai, console or sso.
        #[arg(long, value_parser = ["browser", "device", "claudeai", "console", "sso"])]
        method: Option<String>,
    },
    /// Start a Codex, Claude Code or Antigravity conversation using its installed CLI.
    Run {
        #[arg(long, default_value = "codex", value_parser = ["codex", "claude", "antigravity"])]
        agent: String,
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
    /// Attach to a run already owned by the app or another terminal. Watching
    /// only reads activity and answers approvals; it never starts, continues or
    /// replays a model turn, and detaching leaves the agent running.
    Watch {
        #[arg(long)]
        run: String,
    },
}

/// Whether this process started the run it is reading. An owner stops the run
/// when its own command ends; a viewer only detaches.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Owner,
    Viewer,
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
    claude: Option<String>,
    #[serde(default)]
    antigravity: Option<String>,
}

impl ProfileDefaults {
    fn selected(&self, agent: &str) -> Option<&str> {
        match agent {
            "codex" => self.codex.as_deref(),
            "claude" => self.claude.as_deref(),
            "antigravity" => self.antigravity.as_deref(),
            _ => None,
        }
    }
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
        agent: String,
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
                .map(|profile| json!({
                    "id":profile.id,"name":profile.name,"agent_id":profile.agent_id,"managed":profile.managed,
                    "selected":profiles.defaults.selected(&profile.agent_id) == Some(profile.id.as_str()),
                    "cached_auth_status":profile.account.auth_status,"refreshing":profile.refreshing
                })).collect();
            print_json(&views)
        }
        AgentCommand::Add { agent, name } => {
            let result: Value = remote
                .request(
                    Method::POST,
                    "account-profiles",
                    Some(json!({ "agent_id": agent, "name": name })),
                )
                .await?;
            print_json(&result)
        }
        AgentCommand::Remove { profile } => {
            let result: Value = remote
                .request(
                    Method::POST,
                    &format!("account-profiles/{}/remove", segment(&profile)),
                    Some(json!({})),
                )
                .await?;
            print_json(&result)
        }
        AgentCommand::Use { profile } => {
            let result: Value = remote
                .request(
                    Method::POST,
                    &format!("account-profiles/{}/select", segment(&profile)),
                    Some(json!({})),
                )
                .await?;
            print_json(&result)
        }
        AgentCommand::Login { profile, method } => {
            login(remote, &profile, method.as_deref(), cancelled).await
        }
        AgentCommand::Run {
            agent,
            project,
            profile,
            message,
        } => {
            start(
                remote,
                Selection::New {
                    agent,
                    project,
                    profile,
                },
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
        AgentCommand::Watch { run } => attach(remote, &run, cancelled).await,
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

/// Attempts for one read-only request, including the first.
const READ_ATTEMPTS: usize = 3;

/// A transport failure or an interrupted/undecodable response. The request's
/// outcome carried no information, so repeating the same read changes nothing
/// on the host. Retrying reveals nothing about why a read failed.
fn read_retryable(error: &AppError) -> bool {
    matches!(error.code, "host_unreachable" | "invalid_response")
}

/// Repeat one read-only GET a bounded number of times. Only run and event reads
/// use this: starts, continuations, decisions, recovery and interruption are
/// never repeated, because their outcome may be unknown rather than absent.
async fn read_with_retry<T, R, F>(read: R) -> Result<T>
where
    R: Fn() -> F,
    F: std::future::Future<Output = Result<T>>,
{
    for attempt in 1..READ_ATTEMPTS {
        match read().await {
            Err(error) if read_retryable(&error) => {
                let next_attempt = attempt + 1;
                let _ = write_stderr(&format!(
                    "\nA read from the app host did not complete ({}). Retrying read {next_attempt} of {READ_ATTEMPTS}.\n",
                    terminal_text(&error.message)
                ));
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            result => return result,
        }
    }
    read().await
}

async fn read_run(remote: &Remote, id: &str) -> Result<AdapterRun> {
    read_with_retry(|| async {
        Ok(remote
            .request::<RunResponse>(Method::GET, &format!("adapter-runs/{}", segment(id)), None)
            .await?
            .run)
    })
    .await
}

async fn read_events(remote: &Remote, id: &str, after: u64) -> Result<Vec<AdapterEvent>> {
    read_with_retry(|| async {
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
    })
    .await
}

#[derive(Clone, Deserialize, Serialize)]
struct LoginView {
    status: String,
    auth_url: Option<String>,
    user_code: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct LoginResponse {
    login: Option<LoginView>,
}

/// Starts one official sign-in and follows its cached state. The URL and
/// device code are printed as soon as the CLI provides them; cancelling this
/// command detaches from polling without cancelling the sign-in itself.
async fn login(
    remote: &Remote,
    profile: &str,
    method: Option<&str>,
    cancelled: &CancellationToken,
) -> Result<()> {
    let body = match method {
        Some(method) => json!({ "method": method }),
        None => json!({}),
    };
    let started: LoginResponse = remote
        .request(
            Method::POST,
            &format!("account-profiles/{}/login", segment(profile)),
            Some(body),
        )
        .await?;
    let mut announced = false;
    let mut current = started.login;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(11 * 60);
    loop {
        let Some(job) = &current else {
            return Err(AppError::local("The host returned no sign-in state."));
        };
        if !announced && (job.auth_url.is_some() || job.user_code.is_some()) {
            announced = true;
            if let Some(url) = &job.auth_url {
                write_stdout(&format!("Sign in at: {url}\n"))?;
            }
            if let Some(code) = &job.user_code {
                write_stdout(&format!("Device code: {code}\n"))?;
            }
        }
        if !matches!(job.status.as_str(), "starting" | "awaiting_user") {
            return print_json(&job);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(AppError::local(
                "Sign-in is still pending. Check its status in the app's Accounts view.",
            ));
        }
        tokio::select! {
            () = cancelled.cancelled() => {
                return Err(AppError::local(
                    "Stopped watching this sign-in. It continues in the host; check Accounts in the app.",
                ));
            }
            () = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
        let next: LoginResponse = remote
            .request(
                Method::GET,
                &format!("account-profiles/{}/login", segment(profile)),
                None,
            )
            .await?;
        current = next.login;
    }
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
    let (agent, project, requested_profile, previous) = match selection {
        Selection::New {
            agent,
            project,
            profile,
        } => {
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
            (agent, project.project, profile, None)
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
            (previous.adapter_id, project, Some(profile), Some(id))
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
        .or_else(|| profiles.defaults.selected(&agent).map(str::to_owned))
        .ok_or_else(|| {
            AppError::invalid(
                "Select an account for this agent with --profile, or choose its default in the app.",
            )
        })?;
    let profile = profiles
        .profiles
        .iter()
        .find(|p| p.id == profile_id && p.agent_id == agent)
        .ok_or_else(|| {
            AppError::invalid("The selected account profile is not available for this agent.")
        })?;
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
        json!({"adapter_id":agent,"project_id":project.id,"profile_id":profile_id,
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
            drive(remote, id, input, Role::Owner).await
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

/// What a viewer can do after detaching. It never acts on the run itself.
fn viewer_guidance(id: &str) -> String {
    let id = terminal_text(id);
    format!(
        "\nDetaching does not interrupt the run or undo earlier decisions. Reuse the same private --launch-info file to reach this host. In the commands below, replace PATH with the --launch-info path used for this viewer:\n  Inspect: switchya --launch-info PATH agent show --run {id}\n  Reattach: switchya --launch-info PATH agent watch --run {id}\n  Stop explicitly: switchya --launch-info PATH agent interrupt --run {id}\n"
    )
}

/// Read an existing run's activity without owning it. The run keeps its own
/// account binding, and no request here starts, continues or replays work.
async fn attach(remote: &Remote, id: &str, cancelled: &CancellationToken) -> Result<()> {
    view(remote, id, &mut interactive_input(), cancelled).await
}

async fn view(
    remote: &Remote,
    id: &str,
    input: &mut Input,
    cancelled: &CancellationToken,
) -> Result<()> {
    let result = tokio::select! {
        biased;
        () = cancelled.cancelled() => Err(AppError::new(StatusCode::CONFLICT, "detached",
            "Detached from this run. This viewer did not interrupt the agent.")),
        result = async {
            // The first read may stall just like later reads. Cancellation must
            // cover the whole attachment, including metadata and read retries.
            let run = read_run(remote, id).await?;
            write_stderr(&format!(
                "Run: {}\nConversation: {}\nAccount: {}\nState: {:?}\n{}\nAttached as a viewer. Ctrl-C detaches without interrupting the agent.\n",
                terminal_text(&run.id),
                terminal_text(&run.conversation_id),
                terminal_text(run.profile_name.as_deref().unwrap_or("Unknown")),
                run.state,
                terminal_text(&run.permission_boundary),
            ))?;
            drive(remote, id, input, Role::Viewer).await
        } => result,
    };
    // Losing this viewer must not act on the run, so nothing is interrupted here.
    if let Err(error) = &result
        && error.code != "run_stopped"
    {
        let _ = write_stderr(&viewer_guidance(id));
    }
    result
}

async fn drive(remote: &Remote, id: &str, input: &mut Input, role: Role) -> Result<()> {
    let mut after = 0;
    let mut streamed = HashMap::new();
    loop {
        let events = read_events(remote, id, after).await?;
        let view = read_run(remote, id).await?;
        if history_was_trimmed(after, &events, view.first_retained_seq) {
            write_stderr("\nSome earlier activity is no longer retained.\n")?;
        }
        for event in &events {
            if event.run_id != id || event.seq <= after {
                return Err(AppError::local(
                    "The host returned inconsistent agent activity.",
                ));
            }
            after = event.seq;
            render_event(event, &mut streamed)?;
        }
        if events.len() == 200 {
            continue;
        }
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
                // A viewer leaves the request pending for the app or another
                // terminal; an owner stops the run it started.
                return Err(AppError::new(
                    StatusCode::CONFLICT,
                    "approval_required",
                    match role {
                        Role::Owner => {
                            "This operation requires interactive review. It was not approved; interruption is being requested."
                        }
                        Role::Viewer => {
                            "This operation requires interactive review. It is still pending: nothing was approved, denied or interrupted."
                        }
                    },
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

fn history_was_trimmed(after: u64, events: &[AdapterEvent], first_retained_seq: u64) -> bool {
    // Completed messages replace their streamed deltas, leaving normal sequence
    // gaps. Only a missing prefix beyond the recorded retention boundary means
    // the viewer could have lost activity; compaction keeps the final reply.
    let next = after.saturating_add(1);
    first_retained_seq > next && events.first().is_some_and(|event| event.seq > next)
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

/// Fields that only identify an item. An item carrying nothing else announces
/// its own existence and has no activity to report. A reported failure or
/// decline is an outcome, so it is always rendered.
fn lifecycle_only(item: &Value) -> bool {
    if !matches!(
        item["status"].as_str(),
        None | Some("" | "inProgress" | "completed")
    ) {
        return false;
    }
    item.as_object().is_some_and(|fields| {
        fields.iter().all(|(key, value)| {
            matches!(key.as_str(), "id" | "type" | "status")
                || value.is_null()
                || value.as_str() == Some("")
                || value.as_array().is_some_and(|items| items.is_empty())
                || value.as_object().is_some_and(serde_json::Map::is_empty)
        })
    })
}

fn humanize(value: &str) -> String {
    let mut words = String::new();
    for character in value.chars() {
        if character.is_uppercase() && words.ends_with(|last: char| last.is_alphanumeric()) {
            words.push(' ');
        }
        words.extend(character.to_lowercase());
    }
    let mut characters = words.trim().chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => "Activity".to_owned(),
    }
}

fn status_label(item: &Value) -> &str {
    match item["status"].as_str() {
        Some("inProgress") => "running",
        Some("") | None => "status not reported",
        Some(other) => other,
    }
}

const OUTPUT_LINES: usize = 40;
const OUTPUT_CHARACTERS: usize = 4000;
const HEADLINE_LINES: usize = 3;
const HEADLINE_CHARACTERS: usize = 400;

fn shortened_note(what: &str, run_id: &str) -> String {
    format!("\n  … {what} shortened; `switchya agent show --run {run_id}` has the exact event.")
}

/// The start of a reported command or tool input, bounded for display. A single
/// real write command can be tens of thousands of characters.
fn bounded_headline(text: &str) -> (String, bool) {
    let lines: Vec<&str> = text.lines().take(HEADLINE_LINES + 1).collect();
    let mut shortened = lines.len() > HEADLINE_LINES;
    let mut headline = lines[..lines.len().min(HEADLINE_LINES)].join("\n");
    if headline.chars().count() > HEADLINE_CHARACTERS {
        headline = headline.chars().take(HEADLINE_CHARACTERS).collect();
        shortened = true;
    }
    (headline, shortened)
}

/// Reported output with its real line breaks, bounded to a readable tail. The
/// exact event stays available through `switchya agent show`.
fn bounded_output(text: &str) -> Option<(String, bool)> {
    let text = text.trim_end_matches(['\n', '\r']);
    if text.trim().is_empty() {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    let kept = lines.len().min(OUTPUT_LINES);
    let mut output = lines[lines.len() - kept..].join("\n");
    let mut elided = kept < lines.len();
    let characters = output.chars().count();
    if characters > OUTPUT_CHARACTERS {
        output = output
            .chars()
            .skip(characters - OUTPUT_CHARACTERS)
            .collect();
        elided = true;
    }
    Some((output, elided))
}

fn describe_change(change: &Value) -> String {
    let label = match change["kind"]["type"].as_str() {
        Some("add") => "new file",
        Some("delete") => "delete",
        Some("update") => "edit",
        _ => "change",
    };
    let path = change["path"].as_str().unwrap_or("path not reported");
    match change["kind"]["move_path"].as_str() {
        Some(moved) if !moved.is_empty() => format!("  {label} {path} → {moved}"),
        _ => format!("  {label} {path}"),
    }
}

/// Concise readable status for one completed item, or `None` when the item only
/// reports its own lifecycle. Exact data stays in `switchya agent show`.
fn item_summary(item: &Value, run_id: &str) -> Option<String> {
    if lifecycle_only(item) {
        return None;
    }
    let status = status_label(item);
    Some(match item["type"].as_str().unwrap_or_default() {
        "commandExecution" => {
            let command = item["command"]
                .as_str()
                .filter(|command| !command.is_empty())
                .map(bounded_headline);
            let mut text = match &command {
                Some((command, _)) => format!("$ {command}"),
                None => "$ command not reported".to_owned(),
            };
            if let Some((_, true)) = command {
                text.push_str(&shortened_note("command", run_id));
            }
            if let Some(cwd) = item["cwd"].as_str().filter(|cwd| !cwd.is_empty()) {
                text.push_str(&format!("\n  in {cwd}"));
            }
            match item["exitCode"].as_i64() {
                Some(code) => text.push_str(&format!("\n  {status}, exit code {code}")),
                None => text.push_str(&format!("\n  {status}, exit code not reported")),
            }
            append_output(&mut text, item["aggregatedOutput"].as_str(), run_id);
            text
        }
        "fileChange" => {
            let changes = item["changes"].as_array().map_or(&[][..], Vec::as_slice);
            let mut text = match changes.len() {
                0 => format!("File changes ({status}), files not reported"),
                1 => format!("Edited 1 file ({status})"),
                count => format!("Edited {count} files ({status})"),
            };
            for change in changes.iter().take(OUTPUT_LINES) {
                text.push('\n');
                text.push_str(&describe_change(change));
            }
            if changes.len() > OUTPUT_LINES {
                text.push_str(&format!(
                    "\n  … {} more files; `switchya agent show --run {run_id}` lists every change.",
                    changes.len() - OUTPUT_LINES
                ));
            }
            text
        }
        "claudeTool" => {
            let input = &item["input"];
            let headline = ["file_path", "notebook_path", "command", "pattern"]
                .iter()
                .find_map(|field| input[field].as_str())
                .filter(|value| !value.is_empty())
                .map(bounded_headline);
            let name = item["name"].as_str().unwrap_or("Tool");
            let mut text = match &headline {
                Some((headline, _)) => format!("{name} {headline} ({status})"),
                None => format!("{name} input not reported ({status})"),
            };
            if let Some((_, true)) = headline {
                text.push_str(&shortened_note("tool input", run_id));
            }
            append_output(&mut text, item["output"].as_str(), run_id);
            text
        }
        kind => {
            // Unknown items stay visible: their identity and remaining fields, bounded.
            let mut text = match item["id"].as_str().filter(|id| !id.is_empty()) {
                Some(id) => format!("{} ({status}), item {id}", humanize(kind)),
                None => format!("{} ({status})", humanize(kind)),
            };
            let details: serde_json::Map<_, _> = item
                .as_object()
                .map(|fields| {
                    fields
                        .iter()
                        .filter(|(key, _)| !matches!(key.as_str(), "id" | "type" | "status"))
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect()
                })
                .unwrap_or_default();
            if !details.is_empty() {
                append_output(
                    &mut text,
                    serde_json::to_string(&Value::Object(details))
                        .ok()
                        .as_deref(),
                    run_id,
                );
            }
            text
        }
    })
}

fn append_output(text: &mut String, output: Option<&str>, run_id: &str) {
    let Some(reported) = output else {
        text.push_str("\n  output not reported");
        return;
    };
    let Some((output, elided)) = bounded_output(reported) else {
        text.push_str("\n  no output");
        return;
    };
    text.push('\n');
    text.push_str(&output);
    if elided {
        text.push_str(&shortened_note("output", run_id));
    }
}

fn render_event(event: &AdapterEvent, streamed: &mut HashMap<String, String>) -> Result<()> {
    if let Some(text) = assistant_output(event, streamed) {
        return write_stdout(&terminal_text(&text));
    }
    match event.kind.as_str() {
        "item_completed" => {
            let item = &event.payload["item"];
            if item["type"] != "agentMessage"
                && let Some(summary) = item_summary(item, &event.run_id)
            {
                write_stderr(&format!("\n{}\n", terminal_text(&summary)))?;
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
            return Err(AppError::new(StatusCode::CONFLICT, "recovery_required", "The agent did not confirm the outcome. Review the project and acknowledge recovery explicitly."));
        }
        Ok(view)
    }).await.map_err(|_| AppError::new(StatusCode::CONFLICT, "interrupt_unconfirmed", "The host has not confirmed interruption."))?
}

#[cfg(test)]
mod tests;
