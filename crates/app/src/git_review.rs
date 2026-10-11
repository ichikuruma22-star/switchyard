//! Read-only Git commands, bounded output and literal paths. Repository-supplied
//! diff helpers, text conversion, hooks, fsmonitor and inherited GIT_* overrides
//! must not turn opening a Changes view into command execution.
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
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    path::{Path as FsPath, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

const STATUS_LIMIT: usize = 2 * 1024 * 1024;
const DIFF_LIMIT: usize = 256 * 1024;

struct Output {
    bytes: Vec<u8>,
    success: bool,
    code: Option<i32>,
    truncated: bool,
}

fn git_command(root: &FsPath) -> Result<Command> {
    // Resolve only explicit absolute PATH entries. The inspected project must
    // never supply git.exe through Windows' current-directory search behavior.
    let executable = std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .filter(|path| path.is_absolute())
                .map(|path| path.join(if cfg!(windows) { "git.exe" } else { "git" }))
                .find(|path| path.is_file() && !path.starts_with(root))
        })
        .ok_or_else(|| {
            AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "git_unavailable",
                "Git is not available on the system PATH. Install Git and reopen Switchya.",
            )
        })?;
    let mut command = Command::new(executable);
    command
        .args([
            "--no-pager",
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "diff.external=",
            "-c",
            "color.ui=false",
        ])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    Ok(command)
}

async fn git(root: &FsPath, args: &[&str], limit: usize) -> Result<Output> {
    let mut command = git_command(root)?;
    if matches!(args.first(), Some(&"status" | &"diff")) {
        // --no-ext-diff/--no-textconv do not disable clean/process filters.
        // Read names only, then override every configured filter helper.
        let filters = git_output(
            git_command(root)?,
            &[
                "config",
                "--null",
                "--name-only",
                "--get-regexp",
                r"^filter\..*\.(clean|smudge|process|required)$",
            ],
            32 * 1024,
        )
        .await?;
        if filters.truncated || (!filters.success && filters.code != Some(1)) {
            return Err(AppError::invalid(
                "Git filter configuration could not be inspected safely.",
            ));
        }
        let names = std::str::from_utf8(&filters.bytes)
            .map_err(|_| AppError::invalid("Git filter names are not valid Unicode."))?;
        for name in names.split('\0').filter(|name| !name.is_empty()) {
            if name.contains('=') || name.chars().any(char::is_control) {
                return Err(AppError::invalid(
                    "Git has an unsupported filter name. Inspect this repository in your own terminal.",
                ));
            }
            command.arg("-c").arg(format!(
                "{name}={}",
                if name.ends_with(".required") {
                    "false"
                } else {
                    ""
                }
            ));
        }
    }
    git_output(command, args, limit).await
}

async fn git_output(mut command: Command, args: &[&str], limit: usize) -> Result<Output> {
    command.args(args);
    let mut child = command.spawn().map_err(|_| {
        AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "git_unavailable",
            "Git could not be started. Install Git and reopen Switchya.",
        )
    })?;
    let mut stdout = child
        .stdout
        .take()
        .expect("Git stdout was piped")
        .take((limit + 1) as u64);
    let operation = async {
        let mut bytes = Vec::new();
        stdout
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| AppError::local("Git output could not be read."))?;
        let truncated = bytes.len() > limit;
        if truncated {
            bytes.truncate(limit);
            let _ = child.start_kill();
        }
        let status = child
            .wait()
            .await
            .map_err(|_| AppError::local("Git did not return a status."))?;
        Ok(Output {
            bytes,
            success: status.success() || truncated,
            code: status.code(),
            truncated,
        })
    };
    tokio::time::timeout(Duration::from_secs(8), operation)
        .await
        .map_err(|_| {
            AppError::new(
                StatusCode::REQUEST_TIMEOUT,
                "git_timeout",
                "Git inspection took too long. Narrow the project folder or try again.",
            )
        })?
}

#[derive(Debug, Serialize)]
struct Change {
    path: String,
    original_path: Option<String>,
    staged: String,
    unstaged: String,
    untracked: bool,
    protected: bool,
}

/// Porcelain -z places rename destinations first and gives names verbatim.
fn parse_status(bytes: &[u8], prefix: &str) -> Result<Vec<Change>> {
    let mut records = bytes.split(|b| *b == 0).filter(|part| !part.is_empty());
    let mut changes = Vec::new();
    while let Some(record) = records.next() {
        if record.len() < 4 || record[2] != b' ' {
            return Err(AppError::local("Git returned an unreadable change list."));
        }
        let status = &record[..2];
        let path = std::str::from_utf8(&record[3..]).map_err(|_| {
            AppError::invalid("Git contains a file name that is not valid Unicode.")
        })?;
        let original = if status.contains(&b'R') || status.contains(&b'C') {
            Some(
                std::str::from_utf8(
                    records
                        .next()
                        .ok_or_else(|| AppError::local("Git rename data was incomplete."))?,
                )
                .map_err(|_| {
                    AppError::invalid("Git contains a file name that is not valid Unicode.")
                })?,
            )
        } else {
            None
        };
        let Some(path) = path.strip_prefix(prefix) else {
            continue;
        };
        let original = original.and_then(|name| name.strip_prefix(prefix));
        let protected = switchyard_agent_tools::check_project_path(path).is_err()
            || original
                .is_some_and(|name| switchyard_agent_tools::check_project_path(name).is_err());
        changes.push(Change {
            path: path.into(),
            original_path: original.map(str::to_owned),
            staged: char::from(status[0]).to_string(),
            unstaged: char::from(status[1]).to_string(),
            untracked: status == b"??",
            protected,
        });
        if changes.len() > 5_000 {
            return Err(AppError::invalid(
                "More than 5,000 changes. Open a narrower project folder to review them.",
            ));
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

struct Snapshot {
    branch: String,
    changes: Vec<Change>,
}

async fn snapshot(root: &FsPath) -> Result<Option<Snapshot>> {
    let top = git(root, &["rev-parse", "--show-toplevel"], 4096).await?;
    if !top.success {
        return Ok(None);
    }
    let top = String::from_utf8(top.bytes)
        .map_err(|_| AppError::invalid("Git repository path is not valid Unicode."))?;
    let top = PathBuf::from(top.trim_end_matches(['\r', '\n']))
        .canonicalize()
        .map_err(|_| AppError::invalid("Git repository root is unavailable."))?;
    let relative = root
        .strip_prefix(&top)
        .map_err(|_| AppError::invalid("Git resolved outside the selected project."))?;
    let prefix = if relative.as_os_str().is_empty() {
        String::new()
    } else {
        format!("{}/", relative.to_string_lossy().replace('\\', "/"))
    };
    let status = git(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=all",
            "--",
            ".",
        ],
        STATUS_LIMIT,
    )
    .await?;
    if !status.success || status.truncated {
        return Err(AppError::invalid(
            "Git's change list is too large or unavailable. Open a narrower project folder.",
        ));
    }
    let branch = git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"], 4096).await?;
    Ok(Some(Snapshot {
        branch: if branch.success {
            String::from_utf8_lossy(&branch.bytes).trim().to_owned()
        } else {
            "Detached HEAD".into()
        },
        changes: parse_status(&status.bytes, &prefix)?,
    }))
}

pub(crate) async fn changes(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>> {
    api::no_query(request.uri())?;
    let root = workspace::root(&state, &id)?;
    let _slot = state.workspace_reads.try_acquire().map_err(|_| {
        AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "workspace_busy",
            "Project inspection is busy. Try again shortly.",
        )
    })?;
    match snapshot(&root).await? {
        None => Ok(Json(json!({"available":false,"changes":[]}))),
        Some(snapshot) => Ok(Json(
            json!({"available":true,"branch":snapshot.branch,"changes":snapshot.changes}),
        )),
    }
}

pub(crate) async fn diff(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>> {
    let query = api::query(request.uri(), &["path", "scope"])?;
    let path = query
        .get("path")
        .ok_or_else(|| AppError::invalid("Choose a changed file."))?;
    let scope = query.get("scope").map(String::as_str).unwrap_or("unstaged");
    if !matches!(scope, "staged" | "unstaged") {
        return Err(AppError::invalid("Diff scope must be staged or unstaged."));
    }
    switchyard_agent_tools::check_project_path(path).map_err(workspace::file_error)?;
    let root = workspace::root(&state, &id)?;
    switchyard_agent_tools::check_project_file_path(&root, path).map_err(workspace::file_error)?;
    let _slot = state.workspace_reads.try_acquire().map_err(|_| {
        AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "workspace_busy",
            "Project inspection is busy. Try again shortly.",
        )
    })?;
    let snapshot = snapshot(&root)
        .await?
        .ok_or_else(|| AppError::invalid("This project is not a Git repository."))?;
    let change = snapshot
        .changes
        .iter()
        .find(|change| change.path == *path)
        .ok_or_else(|| {
            AppError::new(
                StatusCode::CONFLICT,
                "change_no_longer_present",
                "This file is no longer changed. Refresh the change list.",
            )
        })?;
    if change.protected {
        return Err(AppError::invalid(
            "This change contains a protected file path.",
        ));
    }
    if change.untracked {
        return Err(AppError::invalid(
            "Untracked files have no Git diff. Open the file preview.",
        ));
    }
    let mut args = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--ignore-submodules=all",
        "--color=never",
        "--unified=3",
    ];
    if scope == "staged" {
        args.push("--cached");
    }
    args.extend(["--", path]);
    if let Some(original) = change.original_path.as_deref() {
        switchyard_agent_tools::check_project_file_path(&root, original)
            .map_err(workspace::file_error)?;
        args.push(original);
    }
    let output = git(&root, &args, DIFF_LIMIT).await?;
    if !output.success {
        return Err(AppError::local(
            "Git could not produce this diff. Refresh the change list.",
        ));
    }
    Ok(Json(
        json!({"path":path,"scope":scope,"diff":String::from_utf8_lossy(&output.bytes),"truncated":output.truncated}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn porcelain_keeps_rename_order_spaces_and_two_independent_statuses() {
        let changes = parse_status(
            b"MM src/a b.rs\0R  src/new.rs\0src/old.rs\0?? src/line\nname.rs\0 M other/no.rs\0",
            "src/",
        )
        .unwrap();
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].path, "a b.rs");
        assert_eq!(
            (&changes[0].staged, &changes[0].unstaged),
            (&"M".into(), &"M".into())
        );
        assert_eq!(changes[2].original_path.as_deref(), Some("old.rs"));
        assert!(changes[1].untracked);
    }
    #[test]
    fn metadata_secrets_and_pathspec_syntax_are_not_read_as_diffs() {
        let changes = parse_status(b" M .env\0?? :(glob)**\0 M normal.txt\0", "").unwrap();
        assert_eq!(changes.iter().filter(|change| change.protected).count(), 2);
        assert!(parse_status(b"R  incomplete\0", "").is_err());
    }
}
