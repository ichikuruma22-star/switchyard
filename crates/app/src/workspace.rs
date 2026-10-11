//! Project inspection and durable UI navigation. No agent operation is approved
//! or replayed by these routes.
use crate::{
    AppError, Result,
    api::{self, AppState},
};
use axum::{
    Json,
    extract::{Path, Request, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Selection {
    pub project_id: String,
    pub session_id: String,
    pub conversation_id: String,
    pub mode: String,
    pub tool: String,
    pub theme: String,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            project_id: String::new(),
            session_id: String::new(),
            conversation_id: String::new(),
            mode: "external".into(),
            tool: "conversation".into(),
            theme: "system".into(),
        }
    }
}

pub(crate) async fn selection(
    State(state): State<Arc<AppState>>,
    request: Request,
) -> Result<Json<Value>> {
    api::no_query(request.uri())?;
    Ok(Json(json!({"selection": state.engine.workspace_state()?})))
}

pub(crate) async fn save_selection(
    State(state): State<Arc<AppState>>,
    request: Request,
) -> Result<Json<Value>> {
    let selection: Selection = api::body(request).await?;
    if !matches!(selection.mode.as_str(), "native" | "external")
        || !matches!(
            selection.tool.as_str(),
            "conversation" | "files" | "changes"
        )
        || !matches!(selection.theme.as_str(), "system" | "light" | "dark")
        || [
            &selection.project_id,
            &selection.session_id,
            &selection.conversation_id,
        ]
        .iter()
        .any(|id| {
            id.len() > 128
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
    {
        return Err(AppError::invalid("Invalid workspace selection."));
    }
    let project = if selection.project_id.is_empty() {
        None
    } else {
        Some(state.engine.project(&selection.project_id)?)
    };
    if !selection.session_id.is_empty()
        && state
            .engine
            .session(&selection.session_id)?
            .session
            .project_id
            != selection.project_id
    {
        return Err(AppError::invalid("The session belongs to another project."));
    }
    if !selection.conversation_id.is_empty()
        && !state.adapters.list_runs().iter().any(|run| {
            run.conversation_id == selection.conversation_id
                && project
                    .as_ref()
                    .is_some_and(|project| project.root == run.project_path)
        })
    {
        return Err(AppError::invalid(
            "The conversation does not belong to this project.",
        ));
    }
    let value = serde_json::to_value(selection)
        .map_err(|_| AppError::invalid("Invalid workspace selection."))?;
    state.engine.save_workspace_state(&value)?;
    Ok(Json(json!({"selection":value})))
}

pub(crate) fn root(state: &AppState, id: &str) -> Result<PathBuf> {
    let project = state.engine.project(id)?;
    switchyard_agent_tools::project_root(std::path::Path::new(&project.root)).map_err(file_error)
}

pub(crate) fn file_error(error: switchyard_agent_tools::ToolError) -> AppError {
    match error {
        switchyard_agent_tools::ToolError::Invalid(message) => AppError::invalid(message),
        switchyard_agent_tools::ToolError::Io(error)
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            AppError::new(
                StatusCode::NOT_FOUND,
                "file_not_found",
                "This file or folder no longer exists.",
            )
        }
        _ => AppError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "file_unavailable",
            "This file or folder cannot be read. Check its permissions and refresh the project.",
        ),
    }
}

pub(crate) async fn directory(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>> {
    let query = api::query(request.uri(), &["path"])?;
    let path = query.get("path").cloned().unwrap_or_else(|| ".".into());
    let root = root(&state, &id)?;
    let _slot = state
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
    let result = tokio::task::spawn_blocking(move || {
        let _slot = _slot;
        switchyard_agent_tools::project_directory(&root, &path)
    })
    .await
    .map_err(|_| AppError::local("The folder could not be read."))?
    .map_err(file_error)?;
    Ok(Json(result))
}

pub(crate) async fn file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>> {
    let query = api::query(request.uri(), &["path"])?;
    let path = query
        .get("path")
        .cloned()
        .ok_or_else(|| AppError::invalid("Choose a project-relative file path."))?;
    let root = root(&state, &id)?;
    let _slot = state
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
    let result = tokio::task::spawn_blocking(move || {
        let _slot = _slot;
        switchyard_agent_tools::project_file(&root, &path)
    })
    .await
    .map_err(|_| AppError::local("The file could not be read."))?;
    if result.status != "completed" {
        return Err(AppError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "file_preview_unavailable",
            result
                .error
                .unwrap_or_else(|| "This file cannot be previewed.".into()),
        ));
    }
    Ok(Json(result.output))
}
