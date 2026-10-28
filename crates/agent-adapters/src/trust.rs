//! Which agent requests a project's opt-in approval setting answers without a
//! person. Every other request waits for one.

use crate::{
    AdapterApproval,
    types::{ApprovalKind, AutoApproval},
};
use serde_json::{Value, json};
use std::{
    fmt,
    path::{Component, Path, PathBuf, Prefix},
    sync::Arc,
};
use switchyard_agent_tools::{check_unreviewed_edit, path_within};

/// Reads the project's approval setting each time a request arrives, so a
/// change, turning trust off in particular, reaches runs already in progress.
#[derive(Clone, Default)]
pub struct AutoApprovalSource(Option<Arc<dyn Fn() -> AutoApproval + Send + Sync>>);

impl AutoApprovalSource {
    pub fn new(read: impl Fn() -> AutoApproval + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(read)))
    }

    /// The setting now. Without a source every request asks a person.
    pub fn current(&self) -> AutoApproval {
        self.0.as_ref().map(|read| read()).unwrap_or_default()
    }
}

impl fmt::Debug for AutoApprovalSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            Some(_) => "AutoApprovalSource(project setting)",
            None => "AutoApprovalSource(off)",
        })
    }
}

/// A run's approval setting and the locations whose edits always go to a person.
pub(crate) struct Trust {
    source: AutoApprovalSource,
    /// Canonical project folder, the agent's working folder.
    project: PathBuf,
    protected: Vec<PathBuf>,
}

impl Trust {
    pub(crate) fn new(
        source: AutoApprovalSource,
        project: PathBuf,
        protected: Vec<PathBuf>,
    ) -> Self {
        let protected = protected
            .into_iter()
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| path.canonicalize().unwrap_or(path))
            .collect();
        Self {
            source,
            project,
            protected,
        }
    }

    /// The setting that approves this request without a person, if any. An
    /// edit qualifies only when every path it writes is an ordinary project
    /// file outside the protected locations.
    pub(crate) fn approves(
        &self,
        kind: Option<ApprovalKind>,
        targets: Option<&[String]>,
    ) -> Option<AutoApproval> {
        let policy = self.source.current();
        if !policy.allows(kind) {
            return None;
        }
        if kind == Some(ApprovalKind::Edit)
            && !targets.is_some_and(|targets| {
                !targets.is_empty() && targets.iter().all(|target| self.editable(target))
            })
        {
            return None;
        }
        Some(policy)
    }

    fn editable(&self, target: &str) -> bool {
        if target.is_empty() || target.len() > 4096 || target.contains('\0') {
            return false;
        }
        let path = Path::new(target);
        let relative = if path.is_absolute() {
            match relative_to(path, &self.project) {
                Some(relative) => relative,
                None => return false,
            }
        } else {
            // Agents resolve relative names against their working folder.
            target.to_owned()
        };
        let full = self.project.join(&relative);
        check_unreviewed_edit(&self.project, &relative).is_ok()
            && !self
                .protected
                .iter()
                .any(|protected| path_within(&full, protected) || path_within(protected, &full))
    }
}

/// `path` below `root` as a relative name. Root components must match exactly,
/// except for letter case and `\\?\` prefixes on Windows.
fn relative_to(path: &Path, root: &Path) -> Option<String> {
    let mut path = path
        .components()
        .filter(|component| !matches!(component, Component::CurDir));
    for expected in root
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
    {
        if !same(path.next()?, expected) {
            return None;
        }
    }
    let rest: Vec<_> = path
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    (!rest.is_empty()).then(|| rest.join("/"))
}

fn same(a: Component<'_>, b: Component<'_>) -> bool {
    match (a, b) {
        (Component::Prefix(a), Component::Prefix(b)) => match (a.kind(), b.kind()) {
            (
                Prefix::Disk(a) | Prefix::VerbatimDisk(a),
                Prefix::Disk(b) | Prefix::VerbatimDisk(b),
            ) => a.eq_ignore_ascii_case(&b),
            (
                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share),
                Prefix::UNC(other_server, other_share)
                | Prefix::VerbatimUNC(other_server, other_share),
            ) => {
                server.to_string_lossy().to_lowercase()
                    == other_server.to_string_lossy().to_lowercase()
                    && share.to_string_lossy().to_lowercase()
                        == other_share.to_string_lossy().to_lowercase()
            }
            _ => a.as_os_str() == b.as_os_str(),
        },
        (Component::Normal(a), Component::Normal(b)) if cfg!(windows) => {
            a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
        }
        (a, b) => a == b,
    }
}

/// The files a Claude Code edit tool writes. Claude Code takes absolute paths;
/// a relative one could resolve against a folder a command moved to, so it
/// returns `None`, as do other tools.
pub(crate) fn claude_targets(name: &str, input: &Value) -> Option<Vec<String>> {
    claude_paths(name, input).filter(|paths| paths.iter().all(|path| Path::new(path).is_absolute()))
}

fn claude_paths(name: &str, input: &Value) -> Option<Vec<String>> {
    let key = match name {
        "Write" | "Edit" | "MultiEdit" => "file_path",
        "NotebookEdit" => "notebook_path",
        _ => return None,
    };
    input
        .get(key)
        .and_then(Value::as_str)
        .map(|path| vec![path.to_owned()])
}

/// Every path a Codex file change writes, including move destinations. An
/// unknown change kind or field returns `None`.
pub(crate) fn codex_targets(item: Option<&Value>) -> Option<Vec<String>> {
    let mut targets = Vec::new();
    for change in item?.get("changes")?.as_array()? {
        targets.push(change.get("path")?.as_str()?.to_owned());
        let kind = change.get("kind")?.as_object()?;
        if !matches!(kind.get("type")?.as_str()?, "add" | "delete" | "update") {
            return None;
        }
        for (key, value) in kind {
            match (key.as_str(), value) {
                ("type", _) | ("move_path" | "movePath", Value::Null) => {}
                ("move_path" | "movePath", Value::String(path)) => targets.push(path.clone()),
                _ => return None,
            }
        }
    }
    Some(targets)
}

/// A short record of what a decision covered, kept with the decision so the
/// run's history still names it after the full request leaves the window.
pub(crate) fn request_summary(approval: &AdapterApproval) -> Value {
    let preview = &approval.preview;
    match approval.method.as_str() {
        "item/commandExecution/requestApproval" => json!({
            "command": preview.pointer("/params/command"),
            "cwd": preview.pointer("/params/cwd"),
        }),
        "item/fileChange/requestApproval" => {
            // Name every path, even in a change too unusual to approve itself.
            let paths = codex_targets(preview.get("item")).unwrap_or_else(|| {
                preview
                    .pointer("/item/changes")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|change| change.get("path").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect()
            });
            json!({"paths": paths})
        }
        _ => {
            let tool = preview
                .pointer("/request/tool_name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let input = preview.pointer("/request/input").unwrap_or(&Value::Null);
            match claude_paths(tool, input) {
                Some(paths) => json!({"tool": tool, "paths": paths}),
                None => json!({"tool": tool, "command": input.get("command")}),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trust(project: &Path, policy: AutoApproval, protected: Vec<PathBuf>) -> Trust {
        Trust::new(
            AutoApprovalSource::new(move || policy),
            project.canonicalize().unwrap(),
            protected,
        )
    }

    const EDITS: AutoApproval = AutoApproval {
        edits: true,
        commands: false,
    };

    #[test]
    fn edits_auto_approve_only_ordinary_project_files() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::create_dir(directory.path().join("state")).unwrap();
        let state = project.join("state");
        std::fs::create_dir(&state).unwrap();
        let trust = trust(&project, EDITS, vec![state, PathBuf::new()]);
        let root = trust.project.clone();
        let inside = |relative: &str| root.join(relative).to_string_lossy().into_owned();
        let approve = |targets: &[String]| trust.approves(Some(ApprovalKind::Edit), Some(targets));
        for targets in [
            vec!["src/main.rs".to_owned()],
            vec![inside("src/main.rs")],
            vec![inside("new/file.txt"), "README.md".into()],
        ] {
            assert_eq!(approve(&targets), Some(EDITS), "{targets:?}");
        }
        let outside = directory.path().join("state/notes.txt");
        for targets in [
            vec![],
            vec![outside.to_string_lossy().into_owned()],
            vec![inside("../outside.txt")],
            vec![inside(".git/hooks/pre-commit")],
            vec![inside(".claude/settings.json")],
            vec![inside(".bashrc")],
            vec![inside("STATE/notes.txt")],
            vec!["src/ok.rs".into(), inside(".env")],
            vec![String::new()],
        ] {
            assert_eq!(approve(&targets), None, "{targets:?}");
        }
        // Without the targets an edit always asks.
        assert_eq!(trust.approves(Some(ApprovalKind::Edit), None), None);
        // Commands and unknown requests follow the setting alone.
        assert_eq!(trust.approves(Some(ApprovalKind::Command), None), None);
        assert_eq!(trust.approves(None, Some(&["src/main.rs".into()])), None);
    }

    #[test]
    fn the_setting_is_read_for_every_request() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let directory = tempfile::tempdir().unwrap();
        let on = Arc::new(AtomicBool::new(true));
        let read = on.clone();
        let trust = Trust::new(
            AutoApprovalSource::new(move || AutoApproval {
                edits: false,
                commands: read.load(Ordering::SeqCst),
            }),
            directory.path().canonicalize().unwrap(),
            Vec::new(),
        );
        assert!(trust.approves(Some(ApprovalKind::Command), None).is_some());
        on.store(false, Ordering::SeqCst);
        assert!(trust.approves(Some(ApprovalKind::Command), None).is_none());
        assert_eq!(
            AutoApprovalSource::default().current(),
            AutoApproval::default()
        );
    }

    #[test]
    fn change_targets_include_moves_and_refuse_unknown_shapes() {
        let item = json!({"type":"fileChange","changes":[
            {"path":"a.txt","kind":{"type":"add"},"diff":"+a"},
            {"path":"b.txt","kind":{"type":"update","move_path":"c.txt"},"diff":""},
            {"path":"d.txt","kind":{"type":"update","move_path":null},"diff":""},
        ]});
        assert_eq!(
            codex_targets(Some(&item)),
            Some(vec![
                "a.txt".into(),
                "b.txt".into(),
                "c.txt".into(),
                "d.txt".into()
            ])
        );
        for change in [
            json!({"path":"a.txt","kind":{"type":"rename"}}),
            json!({"path":"a.txt","kind":{"type":"update","target":"x"}}),
            json!({"path":"a.txt","kind":"update"}),
            json!({"kind":{"type":"add"}}),
        ] {
            assert_eq!(codex_targets(Some(&json!({"changes":[change]}))), None);
        }
        let notebook = std::env::temp_dir().join("n.ipynb");
        let notebook = notebook.to_string_lossy();
        assert_eq!(
            claude_targets("NotebookEdit", &json!({"notebook_path":notebook})),
            Some(vec![notebook.clone().into_owned()])
        );
        // Claude Code takes absolute paths; anything else asks a person.
        assert_eq!(claude_targets("Write", &json!({"file_path":"n.txt"})), None);
        assert_eq!(claude_targets("Edit", &json!({"path":"x"})), None);
        assert_eq!(claude_targets("Bash", &json!({"command":"ls"})), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_roots_match_across_case_and_verbatim_prefixes() {
        assert_eq!(
            relative_to(
                Path::new(r"c:\Projects\App\src\main.rs"),
                Path::new(r"\\?\C:\projects\app")
            ),
            Some("src/main.rs".into())
        );
        assert_eq!(
            relative_to(
                Path::new(r"D:\projects\app\x"),
                Path::new(r"C:\projects\app")
            ),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_roots_match_exactly() {
        assert_eq!(
            relative_to(Path::new("/srv/app/src/main.rs"), Path::new("/srv/app")),
            Some("src/main.rs".into())
        );
        assert_eq!(
            relative_to(Path::new("/srv/App/src/main.rs"), Path::new("/srv/app")),
            None
        );
        assert_eq!(
            relative_to(Path::new("/srv/app"), Path::new("/srv/app")),
            None
        );
    }
}
