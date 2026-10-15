use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize)]
pub struct AdapterStatus {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub supported: bool,
    pub version: Option<String>,
    pub executable: Option<String>,
    pub status: String,
    pub permission_boundary: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRequest {
    pub adapter_id: String,
    pub project_path: PathBuf,
    pub prompt: String,
    /// Caller UUID: retries return the same run only for the same complete request.
    pub command_id: String,
    /// Continue the saved agent conversation of this earlier run. It must be the latest
    /// finished run of its conversation, in the same project and account.
    #[serde(default)]
    pub continue_run_id: Option<String>,
    /// Resolved by the trusted account manager, never accepted from HTTP JSON.
    #[serde(skip)]
    pub profile: Option<ProfileBinding>,
    /// The project's opt-in approval setting when the run starts. Resolved by the
    /// app from saved project state, never accepted from HTTP JSON.
    #[serde(skip)]
    pub auto_approve: AutoApproval,
}

/// Which complete, single-operation requests Switchya answers itself instead of
/// asking. Requests that cannot be allowed once (deny-only) always wait for a
/// person, and the agent's own sandbox and permission mode are unchanged.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutoApproval {
    pub edits: bool,
    pub commands: bool,
}

/// The category an approval request belongs to, when Switchya can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApprovalKind {
    Edit,
    Command,
}

impl AutoApproval {
    pub(crate) fn allows(&self, kind: Option<ApprovalKind>) -> bool {
        match kind {
            Some(ApprovalKind::Edit) => self.edits,
            Some(ApprovalKind::Command) => self.commands,
            None => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileBinding {
    pub id: String,
    pub name: String,
    pub agent_id: String,
    pub home: PathBuf,
    pub managed: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Starting,
    Running,
    AwaitingApproval,
    Interrupting,
    Interrupted,
    Completed,
    Failed,
    RecoveryRequired,
}

impl RunState {
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Running | Self::AwaitingApproval | Self::Interrupting
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdapterRun {
    pub id: String,
    pub adapter_id: String,
    pub command_id: String,
    /// Shared by every run that continues the same agent conversation. Equal to the
    /// first run's id.
    pub conversation_id: String,
    /// The earlier run whose saved thread this run resumed.
    pub continued_from: Option<String>,
    /// First line of the task, at most 80 characters. Display as text.
    #[serde(default)]
    pub title: String,
    pub profile_id: Option<String>,
    pub profile_name: Option<String>,
    pub project_path: String,
    pub state: RunState,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub model: Option<String>,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub last_seq: u64,
    pub first_retained_seq: u64,
    pub pending_approvals: Vec<AdapterApproval>,
    /// True when neither Switchya nor the agent keeps this run after the host exits.
    pub ephemeral: bool,
    /// Set when this host could not save part of the run's history.
    #[serde(default)]
    pub history_error: Option<String>,
    pub permission_boundary: String,
    /// Approval setting this run started with. Changes apply to later runs.
    #[serde(default)]
    pub auto_approve: AutoApproval,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdapterApproval {
    pub id: String,
    pub expected_hash: String,
    pub method: String,
    /// Exact CLI request plus the associated item, when present. Render as text.
    pub preview: Value,
    pub can_allow_once: bool,
    pub permission_boundary: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    AllowOnce,
    Deny,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdapterEvent {
    pub schema_version: u32,
    pub run_id: String,
    pub seq: u64,
    pub at_ms: u64,
    pub kind: String,
    pub payload: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Protocol(String),
    #[error("{0}")]
    Limit(String),
}

impl AdapterError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid_request",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Unavailable(_) => "adapter_unavailable",
            Self::Protocol(_) => "adapter_protocol_error",
            Self::Limit(_) => "limit_reached",
        }
    }
}

pub type Result<T> = std::result::Result<T, AdapterError>;

pub const PERMISSION_BOUNDARY: &str = "Codex uses its read-only shell sandbox with network disabled, on-request approvals and human review. Read-only operations may run without prompting; writes or elevation require CLI approval. Codex enforces that policy; Switchya does not add an OS security sandbox. Allow once may permit the displayed command or file change with your OS permissions. The selected profile stays bound to this run and to any continuation of its conversation; configured integrations may have separate permissions. Saved history is never replayed: continuing sends only your new message, and interrupted work requires review first.";

pub const CLAUDE_PERMISSION_BOUNDARY: &str = "Claude Code uses default permission mode and the selected account's CLI settings. Read-only tools and tools allowed by existing permission rules may run without prompting. Switchya displays the permission requests Claude sends; Allow once approves only the displayed tool input and does not save a permission rule. Tools, hooks and integrations run with your OS permissions; Switchya does not add an OS security sandbox. The account and project remain bound to this conversation. Continuing sends only your new message, and uncertain work requires review first.";
pub const ANTIGRAVITY_PERMISSION_BOUNDARY: &str = "Antigravity runs in plan mode: it can inspect the project and answer, but cannot apply edits. Tool activity runs with your OS permissions inside the CLI's own plan-mode policy; Switchya does not add an OS security sandbox and shows the activity Antigravity reports. Antigravity does not expose an account-status command, so Switchya cannot confirm sign-in before a run; the CLI reports authentication failures itself. The account and project remain bound to this conversation. Continuing sends only your new message, and uncertain work requires review first.";

pub(crate) fn permission_boundary(agent: &str) -> &'static str {
    if agent == "claude" {
        CLAUDE_PERMISSION_BOUNDARY
    } else if agent == "antigravity" {
        ANTIGRAVITY_PERMISSION_BOUNDARY
    } else {
        PERMISSION_BOUNDARY
    }
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
