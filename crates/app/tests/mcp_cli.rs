//! Exercise the shipped executable, real stdio pipes and the host's private
//! discovery record. No model or external service is contacted.
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use switchyard_app::{AppOptions, LaunchInfo, start_host};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    sequence: u64,
}
impl Client {
    fn start(directory: &Path, project: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_switchya"))
            .arg("--config")
            .arg(directory.join("gateway.toml"))
            .arg("--data-dir")
            .arg(directory.join("data"))
            .arg("mcp")
            .arg("--project")
            .arg(project)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            input,
            output,
            sequence: 0,
        }
    }
    async fn send(&mut self, value: Value) {
        let input = self.input.as_mut().unwrap();
        input
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
        input.flush().await.unwrap();
    }
    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.sequence += 1;
        let id = self.sequence;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        loop {
            let mut line = String::new();
            let read =
                tokio::time::timeout(Duration::from_secs(15), self.output.read_line(&mut line))
                    .await
                    .expect("MCP request timed out")
                    .unwrap();
            assert!(read > 0, "MCP exited before replying to {method}");
            let value: Value =
                serde_json::from_str(&line).expect("stdout must contain only MCP JSON");
            if value["id"] == id {
                return value;
            }
        }
    }
    async fn initialize(&mut self) {
        let response=self.request("initialize",json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"stdio-integration","version":"1"}})).await;
        assert_eq!(
            response["result"]["serverInfo"]["name"], "switchya",
            "{response}"
        );
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
    }
    async fn close(mut self) {
        drop(self.input.take());
        let status = tokio::time::timeout(Duration::from_secs(8), self.child.wait())
            .await
            .expect("MCP did not exit after stdin closed")
            .unwrap();
        assert!(status.success(), "MCP child exited with {status}");
    }
}

#[tokio::test]
async fn real_stdio_clients_discover_one_host_share_context_and_release_it_on_eof() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("README.md"), "Real stdio fixture").unwrap();
    let mut owner = Client::start(temp.path(), &root);
    owner.initialize().await;
    let record = temp.path().join("data/.switchya/host.json");
    let original = LaunchInfo::read(&record).unwrap();
    let mut peer = Client::start(temp.path(), &root);
    peer.initialize().await;
    assert_eq!(
        LaunchInfo::read(&record).unwrap().pid,
        original.pid,
        "second client must share the first host"
    );
    let id = uuid::Uuid::new_v4().to_string();
    let note = json!({"id":id,"title":"Real pipe handoff","summary":"The first client recorded this note.","source":"stdio owner","verification":"No model or shell tool was called."});
    let saved = owner
        .request(
            "tools/call",
            json!({"name":"switchya_save_checkpoint","arguments":note}),
        )
        .await;
    assert!(saved["result"]["isError"] != true, "{saved}");
    let read = peer
        .request(
            "tools/call",
            json!({"name":"switchya_read_checkpoint","arguments":{"id":id}}),
        )
        .await;
    assert_eq!(
        read["result"]["structuredContent"],
        saved["result"]["structuredContent"]
    );
    peer.close().await;
    assert!(
        record.exists(),
        "closing a client must not shut down the host it joined"
    );
    owner.close().await;
    assert!(
        !record.exists(),
        "the owner must remove its private discovery record"
    );
    let reopened = start_host(AppOptions::new(
        temp.path().join("gateway.toml"),
        temp.path().join("data"),
    ))
    .await
    .unwrap();
    assert_ne!(LaunchInfo::read(&record).unwrap().token, original.token);
    let mut next = Client::start(temp.path(), &root);
    next.initialize().await;
    let restored = next
        .request(
            "tools/call",
            json!({"name":"switchya_read_checkpoint","arguments":{"id":id}}),
        )
        .await;
    assert_eq!(
        restored["result"]["structuredContent"],
        saved["result"]["structuredContent"]
    );
    next.close().await;
    reopened.shutdown().await.unwrap();
    assert!(!record.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn interrupted_stdio_server_releases_its_host_even_with_stdin_still_open() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let mut client = Client::start(temp.path(), &root);
    client.initialize().await;
    let pid = client.child.id().unwrap();
    // SAFETY: this unreaped Child still owns the exact process ID we spawned.
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGINT) }, 0);
    let status = tokio::time::timeout(Duration::from_secs(8), client.child.wait())
        .await
        .expect("MCP shutdown waited on an open stdin pipe")
        .unwrap();
    assert!(status.success());
    assert!(!temp.path().join("data/.switchya/host.json").exists());
}
