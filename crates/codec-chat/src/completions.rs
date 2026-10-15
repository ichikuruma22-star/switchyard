//! Shim for the legacy `POST /v1/completions` endpoint.
//!
//! The gateway serves text completions by running them as Chat Completions:
//! the request is rewritten into a one-message chat request, and the chat
//! response (or each stream chunk) is rewritten back into the
//! `text_completion` shape. These are functions over JSON so the server can
//! apply them around the normal Chat pipeline; a stream only needs
//! [`CompletionsStream`] to carry log-probability offsets from one chunk to
//! the next.

use serde_json::{Map, Value, json};
use std::collections::HashMap;
use switchyard_core::error::CodecError;

/// Prompt used when the client sent none. A chat model needs *something* to
/// answer, and an empty user message is rejected by several upstreams.
const EMPTY_PROMPT: &str = "Complete this:";

/// Fields with the same meaning in both APIs, copied when present and not
/// `null`.
const COPIED_FIELDS: &[&str] = &[
    "max_tokens",
    "temperature",
    "top_p",
    "n",
    "stop",
    "seed",
    "presence_penalty",
    "frequency_penalty",
    "logit_bias",
    "user",
    "stream",
    "stream_options",
];

/// Rewrites a `/v1/completions` request as a Chat Completions request.
///
/// * `prompt` becomes the single user message. An array of strings is joined
///   with newlines (batched prompts are not supported); an empty or missing
///   prompt becomes `"Complete this:"`. Token-id prompts (an array of
///   integers, or of such arrays) cannot be read without the model's
///   tokenizer and are refused, as is any other prompt that is not text.
/// * `logprobs` is an integer in the legacy API (how many alternatives to
///   return) and a boolean plus `top_logprobs` in Chat.
/// * `echo`, `suffix` and `best_of` have no Chat equivalent and are dropped.
pub fn completions_request_to_chat(body: &Value) -> Result<Value, CodecError> {
    let mut out = Map::new();
    out.insert(
        "model".into(),
        body.get("model").cloned().unwrap_or_else(|| json!("")),
    );
    let not_text = || {
        CodecError::invalid_param(
            "prompt",
            "`prompt` must be a string or an array of strings; token-id prompts are not supported",
        )
    };
    let prompt = match body.get("prompt") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(not_text)?
            .join("\n"),
        Some(_) => return Err(not_text()),
    };
    let prompt = if prompt.is_empty() {
        EMPTY_PROMPT.to_string()
    } else {
        prompt
    };
    out.insert(
        "messages".into(),
        json!([{"role": "user", "content": prompt}]),
    );
    for key in COPIED_FIELDS {
        if let Some(value) = body.get(*key).filter(|v| !v.is_null()) {
            out.insert((*key).into(), value.clone());
        }
    }
    match body.get("logprobs") {
        Some(Value::Bool(flag)) => {
            out.insert("logprobs".into(), json!(flag));
        }
        Some(Value::Number(n)) => {
            let wanted = n.as_u64().unwrap_or(0);
            out.insert("logprobs".into(), json!(true));
            if wanted > 0 {
                // Chat accepts at most 20 alternatives.
                out.insert("top_logprobs".into(), json!(wanted.min(20)));
            }
        }
        _ => {}
    }
    if out.get("logprobs") == Some(&Value::Bool(true))
        && !out.contains_key("top_logprobs")
        && let Some(top) = body.get("top_logprobs").filter(|v| v.is_number())
    {
        out.insert("top_logprobs".into(), top.clone());
    }
    Ok(Value::Object(out))
}

/// Text of a chat message or delta `content`: a string, or the text parts of
/// a part array.
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text.as_str()),
                other => other.get("text").and_then(Value::as_str),
            })
            .collect(),
        _ => String::new(),
    }
}

/// Copies the envelope fields shared by both shapes.
fn envelope(source: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(id) = source.get("id") {
        out.insert("id".into(), id.clone());
    }
    out.insert("object".into(), json!("text_completion"));
    for key in ["created", "model"] {
        if let Some(value) = source.get(key) {
            out.insert(key.into(), value.clone());
        }
    }
    out
}

fn finish_extras(source: &Value, out: &mut Map<String, Value>) {
    for key in ["system_fingerprint", "service_tier"] {
        if let Some(value) = source.get(key).filter(|v| !v.is_null()) {
            out.insert(key.into(), value.clone());
        }
    }
}

/// The token entries of a choice's Chat `logprobs`.
fn logprob_tokens(logprobs: Option<&Value>) -> Option<&Vec<Value>> {
    logprobs?.get("content")?.as_array()
}

/// How many characters of the completion text a Chat log-probability entry
/// covers. A character split across tokens arrives as escaped text (such as
/// `\xe2\x80`) next to its raw `bytes`, so when the entry lists its bytes the
/// UTF-8 lead bytes are counted: the character counts once, at the token
/// that starts it. Without `bytes` the token text is counted.
fn token_chars(entry: &Value, token: &str) -> usize {
    match entry.get("bytes").and_then(Value::as_array) {
        Some(bytes) => bytes
            .iter()
            .filter_map(Value::as_u64)
            .filter(|byte| byte & 0xC0 != 0x80)
            .count(),
        None => token.chars().count(),
    }
}

/// Rewrites a choice's Chat `logprobs` (`{"content": [{token, logprob,
/// bytes, top_logprobs}], "refusal": …}`) in the legacy shape: parallel
/// `tokens`, `token_logprobs`, `top_logprobs` (one `{token: logprob}` object
/// per token) and `text_offset` arrays.
///
/// `offset` is the character offset of the first token and is moved past
/// the last one. Offsets count from the start of the completion text: the
/// prompt is never echoed. `null` (or no `content`) stays `null`; refusal
/// log probabilities have no legacy counterpart.
fn legacy_logprobs(logprobs: Option<&Value>, offset: &mut usize) -> Value {
    let Some(entries) = logprob_tokens(logprobs) else {
        return Value::Null;
    };
    let mut tokens = Vec::with_capacity(entries.len());
    let mut token_logprobs = Vec::with_capacity(entries.len());
    let mut top_logprobs = Vec::with_capacity(entries.len());
    let mut text_offset = Vec::with_capacity(entries.len());
    for entry in entries {
        let token = entry
            .get("token")
            .and_then(Value::as_str)
            .unwrap_or_default();
        tokens.push(json!(token));
        token_logprobs.push(entry.get("logprob").cloned().unwrap_or(Value::Null));
        let mut alternatives = Map::new();
        let listed = entry.get("top_logprobs").and_then(Value::as_array);
        for alternative in listed.into_iter().flatten() {
            if let Some(text) = alternative.get("token").and_then(Value::as_str) {
                // The likelier one wins when two alternatives print alike.
                alternatives
                    .entry(text)
                    .or_insert_with(|| alternative.get("logprob").cloned().unwrap_or(Value::Null));
            }
        }
        top_logprobs.push(Value::Object(alternatives));
        text_offset.push(json!(*offset));
        *offset += token_chars(entry, token);
    }
    json!({
        "tokens": tokens,
        "token_logprobs": token_logprobs,
        "top_logprobs": top_logprobs,
        "text_offset": text_offset,
    })
}

/// Rewrites a `chat.completion` object as a `text_completion` object.
///
/// Every choice keeps its `index` and `finish_reason`; `text` is the message
/// content (the empty string for answers without text, such as tool calls)
/// and `logprobs` is rewritten in the legacy shape. Bodies that are not chat
/// completions (an error envelope) are returned unchanged.
pub fn chat_response_to_completions(body: &Value) -> Value {
    let Some(choices) = body.get("choices").and_then(Value::as_array) else {
        return body.clone();
    };
    let mut out = envelope(body);
    let choices: Vec<Value> = choices
        .iter()
        .enumerate()
        .map(|(position, choice)| {
            json!({
                "index": choice.get("index").cloned().unwrap_or_else(|| json!(position)),
                "text": content_text(choice.get("message").and_then(|m| m.get("content"))),
                "logprobs": legacy_logprobs(choice.get("logprobs"), &mut 0),
                "finish_reason": choice.get("finish_reason").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    out.insert("choices".into(), Value::Array(choices));
    if let Some(usage) = body.get("usage").filter(|u| !u.is_null()) {
        out.insert("usage".into(), usage.clone());
    }
    finish_extras(body, &mut out);
    Value::Object(out)
}

/// Rewrites a Chat Completions stream as a `text_completion` stream, one
/// chunk at a time. It remembers how much text each choice has produced, so
/// the `text_offset`s of streamed log probabilities continue from chunk to
/// chunk exactly as in the non-streamed answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompletionsStream {
    /// Character offset of each choice's next token, by choice index.
    offsets: HashMap<u64, usize>,
}

impl CompletionsStream {
    /// Rewrites one `chat.completion.chunk` as a `text_completion` stream
    /// chunk; `logprobs` is rewritten in the legacy shape.
    ///
    /// Returns `None` for chunks that carry nothing a completions client can
    /// use: no text, no finish reason, no log probabilities and no usage
    /// (role-only chunks, tool-call and reasoning deltas). Error frames are
    /// passed through unchanged.
    pub fn rewrite(&mut self, chunk: &Value) -> Option<Value> {
        if chunk.get("error").is_some_and(|e| !e.is_null()) {
            return Some(chunk.clone());
        }
        let choices = chunk.get("choices").and_then(Value::as_array)?;
        let usage = chunk.get("usage").filter(|u| !u.is_null());
        let mut useful = usage.is_some();
        let choices: Vec<Value> = choices
            .iter()
            .enumerate()
            .filter_map(|(position, choice)| {
                let text = content_text(choice.get("delta").and_then(|d| d.get("content")));
                let finish = choice
                    .get("finish_reason")
                    .filter(|f| f.as_str().is_some_and(|s| !s.is_empty()));
                let logprobs = choice.get("logprobs");
                let scored = logprob_tokens(logprobs).is_some_and(|tokens| !tokens.is_empty());
                if text.is_empty() && finish.is_none() && !scored {
                    return None;
                }
                useful = true;
                let index = choice
                    .get("index")
                    .cloned()
                    .unwrap_or_else(|| json!(position));
                let key = index.as_u64().unwrap_or(position as u64);
                let offset = self.offsets.entry(key).or_default();
                Some(json!({
                    "index": index,
                    "text": text,
                    "logprobs": legacy_logprobs(logprobs, offset),
                    "finish_reason": finish.cloned().unwrap_or(Value::Null),
                }))
            })
            .collect();
        if !useful {
            return None;
        }
        let mut out = envelope(chunk);
        out.insert("choices".into(), Value::Array(choices));
        if let Some(usage) = usage {
            out.insert("usage".into(), usage.clone());
        }
        finish_extras(chunk, &mut out);
        Some(Value::Object(out))
    }
}

/// Rewrites one `chat.completion.chunk` on its own, as the first chunk of a
/// fresh [`CompletionsStream`]: its log-probability offsets count from the
/// start of this chunk. A whole stream is rewritten with one
/// `CompletionsStream`.
pub fn chat_chunk_to_completions(chunk: &Value) -> Option<Value> {
    CompletionsStream::default().rewrite(chunk)
}
