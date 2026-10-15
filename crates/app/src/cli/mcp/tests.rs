use super::*;
use crate::AppOptions;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

struct Client {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    sequence: u64,
    inline: bool,
}
impl Client {
    async fn request(&mut self, method: &str, mut params: Value) -> Value {
        if self.inline {
            params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"switchya-protocol-test","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}});
        }
        self.sequence += 1;
        let id = self.sequence;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        loop {
            let mut line = String::new();
            let read = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                self.reader.read_line(&mut line),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(read > 0, "MCP stream closed waiting for {method}");
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == id {
                return value;
            }
        }
    }
    async fn send(&mut self, value: Value) {
        self.writer
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
        self.writer.flush().await.unwrap();
    }
    async fn tool(&mut self, name: &str, args: Value) -> Value {
        self.request("tools/call", json!({"name":name,"arguments":args}))
            .await
    }
}

async fn connect(
    host: &crate::RunningHost,
    path: PathBuf,
    version: &str,
) -> (Client, tokio::task::JoinHandle<()>) {
    let server = ProjectServer::open(Remote::new(host.launch.clone()).unwrap(), path)
        .await
        .unwrap();
    let (connection, transport) = tokio::io::duplex(1024 * 1024);
    let running = tokio::spawn(async move {
        server
            .serve(transport)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let (reader, writer) = tokio::io::split(connection);
    let mut client = Client {
        reader: BufReader::new(reader),
        writer,
        sequence: 0,
        inline: version == "2026-07-28",
    };
    if client.inline {
        let reply = client.request("server/discover", json!({})).await;
        assert!(reply.get("result").is_some(), "{reply}");
    } else {
        let reply=client.request("initialize",json!({"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"switchya-protocol-test","version":"1"}})).await;
        assert_eq!(reply["result"]["protocolVersion"], version, "{reply}");
        client
            .send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
    }
    (client, running)
}

#[tokio::test]
async fn two_protocol_clients_share_handoffs_and_keep_their_project_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let other = temp.path().join("other");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&other).unwrap();
    std::fs::write(
        root.join("README.md"),
        "A fixture project\nneedle: project state\nthird line",
    )
    .unwrap();
    std::fs::write(root.join(".env"), "SECRET_FIXTURE_DO_NOT_READ").unwrap();
    std::fs::write(other.join("README.md"), "OTHER_PROJECT_PRIVATE").unwrap();
    let options = AppOptions::new(temp.path().join("gateway.toml"), temp.path().join("data"));
    let host = start_host(options.clone()).await.unwrap();
    host.write_launch_info(root.join("copied-launch.json"))
        .unwrap();
    let (mut legacy, first) = connect(&host, root.clone(), "2025-11-25").await;
    let (mut modern, second) = connect(&host, root.clone(), "2026-07-28").await;
    let (mut isolated, third) = connect(&host, other.clone(), "2025-11-25").await;
    let remote = Remote::new(host.launch.clone()).unwrap();
    let project = remote
        .request::<ProjectResponse>(Method::POST, "projects", Some(json!({"path":root})))
        .await
        .unwrap()
        .project;
    let other_project = remote
        .request::<ProjectResponse>(Method::POST, "projects", Some(json!({"path":other})))
        .await
        .unwrap()
        .project;
    let session: Value = remote
        .request(
            Method::POST,
            "sessions",
            Some(json!({"project_id":project.id,"model":"mock-echo"})),
        )
        .await
        .unwrap();
    let other_session: Value = remote
        .request(
            Method::POST,
            "sessions",
            Some(json!({"project_id":other_project.id,"model":"mock-echo"})),
        )
        .await
        .unwrap();
    let history = modern.tool("switchya_list_conversations", json!({})).await;
    let entries = history["result"]["structuredContent"]["data"]["conversations"]
        .as_array()
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["id"], session["session"]["id"]);
    let events = modern
        .tool(
            "switchya_read_conversation",
            json!({"kind":"native","id":session["session"]["id"]}),
        )
        .await;
    assert!(
        events["result"]["structuredContent"]["data"]["events"].is_array(),
        "{events}"
    );
    let denied = modern
        .tool(
            "switchya_read_conversation",
            json!({"kind":"native","id":other_session["session"]["id"]}),
        )
        .await;
    assert_eq!(denied["result"]["isError"], true, "{denied}");
    let settings: Value = remote
        .request(
            Method::GET,
            &format!("projects/{}/mcp-config", project.id),
            None,
        )
        .await
        .unwrap();
    assert!(
        settings["codex_configuration"]
            .as_str()
            .unwrap()
            .starts_with("[mcp_servers.switchya]\n")
    );
    assert!(!settings.to_string().contains(&host.launch.token));
    let redacted = legacy
        .tool("switchya_read_file", json!({"path":"copied-launch.json"}))
        .await;
    assert!(!redacted.to_string().contains(&host.launch.token));
    assert_eq!(
        redacted["result"]["structuredContent"]["data"]["credential_redacted"],
        true
    );
    let tools = legacy.request("tools/list", json!({})).await;
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 11);
    assert!(
        tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool["outputSchema"]["type"] == "object")
    );
    let file = legacy
        .tool(
            "switchya_read_file",
            json!({"path":"README.md","start_line":2,"max_lines":1}),
        )
        .await;
    assert_eq!(
        file["result"]["structuredContent"]["data"]["content"], "needle: project state",
        "{file}"
    );
    assert_eq!(file["result"]["structuredContent"]["data"]["next_line"], 3);
    let found = modern
        .tool("switchya_search", json!({"query":"needle"}))
        .await;
    assert!(
        !found["result"]["isError"].as_bool().unwrap_or(false),
        "{found}"
    );
    assert!(found.to_string().contains("needle: project state"));
    for arguments in [
        json!({"path":"../other/README.md"}),
        json!({"path":".env"}),
        json!({"path":"README.md","project_id":"other"}),
    ] {
        let denied = legacy.tool("switchya_read_file", arguments).await;
        assert_eq!(denied["result"]["isError"], true, "{denied}");
        assert!(!denied.to_string().contains("OTHER_PROJECT_PRIVATE"));
        assert!(!denied.to_string().contains("SECRET_FIXTURE_DO_NOT_READ"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let note = json!({"id":id,"title":"First handoff","summary":"Read the project; no changes made.","source":"Protocol test","next_steps":["Inspect the next module"],"verification":"Read README.md through MCP.","risks":["No model was called."]});
    let saved = legacy.tool("switchya_save_checkpoint", note.clone()).await;
    assert!(
        !saved["result"]["isError"].as_bool().unwrap_or(false),
        "{saved}"
    );
    let repeated = modern.tool("switchya_save_checkpoint", note.clone()).await;
    assert_eq!(
        saved["result"]["structuredContent"],
        repeated["result"]["structuredContent"]
    );
    let mut changed = note.clone();
    changed["summary"] = json!("Conflicting text");
    assert_eq!(
        modern.tool("switchya_save_checkpoint", changed).await["result"]["isError"],
        true
    );
    assert_eq!(
        isolated
            .tool("switchya_read_checkpoint", json!({"id":id}))
            .await["result"]["isError"],
        true
    );
    let mut newer = note.clone();
    newer["id"] = json!(uuid::Uuid::new_v4().to_string());
    newer["title"] = json!("Newer handoff");
    legacy.tool("switchya_save_checkpoint", newer).await;
    let page = modern
        .tool("switchya_list_checkpoints", json!({"limit":1}))
        .await;
    let data = &page["result"]["structuredContent"]["data"];
    assert_eq!(data["checkpoints"][0]["title"], "Newer handoff");
    let older = modern
        .tool(
            "switchya_list_checkpoints",
            json!({"limit":1,"before":data["next_before"]}),
        )
        .await;
    assert_eq!(
        older["result"]["structuredContent"]["data"]["checkpoints"][0]["id"],
        id
    );
    let context = modern.tool("switchya_project_context", json!({})).await;
    assert!(context.to_string().contains("First handoff"), "{context}");
    assert!(!context.to_string().contains("SECRET_FIXTURE_DO_NOT_READ"));
    drop(legacy);
    drop(modern);
    drop(isolated);
    for task in [first, second, third] {
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
    }
    host.shutdown().await.unwrap();
    let reopened = start_host(options).await.unwrap();
    let (mut reader, task) = connect(&reopened, root, "2025-11-25").await;
    let restored = reader
        .tool("switchya_read_checkpoint", json!({"id":id}))
        .await;
    assert_eq!(
        restored["result"]["structuredContent"],
        saved["result"]["structuredContent"]
    );
    drop(reader);
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn input_limit_applies_per_message_before_json_decoding() {
    use tokio::io::AsyncReadExt;
    let many = format!("{}\n{}\n", "a".repeat(200_000), "b".repeat(200_000));
    let mut input = LimitedInput {
        reader: many.as_bytes(),
        line_bytes: 0,
    };
    let mut result = Vec::new();
    input.read_to_end(&mut result).await.unwrap();
    assert_eq!(result.len(), many.len());
    let oversized = "x".repeat(256 * 1024 + 1);
    let mut input = LimitedInput {
        reader: oversized.as_bytes(),
        line_bytes: 0,
    };
    assert_eq!(
        input.read_to_end(&mut Vec::new()).await.unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
}

#[tokio::test]
async fn a_project_path_that_is_not_unicode_is_rejected_without_a_request() {
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(b"project-\xff"))
    };
    #[cfg(windows)]
    let path = {
        use std::os::windows::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_wide(&[0x70, 0xD800]))
    };
    // No host listens here: the path is refused before any request is made.
    let remote = Remote::new(LaunchInfo::new(9)).unwrap();
    let error = match ProjectServer::open(remote, path).await {
        Err(error) => error,
        Ok(_) => panic!("a path that is not Unicode opened a project"),
    };
    assert_eq!(error.code, "invalid_request");
}
