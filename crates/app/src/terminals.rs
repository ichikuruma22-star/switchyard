//! User-controlled interactive terminals. They are separate from agent tools:
//! opening a view never spawns a shell, and no model can write terminal input.
use crate::{
    AppError, Result,
    api::{self, AppState},
    workspace,
};
use axum::{
    Json,
    extract::{Path, Request, State},
    http::StatusCode,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 1024 * 1024;
const ACTIVE_LIMIT: usize = 8;
const HISTORY_LIMIT: usize = 64;

#[derive(Default)]
struct Transcript {
    chunks: VecDeque<(u64, Vec<u8>)>,
    bytes: usize,
    seq: u64,
    done: bool,
    exit_code: Option<u32>,
    closed: bool,
    error: Option<String>,
}
impl Transcript {
    fn push(&mut self, bytes: &[u8]) {
        self.seq += 1;
        self.bytes += bytes.len();
        self.chunks.push_back((self.seq, bytes.to_vec()));
        while self.bytes > OUTPUT_LIMIT {
            if let Some((_, bytes)) = self.chunks.pop_front() {
                self.bytes -= bytes.len();
            }
        }
    }
}
struct Input {
    writer: Box<dyn Write + Send>,
    sequence: u64,
    last: String,
    delivered: bool,
}
struct Terminal {
    id: String,
    project_id: String,
    shell: String,
    output: Mutex<Transcript>,
    input: Mutex<Input>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    dimensions: Mutex<PtySize>,
    stop: AtomicBool,
    finished: AtomicBool,
    input_sequence: AtomicU64,
}
impl Terminal {
    fn snapshot(&self, after: u64) -> Value {
        let output = self.output.lock();
        let size = *self.dimensions.lock();
        let first = output
            .chunks
            .front()
            .map_or(output.seq + 1, |(seq, _)| *seq);
        let chunks: Vec<_> = output
            .chunks
            .iter()
            .filter(|(seq, _)| *seq > after)
            .take(32)
            .map(|(seq, bytes)| json!({"seq":seq,"data":STANDARD.encode(bytes)}))
            .collect();
        json!({"id":self.id,"project_id":self.project_id,"shell":self.shell,"state":if output.closed {"exited"} else if self.stop.load(Ordering::Acquire) {"closing"} else {"running"},"exit_code":output.exit_code,"error":output.error,"cols":size.cols,"rows":size.rows,"first_seq":first,"last_seq":output.seq,"input_sequence":self.input_sequence.load(Ordering::Acquire),"output_done":output.done,"chunks":chunks})
    }
}

#[derive(Clone)]
pub(crate) struct TerminalManager {
    entries: Arc<Mutex<BTreeMap<String, Arc<Terminal>>>>,
    starts: Arc<tokio::sync::Mutex<()>>,
    stopping: Arc<AtomicBool>,
    _lifetime: Arc<StopOnDrop>,
}
struct StopOnDrop {
    entries: Arc<Mutex<BTreeMap<String, Arc<Terminal>>>>,
    stopping: Arc<AtomicBool>,
}
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        for terminal in self.entries.lock().values() {
            terminal.stop.store(true, Ordering::Release);
        }
    }
}
impl Default for TerminalManager {
    fn default() -> Self {
        let entries: Arc<Mutex<BTreeMap<String, Arc<Terminal>>>> = Default::default();
        let stopping = Arc::new(AtomicBool::new(false));
        Self {
            entries: entries.clone(),
            starts: Default::default(),
            stopping: stopping.clone(),
            _lifetime: Arc::new(StopOnDrop { entries, stopping }),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    id: String,
    project_id: String,
    cols: u16,
    rows: u16,
}

fn dimensions(cols: u16, rows: u16) -> Result<PtySize> {
    if !(2..=500).contains(&cols) || !(1..=300).contains(&rows) {
        return Err(AppError::invalid(
            "Terminal dimensions must be 2–500 columns and 1–300 rows.",
        ));
    }
    Ok(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })
}
fn conflict(message: &str) -> AppError {
    AppError::new(StatusCode::CONFLICT, "terminal_conflict", message)
}
fn terminal_error() -> AppError {
    AppError::local("The system terminal could not be started. Check that your shell is available.")
}

impl TerminalManager {
    fn get(&self, id: &str) -> Result<Arc<Terminal>> {
        self.entries.lock().get(id).cloned().ok_or_else(|| {
            AppError::new(
                StatusCode::NOT_FOUND,
                "terminal_not_found",
                "This terminal is no longer available. Open a new terminal.",
            )
        })
    }
    async fn start(&self, input: Start, root: PathBuf) -> Result<Arc<Terminal>> {
        if uuid::Uuid::parse_str(&input.id).is_err() {
            return Err(AppError::invalid(
                "A terminal needs a UUID retained across retries.",
            ));
        }
        let size = dimensions(input.cols, input.rows)?;
        let guard = self.starts.clone().lock_owned().await;
        let manager = self.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            if manager.stopping.load(Ordering::Acquire) { return Err(conflict("The app is shutting down.")); }
            {
                let entries = manager.entries.lock();
                if let Some(existing) = entries.get(&input.id) {
                    if existing.project_id != input.project_id { return Err(conflict("This terminal id belongs to another project.")); }
                    return Ok(existing.clone());
                }
                if entries.values().filter(|entry| !entry.finished.load(Ordering::Acquire)).count() >= ACTIVE_LIMIT { return Err(conflict("Eight terminals are already open. Close a terminal before starting another.")); }
                if entries.len() >= HISTORY_LIMIT { return Err(conflict("This host has reached its terminal history limit. Reopen the app to start a new terminal.")); }
            }
            let pair = native_pty_system().openpty(size).map_err(|_| terminal_error())?;
            #[cfg(windows)]
            let (mut command, shell) = {
                let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
                let mut command = CommandBuilder::new(PathBuf::from(root).join("System32/WindowsPowerShell/v1.0/powershell.exe"));
                command.args(["-NoLogo", "-NoProfile"]);
                (command, "PowerShell")
            };
            #[cfg(unix)]
            let (mut command, shell) = {
                if std::path::Path::new("/bin/bash").is_file() {
                    let mut command = CommandBuilder::new("/bin/bash");
                    // A login shell with huponexit also hangs up its jobs on
                    // an ordinary `exit`. User startup scripts stay opt-in.
                    command.args(["--noprofile", "--norc", "--login", "-O", "huponexit", "-i"]);
                    (command, "Bash")
                } else {
                    let mut command = CommandBuilder::new("/bin/sh"); command.arg("-i"); (command, "/bin/sh")
                }
            };
            command.cwd(crate::terminal_platform::shell_directory(&root));
            command.env("TERM", "xterm-256color");
            command.env("COLORTERM", "truecolor");
            // Retain the normal command environment, but not host credentials or
            // shell startup overrides supplied to the app process.
            for (name, _) in std::env::vars_os() {
                let upper = name.to_string_lossy().to_ascii_uppercase();
                if upper.ends_with("_KEY") || ["TOKEN","SECRET","PASSWORD","PASSWD","CREDENTIAL","APIKEY","API_KEY","AUTHORIZATION"].iter().any(|part| upper.contains(part)) || matches!(upper.as_str(), "ENV" | "BASH_ENV" | "PROMPT_COMMAND") { command.env_remove(name); }
            }
            let mut child = pair.slave.spawn_command(command).map_err(|_| terminal_error())?;
            drop(pair.slave);
            let ownership = match crate::terminal_platform::Ownership::attach(child.as_ref(), pair.master.as_ref()) {
                Ok(ownership) => ownership,
                Err(_) => { let _ = child.kill(); let _ = child.wait(); return Err(terminal_error()); }
            };
            let handles = (pair.master.try_clone_reader(), pair.master.take_writer());
            let (mut reader, writer) = match handles {
                (Ok(reader), Ok(writer)) => (reader, writer),
                _ => { let _ = ownership.terminate(pair.master.as_ref()); let _ = child.kill(); let _ = child.wait(); return Err(terminal_error()); }
            };
            let terminal = Arc::new(Terminal { id: input.id.clone(), project_id: input.project_id, shell: shell.into(), output: Default::default(), input: Mutex::new(Input {writer, sequence:0, last:String::new(), delivered:false}), master: Mutex::new(Some(pair.master)), dimensions: Mutex::new(size), stop: AtomicBool::new(false), finished: AtomicBool::new(false), input_sequence: AtomicU64::new(0) });
            let reading = terminal.clone();
            std::thread::spawn(move || {
                let mut bytes = [0u8;8192];
                let mut stopped_at = None;
                loop {
                    if reading.stop.load(Ordering::Acquire) && stopped_at.get_or_insert_with(Instant::now).elapsed() > Duration::from_millis(100) { break; }
                    match reader.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(count) => reading.output.lock().push(&bytes[..count]),
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if reading.stop.load(Ordering::Acquire) { break; }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(error) => {
                            // Unix PTYs report EIO at EOF; a closed ConPTY pipe
                            // reports BrokenPipe. Other errors stop the shell.
                            #[cfg(unix)]
                            let eof = error.raw_os_error() == Some(libc::EIO);
                            #[cfg(windows)]
                            let eof = error.kind() == std::io::ErrorKind::BrokenPipe;
                            if !eof { reading.output.lock().error = Some("Terminal output could not be read; the shell was stopped.".into()); reading.stop.store(true, Ordering::Release); }
                            break;
                        }
                    }
                }
                reading.output.lock().done = true;
            });
            let running = terminal.clone();
            std::thread::spawn(move || {
                loop {
                    if running.stop.load(Ordering::Acquire) { break; }
                    match ownership.has_exited(child.as_mut()) {
                        Ok(true) => break,
                        Ok(false) => std::thread::sleep(Duration::from_millis(30)),
                        Err(_) => { running.output.lock().error = Some("The shell status could not be read; terminal cleanup was requested.".into()); break; }
                    }
                }
                running.stop.store(true, Ordering::Release);
                // Let an interactive shell hang up its background jobs before
                // forcing the remaining foreground process group to stop.
                #[cfg(unix)]
                if ownership.hang_up().is_ok() { std::thread::sleep(Duration::from_millis(200)); }
                if let Some(master) = running.master.lock().take() {
                    if ownership.terminate(master.as_ref()).is_err() { running.output.lock().error = Some("Terminal process cleanup could not be confirmed.".into()); }
                    let _ = child.kill();
                    let exit = child.wait().ok().map(|status| status.exit_code());
                    drop(master);
                    let mut output = running.output.lock(); output.exit_code = exit; output.closed = true;
                }
                running.finished.store(true, Ordering::Release);
            });
            manager.entries.lock().insert(input.id, terminal.clone());
            if manager.stopping.load(Ordering::Acquire) { terminal.stop.store(true, Ordering::Release); }
            Ok(terminal)
        }).await.map_err(|_| terminal_error())?
    }

    pub(crate) async fn shutdown(&self) -> Result<()> {
        self.stopping.store(true, Ordering::Release);
        // Wait for any in-flight start to register its owned process first.
        let _starts = self.starts.lock().await;
        let entries: Vec<_> = self.entries.lock().values().cloned().collect();
        for entry in &entries {
            entry.stop.store(true, Ordering::Release);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while entries
            .iter()
            .any(|entry| !entry.finished.load(Ordering::Acquire))
        {
            if Instant::now() > deadline {
                return Err(AppError::local(
                    "Terminal shutdown did not complete in time.",
                ));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        Ok(())
    }
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    request: Request,
) -> Result<Json<Value>> {
    let query = api::query(request.uri(), &["project_id"])?;
    let project = query.get("project_id");
    if let Some(id) = project {
        state.engine.project(id)?;
    }
    let entries: Vec<_> = state
        .terminals
        .entries
        .lock()
        .values()
        .filter(|entry| project.is_none_or(|id| *id == entry.project_id))
        .cloned()
        .collect();
    Ok(Json(
        json!({"terminals":entries.iter().map(|entry| entry.snapshot(u64::MAX)).collect::<Vec<_>>()}),
    ))
}

pub(crate) async fn start(
    State(state): State<Arc<AppState>>,
    request: Request,
) -> Result<Json<Value>> {
    let input: Start = api::body(request).await?;
    let root = workspace::root(&state, &input.project_id)?;
    Ok(Json(
        json!({"terminal":state.terminals.start(input, root).await?.snapshot(0)}),
    ))
}

pub(crate) async fn output(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>> {
    let query = api::query(request.uri(), &["after_seq"])?;
    let after = query
        .get("after_seq")
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| AppError::invalid("Terminal cursor must be an unsigned integer."))
        })
        .transpose()?
        .unwrap_or(0);
    Ok(Json(
        json!({"terminal":state.terminals.get(&id)?.snapshot(after)}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteInput {
    sequence: u64,
    data: String,
}

pub(crate) async fn input(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>> {
    let input: WriteInput = api::body(request).await?;
    if input.data.is_empty() || input.data.len() > 8192 {
        return Err(AppError::invalid(
            "Terminal input must contain between 1 and 8,192 bytes.",
        ));
    }
    let terminal = state.terminals.get(&id)?;
    tokio::task::spawn_blocking(move || {
        let mut target = terminal.input.try_lock().ok_or_else(|| conflict("Terminal input is still being written. Retry this same input sequence."))?;
        if input.sequence == target.sequence && input.data == target.last {
            return if target.delivered { Ok(Json(json!({"sequence":input.sequence}))) } else { Err(conflict("The previous terminal input was not fully delivered. Open a new terminal.")) };
        }
        if input.sequence != target.sequence + 1 { return Err(conflict("Terminal input sequence changed. Reconnect before typing again.")); }
        if terminal.stop.load(Ordering::Acquire) { return Err(conflict("The terminal has closed.")); }
        // Reserve the sequence before writing. A failed or partial write closes
        // the terminal and can never be replayed into the shell.
        target.sequence = input.sequence; target.last = input.data.clone(); target.delivered = false;
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut bytes = input.data.as_bytes();
        while !bytes.is_empty() {
            match target.writer.write(bytes) {
                Ok(count) if count > 0 => bytes = &bytes[count..],
                Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) && Instant::now() < deadline && !terminal.stop.load(Ordering::Acquire) => std::thread::sleep(Duration::from_millis(5)),
                _ => { terminal.stop.store(true, Ordering::Release); return Err(AppError::local("Terminal input could not be fully delivered. The shell was stopped; no input will be replayed.")); }
            }
        }
        target.delivered = true;
        terminal.input_sequence.store(input.sequence, Ordering::Release);
        Ok(Json(json!({"sequence":input.sequence})))
    }).await.map_err(|_| AppError::local("Terminal input failed."))?
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resize {
    cols: u16,
    rows: u16,
}

pub(crate) async fn resize(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>> {
    let input: Resize = api::body(request).await?;
    let size = dimensions(input.cols, input.rows)?;
    let terminal = state.terminals.get(&id)?;
    tokio::task::spawn_blocking(move || {
        let master = terminal.master.lock();
        let master = master
            .as_ref()
            .ok_or_else(|| conflict("The terminal has closed."))?;
        master
            .resize(size)
            .map_err(|_| AppError::local("Terminal size could not be changed."))?;
        *terminal.dimensions.lock() = size;
        Ok(Json(json!({"cols":size.cols,"rows":size.rows})))
    })
    .await
    .map_err(|_| AppError::local("Terminal resize failed."))?
}

pub(crate) async fn close(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>> {
    api::no_query(request.uri())?;
    let terminal = state.terminals.get(&id)?;
    terminal.stop.store(true, Ordering::Release);
    Ok(Json(json!({"terminal":terminal.snapshot(u64::MAX)})))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_transcript_is_bounded_and_retains_monotonic_cursors() {
        let mut transcript = Transcript::default();
        for _ in 0..200 {
            transcript.push(&[b'x'; 8192]);
        }
        assert!(transcript.bytes <= OUTPUT_LIMIT);
        assert_eq!(transcript.seq, 200);
        assert!(transcript.chunks.front().unwrap().0 > 1);
        assert!(dimensions(0, 24).is_err());
    }
}
