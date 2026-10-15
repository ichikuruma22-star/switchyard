//! Regression test: a provider header written as a secret reference
//! (`env:NAME`, `${NAME}`) — which the dashboard shows as read from the
//! environment — was sent upstream as the literal reference. An upstream
//! that authenticates with that header refused every request, and the
//! refusal rested the credential as an `Auth` failure.

// The shared support module re-exports more than one review file uses.
#[allow(unused_imports)]
mod support;

use support::Harness;
use switchyard_core::Protocol;

fn provider(header_value: &str) -> String {
    format!(
        r#"
[[providers]]
name = "chat"
kind = "openai-compat"
base_url = "{{base}}/v1"
api_keys = ["key-chat-1"]
headers = {{ "X-Upstream-Key" = "{header_value}", "X-Team" = "platform" }}
[[providers.models]]
id = "up-chat"
alias = "m-chat"
"#
    )
}

#[tokio::test]
async fn a_header_reference_is_sent_resolved() {
    // Cargo sets this variable for the tests it runs.
    let expected = std::env::var("CARGO_PKG_NAME").expect("cargo sets CARGO_PKG_NAME");
    let harness = Harness::start(&provider("env:CARGO_PKG_NAME")).await;

    let output = harness.ask(Protocol::OpenaiChat, "m-chat", false).await;
    assert_eq!(
        output.status,
        200,
        "{}",
        String::from_utf8_lossy(&output.body)
    );
    let sent = harness.fake.last();
    assert_eq!(sent.header("x-upstream-key"), Some(expected.as_str()));
    assert_eq!(sent.header("x-team"), Some("platform"));
}

#[tokio::test]
async fn a_header_reference_to_an_unset_variable_is_never_sent() {
    let harness = Harness::start(&provider("${SWITCHYARD_TEST_UNSET_HEADER_VARIABLE}")).await;

    let output = harness.ask(Protocol::OpenaiChat, "m-chat", false).await;
    assert!(output.status >= 400, "{}", output.status);
    assert_eq!(harness.fake.count(), 0, "a request went upstream");
    let snapshot = harness.gateway.scheduler().snapshot();
    assert_eq!(
        snapshot[0].credentials[0].unusable_reason.as_deref(),
        Some(
            "header `X-Upstream-Key`: environment variable \
             SWITCHYARD_TEST_UNSET_HEADER_VARIABLE is not set"
        )
    );
}
