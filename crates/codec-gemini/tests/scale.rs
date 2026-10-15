//! The work done on a request must grow in proportion to its size. Bodies of
//! tens of megabytes are accepted, so anything quadratic in the number of
//! tools, tool calls or schema keys would let one request pin a worker.
//!
//! Each case below is tens of thousands of items wide. Done in linear time it
//! takes milliseconds; a quadratic pass over the same input takes minutes.
//! The time limit is therefore generous enough to hold on a busy machine and
//! still tell the two apart.

use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};
use switchyard_codec_gemini::{GeminiCodec, sanitize_schema, sanitize_schema_legacy};
use switchyard_core::ir::{
    FinishReason, FunctionTool, Message, Part, Request, Response, Role, Tool, ToolChoice,
};
use switchyard_core::{ClientCtx, Codec, Protocol, RequestPath, UpstreamCtx};

const WIDE: usize = 40_000;
/// Function names are short, so comparing two costs next to nothing: lists
/// of names are made longer than [`WIDE`] for a quadratic pass to show.
const NAMES: usize = 200_000;
const LIMIT: Duration = Duration::from_secs(20);

fn timed<T>(what: &str, work: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let out = work();
    let took = started.elapsed();
    assert!(took < LIMIT, "{what} took {took:?}");
    out
}

#[test]
fn schema_with_very_many_bare_properties_and_required_flags() {
    // Every key is a stray (bare) property with a boolean `required`.
    let mut node = Map::new();
    for i in 0..WIDE {
        node.insert(
            format!("p{i}"),
            json!({"type": ["string", "null"], "required": true}),
        );
    }
    let names: Vec<String> = (0..WIDE).map(|i| format!("p{i}")).collect();
    node.insert("required".into(), json!(names));
    let schema = Value::Object(node);
    for clean in [sanitize_schema, sanitize_schema_legacy] {
        let cleaned = timed("cleaning a wide schema", || clean(&schema));
        assert_eq!(cleaned["properties"].as_object().map(Map::len), Some(WIDE));
        // Every property is nullable, so none of them stays required.
        assert!(cleaned.get("required").is_none());
    }
}

#[test]
fn schema_with_very_long_required_lists_in_all_of() {
    let names: Vec<String> = (0..WIDE).map(|i| format!("p{i}")).collect();
    let mut properties = Map::new();
    for name in &names {
        properties.insert(name.clone(), json!({"type": "string"}));
    }
    let schema = json!({
        "type": "object",
        "properties": properties,
        "required": names,
        "allOf": [{"required": names}, {"required": names}, {"required": ["p0", "extra"]}]
    });
    let cleaned = timed("merging long required lists", || sanitize_schema(&schema));
    assert_eq!(cleaned["required"].as_array().map(Vec::len), Some(WIDE));
}

#[test]
fn schema_with_a_very_wide_union() {
    let branches: Vec<Value> = (0..WIDE)
        .map(|i| json!({"type": format!("t{i}")}))
        .collect();
    let schema = json!({"type": "object", "properties": {"v": {"anyOf": branches}}});
    let cleaned = timed("flattening a wide union", || sanitize_schema(&schema));
    assert_eq!(cleaned["properties"]["v"]["type"], json!("t0"));
}

#[test]
fn schema_with_a_very_wide_union_of_object_variants() {
    // Every variant has a discriminator value and a property of its own.
    let branches: Vec<Value> = (0..WIDE)
        .map(|i| {
            let mut properties = Map::new();
            properties.insert(
                "kind".into(),
                json!({"type": "string", "enum": [format!("k{i}")]}),
            );
            properties.insert(format!("p{i}"), json!({"type": "string"}));
            json!({"type": "object", "properties": properties, "required": ["kind", format!("p{i}")]})
        })
        .collect();
    let schema = json!({"type": "object", "properties": {"v": {"oneOf": branches}}});
    let cleaned = timed("merging a wide union of objects", || {
        sanitize_schema(&schema)
    });
    let merged = &cleaned["properties"]["v"];
    assert_eq!(
        merged["properties"].as_object().map(Map::len),
        Some(WIDE + 1)
    );
    assert_eq!(
        merged["properties"]["kind"]["enum"]
            .as_array()
            .map(Vec::len),
        Some(WIDE)
    );
    assert_eq!(merged["required"], json!(["kind"]));
}

#[test]
fn very_many_allowed_function_names_narrow_the_tool_list_in_linear_time() {
    let declarations: Vec<Value> = (0..NAMES)
        .map(|i| json!({"name": format!("t{i}")}))
        .collect();
    // Every other declared function is allowed; the other names in the list
    // match no declaration.
    let allowed: Vec<String> = (0..NAMES)
        .map(|i| {
            if i % 2 == 0 {
                format!("t{i}")
            } else {
                format!("a{i}")
            }
        })
        .collect();
    let body = json!({
        "contents": [{"role": "user", "parts": [{"text": "go"}]}],
        "tools": [{"functionDeclarations": declarations}],
        "toolConfig": {"functionCallingConfig": {"mode": "ANY", "allowedFunctionNames": allowed}}
    });
    let path = RequestPath {
        model: Some("gemini-2.5-pro"),
        stream: Some(false),
    };
    let request = timed("narrowing a wide tool list", || {
        GeminiCodec
            .decode_request(&body, &path)
            .expect("request decodes")
    });
    assert_eq!(request.tool_choice, Some(ToolChoice::Required));
    assert_eq!(request.tools.len(), NAMES / 2);
    assert_eq!(request.tools[1].name(), Some("t2"));
}

#[test]
fn reply_to_a_request_declaring_very_many_functions_encodes_in_linear_time() {
    // Declared names the client is handed back in its own spelling.
    let declarations: Vec<Value> = (0..NAMES)
        .map(|i| json!({"name": format!("mcp.f{i}")}))
        .collect();
    let ctx = ClientCtx::new("gemini-2.5-pro").with_request(Arc::new(json!({
        "contents": [{"role": "user", "parts": [{"text": "go"}]}],
        "tools": [{"functionDeclarations": declarations}]
    })));
    let mut response = Response::new("chatcmpl-1", "upstream-model");
    response.parts = vec![Part::tool_call("call_1", "mcp_f7", "{}")];
    response.finish = FinishReason::ToolCalls;
    let body = timed("encoding a reply", || {
        GeminiCodec
            .encode_response(&response, &ctx)
            .expect("response encodes")
    });
    assert_eq!(
        body["candidates"][0]["content"]["parts"][0]["functionCall"]["name"],
        json!("mcp.f7")
    );
}

#[test]
fn request_with_very_many_tools_and_calls_encodes_in_linear_time() {
    let mut request = Request::new("gemini-2.5-pro", Protocol::OpenaiChat);
    request.tools = (0..WIDE)
        .map(|i| {
            Tool::Function(FunctionTool {
                name: format!("tool_{i}"),
                description: None,
                parameters: Value::Null,
                strict: None,
                cache_control: None,
            })
        })
        .collect();
    // Every call has the same (empty) id, as some compatible servers send.
    let calls: Vec<Part> = (0..WIDE)
        .map(|i| Part::tool_call("", format!("tool_{i}"), "{}"))
        .collect();
    let results: Vec<Part> = (0..WIDE)
        .map(|i| Part::tool_result_text("", format!("result {i}")))
        .collect();
    request.messages = vec![
        Message::user_text("go"),
        Message::new(Role::Assistant, calls),
        Message::new(Role::User, results),
    ];
    let body = timed("encoding a wide request", || {
        GeminiCodec
            .encode_request(&request, &UpstreamCtx::default())
            .expect("request encodes")
    });
    assert_eq!(
        body["tools"][0]["functionDeclarations"]
            .as_array()
            .map(Vec::len),
        Some(WIDE)
    );
    let responses = body["contents"][2]["parts"].as_array().expect("parts");
    assert_eq!(responses.len(), WIDE);
    // Paired in order, each named after its own call.
    let last = &responses[WIDE - 1]["functionResponse"];
    assert_eq!(last["name"], json!(format!("tool_{}", WIDE - 1)));
    assert_eq!(
        last["response"],
        json!({"result": format!("result {}", WIDE - 1)})
    );
}

#[test]
fn request_with_very_many_calls_decodes_in_linear_time() {
    // Responses arrive in reverse order of the calls and are paired by name.
    let calls: Vec<Value> = (0..WIDE)
        .map(|i| json!({"functionCall": {"name": format!("f{}", i % 7), "args": {"i": i}}}))
        .collect();
    let responses: Vec<Value> = (0..WIDE)
        .rev()
        .map(|i| json!({"functionResponse": {"name": format!("f{}", i % 7), "response": {"result": i}}}))
        .collect();
    let body = json!({"contents": [
        {"role": "user", "parts": [{"text": "go"}]},
        {"role": "model", "parts": calls},
        {"role": "user", "parts": responses}
    ]});
    let path = RequestPath {
        model: Some("gemini-2.5-pro"),
        stream: Some(false),
    };
    let request = timed("decoding a wide request", || {
        GeminiCodec
            .decode_request(&body, &path)
            .expect("request decodes")
    });
    let ids: std::collections::HashSet<&str> = request.messages[1]
        .tool_calls()
        .map(|call| call.id.as_str())
        .collect();
    assert_eq!(ids.len(), WIDE, "every call has an id of its own");
    let answered: std::collections::HashSet<&str> = request.messages[2]
        .tool_results()
        .map(|result| result.call_id.as_str())
        .collect();
    assert_eq!(answered, ids, "every call is answered exactly once");
}
