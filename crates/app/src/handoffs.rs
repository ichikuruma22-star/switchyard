//! Project-scoped, bounded context for the workbench and MCP clients.
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
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use switchyard_agent::CheckpointDraft;

pub(crate) async fn mcp_config(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>> {
    api::no_query(request.uri())?;
    let project = state.engine.project(&id)?;
    let executable = std::env::current_exe().ok();
    let name = if cfg!(windows) {
        "switchya.exe"
    } else {
        "switchya"
    };
    let executable = executable
        .as_ref()
        .and_then(|path| path.parent())
        .map(|parent| parent.join(name))
        .filter(|path| path.is_file());
    let command = executable
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "switchya".into());
    let launch = crate::terminal_platform::shell_directory(&crate::launch::HostRecord::path(
        &state.data_dir,
    ));
    let root = crate::terminal_platform::shell_directory(std::path::Path::new(&project.root));
    let args = json!(["--launch-info", launch, "mcp", "--project", root]);
    let codex_configuration = format!(
        "[mcp_servers.switchya]\ncommand = {}\nargs = {args}\n",
        json!(command)
    );
    Ok(Json(
        json!({"configuration":{"mcpServers":{"switchya":{"command":command,"args":args}}},"codex_configuration":codex_configuration,"requires_running_host":true}),
    ))
}

fn number(
    query: &BTreeMap<String, String>,
    name: &str,
    default: usize,
    max: usize,
) -> Result<usize> {
    let value = query.get(name).map_or(Ok(default), |value| {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(AppError::invalid(format!(
                "{name} must be a positive decimal integer."
            )));
        }
        value
            .parse()
            .map_err(|_| AppError::invalid(format!("{name} is out of range.")))
    })?;
    if !(1..=max).contains(&value) {
        return Err(AppError::invalid(format!(
            "{name} must be from 1 to {max}."
        )));
    }
    Ok(value)
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>> {
    let query = api::query(request.uri(), &["before", "limit"])?;
    let before = query
        .get("before")
        .map(|value| {
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(AppError::invalid(
                    "Use the next_before cursor from the previous page.",
                ));
            }
            value.parse::<i64>().map_err(|_| {
                AppError::invalid("Use the next_before cursor from the previous page.")
            })
        })
        .transpose()?;
    let limit = number(&query, "limit", 20, 100)?;
    Ok(Json(state.engine.checkpoints(&id, before, limit)?))
}

pub(crate) async fn read(
    State(state): State<Arc<AppState>>,
    Path((id, note)): Path<(String, String)>,
    request: Request,
) -> Result<Json<Value>> {
    api::no_query(request.uri())?;
    Ok(Json(
        json!({"checkpoint":state.engine.checkpoint(&id,&note)?}),
    ))
}

pub(crate) async fn save(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>> {
    let note: CheckpointDraft = api::body(request).await?;
    Ok(Json(
        json!({"checkpoint":state.engine.save_checkpoint(&id,note)?}),
    ))
}

pub(crate) async fn search(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>> {
    let query = api::query(request.uri(), &["query", "path", "limit"])?;
    let text = query
        .get("query")
        .ok_or_else(|| AppError::invalid("Provide query text to search for."))?
        .clone();
    let path = query.get("path").cloned().unwrap_or_else(|| ".".into());
    let limit = number(&query, "limit", 30, 100)?;
    let root = workspace::root(&state, &id)?;
    let slot = state
        .workspace_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            AppError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "workspace_busy",
                "Project inspection is busy. Try again shortly.",
            )
        })?;
    // Cancel the bounded filesystem walk if the HTTP request is dropped.
    let cancel = tokio_util::sync::CancellationToken::new();
    let _guard = cancel.clone().drop_guard();
    let result = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        switchyard_agent_tools::project_search(&root, &text, &path, limit, &cancel)
    })
    .await
    .map_err(|_| AppError::local("Project search could not finish."))?
    .map_err(workspace::file_error)?;
    if result.status != "completed" {
        return Err(AppError::invalid(
            result
                .error
                .unwrap_or_else(|| "Project search was interrupted.".into()),
        ));
    }
    Ok(Json(result.output))
}

pub(crate) async fn conversations(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>> {
    let query = api::query(request.uri(), &["before", "limit"])?;
    let limit = number(&query, "limit", 20, 100)?;
    let project = state.engine.project(&id)?;
    let mut items = Vec::new();
    for session in state.engine.list_sessions(Some(&id))? {
        let key = format!("{:020}:native:{}", session.created_at_ms, session.id);
        items.push((key,json!({"kind":"native","id":session.id,"title":session.title,"model":session.model,"state":session.state,"created_at_ms":session.created_at_ms,"last_seq":session.last_seq})));
    }
    // Adapter runs are individual turns: conversation_id joins continuations.
    for run in state
        .adapters
        .list_runs()
        .into_iter()
        .filter(|run| run.project_path == project.root)
    {
        let key = format!("{:020}:adapter:{}", run.started_at_ms, run.id);
        items.push((key,json!({"kind":"adapter","id":run.id,"conversation_id":run.conversation_id,"continued_from":run.continued_from,"title":run.title,"adapter":run.adapter_id,"state":run.state,"created_at_ms":run.started_at_ms,"last_seq":run.last_seq})));
    }
    items.sort_unstable_by(|left, right| right.0.cmp(&left.0));
    if let Some(before) = query.get("before") {
        if before.len() > 160
            || !before
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":_-".contains(&byte))
        {
            return Err(AppError::invalid(
                "Use the next_before cursor from the previous page.",
            ));
        }
        items.retain(|(key, _)| key < before);
    }
    let next = if items.len() > limit {
        Some(items[limit - 1].0.clone())
    } else {
        None
    };
    Ok(Json(
        json!({"conversations":items.into_iter().take(limit).map(|(_,item)|item).collect::<Vec<_>>(),"next_before":next}),
    ))
}

pub(crate) async fn conversation(
    State(state): State<Arc<AppState>>,
    Path((id, kind, conversation)): Path<(String, String, String)>,
    request: Request,
) -> Result<Json<Value>> {
    let (after, limit) = api::event_cursor(request.uri(), 100)?;
    let project = state.engine.project(&id)?;
    let (metadata, events, last_seq, first_retained_seq) = match kind.as_str() {
        "native" => {
            let view = state.engine.session(&conversation)?;
            if view.session.project_id != id {
                return Err(AppError::new(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "Conversation not found in this project.",
                ));
            }
            let events = state.engine.events(&conversation, after, limit)?;
            (
                json!({"kind":kind,"id":conversation,"title":view.session.title,"model":view.session.model,"state":view.session.state}),
                serde_json::to_value(events)
                    .map_err(|_| AppError::local("Could not read conversation events."))?,
                view.session.last_seq,
                1,
            )
        }
        "adapter" => {
            let run = state.adapters.run(&conversation)?;
            if run.project_path != project.root {
                return Err(AppError::new(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "Conversation not found in this project.",
                ));
            }
            let events = state.adapters.events(&conversation, after, limit)?;
            (
                json!({"kind":kind,"id":conversation,"conversation_id":run.conversation_id,"continued_from":run.continued_from,"title":run.title,"adapter":run.adapter_id,"state":run.state}),
                serde_json::to_value(events)
                    .map_err(|_| AppError::local("Could not read conversation events."))?,
                run.last_seq,
                run.first_retained_seq,
            )
        }
        _ => {
            return Err(AppError::invalid(
                "Conversation kind must be native or adapter.",
            ));
        }
    };
    let mut bounded = Vec::new();
    let mut bytes = 0;
    for mut event in events.as_array().cloned().unwrap_or_default() {
        let payload = serde_json::to_string(&event["payload"])
            .map_err(|_| AppError::local("Could not format a conversation event."))?;
        if payload.len() > 48 * 1024 {
            event["payload"] = json!({"excerpt":payload.chars().take(12*1024).collect::<String>(),"truncated":true});
        }
        let size = serde_json::to_vec(&event)
            .map_err(|_| AppError::local("Could not format a conversation event."))?
            .len();
        if !bounded.is_empty() && bytes + size > 256 * 1024 {
            break;
        }
        bytes += size;
        bounded.push(event);
    }
    let events = json!(bounded);
    let next = events
        .as_array()
        .and_then(|events| events.last())
        .and_then(|event| event["seq"].as_u64())
        .unwrap_or(after);
    Ok(Json(
        json!({"conversation":metadata,"events":events,"next_after_seq":next,"has_more":next<last_seq,"last_seq":last_seq,"first_retained_seq":first_retained_seq,"history_gap":after.saturating_add(1)<first_retained_seq}),
    ))
}
