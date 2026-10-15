use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// All session data is private local state under this directory.
#[derive(Clone, Debug)]
pub struct EngineOptions {
    pub data_dir: PathBuf,
    pub max_steps: u32,
    pub max_output_tokens: u64,
    pub max_context_bytes: usize,
    /// Files or directories whose edits always need a human decision, even in
    /// a project that auto-approves edits. The data directory is always one.
    pub review_paths: Vec<PathBuf>,
}

impl EngineOptions {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            max_steps: 16,
            max_output_tokens: 4096,
            max_context_bytes: 2 * 1024 * 1024,
            review_paths: Vec::new(),
        }
    }
}

/// Opt-in trust that replaces the per-operation human decision for file edits
/// and/or shell commands. Both are off by default. Auto-approved operations are
/// journalled like reviewed ones; tool preparation still refuses protected
/// paths (repository metadata, likely secrets, links and short-name aliases).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AutoApprove {
    #[serde(default)]
    pub edits: bool,
    #[serde(default)]
    pub commands: bool,
}

impl AutoApprove {
    pub const OFF: Self = Self {
        edits: false,
        commands: false,
    };

    pub fn any(&self) -> bool {
        self.edits || self.commands
    }

    /// Only mutating tools need approval; anything unrecognized stays manual.
    pub fn allows(&self, tool: &str) -> bool {
        match tool {
            "apply_patch" => self.edits,
            "run_command" => self.commands,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicySource {
    #[default]
    Project,
    Session,
}

/// The trust actually applied to a session's next operation and where it came from.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EffectiveAutoApprove {
    pub edits: bool,
    pub commands: bool,
    pub source: PolicySource,
}

impl EffectiveAutoApprove {
    pub fn resolve(project: &Project, session: &Session) -> Self {
        let (policy, source) = match session.auto_approve {
            Some(policy) => (policy, PolicySource::Session),
            None => (project.auto_approve, PolicySource::Project),
        };
        Self {
            edits: policy.edits,
            commands: policy.commands,
            source,
        }
    }

    pub fn policy(&self) -> AutoApprove {
        AutoApprove {
            edits: self.edits,
            commands: self.commands,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Idle,
    Running,
    AwaitingApproval,
    Interrupted,
    RecoveryRequired,
    Failed,
    Completed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root: String,
    pub created_at_ms: i64,
    /// Project-wide trust; sessions inherit it unless they override it.
    #[serde(default)]
    pub auto_approve: AutoApprove,
}

/// A portable progress note. Its content is reported context, never authority
/// to approve an operation or proof that a command actually succeeded.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CheckpointDraft {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub source: String,
    #[serde(default)]
    pub next_steps: Vec<String>,
    #[serde(default)]
    pub verification: String,
    #[serde(default)]
    pub risks: Vec<String>,
}

impl CheckpointDraft {
    pub(crate) fn validate(&self) -> Result<()> {
        let bounded = |text: &str, max: usize| {
            text.len() <= max
                && !text
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        };
        if !uuid::Uuid::parse_str(&self.id).is_ok_and(|id| id.to_string() == self.id)
            || self.title.trim().is_empty()
            || !bounded(&self.title, 200)
            || self.summary.trim().is_empty()
            || !bounded(&self.summary, 16_384)
            || self.source.trim().is_empty()
            || !bounded(&self.source, 120)
            || !bounded(&self.verification, 16_384)
            || self.next_steps.len() > 20
            || self.risks.len() > 20
            || self
                .next_steps
                .iter()
                .chain(&self.risks)
                .any(|item| !bounded(item, 1024))
            || serde_json::to_vec(self)?.len() > 65_536
        {
            return Err(AgentError::Invalid("A checkpoint needs a UUID, title, summary and source, with at most 20 next steps and 20 risks. Keep the complete note below 64 KiB and omit terminal control characters.".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Checkpoint {
    pub project_id: String,
    pub created_at_ms: i64,
    #[serde(flatten)]
    pub content: CheckpointDraft,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub model: String,
    pub title: String,
    pub state: SessionState,
    pub active_run_id: Option<String>,
    pub revision: u64,
    pub last_seq: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// `None` inherits the project's trust; `Some` overrides it for this session.
    #[serde(default)]
    pub auto_approve: Option<AutoApprove>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub session_id: String,
    pub command_id: String,
    pub state: SessionState,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub schema_version: u32,
    pub session_id: String,
    pub run_id: Option<String>,
    pub seq: u64,
    pub at_ms: i64,
    pub kind: String,
    pub payload: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Operation {
    pub id: String,
    pub session_id: String,
    pub run_id: String,
    pub call_id: String,
    pub name: String,
    pub arguments_hash: String,
    /// Exact normalized arguments bound to this single-use approval.
    pub arguments: Value,
    pub requires_approval: bool,
    pub state: String,
    pub preview: Value,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    AllowOnce,
    Deny,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionView {
    pub session: Session,
    pub project: Project,
    pub pending_operation: Option<Operation>,
    #[serde(default)]
    pub auto_approve: EffectiveAutoApprove,
    /// Initial window of public events. Fetch later events with a sequence cursor.
    pub events: Vec<Event>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EngineStatus {
    pub active_runs: usize,
    pub permission_mode: String,
    pub shell_is_sandboxed: bool,
    pub max_steps: u32,
    pub max_output_tokens: u64,
    pub max_context_bytes: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Permission(String),
    #[error("{0}")]
    Gateway(String),
    #[error("local session storage failed: {0}")]
    Storage(String),
    #[error("the run was interrupted")]
    Interrupted,
    #[error("{0}")]
    Limit(String),
}

impl AgentError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid_request",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Permission(_) => "permission_denied",
            Self::Gateway(_) => "gateway_failed",
            Self::Storage(_) => "storage_failed",
            Self::Interrupted => "interrupted",
            Self::Limit(_) => "limit_reached",
        }
    }
}

impl From<rusqlite::Error> for AgentError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<serde_json::Error> for AgentError {
    fn from(error: serde_json::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AgentError>;

pub(crate) fn now_ms() -> i64 {
    switchyard_core::util::now_unix_ms()
}

pub(crate) fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}
