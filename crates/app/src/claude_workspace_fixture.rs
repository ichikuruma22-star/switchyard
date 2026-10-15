//! Opt-in local integration fixture for the installed Claude CLI and real HTTP/UI
//! routes. The only credential is a dummy API key pointed at a loopback mock.
//! Account status is supplied by the test: this never queries or signs in to a
//! real account. No test controls or auth bypass exist in production builds.
use super::*;
use crate::{AppOptions, LaunchInfo, api, startup, terminals};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
#[ignore = "requires an installed Claude CLI and an explicitly supplied local mock API"]
async fn live_claude_workspace_fixture() {
    crate::initialize_tls();
    let endpoint = std::env::var("SWITCHYA_CLAUDE_MOCK_URL").expect("supply a local mock API URL");
    let parsed = url::Url::parse(&endpoint).unwrap();
    assert_eq!(parsed.scheme(), "http");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert!(parsed.port().is_some());
    assert_eq!(parsed.path(), "/");
    assert!(parsed.username().is_empty() && parsed.password().is_none());
    let root = PathBuf::from(
        std::env::var_os("SWITCHYA_CLAUDE_FIXTURE_DIR").expect("supply a new fixture directory"),
    );
    assert!(root.is_absolute());
    std::fs::create_dir(&root).expect("the fixture directory must be new");
    let root = root.canonicalize().unwrap();
    let project = root.join("project");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(
        project.join("README.md"),
        "# Claude integration fixture\nNo provider account is connected.\n",
    )
    .unwrap();
    let options = AppOptions::new(root.join("gateway.toml"), root.join("data"));
    let mut profile_id = None;
    for epoch in 1..=2 {
        let runtime = startup::AppRuntime::start(&options).await.unwrap();
        let accounts = AccountManager::open(&options.data_dir).unwrap();
        let id = profile_id
            .get_or_insert_with(|| {
                accounts
                    .create_profile("claude", "Local API fixture")
                    .unwrap()
                    .id
            })
            .clone();
        let settings = accounts.inner.homes.join(&id).join("settings.json");
        if epoch == 1 {
            std::fs::write(
                settings,
                serde_json::to_vec_pretty(&json!({"env":{
                    "ANTHROPIC_API_KEY":"local-fixture-not-a-credential",
                    "ANTHROPIC_BASE_URL":endpoint,
                    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":"1",
                    "DISABLE_TELEMETRY":"1"
                }}))
                .unwrap(),
            )
            .unwrap();
        }
        {
            let mut state = lock(&accounts.inner.state);
            let cached = state.cache.get_mut(&id).unwrap();
            cached.account.auth_status = "signed_in".into();
            cached.account.message =
                Some("Local API fixture; live account sign-in is not being verified.".into());
        }
        accounts.select_profile(&id).unwrap();
        let adapters = AdapterManager::open(&options.data_dir).unwrap();
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let launch = LaunchInfo::new(port);
        let cancel = CancellationToken::new();
        let terminal_manager = terminals::TerminalManager::default();
        let state = Arc::new(api::AppState {
            engine: runtime.engine.clone(),
            gateway: runtime.gateway.clone(),
            config_path: runtime.config_path.clone(),
            data_dir: options.data_dir.canonicalize().unwrap(),
            accounts: accounts.clone(),
            adapters: adapters.clone(),
            workspace_reads: Arc::new(Semaphore::new(4)),
            terminals: terminal_manager.clone(),
        });
        let router = api::router(state, port, launch.token.clone(), cancel.clone());
        let server_cancel = cancel.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(server_cancel.cancelled_owned())
                .await
                .unwrap();
        });
        launch
            .write_new(root.join(format!("launch-{epoch}.json")))
            .unwrap();
        std::fs::write(root.join("fixture-info.json"), serde_json::to_vec_pretty(&json!({"epoch":epoch,"project":project,"profile_id":id,"live_sign_in_verified":false,"model_api":"loopback mock"})).unwrap()).unwrap();
        println!("Local Claude API/UI fixture ready: epoch {epoch}");
        let deadline = Instant::now() + Duration::from_secs(20 * 60);
        while !root.join(format!("stop-{epoch}")).exists() {
            assert!(
                Instant::now() < deadline,
                "fixture controller did not finish within twenty minutes"
            );
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        assert!(
            adapters
                .list_runs()
                .iter()
                .all(|run| !run.state.is_active()),
            "finish or interrupt all fixture runs before stopping"
        );
        cancel.cancel();
        adapters.shutdown().await;
        accounts.shutdown().await;
        terminal_manager.shutdown().await.unwrap();
        runtime.shutdown().await.unwrap();
        server.await.unwrap();
    }
}
