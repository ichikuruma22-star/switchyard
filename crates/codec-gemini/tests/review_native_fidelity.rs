//! Review evidence: details of a native Gemini request that are damaged when
//! it goes through the canonical model (Gemini client -> decode -> encode for
//! a Gemini upstream, which is what happens whenever the history carries a
//! wrapped foreign signature, e.g. after a fail-over from another provider;
//! and Gemini client -> another vendor).

use pretty_assertions::assert_eq;
use serde_json::json;
use switchyard_codec_gemini::GeminiCodec;
use switchyard_core::ir::{Message, OpaquePart, Part, Request, Role, Tool};
use switchyard_core::{Codec, Protocol, RequestPath, UpstreamCtx};

fn path() -> RequestPath<'static> {
    RequestPath {
        model: Some("gemini-2.5-pro"),
        stream: Some(false),
    }
}

/// `fileData` without a `mimeType` (the documented way to pass a YouTube URL)
/// comes back out with an invented `"mimeType": "application/pdf"`, which
/// tells Gemini to read a video page as a PDF. A provider file handle in the
/// same position is re-emitted without a MIME type; a plain URL must be
/// treated the same way when nothing is known about its type.
#[test]
fn file_data_without_a_mime_type_does_not_become_a_pdf() {
    let body = json!({"contents": [{"role": "user", "parts": [
        {"fileData": {"fileUri": "https://www.youtube.com/watch?v=9hE5-98ZeCg"}},
        {"text": "summarise this video"}
    ]}]});
    let request = GeminiCodec
        .decode_request(&body, &path())
        .expect("request decodes");
    let encoded = GeminiCodec
        .encode_request(&request, &UpstreamCtx::default())
        .expect("request encodes");
    assert_eq!(
        encoded["contents"],
        json!([{"role": "user", "parts": [
            {"fileData": {"fileUri": "https://www.youtube.com/watch?v=9hE5-98ZeCg"}},
            {"text": "summarise this video"}
        ]}])
    );
}

/// `videoMetadata` (clip offsets, frame rate) and `mediaResolution` sit on a
/// media part next to its `fileData` / `inlineData`. Decoding read only the
/// payload, so a re-encoded request made Gemini process the whole video at
/// the default frame rate, and the image at the default resolution.
#[test]
fn media_part_settings_survive_re_encoding_for_gemini() {
    let parts = json!([
        {
            "fileData": {"fileUri": "https://www.youtube.com/watch?v=9hE5-98ZeCg"},
            "videoMetadata": {"startOffset": "60s", "endOffset": "90s", "fps": 5}
        },
        {
            "inlineData": {"mimeType": "image/png", "data": "iVBORw0KGgo="},
            "mediaResolution": {"level": "MEDIA_RESOLUTION_LOW"}
        },
        {"text": "what happens in this clip"}
    ]);
    let body = json!({"contents": [{"role": "user", "parts": parts}]});
    let request = GeminiCodec
        .decode_request(&body, &path())
        .expect("request decodes");
    // The media itself stays canonical, so that other protocols keep it; the
    // settings ride behind it as Gemini's own, which they leave out.
    let kinds: Vec<&str> = request.messages[0]
        .parts
        .iter()
        .map(|part| match part {
            Part::Document(_) => "document",
            Part::Image(_) => "image",
            Part::Opaque(opaque) if opaque.origin == Protocol::Gemini => "gemini",
            Part::Text(_) => "text",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["document", "gemini", "image", "gemini", "text"]);
    let encoded = GeminiCodec
        .encode_request(&request, &UpstreamCtx::default())
        .expect("request encodes");
    assert_eq!(
        encoded["contents"],
        json!([{"role": "user", "parts": parts}])
    );

    // The snake_case spelling comes back in the canonical one.
    let body = json!({"contents": [{"role": "user", "parts": [
        {
            "file_data": {"file_uri": "https://example.com/clip.mp4", "mime_type": "video/mp4"},
            "video_metadata": {"start_offset": "5s"},
            "media_resolution": "MEDIA_RESOLUTION_HIGH"
        }
    ]}]});
    let request = GeminiCodec
        .decode_request(&body, &path())
        .expect("request decodes");
    let encoded = GeminiCodec
        .encode_request(&request, &UpstreamCtx::default())
        .expect("request encodes");
    assert_eq!(
        encoded["contents"][0]["parts"],
        json!([{
            "fileData": {"mimeType": "video/mp4", "fileUri": "https://example.com/clip.mp4"},
            "videoMetadata": {"start_offset": "5s"},
            "mediaResolution": "MEDIA_RESOLUTION_HIGH"
        }])
    );
}

/// Media settings mean nothing without the media part right before them, and
/// Gemini refuses a part without payload: they are never sent on their own.
#[test]
fn media_settings_without_their_media_are_left_out() {
    let settings = Part::Opaque(OpaquePart {
        origin: Protocol::Gemini,
        raw: json!({"videoMetadata": {"fps": 5}}),
    });
    let mut request = Request::new("gemini-2.5-pro", Protocol::Gemini);
    request.messages = vec![Message::new(Role::User, vec![Part::text("look"), settings])];
    let encoded = GeminiCodec
        .encode_request(&request, &UpstreamCtx::default())
        .expect("request encodes");
    assert_eq!(
        encoded["contents"],
        json!([{"role": "user", "parts": [{"text": "look"}]}])
    );
}

/// Gemini's `Schema` message declares `minItems`, `maxItems`, `minLength`,
/// `maxLength`, `minProperties` and `maxProperties` as `int64`, which proto3
/// JSON renders as **strings** (`"minItems": "1"`; the REST reference lists
/// them as "string (int64 format)" and Google's JS SDK types them as
/// `string`). `from_gemini_schema` is documented as turning the dialect into
/// standard JSON Schema, where these keywords must be numbers: Anthropic and
/// OpenAI reject `"minItems": "1"` as an invalid schema.
#[test]
fn int64_string_constraints_of_the_gemini_dialect_become_numbers() {
    let body = json!({
        "contents": [{"role": "user", "parts": [{"text": "x"}]}],
        "tools": [{"functionDeclarations": [{"name": "tag", "parameters": {
            "type": "OBJECT",
            "minProperties": "1",
            "properties": {"tags": {
                "type": "ARRAY", "minItems": "1", "maxItems": "3",
                "items": {"type": "STRING", "minLength": "2", "maxLength": "10"}
            }},
            "required": ["tags"]
        }}]}]
    });
    let request = GeminiCodec
        .decode_request(&body, &path())
        .expect("request decodes");
    let Tool::Function(function) = &request.tools[0] else {
        panic!("expected a function tool");
    };
    assert_eq!(
        function.parameters,
        json!({
            "type": "object",
            "minProperties": 1,
            "properties": {"tags": {
                "type": "array", "minItems": 1, "maxItems": 3,
                "items": {"type": "string", "minLength": 2, "maxLength": 10}
            }},
            "required": ["tags"]
        })
    );
}
