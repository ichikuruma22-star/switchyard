//! Terminal entry point for the shared local workspace engine.
mod adapters;
mod mcp;

use crate::{
    AppError, AppOptions, LaunchInfo, Result, default_paths, start_host, startup::AppRuntime,
};
use axum::http::StatusCode;
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;
use switchyard_agent::{
    AutoApprove, Decision, EffectiveAutoApprove, Event, PolicySource, Project, Run, Session,
    SessionState, SessionView,
};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "switchya", version, about = "Switchya local agent workspace")]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    client_key_name: Option<String>,
    /// Use an active host through its explicitly exported private launch file.
    #[arg(long, global = true)]
    launch_info: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Share one project's context and handoffs with an MCP client over stdio.
    Mcp {
        /// Only this project is accessible to the connected agent.
        #[arg(long)]
        project: PathBuf,
    },
    /// Open the local browser workspace (also the default command).
    Serve(ServeArgs),
    /// Use saved Codex, Claude Code or Antigravity conversations through the app's host.
    Agent {
        #[command(subcommand)]
        command: adapters::AgentCommand,
    },
    /// Start or continue a session interactively.
    Chat(SessionArgs),
    /// Submit one prompt. Noninteractive runs never approve file writes or
    /// commands themselves; only an opted-in auto-approve setting does.
    Run {
        #[command(flatten)]
        session: SessionArgs,
        #[arg(long)]
        prompt: String,
    },
    /// Show or change a project's auto-approval of edits and commands (off by default).
    Trust {
        /// Project folder; defaults to the current directory.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Apply file edits without asking. Protected paths stay blocked.
        #[arg(long, value_enum)]
        edits: Option<Toggle>,
        /// Run shell commands without asking, with your full OS permissions.
        #[arg(long, value_enum)]
        commands: Option<Toggle>,
    },
    /// List the models actually available to the selected gateway client key.
    Models,
    /// List durable sessions and IDs for resuming browser- or CLI-created work.
    Sessions,
    /// Record manual inspection of an uncertain operation; never replays it.
    Recover {
        #[arg(long)]
        session: String,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        note: String,
    },
}

#[derive(Args, Default)]
struct ServeArgs {
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// Export credentials to a NEW private launch file for another terminal.
    #[arg(long)]
    write_launch_info: Option<PathBuf>,
}

#[derive(Args)]
struct SessionArgs {
    #[arg(long, conflicts_with = "session")]
    project: Option<PathBuf>,
    #[arg(long, conflicts_with = "session")]
    model: Option<String>,
    #[arg(long)]
    session: Option<String>,
    /// Save this session's approval setting: auto-approve `edits`, `commands`
    /// or `all`; `off` asks for every edit and command; `inherit` follows the
    /// project's `switchya trust` setting.
    #[arg(long, value_enum)]
    auto_approve: Option<AutoApproveArg>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Toggle {
    On,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum AutoApproveArg {
    Edits,
    Commands,
    All,
    Off,
    Inherit,
}

impl AutoApproveArg {
    /// `None` clears the session override.
    fn policy(self) -> Option<AutoApprove> {
        let (edits, commands) = match self {
            Self::Edits => (true, false),
            Self::Commands => (false, true),
            Self::All => (true, true),
            Self::Off => (false, false),
            Self::Inherit => return None,
        };
        Some(AutoApprove { edits, commands })
    }
}

fn describe_auto_approve(policy: AutoApprove) -> &'static str {
    match (policy.edits, policy.commands) {
        (false, false) => "off; every edit and command needs your decision",
        (true, false) => "file edits apply without asking; commands need your decision",
        (false, true) => "commands run without asking; file edits need your decision",
        (true, true) => "file edits and commands proceed without asking",
    }
}

fn auto_approve_line(effective: &EffectiveAutoApprove) -> String {
    let source = match effective.source {
        PolicySource::Project => "project setting",
        PolicySource::Session => "this session's setting",
    };
    let mut line = format!(
        "Auto-approve ({source}): {}.",
        describe_auto_approve(effective.policy())
    );
    if effective.commands {
        line.push_str(" Commands run with your full OS permissions; this is not a sandbox.");
    }
    if effective.edits || effective.commands {
        line.push_str(" Every operation is still recorded.");
    }
    line
}

pub async fn run() -> ExitCode {
    let cli = Cli::parse();
    let (cancelled, listener) = match cancellation_listener().await {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "Switchya: {}", error.message);
            return ExitCode::FAILURE;
        }
    };
    let result = execute(cli, &cancelled).await;
    listener.abort();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(
                std::io::stderr(),
                "Switchya: {}",
                terminal_text(&error.message)
            );
            ExitCode::FAILURE
        }
    }
}

async fn cancellation_listener() -> Result<(CancellationToken, tokio::task::JoinHandle<()>)> {
    let cancelled = CancellationToken::new();
    let signal_cancelled = cancelled.clone();
    let (ready, registered) = tokio::sync::oneshot::channel();
    let listener = tokio::spawn(async move {
        let signal = tokio::signal::ctrl_c();
        tokio::pin!(signal);
        tokio::select! {
            biased;
            result = &mut signal => { if result.is_ok() { signal_cancelled.cancel(); } },
            () = async { let _ = ready.send(()); std::future::pending::<()>().await } => {},
        }
    });
    // Ctrl-C is polled and registered before the readiness branch can run.
    registered
        .await
        .map_err(|_| AppError::local("Could not register Ctrl-C handling."))?;
    Ok((cancelled, listener))
}

fn options(cli: &Cli) -> Result<AppOptions> {
    let defaults = if cli.config.is_none() || cli.data_dir.is_none() {
        Some(default_paths()?)
    } else {
        None
    };
    let config = cli
        .config
        .clone()
        .or_else(|| defaults.as_ref().map(|paths| paths.0.clone()))
        .unwrap();
    let data = cli
        .data_dir
        .clone()
        .or_else(|| defaults.as_ref().map(|paths| paths.1.clone()))
        .unwrap();
    let mut options = AppOptions::new(config, data);
    options.client_key_name = cli.client_key_name.clone();
    Ok(options)
}

async fn execute(mut cli: Cli, cancelled: &CancellationToken) -> Result<()> {
    let command = cli
        .command
        .take()
        .unwrap_or(Command::Serve(ServeArgs::default()));
    if let Command::Agent { command } = command {
        return adapters::execute(&cli, command, cancelled).await;
    }
    if let Command::Mcp { project } = command {
        return mcp::execute(&cli, project, cancelled).await;
    }
    if let Command::Serve(args) = command {
        if cli.launch_info.is_some() {
            return Err(AppError::invalid(
                "--launch-info connects chat/run to an existing host; it cannot start a host.",
            ));
        }
        let mut options = options(&cli)?;
        options.port = args.port;
        let host = start_host(options).await?;
        let result = async {
            if let Some(path) = args.write_launch_info { host.write_launch_info(path)?; }
            write_stderr(&format!("Switchya is listening at {}. Press Ctrl-C to stop.\n", host.base_url()))?;
            if std::io::stdout().is_terminal() { write_stdout(&format!("{}\n", host.launch_url()))?; }
            else { write_stderr("The credential-bearing launch URL is hidden from redirected output. Use --write-launch-info with a new file to connect.\n")?; }
            cancelled.cancelled().await;
            Ok(())
        }.await;
        let shutdown = host.shutdown().await;
        return result.and(shutdown);
    }
    let backend = if let Some(path) = &cli.launch_info {
        if cli.config.is_some() || cli.data_dir.is_some() || cli.client_key_name.is_some() {
            return Err(AppError::invalid(
                "--launch-info uses the active host's configuration; omit local configuration flags.",
            ));
        }
        Backend::Remote(Remote::new(LaunchInfo::read(path)?)?)
    } else {
        Backend::Direct(AppRuntime::start(&options(&cli)?).await?)
    };
    let result = match command {
        Command::Models => {
            let models = backend.models().await;
            models.and_then(|models| write_stdout(&format!("{}\n", terminal_text(&serde_json::to_string_pretty(&models).map_err(|_| AppError::local("Could not format model information."))?))))
        }
        Command::Sessions => backend.sessions().await.and_then(|sessions| write_stdout(&format!("{}\n", terminal_text(&serde_json::to_string_pretty(&sessions).map_err(|_| AppError::local("Could not format session information."))?)))),
        Command::Recover { session, expected_revision, note } => backend.recover(&session, expected_revision, &note).await.and_then(|session| write_stderr(&format!("Recovery review recorded for {} at revision {}. Unknown effects remain unknown; a new turn can now be submitted.\n", terminal_text(&session.id), session.revision))),
        Command::Trust { project, edits, commands } => trust(&backend, project, edits, commands).await,
        Command::Chat(args) => chat(&backend, args, cancelled).await,
        Command::Run { session, prompt } => async {
            let view = select_session(&backend, session).await?;
            write_stderr(&format!("{}\n", auto_approve_line(&view.auto_approve)))?;
            let mut input = interactive_input();
            turn(&backend, &view.session.id, &prompt, &mut input, cancelled).await
        }.await,
        Command::Serve(_) | Command::Agent { .. } | Command::Mcp { .. } => unreachable!(),
    };
    let shutdown = backend.shutdown().await;
    result.and(shutdown)
}

enum Backend {
    Direct(AppRuntime),
    Remote(Remote),
}
struct Remote {
    client: reqwest::Client,
    launch: LaunchInfo,
}
#[derive(Deserialize)]
struct Models {
    models: Vec<Value>,
}
#[derive(Deserialize)]
struct Sessions {
    sessions: Vec<Session>,
}
#[derive(Deserialize)]
struct ProjectResponse {
    project: Project,
}
#[derive(Deserialize)]
struct SessionResponse {
    session: Session,
}
#[derive(Deserialize)]
struct RunResponse {
    run: Run,
}
#[derive(Deserialize)]
struct Events {
    events: Vec<Event>,
}

impl Remote {
    fn new(launch: LaunchInfo) -> Result<Self> {
        launch.validate()?;
        crate::initialize_tls();
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| AppError::local("Could not create the local host client."))?;
        Ok(Self { client, launch })
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        let mut request = self
            .client
            .request(method, format!("{}/api/{path}", self.launch.base_url))
            .bearer_auth(&self.launch.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request.send().await.map_err(|_| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "host_unreachable",
                "Could not contact the active app host. Check that it is still running.",
            )
        })?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "invalid_response",
                "The active host response was interrupted.",
            )
        })? {
            if bytes.len().saturating_add(chunk.len()) > 8 * 1024 * 1024 {
                return Err(AppError::local(
                    "The active host response exceeded the 8 MiB limit.",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            let message = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|value| value["error"]["message"].as_str().map(str::to_owned))
                .unwrap_or_else(|| {
                    format!(
                        "The active host rejected the request (HTTP {}).",
                        status.as_u16()
                    )
                })
                .replace(&self.launch.token, "[redacted]");
            return Err(AppError::new(status, "host_rejected", message));
        }
        serde_json::from_slice(&bytes).map_err(|_| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "invalid_response",
                "The active host returned an invalid response.",
            )
        })
    }
}

fn segment(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

impl Backend {
    async fn sessions(&self) -> Result<Vec<Session>> {
        match self {
            Self::Direct(runtime) => Ok(runtime.engine.list_sessions(None)?),
            Self::Remote(remote) => Ok(remote
                .request::<Sessions>(reqwest::Method::GET, "sessions", None)
                .await?
                .sessions),
        }
    }
    async fn models(&self) -> Result<Vec<Value>> {
        match self {
            Self::Direct(runtime) => Ok(runtime.engine.models()?),
            Self::Remote(remote) => Ok(remote
                .request::<Models>(reqwest::Method::GET, "models", None)
                .await?
                .models),
        }
    }
    async fn project(&self, path: &Path) -> Result<Project> {
        match self {
            Self::Direct(runtime) => Ok(runtime.engine.open_project(path)?),
            Self::Remote(remote) => {
                let path = path
                    .to_str()
                    .ok_or_else(|| AppError::invalid("Project path must contain valid Unicode."))?;
                Ok(remote
                    .request::<ProjectResponse>(
                        reqwest::Method::POST,
                        "projects",
                        Some(json!({"path":path})),
                    )
                    .await?
                    .project)
            }
        }
    }
    async fn create(&self, project_id: &str, model: &str) -> Result<Session> {
        match self {
            Self::Direct(runtime) => Ok(runtime.engine.create_session(project_id, model)?),
            Self::Remote(remote) => Ok(remote
                .request::<SessionResponse>(
                    reqwest::Method::POST,
                    "sessions",
                    Some(json!({"project_id":project_id,"model":model})),
                )
                .await?
                .session),
        }
    }
    async fn session(&self, id: &str) -> Result<SessionView> {
        match self {
            Self::Direct(runtime) => Ok(runtime.engine.session(id)?),
            Self::Remote(remote) => {
                remote
                    .request(
                        reqwest::Method::GET,
                        &format!("sessions/{}", segment(id)),
                        None,
                    )
                    .await
            }
        }
    }
    async fn events(&self, id: &str, after: u64) -> Result<Vec<Event>> {
        match self {
            Self::Direct(runtime) => Ok(runtime.engine.events(id, after, 200)?),
            Self::Remote(remote) => Ok(remote
                .request::<Events>(
                    reqwest::Method::GET,
                    &format!(
                        "sessions/{}/events?after_seq={after}&limit=200",
                        segment(id)
                    ),
                    None,
                )
                .await?
                .events),
        }
    }
    async fn submit(
        &self,
        id: &str,
        command_id: &str,
        text: &str,
        cancelled: &CancellationToken,
    ) -> Result<Run> {
        match self {
            Self::Direct(runtime) => Ok(runtime.engine.submit_turn(id, command_id, text)?),
            Self::Remote(remote) => {
                let path = format!("sessions/{}/turns", segment(id));
                let input = json!({"command_id":command_id,"text":text});
                let first = remote
                    .request::<RunResponse>(reqwest::Method::POST, &path, Some(input.clone()))
                    .await;
                let response = match first {
                    Err(error)
                        if matches!(error.code, "host_unreachable" | "invalid_response")
                            && !cancelled.is_cancelled() =>
                    {
                        remote
                            .request::<RunResponse>(reqwest::Method::POST, &path, Some(input))
                            .await?
                    }
                    result => result?,
                };
                Ok(response.run)
            }
        }
    }
    async fn decide(
        &self,
        id: &str,
        operation: &switchyard_agent::Operation,
        decision: Decision,
    ) -> Result<()> {
        match self {
            Self::Direct(runtime) => Ok(runtime.engine.decide_operation(
                id,
                &operation.id,
                &operation.arguments_hash,
                decision,
            )?),
            Self::Remote(remote) => {
                remote
                    .request::<Value>(
                        reqwest::Method::POST,
                        &format!(
                            "sessions/{}/operations/{}/decision",
                            segment(id),
                            segment(&operation.id)
                        ),
                        Some(json!({"expected_hash":operation.arguments_hash,"decision":decision})),
                    )
                    .await?;
                Ok(())
            }
        }
    }
    async fn interrupt(&self, id: &str, run_id: &str) -> Result<()> {
        match self {
            Self::Direct(runtime) => Ok(runtime.engine.interrupt(id, run_id)?),
            Self::Remote(remote) => {
                remote
                    .request::<Value>(
                        reqwest::Method::POST,
                        &format!("sessions/{}/interrupt", segment(id)),
                        Some(json!({"run_id":run_id})),
                    )
                    .await?;
                Ok(())
            }
        }
    }
    async fn recover(&self, id: &str, revision: u64, note: &str) -> Result<Session> {
        match self {
            Self::Direct(runtime) => Ok(runtime.engine.acknowledge_recovery(id, revision, note)?),
            Self::Remote(remote) => Ok(remote
                .request::<SessionResponse>(
                    reqwest::Method::POST,
                    &format!("sessions/{}/recovery", segment(id)),
                    Some(json!({"expected_revision":revision,"note":note})),
                )
                .await?
                .session),
        }
    }
    async fn set_project_auto_approve(
        &self,
        project_id: &str,
        policy: AutoApprove,
    ) -> Result<Project> {
        match self {
            Self::Direct(runtime) => Ok(runtime
                .engine
                .set_project_auto_approve(project_id, policy)?),
            Self::Remote(remote) => Ok(remote
                .request::<ProjectResponse>(
                    reqwest::Method::POST,
                    &format!("projects/{}/auto-approve", segment(project_id)),
                    Some(json!({"edits":policy.edits,"commands":policy.commands})),
                )
                .await?
                .project),
        }
    }
    async fn set_session_auto_approve(
        &self,
        session_id: &str,
        policy: Option<AutoApprove>,
    ) -> Result<Session> {
        match self {
            Self::Direct(runtime) => Ok(runtime
                .engine
                .set_session_auto_approve(session_id, policy)?),
            Self::Remote(remote) => Ok(remote
                .request::<SessionResponse>(
                    reqwest::Method::POST,
                    &format!("sessions/{}/auto-approve", segment(session_id)),
                    Some(json!({ "policy": policy })),
                )
                .await?
                .session),
        }
    }
    async fn shutdown(&self) -> Result<()> {
        match self {
            Self::Direct(runtime) => runtime.shutdown().await,
            Self::Remote(_) => Ok(()),
        }
    }
}

async fn select_session(backend: &Backend, args: SessionArgs) -> Result<SessionView> {
    let auto_approve = args.auto_approve;
    let view = open_session(backend, args).await?;
    let Some(choice) = auto_approve else {
        return Ok(view);
    };
    backend
        .set_session_auto_approve(&view.session.id, choice.policy())
        .await?;
    backend.session(&view.session.id).await
}

async fn trust(
    backend: &Backend,
    project: Option<PathBuf>,
    edits: Option<Toggle>,
    commands: Option<Toggle>,
) -> Result<()> {
    let path = project_path(project)?;
    let mut project = backend.project(&path).await?;
    if edits.is_some() || commands.is_some() {
        let policy = AutoApprove {
            edits: edits.map_or(project.auto_approve.edits, |t| t == Toggle::On),
            commands: commands.map_or(project.auto_approve.commands, |t| t == Toggle::On),
        };
        project = backend
            .set_project_auto_approve(&project.id, policy)
            .await?;
    }
    write_stdout(&trust_report(&project))
}

fn trust_report(project: &Project) -> String {
    let policy = project.auto_approve;
    let mut text = format!(
        "Project: {}\nAuto-approve: {}.\n",
        terminal_text(&project.root),
        describe_auto_approve(policy)
    );
    if policy.commands {
        text.push_str("Commands run with your account's full OS and network permissions. The project folder is not a sandbox.\n");
    }
    if policy.any() {
        text.push_str("Protected paths (repository metadata, likely secrets, links, short-name aliases) stay blocked. Edits outside the project, to Switchya's own state, the gateway configuration or agent account settings, and to files other programs load by themselves (shell profiles, Git, editor and agent settings) still ask, as does every edit in a project that contains your home folder. Every operation is recorded. The setting also applies to existing-agent runs, including ones already running. Sessions with their own --auto-approve setting keep it.\n");
    }
    text
}

fn project_path(project: Option<PathBuf>) -> Result<PathBuf> {
    let path = match project {
        Some(path) => path,
        None => std::env::current_dir()
            .map_err(|_| AppError::local("Could not determine the current project directory."))?,
    };
    path.canonicalize()
        .map_err(|_| AppError::invalid("Project path must be an accessible directory."))
}

async fn open_session(backend: &Backend, args: SessionArgs) -> Result<SessionView> {
    if let Some(id) = args.session {
        return backend.session(&id).await;
    }
    let Some(model) = args.model else {
        return Err(AppError::invalid(
            "A new session requires --model. Run `switchya models` to see permitted models; configure a real provider in the browser workspace or gateway configuration.",
        ));
    };
    let path = project_path(args.project)?;
    let project = backend.project(&path).await?;
    let session = backend.create(&project.id, &model).await?;
    write_stderr(&format!("Session: {}\n", terminal_text(&session.id)))?;
    backend.session(&session.id).await
}

struct TerminalInput {
    receiver: tokio::sync::mpsc::Receiver<std::io::Result<String>>,
    reviewed_operations: std::collections::HashSet<String>,
}

impl TerminalInput {
    fn new(receiver: tokio::sync::mpsc::Receiver<std::io::Result<String>>) -> Self {
        Self {
            receiver,
            reviewed_operations: std::collections::HashSet::new(),
        }
    }
}

type Input = Option<TerminalInput>;
fn interactive_input() -> Input {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return None;
    }
    let (send, receive) = tokio::sync::mpsc::channel(8);
    // A plain reader thread does not hold Tokio's blocking pool open on Ctrl-C.
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            if send.blocking_send(line).is_err() {
                break;
            }
        }
    });
    Some(TerminalInput::new(receive))
}

async fn line(input: &mut Input) -> Result<Option<String>> {
    let input = input.as_mut().ok_or_else(|| {
        AppError::invalid("Interactive review requires a terminal for input and stderr.")
    })?;
    input
        .receiver
        .recv()
        .await
        .transpose()
        .map_err(|_| AppError::local("Could not read terminal input."))
}

async fn chat(backend: &Backend, args: SessionArgs, cancelled: &CancellationToken) -> Result<()> {
    let mut input = interactive_input();
    if input.is_none() {
        return Err(AppError::invalid(
            "Chat requires an interactive terminal. Use `switchya run --prompt ...` for noninteractive input.",
        ));
    }
    let view = select_session(backend, args).await?;
    write_stderr(&format!(
        "Project: {}\nModel: {}\n{}\nEnter a prompt, or /exit. Commands run with your OS permissions.\n",
        terminal_text(&view.project.root),
        terminal_text(&view.session.model),
        auto_approve_line(&view.auto_approve)
    ))?;
    if let Some(run_id) = &view.session.active_run_id {
        let after = view
            .events
            .first()
            .map_or(0, |event| event.seq.saturating_sub(1));
        watch(
            backend,
            &view.session.id,
            run_id,
            after,
            &mut input,
            cancelled,
        )
        .await?;
    }
    loop {
        write_stderr("\nYou> ")?;
        let text = tokio::select! { result = line(&mut input) => result?, () = cancelled.cancelled() => None };
        let Some(text) = text else {
            return Ok(());
        };
        if text.trim() == "/exit" {
            return Ok(());
        }
        if text.trim().is_empty() {
            continue;
        }
        if is_reviewed_approval_reply(&input, &text) {
            write_stderr(
                "That reply names an earlier approval and was ignored. Enter a new prompt to continue.\n",
            )?;
            continue;
        }
        turn(backend, &view.session.id, &text, &mut input, cancelled).await?;
    }
}

async fn turn(
    backend: &Backend,
    id: &str,
    text: &str,
    input: &mut Input,
    cancelled: &CancellationToken,
) -> Result<()> {
    if cancelled.is_cancelled() {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "interrupted",
            "Cancelled before submitting a turn.",
        ));
    }
    let before = backend.session(id).await?;
    let command_id = uuid::Uuid::new_v4().to_string();
    if cancelled.is_cancelled() {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "interrupted",
            "Cancelled before submitting a turn.",
        ));
    }
    let run = match backend.submit(id, &command_id, text, cancelled).await {
        Ok(run) => run,
        Err(error) if matches!(error.code, "host_unreachable" | "invalid_response") => {
            // Reconcile by retained command ID without starting another turn.
            if let Ok(Ok(events)) = tokio::time::timeout(
                Duration::from_secs(3),
                backend.events(id, before.session.last_seq),
            )
            .await
                && let Some(run_id) = events
                    .iter()
                    .find(|event| {
                        event.kind == "turn.started"
                            && event.payload["command_id"].as_str() == Some(command_id.as_str())
                    })
                    .and_then(|event| event.run_id.as_deref())
            {
                let _ = backend.interrupt(id, run_id).await;
            }
            return Err(AppError::new(
                StatusCode::BAD_GATEWAY,
                "submission_unconfirmed",
                format!(
                    "Submission could not be confirmed. Inspect session {id} and command {command_id} before submitting another prompt."
                ),
            ));
        }
        Err(error) => return Err(error),
    };
    watch(
        backend,
        id,
        &run.id,
        before.session.last_seq,
        input,
        cancelled,
    )
    .await
}

async fn watch(
    backend: &Backend,
    id: &str,
    run_id: &str,
    after: u64,
    input: &mut Input,
    cancelled: &CancellationToken,
) -> Result<()> {
    let result = tokio::select! {
        biased;
        () = cancelled.cancelled() => Err(AppError::new(StatusCode::CONFLICT, "interrupted", "Interruption requested. Recorded effects remain; review the session before continuing.")),
        result = drive(backend, id, run_id, after, input) => result,
    };
    if result.is_err()
        && let Err(stop_error) = backend.interrupt(id, run_id).await
        && backend
            .session(id)
            .await
            .is_ok_and(|view| view.session.active_run_id.as_deref() == Some(run_id))
    {
        let _ = write_stderr(&format!(
            "Interruption was not confirmed: {}. Reconnect to inspect the active run.\n",
            terminal_text(&stop_error.message)
        ));
    }
    result
}

async fn drive(
    backend: &Backend,
    id: &str,
    run_id: &str,
    mut after: u64,
    input: &mut Input,
) -> Result<()> {
    let mut decided = std::collections::HashSet::new();
    let mut notices = AutoApprovalNotices::default();
    loop {
        let events = backend.events(id, after).await?;
        for event in &events {
            after = after.max(event.seq);
            if event.run_id.as_deref() != Some(run_id) {
                continue;
            }
            if let Some(notice) = notices.observe(event) {
                write_stderr(&format!("\n{}\n", terminal_text(&notice)))?;
            }
            match event.kind.as_str() {
                "model.delta" => {
                    if let Some(text) = event.payload["text"].as_str() {
                        write_stdout(&terminal_text(text))?;
                    }
                }
                "tool.completed" | "tool.rejected" => write_stderr(&format!(
                    "\n{}: {}\n",
                    terminal_text(&event.kind),
                    terminal_text(
                        &serde_json::to_string_pretty(&event.payload).unwrap_or_default()
                    )
                ))?,
                "run.completed" => {
                    write_stderr("\nCompleted. Review changes and observed checks separately.\n")?;
                    return Ok(());
                }
                "run.failed" | "run.interrupted" | "recovery.required" => {
                    return Err(AppError::new(
                        StatusCode::CONFLICT,
                        "run_stopped",
                        event.payload["message"]
                            .as_str()
                            .unwrap_or("The run stopped; inspect the session before continuing."),
                    ));
                }
                _ => {}
            }
        }
        if events.len() == 200 {
            continue;
        }
        let view = backend.session(id).await?;
        if let Some(operation) = view
            .pending_operation
            .filter(|operation| operation.run_id == run_id && !decided.contains(&operation.id))
        {
            if input.is_none() {
                return Err(AppError::new(
                    StatusCode::CONFLICT,
                    "approval_required",
                    "The run requires explicit approval. Noninteractive execution will not approve it; interruption is being requested. Continue in an interactive session, or opt this session in with --auto-approve.",
                ));
            }
            write_stderr(&format!(
                "\nApproval required for {}\nOperation: {}\nArgument hash: {}\nWorking project: {}\nExact arguments:\n{}\nPreview:\n{}\nCommands run with your current OS permissions; this is not an OS sandbox.\n",
                terminal_text(&operation.name),
                terminal_text(&operation.id),
                terminal_text(&operation.arguments_hash),
                terminal_text(&view.project.root),
                terminal_text(
                    &serde_json::to_string_pretty(&operation.arguments).unwrap_or_default()
                ),
                terminal_text(
                    &serde_json::to_string_pretty(&operation.preview).unwrap_or_default()
                )
            ))?;
            let Some(decision) = await_approval(backend, id, &operation, input).await? else {
                continue;
            };
            match backend.decide(id, &operation, decision).await {
                Ok(()) => {
                    decided.insert(operation.id);
                }
                Err(error) if error.status == StatusCode::CONFLICT => {
                    // Another client can resolve this approval after our last
                    // read. A stale reply must not interrupt that client's work.
                    write_stderr(
                        "\nThis approval changed in another client. Refreshing the run.\n",
                    )?;
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
        if view.session.active_run_id.is_none() {
            // A terminal transition can land between the event read and state
            // read. Drain that final event batch before reporting completion.
            if view.session.last_seq > after {
                continue;
            }
            match view.session.state {
                SessionState::Completed => {
                    write_stderr("\nCompleted. Review changes and observed checks separately.\n")?;
                    return Ok(());
                }
                SessionState::Failed
                | SessionState::Interrupted
                | SessionState::RecoveryRequired => {
                    return Err(AppError::new(
                        StatusCode::CONFLICT,
                        "run_stopped",
                        format!(
                            "Session stopped in {:?}; inspect its recorded events before continuing.",
                            view.session.state
                        ),
                    ));
                }
                _ => {}
            }
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
}

/// Terminal notices for auto-approved operations. A proposal is kept only until
/// its operation is decided or finishes, so a long run holds just the
/// operations in flight.
#[derive(Default)]
struct AutoApprovalNotices(std::collections::HashMap<String, Value>);

impl AutoApprovalNotices {
    fn observe(&mut self, event: &Event) -> Option<String> {
        let payload = &event.payload;
        match event.kind.as_str() {
            "tool.proposed" => {
                if let Some(id) = payload["id"].as_str() {
                    self.0.insert(id.to_owned(), payload.clone());
                }
                None
            }
            "approval.decided" => {
                let operation = payload["operation_id"]
                    .as_str()
                    .and_then(|id| self.0.remove(id));
                (payload["decided_by"] == "auto_approve")
                    .then(|| auto_approved_notice(payload, operation.as_ref()))
            }
            "approval.required" | "tool.completed" | "tool.rejected" => {
                if let Some(id) = payload["id"]
                    .as_str()
                    .or_else(|| payload["operation_id"].as_str())
                {
                    self.0.remove(id);
                }
                None
            }
            _ => None,
        }
    }
}

/// One line naming what was auto-approved: the command text or the edited paths.
fn auto_approved_notice(decision: &Value, operation: Option<&Value>) -> String {
    let source = if decision["policy_source"] == "session" {
        "session setting"
    } else {
        "project setting"
    };
    let name = operation
        .and_then(|op| op["name"].as_str())
        .unwrap_or("operation");
    let detail = match name {
        "run_command" => operation
            .and_then(|op| op["arguments"]["command"].as_str())
            .map(|command| format!(": {command}")),
        "apply_patch" => operation
            .and_then(|op| op["arguments"]["edits"].as_array())
            .map(|edits| {
                let paths: Vec<_> = edits.iter().filter_map(|e| e["path"].as_str()).collect();
                format!(": {}", paths.join(", "))
            }),
        _ => None,
    };
    format!(
        "Auto-approved {name} ({source}){}",
        detail.unwrap_or_default()
    )
}

fn same_pending_operation(view: &SessionView, operation: &switchyard_agent::Operation) -> bool {
    view.session.active_run_id.as_deref() == Some(operation.run_id.as_str())
        && view.pending_operation.as_ref().is_some_and(|current| {
            current.id == operation.id
                && current.run_id == operation.run_id
                && current.arguments_hash == operation.arguments_hash
                && current.state == "awaiting_approval"
        })
}

async fn await_approval(
    backend: &Backend,
    id: &str,
    operation: &switchyard_agent::Operation,
    input: &mut Input,
) -> Result<Option<Decision>> {
    if let Some(input) = input.as_mut() {
        input.reviewed_operations.insert(operation.id.clone());
    }
    let prompt = format!(
        "Type allow_once {} or deny {}: ",
        terminal_text(&operation.id),
        terminal_text(&operation.id)
    );
    write_stderr(&prompt)?;
    // Read before accepting even pre-buffered input. A newly-created zero-delay
    // Tokio sleep is not guaranteed ready on its first poll.
    let current = backend.session(id).await?;
    if !same_pending_operation(&current, operation) {
        write_stderr("\nThis approval changed in another client. Refreshing the run.\n")?;
        return Ok(None);
    }
    let refresh = tokio::time::sleep(Duration::from_millis(200));
    tokio::pin!(refresh);
    loop {
        tokio::select! {
            biased;
            _ = &mut refresh => {
                let current = backend.session(id).await?;
                // Schedule from completion so a slow host read cannot leave an
                // overdue timer continuously ahead of ready terminal input.
                refresh.as_mut().reset(tokio::time::Instant::now() + Duration::from_millis(200));
                if !same_pending_operation(&current, operation) {
                    write_stderr("\nThis approval changed in another client. Refreshing the run.\n")?;
                    return Ok(None);
                }
            }
            answer = line(input) => {
                let Some(answer) = answer? else {
                    return Err(AppError::new(
                        StatusCode::CONFLICT,
                        "approval_required",
                        "Input ended before a decision; the operation was not approved.",
                    ));
                };
                if let Some(decision) = approval_answer(&answer, &operation.id) {
                    return Ok(Some(decision));
                }
                write_stderr("The response must name this exact operation.\n")?;
                write_stderr(&prompt)?;
            }
        }
    }
}

fn is_reviewed_approval_reply(input: &Input, answer: &str) -> bool {
    input.as_ref().is_some_and(|input| {
        input
            .reviewed_operations
            .iter()
            .any(|id| approval_answer(answer, id).is_some())
    })
}

fn approval_answer(answer: &str, id: &str) -> Option<Decision> {
    if answer.trim() == format!("allow_once {id}") {
        Some(Decision::AllowOnce)
    } else if answer.trim() == format!("deny {id}") {
        Some(Decision::Deny)
    } else {
        None
    }
}

fn terminal_text(text: &str) -> String {
    text.chars()
        .flat_map(|character| {
            if (character.is_control() && !matches!(character, '\n' | '\t'))
                || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                character.escape_unicode().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}

fn write_stdout(text: &str) -> Result<()> {
    let mut output = std::io::stdout().lock();
    output
        .write_all(text.as_bytes())
        .and_then(|()| output.flush())
        .map_err(|_| {
            AppError::new(
                StatusCode::CONFLICT,
                "output_closed",
                "Output closed; interruption is being requested.",
            )
        })
}

fn write_stderr(text: &str) -> Result<()> {
    let mut output = std::io::stderr().lock();
    output
        .write_all(text.as_bytes())
        .and_then(|()| output.flush())
        .map_err(|_| {
            AppError::new(
                StatusCode::CONFLICT,
                "output_closed",
                "Terminal output closed; interruption is being requested.",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    fn fixture_view(pending: bool) -> SessionView {
        SessionView {
            session: Session {
                id: "sessiontest".into(),
                project_id: "projecttest".into(),
                model: "fixture".into(),
                title: "Fixture".into(),
                state: if pending {
                    SessionState::AwaitingApproval
                } else {
                    SessionState::Idle
                },
                active_run_id: pending.then(|| "runtest".into()),
                revision: 1,
                last_seq: 0,
                created_at_ms: 0,
                updated_at_ms: 0,
                auto_approve: None,
            },
            project: Project {
                id: "projecttest".into(),
                name: "Fixture".into(),
                root: "fixture".into(),
                created_at_ms: 0,
                auto_approve: Default::default(),
            },
            auto_approve: Default::default(),
            pending_operation: pending.then(|| switchyard_agent::Operation {
                id: "operationtest".into(),
                session_id: "sessiontest".into(),
                run_id: "runtest".into(),
                call_id: "calltest".into(),
                name: "run_command".into(),
                arguments_hash: "exact-hash".into(),
                arguments: json!({"command":"fixture-only"}),
                requires_approval: true,
                state: "awaiting_approval".into(),
                preview: json!({"command":"fixture-only","cwd":"fixture"}),
            }),
            events: Vec::new(),
        }
    }

    async fn fixture_server(
        router: Router,
    ) -> (Backend, CancellationToken, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let stopped = CancellationToken::new();
        let shutdown = stopped.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .unwrap();
        });
        (
            Backend::Remote(Remote::new(LaunchInfo::new(port)).unwrap()),
            stopped,
            server,
        )
    }

    #[tokio::test]
    async fn cancellation_during_submission_interrupts_the_returned_run() {
        let posted = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let interrupted = Arc::new(AtomicBool::new(false));
        let router = Router::new()
            .route("/api/sessions/sessiontest", get(|| async { Json(fixture_view(false)) }))
            .route("/api/sessions/sessiontest/turns", post({
                let posted = posted.clone(); let release = release.clone();
                move |Json(input): Json<Value>| { let posted = posted.clone(); let release = release.clone(); async move {
                    posted.notify_one(); release.notified().await;
                    Json(json!({"run": Run { id:"runtest".into(), session_id:"sessiontest".into(), command_id:input["command_id"].as_str().unwrap().into(), state:SessionState::Running, started_at_ms:0, ended_at_ms:None }}))
                } }
            }))
            .route("/api/sessions/sessiontest/interrupt", post({
                let interrupted = interrupted.clone();
                move |Json(input): Json<Value>| { let interrupted = interrupted.clone(); async move {
                    assert_eq!(input["run_id"], "runtest"); interrupted.store(true, Ordering::SeqCst); Json(json!({"ok":true}))
                } }
            }));
        let (backend, stopped, server) = fixture_server(router).await;
        let cancelled = CancellationToken::new();
        let operation_cancelled = cancelled.clone();
        let operation = tokio::spawn(async move {
            turn(
                &backend,
                "sessiontest",
                "fixture prompt",
                &mut None,
                &operation_cancelled,
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(3), posted.notified())
            .await
            .unwrap();
        cancelled.cancel();
        release.notify_one();
        let result = tokio::time::timeout(Duration::from_secs(3), operation)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err().code, "interrupted");
        assert!(interrupted.load(Ordering::SeqCst));
        stopped.cancel();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn attaching_to_an_existing_run_reviews_its_exact_pending_operation() {
        let decided = Arc::new(AtomicBool::new(false));
        let router = Router::new()
            .route(
                "/api/sessions/sessiontest",
                get(|| async { Json(fixture_view(true)) }),
            )
            .route(
                "/api/sessions/sessiontest/events",
                get({
                    let decided = decided.clone();
                    move || {
                        let decided = decided.clone();
                        async move {
                            let events = if decided.load(Ordering::SeqCst) {
                                vec![Event {
                                    schema_version: 1,
                                    session_id: "sessiontest".into(),
                                    run_id: Some("runtest".into()),
                                    seq: 1,
                                    at_ms: 0,
                                    kind: "run.completed".into(),
                                    payload: json!({}),
                                }]
                            } else {
                                Vec::new()
                            };
                            Json(json!({"events":events}))
                        }
                    }
                }),
            )
            .route(
                "/api/sessions/sessiontest/operations/operationtest/decision",
                post({
                    let decided = decided.clone();
                    move |Json(input): Json<Value>| {
                        let decided = decided.clone();
                        async move {
                            assert_eq!(
                                input,
                                json!({"expected_hash":"exact-hash","decision":"deny"})
                            );
                            decided.store(true, Ordering::SeqCst);
                            Json(json!({"ok":true}))
                        }
                    }
                }),
            );
        let (backend, stopped, server) = fixture_server(router).await;
        let (send, receive) = tokio::sync::mpsc::channel(2);
        send.send(Ok("deny operationtest".into())).await.unwrap();
        let mut input = Some(TerminalInput::new(receive));
        tokio::time::timeout(
            Duration::from_secs(3),
            watch(
                &backend,
                "sessiontest",
                "runtest",
                0,
                &mut input,
                &CancellationToken::new(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(decided.load(Ordering::SeqCst));
        drop(backend);
        stopped.cancel();
        server.await.unwrap();
    }

    #[derive(Clone, Copy)]
    enum ApprovalRace {
        Completed,
        NextOperation,
        DecisionConflict,
        SlowRead,
        InputEnded,
        Cancelled,
    }

    async fn exercise_approval_race(race: ApprovalRace) {
        let reads = Arc::new(AtomicUsize::new(0));
        let decisions = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(AtomicBool::new(false));
        let interrupted = Arc::new(AtomicBool::new(false));
        let cancelled = CancellationToken::new();
        let router = Router::new()
            .route("/api/sessions/sessiontest", get({
                let reads = reads.clone();
                let finished = finished.clone();
                let cancelled = cancelled.clone();
                move || {
                    let reads = reads.clone();
                    let finished = finished.clone();
                    let cancelled = cancelled.clone();
                    async move {
                        if matches!(race, ApprovalRace::SlowRead) {
                            tokio::time::sleep(Duration::from_millis(250)).await;
                        }
                        let reviewed = reads.fetch_add(1, Ordering::SeqCst) > 0;
                        let mut view = fixture_view(true);
                        match race {
                            ApprovalRace::Completed if reviewed => {
                                finished.store(true, Ordering::SeqCst);
                                view = fixture_view(false);
                                view.session.state = SessionState::Completed;
                                view.session.last_seq = 1;
                            }
                            ApprovalRace::NextOperation if reviewed => {
                                let operation = view.pending_operation.as_mut().unwrap();
                                operation.id = "operationnext".into();
                                operation.arguments_hash = "next-exact-hash".into();
                            }
                            ApprovalRace::Cancelled if reviewed => cancelled.cancel(),
                            _ => {},
                        }
                        Json(view)
                    }
                }
            }))
            .route("/api/sessions/sessiontest/events", get({
                let finished = finished.clone();
                move || {
                    let finished = finished.clone();
                    async move {
                        let events = if finished.load(Ordering::SeqCst) {
                            vec![Event { schema_version: 1, session_id: "sessiontest".into(), run_id: Some("runtest".into()), seq: 1, at_ms: 0, kind: "run.completed".into(), payload: json!({}) }]
                        } else { Vec::new() };
                        Json(json!({"events": events}))
                    }
                }
            }))
            .route("/api/sessions/sessiontest/operations/{operation_id}/decision", post({
                let decisions = decisions.clone();
                let finished = finished.clone();
                move |axum::extract::Path(operation_id): axum::extract::Path<String>, Json(input): Json<Value>| {
                    let decisions = decisions.clone();
                    let finished = finished.clone();
                    async move {
                        decisions.fetch_add(1, Ordering::SeqCst);
                        match race {
                            ApprovalRace::NextOperation => {
                                assert_eq!(operation_id, "operationnext");
                                assert_eq!(input, json!({"expected_hash":"next-exact-hash","decision":"deny"}));
                                finished.store(true, Ordering::SeqCst);
                                (StatusCode::OK, Json(json!({"ok":true})))
                            }
                            ApprovalRace::DecisionConflict => {
                                assert_eq!(operation_id, "operationtest");
                                assert_eq!(input, json!({"expected_hash":"exact-hash","decision":"deny"}));
                                finished.store(true, Ordering::SeqCst);
                                (StatusCode::CONFLICT, Json(json!({"error":{"message":"Approval was already resolved in the browser."}})))
                            }
                            ApprovalRace::SlowRead => {
                                assert_eq!(operation_id, "operationtest");
                                assert_eq!(input, json!({"expected_hash":"exact-hash","decision":"deny"}));
                                finished.store(true, Ordering::SeqCst);
                                (StatusCode::OK, Json(json!({"ok":true})))
                            }
                            _ => panic!("An expired or unanswered approval must not be submitted"),
                        }
                    }
                }
            }))
            .route("/api/sessions/sessiontest/interrupt", post({
                let interrupted = interrupted.clone();
                move |Json(input): Json<Value>| {
                    let interrupted = interrupted.clone();
                    async move {
                        assert_eq!(input["run_id"], "runtest");
                        interrupted.store(true, Ordering::SeqCst);
                        Json(json!({"ok":true}))
                    }
                }
            }));
        let (backend, stopped, server) = fixture_server(router).await;
        let (send, receive) = tokio::sync::mpsc::channel(2);
        match race {
            ApprovalRace::NextOperation => {
                send.send(Ok("allow_once operationtest".into()))
                    .await
                    .unwrap();
                send.send(Ok("deny operationnext".into())).await.unwrap();
            }
            ApprovalRace::DecisionConflict | ApprovalRace::SlowRead => {
                send.send(Ok("deny operationtest".into())).await.unwrap();
            }
            _ => {}
        }
        // Keep stdin open and idle except in the explicit EOF regression.
        let _send = if matches!(race, ApprovalRace::InputEnded) {
            drop(send);
            None
        } else {
            Some(send)
        };
        let mut input = Some(TerminalInput::new(receive));
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            watch(
                &backend,
                "sessiontest",
                "runtest",
                0,
                &mut input,
                &cancelled,
            ),
        )
        .await
        .expect("CLI did not follow the changed approval state");
        match race {
            ApprovalRace::InputEnded => assert_eq!(result.unwrap_err().code, "approval_required"),
            ApprovalRace::Cancelled => assert_eq!(result.unwrap_err().code, "interrupted"),
            _ => result.unwrap(),
        }
        assert_eq!(
            decisions.load(Ordering::SeqCst),
            usize::from(matches!(
                race,
                ApprovalRace::NextOperation
                    | ApprovalRace::DecisionConflict
                    | ApprovalRace::SlowRead
            ))
        );
        assert_eq!(
            interrupted.load(Ordering::SeqCst),
            matches!(race, ApprovalRace::InputEnded | ApprovalRace::Cancelled)
        );
        assert!(reads.load(Ordering::SeqCst) >= 2);
        if matches!(race, ApprovalRace::Completed) {
            assert!(is_reviewed_approval_reply(
                &input,
                "allow_once operationtest"
            ));
            assert!(is_reviewed_approval_reply(&input, "deny operationtest"));
            assert!(!is_reviewed_approval_reply(
                &input,
                "deny access to that path"
            ));
            assert!(!is_reviewed_approval_reply(&input, "allow_once unknown"));
        }
        stopped.cancel();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn browser_completion_releases_an_idle_cli_approval_prompt() {
        exercise_approval_race(ApprovalRace::Completed).await;
    }

    #[tokio::test]
    async fn stale_answer_cannot_decide_the_next_operation_or_interrupt_the_run() {
        exercise_approval_race(ApprovalRace::NextOperation).await;
    }

    #[tokio::test]
    async fn approval_conflict_refreshes_the_run_without_interrupting_it() {
        exercise_approval_race(ApprovalRace::DecisionConflict).await;
    }

    #[tokio::test]
    async fn slow_session_reads_do_not_starve_a_ready_approval_answer() {
        exercise_approval_race(ApprovalRace::SlowRead).await;
    }

    #[tokio::test]
    async fn eof_during_a_current_approval_still_interrupts_the_run() {
        exercise_approval_race(ApprovalRace::InputEnded).await;
    }

    #[tokio::test]
    async fn cancellation_during_approval_still_interrupts_the_run() {
        exercise_approval_race(ApprovalRace::Cancelled).await;
    }

    #[test]
    fn auto_approve_choices_and_notices_say_exactly_what_runs_unasked() {
        assert_eq!(AutoApproveArg::Inherit.policy(), None);
        assert_eq!(
            AutoApproveArg::Off.policy(),
            Some(AutoApprove {
                edits: false,
                commands: false
            })
        );
        assert_eq!(
            AutoApproveArg::Edits.policy(),
            Some(AutoApprove {
                edits: true,
                commands: false
            })
        );
        assert_eq!(
            AutoApproveArg::All.policy(),
            Some(AutoApprove {
                edits: true,
                commands: true
            })
        );
        let off = EffectiveAutoApprove::default();
        assert_eq!(
            auto_approve_line(&off),
            "Auto-approve (project setting): off; every edit and command needs your decision."
        );
        let commands = EffectiveAutoApprove {
            edits: false,
            commands: true,
            source: PolicySource::Session,
        };
        let line = auto_approve_line(&commands);
        assert!(
            line.starts_with("Auto-approve (this session's setting): commands run without asking")
        );
        assert!(line.contains("not a sandbox"));
        let command = json!({"name":"run_command","arguments":{"command":"cargo test"}});
        assert_eq!(
            auto_approved_notice(&json!({"policy_source":"session"}), Some(&command)),
            "Auto-approved run_command (session setting): cargo test"
        );
        let patch =
            json!({"name":"apply_patch","arguments":{"edits":[{"path":"a.rs"},{"path":"b.rs"}]}});
        assert_eq!(
            auto_approved_notice(&json!({"policy_source":"project"}), Some(&patch)),
            "Auto-approved apply_patch (project setting): a.rs, b.rs"
        );
        assert_eq!(
            auto_approved_notice(&json!({}), None),
            "Auto-approved operation (project setting)"
        );
        let mut project = fixture_view(false).project;
        assert!(trust_report(&project).contains("Auto-approve: off;"));
        project.auto_approve = AutoApprove {
            edits: true,
            commands: true,
        };
        let report = trust_report(&project);
        assert!(report.contains("full OS and network permissions"));
        assert!(report.contains("Every operation is recorded"));
    }

    #[test]
    fn auto_approval_notices_name_the_operation_and_keep_only_operations_in_flight() {
        let at = |seq: u64, kind: &str, payload: Value| Event {
            schema_version: 1,
            session_id: "sessiontest".into(),
            run_id: Some("runtest".into()),
            seq,
            at_ms: 0,
            kind: kind.into(),
            payload,
        };
        let mut notices = AutoApprovalNotices::default();
        let proposed = at(
            1,
            "tool.proposed",
            json!({"id":"op","name":"run_command","arguments":{"command":"cargo test"}}),
        );
        assert_eq!(notices.observe(&proposed), None);
        let decided = at(
            2,
            "approval.decided",
            json!({"operation_id":"op","decision":"allow_once","decided_by":"auto_approve","policy_source":"project"}),
        );
        assert_eq!(
            notices.observe(&decided).as_deref(),
            Some("Auto-approved run_command (project setting): cargo test")
        );
        // A person's decision prints nothing here; finished operations are forgotten.
        notices.observe(&at(
            3,
            "tool.proposed",
            json!({"id":"edit","name":"apply_patch","arguments":{"edits":[{"path":"a.rs"}]}}),
        ));
        let reviewed = at(
            4,
            "approval.decided",
            json!({"operation_id":"edit","decision":"allow_once"}),
        );
        assert_eq!(notices.observe(&reviewed), None);
        notices.observe(&at(
            5,
            "tool.proposed",
            json!({"id":"read","name":"read_file","arguments":{"path":"a.rs"}}),
        ));
        notices.observe(&at(
            6,
            "tool.completed",
            json!({"operation_id":"read","name":"read_file"}),
        ));
        notices.observe(&at(
            7,
            "tool.proposed",
            json!({"id":"wait","name":"run_command"}),
        ));
        notices.observe(&at(8, "approval.required", json!({"id":"wait"})));
        assert!(notices.0.is_empty());
    }

    #[test]
    fn approval_binds_exact_operation_and_terminal_output_cannot_emit_controls() {
        assert_eq!(
            approval_answer("allow_once op_1", "op_1"),
            Some(Decision::AllowOnce)
        );
        for answer in [
            "yes",
            "allow_once",
            "allow_once op_2",
            "allow_once op_1 extra",
        ] {
            assert!(approval_answer(answer, "op_1").is_none());
        }
        let rendered = terminal_text("hello\u{1b}[2J\r\u{202e}world\n");
        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains('\r'));
        assert!(!rendered.contains('\u{202e}'));
        assert!(rendered.ends_with("world\n"));
    }
    #[test]
    fn parser_supports_headless_run_and_resume_without_mutating_project_binding() {
        assert!(
            Cli::try_parse_from([
                "switchya",
                "run",
                "--project",
                ".",
                "--model",
                "mock-echo",
                "--prompt",
                "hello"
            ])
            .is_ok()
        );
        assert!(Cli::try_parse_from(["switchya", "chat", "--session", "session_a"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "switchya",
                "chat",
                "--session",
                "session_a",
                "--project",
                "."
            ])
            .is_err()
        );
    }
}
