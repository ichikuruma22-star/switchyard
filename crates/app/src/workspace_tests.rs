use super::*;
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

struct Fixture {
    directory: tempfile::TempDir,
    host: RunningHost,
    client: Client,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let host = start_host(AppOptions::new(
            directory.path().join("gateway.toml"),
            directory.path().join("data"),
        ))
        .await
        .unwrap();
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(12))
            .build()
            .unwrap();
        Self {
            directory,
            host,
            client,
        }
    }
    async fn request(
        &self,
        method: Method,
        path: &str,
        value: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = self
            .client
            .request(method, format!("{}/api{path}", self.host.base_url()))
            .bearer_auth(&self.host.launch.token);
        if let Some(value) = value {
            request = request.json(&value);
        }
        let response = request.send().await.unwrap();
        (response.status(), response.json().await.unwrap())
    }
    async fn project(&self, path: &Path) -> String {
        let (status, value) = self
            .request(Method::POST, "/projects", Some(json!({"path":path})))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{value}");
        value["project"]["id"].as_str().unwrap().into()
    }
}

#[tokio::test]
async fn workspace_reads_real_files_respects_boundaries_and_restores_selection_on_another_port() {
    let fixture = Fixture::new().await;
    let root = fixture.directory.path().join("project");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::create_dir(root.join("node_modules")).unwrap();
    fs::write(
        root.join("src/hello.txt"),
        "hello <script>text only</script>\n",
    )
    .unwrap();
    fs::write(root.join(".env"), "PRIVATE_FIXTURE=yes").unwrap();
    fs::write(
        fixture.directory.path().join("outside.txt"),
        "OUTSIDE_FIXTURE",
    )
    .unwrap();
    fs::write(root.join("large.txt"), "a".repeat(300_000)).unwrap();
    let id = fixture.project(&root).await;
    let (status, list) = fixture
        .request(Method::GET, &format!("/projects/{id}/files"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["entries"][0]["name"], "src");
    assert_eq!(list["omitted"], 2);
    let (status, file) = fixture
        .request(
            Method::GET,
            &format!("/projects/{id}/file?path=src%2Fhello.txt"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(file["content"], "hello <script>text only</script>\n");
    for path in [
        "..%2Foutside.txt",
        ".env",
        "src%2F..%2F.env",
        "C%3A%2FWindows%2Fwin.ini",
    ] {
        let (status, _) = fixture
            .request(
                Method::GET,
                &format!("/projects/{id}/file?path={path}"),
                None,
            )
            .await;
        assert!(status.is_client_error(), "{path}");
    }
    let (_, large) = fixture
        .request(
            Method::GET,
            &format!("/projects/{id}/file?path=large.txt"),
            None,
        )
        .await;
    assert_eq!(large["truncated"], true);
    assert_eq!(large["content"].as_str().unwrap().len(), 256 * 1024);
    let unauthorized = fixture
        .client
        .get(format!(
            "{}/api/projects/{id}/files",
            fixture.host.base_url()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let selection = json!({"project_id":id,"session_id":"","conversation_id":"","mode":"external","tool":"changes","theme":"dark"});
    let (status, _) = fixture
        .request(Method::POST, "/workspace", Some(selection.clone()))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = fixture
        .request(
            Method::POST,
            "/workspace",
            Some(json!({"project_id":id,"mode":"invalid"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let old_port = fixture.host.base_url().to_owned();
    fixture.host.shutdown().await.unwrap();
    // Reserve the previous port to prove this is server-side persistence.
    let address = old_port.strip_prefix("http://").unwrap();
    let reserve = std::net::TcpListener::bind(address).unwrap();
    let reopened = start_host(AppOptions::new(
        fixture.directory.path().join("gateway.toml"),
        fixture.directory.path().join("data"),
    ))
    .await
    .unwrap();
    assert_ne!(reopened.base_url(), old_port);
    let saved: Value = fixture
        .client
        .get(format!("{}/api/workspace", reopened.base_url()))
        .bearer_auth(&reopened.launch.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved["selection"], selection);
    reopened.shutdown().await.unwrap();
    drop(reserve);
}

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=Switchya fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn git_review_distinguishes_staged_working_and_untracked_files_in_a_subproject() {
    let fixture = Fixture::new().await;
    let root = fixture.directory.path().join("repo");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("src")).unwrap();
    git(&root, &["init", "-q"]);
    fs::write(root.join("src/a b.txt"), "before\n").unwrap();
    fs::write(root.join("outside.txt"), "outside before\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "fixture"]);
    fs::write(root.join("src/a b.txt"), "staged\n").unwrap();
    git(&root, &["add", "src/a b.txt"]);
    fs::write(root.join("src/a b.txt"), "working\n").unwrap();
    fs::write(root.join("outside.txt"), "outside after\n").unwrap();
    fs::write(root.join("src/new.txt"), "new\n").unwrap();
    fs::write(root.join("src/.env"), "FIXTURE_SECRET=yes").unwrap();
    // Reading a diff must not execute repository-defined conversion commands.
    fs::write(root.join(".gitattributes"), "*.txt filter=switchya-test\n").unwrap();
    git(
        &root,
        &[
            "config",
            "filter.switchya-test.clean",
            "echo FILTER_EXECUTED >> git-helper-ran.txt; cat",
        ],
    );
    git(&root, &["config", "filter.switchya-test.required", "true"]);
    let id = fixture.project(&root.join("src")).await;
    let (status, snapshot) = fixture
        .request(Method::GET, &format!("/projects/{id}/changes"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{snapshot}");
    let changes = snapshot["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 3);
    let file = changes
        .iter()
        .find(|change| change["path"] == "a b.txt")
        .unwrap();
    assert_eq!(file["staged"], "M");
    assert_eq!(file["unstaged"], "M");
    assert!(
        changes
            .iter()
            .any(|change| change["path"] == ".env" && change["protected"] == true)
    );
    for (scope, before, after) in [
        ("staged", "-before", "+staged"),
        ("unstaged", "-staged", "+working"),
    ] {
        let (status, diff) = fixture
            .request(
                Method::GET,
                &format!("/projects/{id}/diff?path=a%20b.txt&scope={scope}"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{diff}");
        let patch = diff["diff"].as_str().unwrap();
        assert!(patch.contains(before), "{patch}");
        assert!(patch.contains(after), "{patch}");
        assert!(!patch.contains("outside after"));
    }
    assert!(!root.join("git-helper-ran.txt").exists());
    assert!(!root.join("src/git-helper-ran.txt").exists());
    let (status, _) = fixture
        .request(Method::GET, &format!("/projects/{id}/diff?path=.env"), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = fixture
        .request(
            Method::GET,
            &format!("/projects/{id}/diff?path=new.txt"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    fixture.host.shutdown().await.unwrap();
}

async fn wait_file(path: &Path, expected: &str, fixture: &Fixture, terminal_id: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if fs::read(path).is_ok_and(|bytes| {
            String::from_utf8_lossy(&bytes)
                .replace('\0', "")
                .contains(expected)
        }) {
            return;
        }
        if Instant::now() >= deadline {
            use base64::Engine;
            let (_, output) = fixture
                .request(
                    Method::GET,
                    &format!("/terminals/{terminal_id}?after_seq=0"),
                    None,
                )
                .await;
            let bytes: Vec<_> = output["terminal"]["chunks"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|chunk| {
                    base64::engine::general_purpose::STANDARD
                        .decode(chunk["data"].as_str().unwrap())
                        .unwrap()
                })
                .collect();
            panic!(
                "shell did not write {}; state={}; output={:?}",
                path.display(),
                output["terminal"]["state"],
                String::from_utf8_lossy(&bytes)
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn real_terminal_input_is_ordered_idempotent_resizable_and_stopped_with_the_host() {
    let fixture = Fixture::new().await;
    let root = fixture.directory.path().join("terminal-project");
    fs::create_dir(&root).unwrap();
    let project = fixture.project(&root).await;
    let id = uuid::Uuid::new_v4().to_string();
    let start = json!({"id":id,"project_id":project,"cols":100,"rows":24});
    let unauthorized = fixture
        .client
        .post(format!("{}/api/terminals", fixture.host.base_url()))
        .json(&start)
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    for _ in 0..2 {
        let (status, result) = fixture
            .request(Method::POST, "/terminals", Some(start.clone()))
            .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(result["terminal"]["id"], id);
    }
    let (_, list) = fixture.request(Method::GET, "/terminals", None).await;
    assert_eq!(list["terminals"].as_array().unwrap().len(), 1);
    // ConPTY requests the renderer's cursor position before starting the shell.
    // The browser's xterm handles this; a raw HTTP test must answer it explicitly.
    #[cfg(windows)]
    let sequence = {
        use base64::Engine;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (_, snapshot) = fixture
                .request(Method::GET, &format!("/terminals/{id}"), None)
                .await;
            let bytes: Vec<_> = snapshot["terminal"]["chunks"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|chunk| {
                    base64::engine::general_purpose::STANDARD
                        .decode(chunk["data"].as_str().unwrap())
                        .unwrap()
                })
                .collect();
            if bytes.windows(4).any(|window| window == b"\x1b[6n") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "ConPTY did not request its cursor position"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let (status, result) = fixture
            .request(
                Method::POST,
                &format!("/terminals/{id}/input"),
                Some(json!({"sequence":1,"data":"\u{1b}[1;1R"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        2
    };
    #[cfg(unix)]
    let sequence = 1;
    let first = json!({"sequence":sequence,"data":"echo FIRST >> terminal-check.txt\r"});
    for _ in 0..2 {
        let (status, result) = fixture
            .request(
                Method::POST,
                &format!("/terminals/{id}/input"),
                Some(first.clone()),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{result}");
    }
    wait_file(&root.join("terminal-check.txt"), "FIRST", &fixture, &id).await;
    let (status, result) = fixture
        .request(
            Method::POST,
            &format!("/terminals/{id}/input"),
            Some(json!({"sequence":sequence+1,"data":"echo SECOND >> terminal-check.txt\r"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    wait_file(&root.join("terminal-check.txt"), "SECOND", &fixture, &id).await;
    let bytes = fs::read(root.join("terminal-check.txt")).unwrap();
    let text = String::from_utf8_lossy(&bytes).replace('\0', "");
    assert_eq!(text.matches("FIRST").count(), 1);
    let (status, _) = fixture
        .request(Method::POST, &format!("/terminals/{id}/input"), Some(first))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, result) = fixture
        .request(
            Method::POST,
            &format!("/terminals/{id}/resize"),
            Some(json!({"cols":132,"rows":30})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    // A fast shell can write the fixture files before the terminal reader's
    // bounded output batch has been published (notably on macOS).
    let deadline = Instant::now() + Duration::from_secs(5);
    let snapshot = loop {
        let (_, snapshot) = fixture
            .request(Method::GET, &format!("/terminals/{id}?after_seq=0"), None)
            .await;
        if !snapshot["terminal"]["chunks"]
            .as_array()
            .unwrap()
            .is_empty()
        {
            break snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "Terminal output was not published: {snapshot}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(snapshot["terminal"]["cols"], 132);
    assert_eq!(snapshot["terminal"]["input_sequence"], sequence + 1);
    assert!(
        !snapshot["terminal"]["chunks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    #[cfg(windows)]
    {
        let (status, result) = fixture.request(
            Method::POST,
            &format!("/terminals/{id}/input"),
            Some(json!({"sequence":sequence+2,"data":"(Get-Location).Path | Set-Content -Encoding UTF8 terminal-cwd.txt\r"})),
        ).await;
        assert_eq!(status, StatusCode::OK, "{result}");
        wait_file(
            &root.join("terminal-cwd.txt"),
            "terminal-project",
            &fixture,
            &id,
        )
        .await;
        let cwd = fs::read_to_string(root.join("terminal-cwd.txt")).unwrap();
        let cwd = cwd.trim_start_matches('\u{feff}').trim();
        assert!(
            !cwd.contains(r"\\?\"),
            "PowerShell must use a normal path: {cwd}"
        );
        assert_eq!(
            Path::new(cwd).canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );
    }
    #[cfg(unix)]
    {
        let (status, result) = fixture.request(
            Method::POST, &format!("/terminals/{id}/input"),
            Some(json!({"sequence":sequence+2,"data":"(i=0; while [ \"$i\" -lt 100 ]; do echo pulse >> terminal-heartbeat.txt; i=$((i+1)); sleep 0.1; done) &\r"})),
        ).await;
        assert_eq!(status, StatusCode::OK, "{result}");
        wait_file(&root.join("terminal-heartbeat.txt"), "pulse", &fixture, &id).await;
    }
    let base = fixture.host.base_url().to_owned();
    fixture.host.shutdown().await.unwrap();
    #[cfg(unix)]
    {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let stopped = fs::read(root.join("terminal-heartbeat.txt")).unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(
            fs::read(root.join("terminal-heartbeat.txt")).unwrap(),
            stopped,
            "background commands must stop with their terminal"
        );
    }
    assert!(
        fixture
            .client
            .get(format!("{base}/api/status"))
            .send()
            .await
            .is_err()
    );
}
