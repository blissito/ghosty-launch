//! `complete_turn` — drive one LLM turn end-to-end and fold the streamed
//! events back into a single [`MessageResponse`].
//!
//! The vendored `DeepSeekClient` exposes two surfaces: `create_message` (a
//! single non-streaming POST that already returns a `MessageResponse`) and
//! `create_message_stream` (the production SSE path that the reasoning logic
//! flows through). Most callers want "send a request, get the full answer
//! with its `reasoning_content` + `tool_calls` + usage" without hand-rolling
//! the stream fold. `complete_turn` is that convenience.
//!
//! This module adds NO reasoning logic — it only accumulates the
//! `StreamEvent`s that `client::chat::parse_sse_chunk` already produces:
//! `Thinking` deltas → a `ContentBlock::Thinking`, `Text` deltas → a
//! `ContentBlock::Text`, and `ToolUse` starts + `InputJsonDelta`s → a
//! `ContentBlock::ToolUse` with parsed arguments. Blocks are emitted in the
//! order their indices first appear, mirroring the on-wire order.

use std::collections::BTreeMap;

use anyhow::Result;
use futures_util::StreamExt;
use serde_json::Value;

use crate::llm_client::LlmClient;
use crate::models::{
    ContentBlock, ContentBlockStart, Delta, MessageRequest, MessageResponse, StreamEvent, ToolCaller,
    Usage,
};

/// Accumulator for a single streamed content block, keyed by its stream index.
enum BlockAccumulator {
    Text(String),
    Thinking(String),
    ToolUse {
        id: String,
        name: String,
        /// Concatenated `input_json_delta` fragments; parsed on finalize.
        partial_json: String,
        caller: Option<ToolCaller>,
    },
}

impl BlockAccumulator {
    fn from_start(start: ContentBlockStart) -> Option<Self> {
        match start {
            ContentBlockStart::Text { text } => Some(Self::Text(text)),
            ContentBlockStart::Thinking { thinking } => Some(Self::Thinking(thinking)),
            ContentBlockStart::ToolUse {
                id, name, caller, ..
            } => Some(Self::ToolUse {
                id,
                name,
                partial_json: String::new(),
                caller,
            }),
            // Server-side tools are not produced by the DeepSeek chat path; if a
            // provider ever emits one, drop it rather than misrepresent it.
            ContentBlockStart::ServerToolUse { .. } => None,
        }
    }

    fn apply_delta(&mut self, delta: Delta) {
        match (self, delta) {
            (Self::Text(buf), Delta::TextDelta { text }) => buf.push_str(&text),
            (Self::Thinking(buf), Delta::ThinkingDelta { thinking }) => buf.push_str(&thinking),
            (Self::ToolUse { partial_json, .. }, Delta::InputJsonDelta { partial_json: frag }) => {
                partial_json.push_str(&frag);
            }
            // A delta whose kind doesn't match the open block is malformed; the
            // upstream SSE parser already keeps kinds aligned, so ignore.
            _ => {}
        }
    }

    fn into_content_block(self) -> Option<ContentBlock> {
        match self {
            Self::Text(text) => (!text.is_empty()).then_some(ContentBlock::Text {
                text,
                cache_control: None,
            }),
            Self::Thinking(thinking) => {
                (!thinking.is_empty()).then_some(ContentBlock::Thinking { thinking })
            }
            Self::ToolUse {
                id,
                name,
                partial_json,
                caller,
            } => {
                let input = if partial_json.trim().is_empty() {
                    Value::Object(serde_json::Map::new())
                } else {
                    // Mirror the non-streaming parser: fall back to the raw
                    // string when the accumulated fragments aren't valid JSON.
                    serde_json::from_str(&partial_json)
                        .unwrap_or(Value::String(partial_json))
                };
                Some(ContentBlock::ToolUse {
                    id,
                    name,
                    input,
                    caller,
                })
            }
        }
    }
}

/// Send `request` and stream the response, folding every event into a single
/// [`MessageResponse`] — the same shape `create_message` returns, but routed
/// through the SSE path (so `reasoning_content` replay and the thinking-mode
/// sanitizer apply). The returned `content` carries `Thinking` / `Text` /
/// `ToolUse` blocks in wire order, plus the final server-reported `usage`.
pub async fn complete_turn<C: LlmClient>(
    client: &C,
    request: MessageRequest,
) -> Result<MessageResponse> {
    let model = request.model.clone();
    let mut stream = client.create_message_stream(request).await?;

    // Skeleton filled in from the synthetic `MessageStart`; overwritten as the
    // stream progresses.
    let mut id = String::new();
    let mut role = "assistant".to_string();
    let mut stop_reason: Option<String> = None;
    let mut stop_sequence: Option<String> = None;
    let mut usage = Usage::default();

    // Open blocks keyed by stream index, plus first-seen order so we can emit
    // them in the order the model produced them.
    let mut open: BTreeMap<u32, BlockAccumulator> = BTreeMap::new();
    let mut order: Vec<u32> = Vec::new();
    let mut finished: BTreeMap<u32, ContentBlock> = BTreeMap::new();

    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::MessageStart { message } => {
                if !message.id.is_empty() {
                    id = message.id;
                }
                role = message.role;
            }
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                if let Some(acc) = BlockAccumulator::from_start(content_block) {
                    if !open.contains_key(&index) && !finished.contains_key(&index) {
                        order.push(index);
                    }
                    open.insert(index, acc);
                }
            }
            StreamEvent::ContentBlockDelta { index, delta } => {
                if let Some(acc) = open.get_mut(&index) {
                    acc.apply_delta(delta);
                }
            }
            StreamEvent::ContentBlockStop { index } => {
                if let Some(acc) = open.remove(&index)
                    && let Some(block) = acc.into_content_block()
                {
                    finished.insert(index, block);
                }
            }
            StreamEvent::MessageDelta {
                delta,
                usage: delta_usage,
            } => {
                if let Some(reason) = delta.stop_reason {
                    stop_reason = Some(reason);
                }
                if delta.stop_sequence.is_some() {
                    stop_sequence = delta.stop_sequence;
                }
                if let Some(u) = delta_usage {
                    usage = u;
                }
            }
            StreamEvent::MessageStop => break,
            StreamEvent::Ping => {}
        }
    }

    // Drain any blocks the stream left open (e.g. truncated mid-flight): the
    // close events at the end of the SSE loop normally handle these, but be
    // defensive so a partial answer still surfaces.
    for (index, acc) in std::mem::take(&mut open) {
        if let Some(block) = acc.into_content_block() {
            finished.entry(index).or_insert(block);
        }
    }

    // Emit in first-seen order, falling back to index order for anything the
    // order vector missed.
    let mut content = Vec::with_capacity(finished.len());
    let mut emitted: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for index in &order {
        if let Some(block) = finished.remove(index) {
            content.push(block);
            emitted.insert(*index);
        }
    }
    for (index, block) in finished {
        if emitted.insert(index) {
            content.push(block);
        }
    }

    Ok(MessageResponse {
        id,
        r#type: "message".to_string(),
        role,
        content,
        model,
        stop_reason,
        stop_sequence,
        container: None,
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::mock::{MockLlmClient, canned};
    use crate::models::Message;

    fn user_request(text: &str) -> MessageRequest {
        MessageRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: text.to_string(),
                    cache_control: None,
                }],
            }],
            max_tokens: 1024,
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: Some("high".to_string()),
            stream: Some(true),
            temperature: None,
            top_p: None,
        }
    }

    #[tokio::test]
    async fn folds_simple_text_turn_into_one_text_block() {
        let client = MockLlmClient::new(vec![canned::simple_text_turn("Hello there.")]);
        let response = complete_turn(&client, user_request("hi")).await.unwrap();

        assert_eq!(response.role, "assistant");
        assert_eq!(response.model, "deepseek-v4-pro");
        assert_eq!(response.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(response.content.len(), 1);
        assert!(matches!(
            &response.content[0],
            ContentBlock::Text { text, .. } if text == "Hello there."
        ));
    }

    #[tokio::test]
    async fn folds_reasoning_then_text_in_wire_order() {
        // Thinking block (index 0) closes, then a text block (index 1) — the
        // exact shape the SSE parser emits for a DeepSeek thinking-mode answer.
        let turn = vec![
            canned::message_start("msg_reason"),
            // thinking starts implicitly via the first thinking delta in the
            // real parser; the mock opens it explicitly for determinism.
            StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockStart::Thinking {
                    thinking: String::new(),
                },
            },
            canned::thinking_delta(0, "Let me reason."),
            canned::block_stop(0),
            canned::text_block_start(1),
            canned::text_delta(1, "Final answer."),
            canned::block_stop(1),
            canned::message_delta("end_turn", None),
            canned::message_stop(),
        ];
        let client = MockLlmClient::new(vec![turn]);
        let response = complete_turn(&client, user_request("explain"))
            .await
            .unwrap();

        assert_eq!(response.content.len(), 2);
        assert!(matches!(
            &response.content[0],
            ContentBlock::Thinking { thinking } if thinking == "Let me reason."
        ));
        assert!(matches!(
            &response.content[1],
            ContentBlock::Text { text, .. } if text == "Final answer."
        ));
    }

    #[tokio::test]
    async fn folds_tool_call_turn_and_parses_json_args() {
        let client = MockLlmClient::new(vec![canned::tool_call_turn(
            "call_42",
            "read_file",
            r#"{"path":"src/main.rs"}"#,
        )]);
        let response = complete_turn(&client, user_request("read it"))
            .await
            .unwrap();

        assert_eq!(response.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(response.content.len(), 1);
        match &response.content[0] {
            ContentBlock::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(id, "call_42");
                assert_eq!(name, "read_file");
                assert_eq!(input["path"], "src/main.rs");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn captures_final_usage_from_message_delta() {
        let usage = Usage {
            input_tokens: 120,
            output_tokens: 40,
            prompt_cache_hit_tokens: Some(80),
            prompt_cache_miss_tokens: Some(40),
            reasoning_tokens: Some(15),
            ..Usage::default()
        };
        let turn = vec![
            canned::message_start("msg_usage"),
            canned::text_block_start(0),
            canned::text_delta(0, "ok"),
            canned::block_stop(0),
            canned::message_delta("end_turn", Some(usage.clone())),
            canned::message_stop(),
        ];
        let client = MockLlmClient::new(vec![turn]);
        let response = complete_turn(&client, user_request("hi")).await.unwrap();

        assert_eq!(response.usage.input_tokens, 120);
        assert_eq!(response.usage.output_tokens, 40);
        assert_eq!(response.usage.prompt_cache_hit_tokens, Some(80));
        assert_eq!(response.usage.reasoning_tokens, Some(15));
    }

    #[tokio::test]
    async fn multi_fragment_tool_args_are_concatenated_before_parse() {
        // The streaming path delivers tool arguments as several
        // `input_json_delta` fragments; complete_turn must join them before
        // parsing, exactly like the non-streaming parser concatenates the wire
        // `arguments` string.
        let turn = vec![
            canned::message_start("msg_frag"),
            canned::tool_use_block_start(0, "call_frag", "write_file"),
            canned::tool_input_delta(0, r#"{"path":"a.rs","#),
            canned::tool_input_delta(0, r#""contents":"x"}"#),
            canned::block_stop(0),
            canned::message_delta("tool_use", None),
            canned::message_stop(),
        ];
        let client = MockLlmClient::new(vec![turn]);
        let response = complete_turn(&client, user_request("write"))
            .await
            .unwrap();

        match &response.content[0] {
            ContentBlock::ToolUse { input, .. } => {
                assert_eq!(input["path"], "a.rs");
                assert_eq!(input["contents"], "x");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }
}
