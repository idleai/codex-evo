use super::build_legacy_api_turns_from_rollout_items;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::Turn;
use codex_app_server_protocol::UserInput;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::TurnCompleteEvent;
use codex_protocol::protocol::TurnStartedEvent;
use codex_rollout::ResponseItemEnvelope;
use codex_rollout::RolloutItem;

const MAX_PORTABLE_TURNS: usize = 32;
const MAX_PORTABLE_TEXT_BYTES: usize = 8 * 1024;
const MAX_PORTABLE_HISTORY_TEXT_BYTES: usize = 64 * 1024;
const PORTABLE_TURN_ID_PREFIX: &str = "portable-handoff-turn-";

#[derive(Clone)]
struct PortableTurn {
    id: String,
    user_text: String,
    assistant_text: Option<String>,
    started_at: Option<i64>,
    completed_at: Option<i64>,
    duration_ms: Option<i64>,
}

/// Project persisted history into a bounded sequence that OpenAI-compatible providers can share.
///
/// The projection intentionally keeps only visible user and assistant text. Reasoning payloads,
/// tool calls and outputs, images, audio, provider-owned IDs, and passthrough metadata are omitted.
pub(super) fn portable_handoff_history(items: &[RolloutItem]) -> Vec<RolloutItem> {
    let legacy_turns = build_legacy_api_turns_from_rollout_items(items)
        .into_iter()
        .filter_map(portable_turn)
        .collect::<Vec<_>>();
    let response_item_turns = portable_response_item_turns(items);
    let turns = if legacy_turns.is_empty() {
        response_item_turns
    } else {
        merge_legacy_and_response_turns(legacy_turns, response_item_turns)
    };

    let mut selected = Vec::new();
    let mut text_bytes = 0usize;
    for turn in turns.into_iter().rev().take(MAX_PORTABLE_TURNS) {
        let turn_text_bytes = turn.user_text.len()
            + turn
                .assistant_text
                .as_ref()
                .map_or(0, std::string::String::len);
        if text_bytes + turn_text_bytes > MAX_PORTABLE_HISTORY_TEXT_BYTES {
            break;
        }
        text_bytes += turn_text_bytes;
        selected.push(turn);
    }
    selected.reverse();

    selected
        .into_iter()
        .enumerate()
        .flat_map(|(index, mut turn)| {
            turn.id = format!("{PORTABLE_TURN_ID_PREFIX}{}", index + 1);
            portable_rollout_items(turn)
        })
        .collect()
}

fn merge_legacy_and_response_turns(
    legacy_turns: Vec<PortableTurn>,
    response_item_turns: Vec<PortableTurn>,
) -> Vec<PortableTurn> {
    let mut merged = Vec::new();
    let mut response_start = 0;

    for mut legacy_turn in legacy_turns {
        let matching_response = response_item_turns[response_start..]
            .iter()
            .position(|response_turn| {
                response_turn.id == legacy_turn.id
                    || (!response_turn.id.starts_with(PORTABLE_TURN_ID_PREFIX)
                        && response_turn.user_text == legacy_turn.user_text)
            })
            .map(|offset| response_start + offset);
        if let Some(matching_response) = matching_response {
            // Raw messages from an earlier portable handoff have no corresponding legacy UI
            // events. Preserve marked turns that occurred before the matching native turn.
            merged.extend(
                response_item_turns[response_start..matching_response]
                    .iter()
                    .filter(|turn| turn.id.starts_with(PORTABLE_TURN_ID_PREFIX))
                    .cloned(),
            );
            if legacy_turn.assistant_text.is_none() {
                legacy_turn.assistant_text = response_item_turns[matching_response]
                    .assistant_text
                    .clone();
            }
            response_start = matching_response + 1;
        }
        merged.push(legacy_turn);
    }

    merged.extend(
        response_item_turns[response_start..]
            .iter()
            .filter(|turn| turn.id.starts_with(PORTABLE_TURN_ID_PREFIX))
            .cloned(),
    );
    merged
}

fn portable_response_item_turns(items: &[RolloutItem]) -> Vec<PortableTurn> {
    let mut turns = Vec::new();
    let mut current: Option<PortableTurn> = None;
    let mut started_at = None;
    let mut active_turn_id = None;
    let mut next_turn_id = 1usize;

    for item in items {
        match item {
            RolloutItem::EventMsg(EventMsg::TurnStarted(event)) => {
                if let Some(turn) = current.take() {
                    turns.push(turn);
                }
                started_at = event.started_at;
                active_turn_id = Some(event.turn_id.clone());
            }
            RolloutItem::EventMsg(EventMsg::TurnComplete(event)) => {
                if let Some(mut turn) = current.take() {
                    turn.completed_at = event.completed_at;
                    turn.duration_ms = event.duration_ms;
                    turns.push(turn);
                }
                started_at = None;
                active_turn_id = None;
            }
            RolloutItem::ResponseItem(envelope) => {
                let ResponseItem::Message { role, content, .. } = &envelope.item else {
                    continue;
                };
                let text = content
                    .iter()
                    .filter_map(|content| match content {
                        ContentItem::InputText { text } | ContentItem::OutputText { text }
                            if !text.trim().is_empty() =>
                        {
                            Some(text.as_str())
                        }
                        ContentItem::InputText { .. }
                        | ContentItem::OutputText { .. }
                        | ContentItem::InputImage { .. }
                        | ContentItem::InputAudio { .. } => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let Some(text) = bounded_text(text) else {
                    continue;
                };

                match role.as_str() {
                    "user" => {
                        if let Some(turn) = current.take() {
                            turns.push(turn);
                        }
                        current = Some(PortableTurn {
                            id: active_turn_id
                                .clone()
                                .unwrap_or_else(|| format!("response-item-turn-{next_turn_id}")),
                            user_text: text,
                            assistant_text: None,
                            started_at,
                            completed_at: None,
                            duration_ms: None,
                        });
                        next_turn_id += 1;
                    }
                    "assistant" => {
                        let Some(turn) = current.as_mut() else {
                            continue;
                        };
                        let combined = match turn.assistant_text.take() {
                            Some(existing) => format!("{existing}\n\n{text}"),
                            None => text,
                        };
                        turn.assistant_text = bounded_text(combined);
                    }
                    _ => {}
                }
            }
            RolloutItem::EventMsg(_)
            | RolloutItem::Compacted(_)
            | RolloutItem::TokenUsageRecord(_)
            | RolloutItem::RetainedContext(_)
            | RolloutItem::RealtimeItem(_)
            | RolloutItem::InterAgentCommunication(_)
            | RolloutItem::InterAgentCommunicationMetadata { .. }
            | RolloutItem::TurnContext(_)
            | RolloutItem::WorldState(_)
            | RolloutItem::SecurityRiskScore(_)
            | RolloutItem::SessionMeta(_) => {}
        }
    }
    if let Some(turn) = current {
        turns.push(turn);
    }
    turns
}

fn portable_turn(turn: Turn) -> Option<PortableTurn> {
    let mut user_parts = Vec::new();
    let mut assistant_parts = Vec::new();
    for item in turn.items {
        match item {
            ThreadItem::UserMessage { content, .. } => {
                user_parts.extend(content.into_iter().filter_map(|input| match input {
                    UserInput::Text { text, .. } if !text.trim().is_empty() => Some(text),
                    UserInput::Text { .. }
                    | UserInput::Image { .. }
                    | UserInput::LocalImage { .. }
                    | UserInput::Audio { .. }
                    | UserInput::LocalAudio { .. }
                    | UserInput::Skill { .. }
                    | UserInput::Mention { .. } => None,
                }));
            }
            ThreadItem::AgentMessage { text, .. } if !text.trim().is_empty() => {
                assistant_parts.push(text);
            }
            ThreadItem::HookPrompt { .. }
            | ThreadItem::AgentMessage { .. }
            | ThreadItem::Plan { .. }
            | ThreadItem::Reasoning { .. }
            | ThreadItem::CommandExecution { .. }
            | ThreadItem::FileChange { .. }
            | ThreadItem::McpToolCall { .. }
            | ThreadItem::DynamicToolCall { .. }
            | ThreadItem::FunctionCallOutput { .. }
            | ThreadItem::CollabAgentToolCall { .. }
            | ThreadItem::SubAgentActivity { .. }
            | ThreadItem::WebSearch(_)
            | ThreadItem::ImageView { .. }
            | ThreadItem::Sleep(_)
            | ThreadItem::ImageGeneration(_)
            | ThreadItem::EnteredReviewMode { .. }
            | ThreadItem::ExitedReviewMode { .. }
            | ThreadItem::ContextCompaction { .. } => {}
        }
    }

    let user_text = bounded_text(user_parts.join("\n\n"))?;
    let assistant_text = bounded_text(assistant_parts.join("\n\n"));
    Some(PortableTurn {
        id: turn.id,
        user_text,
        assistant_text,
        started_at: turn.started_at,
        completed_at: turn.completed_at,
        duration_ms: turn.duration_ms,
    })
}

fn bounded_text(text: String) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    if text.len() <= MAX_PORTABLE_TEXT_BYTES {
        return Some(text);
    }
    let mut end = MAX_PORTABLE_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(text[..end].to_string())
}

fn portable_rollout_items(turn: PortableTurn) -> Vec<RolloutItem> {
    let PortableTurn {
        id,
        user_text,
        assistant_text,
        started_at,
        completed_at,
        duration_ms,
    } = turn;
    let mut items = vec![
        RolloutItem::EventMsg(EventMsg::TurnStarted(TurnStartedEvent {
            turn_id: id.clone(),
            root_turn_id: None,
            trace_id: None,
            started_at,
            model_context_window: None,
            collaboration_mode_kind: Default::default(),
        })),
        message("user", ContentItem::InputText { text: user_text }),
    ];
    if let Some(assistant_text) = assistant_text {
        items.push(message(
            "assistant",
            ContentItem::OutputText {
                text: assistant_text,
            },
        ));
    }
    items.push(RolloutItem::EventMsg(EventMsg::TurnComplete(
        TurnCompleteEvent {
            turn_id: id,
            last_agent_message: None,
            error: None,
            started_at,
            completed_at,
            duration_ms,
            time_to_first_token_ms: None,
        },
    )));
    items
}

fn message(role: &str, content: ContentItem) -> RolloutItem {
    RolloutItem::ResponseItem(ResponseItemEnvelope::new(ResponseItem::Message {
        id: None,
        role: role.to_string(),
        content: vec![content],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }))
}

#[cfg(test)]
#[path = "portable_handoff_tests.rs"]
mod tests;
