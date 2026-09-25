use super::*;
use codex_protocol::models::MessagePhase;
use codex_protocol::protocol::UserMessageEvent;
use pretty_assertions::assert_eq;

fn response_item(item: ResponseItem) -> RolloutItem {
    RolloutItem::ResponseItem(ResponseItemEnvelope::new(item))
}

fn message(role: &str, text: &str) -> RolloutItem {
    let content = if role == "assistant" {
        ContentItem::OutputText {
            text: text.to_string(),
        }
    } else {
        ContentItem::InputText {
            text: text.to_string(),
        }
    };
    response_item(ResponseItem::Message {
        id: None,
        role: role.to_string(),
        content: vec![content],
        phase: (role == "assistant").then_some(MessagePhase::FinalAnswer),
        internal_chat_message_metadata_passthrough: None,
    })
}

fn turn_started(turn_id: &str) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::TurnStarted(TurnStartedEvent {
        turn_id: turn_id.to_string(),
        root_turn_id: None,
        trace_id: None,
        started_at: Some(10),
        model_context_window: None,
        collaboration_mode_kind: Default::default(),
    }))
}

fn turn_completed(turn_id: &str) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::TurnComplete(TurnCompleteEvent {
        turn_id: turn_id.to_string(),
        last_agent_message: None,
        error: None,
        started_at: Some(10),
        completed_at: Some(20),
        duration_ms: Some(10_000),
        time_to_first_token_ms: None,
    }))
}

#[test]
fn keeps_visible_text_and_drops_provider_specific_items() {
    let source = vec![
        turn_started("turn-1"),
        message("user", "inspect the repository"),
        response_item(ResponseItem::FunctionCall {
            id: None,
            name: "web_search".to_string(),
            namespace: None,
            arguments: "{\"query\":\"codex\"}".to_string(),
            encrypted_function_args: None,
            call_id: "call-1".to_string(),
            internal_chat_message_metadata_passthrough: None,
        }),
        message("assistant", "Here is the result."),
        turn_completed("turn-1"),
    ];

    let projected = portable_handoff_history(&source);
    let projected_messages = projected
        .iter()
        .filter_map(|item| match item {
            RolloutItem::ResponseItem(envelope) => match &envelope.item {
                ResponseItem::Message {
                    role,
                    content,
                    phase,
                    internal_chat_message_metadata_passthrough,
                    ..
                } => Some((
                    role.clone(),
                    content.clone(),
                    phase.clone(),
                    internal_chat_message_metadata_passthrough.clone(),
                )),
                _ => panic!("portable history retained a provider-specific response item"),
            },
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        projected_messages,
        vec![
            (
                "user".to_string(),
                vec![ContentItem::InputText {
                    text: "inspect the repository".to_string(),
                }],
                None,
                None,
            ),
            (
                "assistant".to_string(),
                vec![ContentItem::OutputText {
                    text: "Here is the result.".to_string(),
                }],
                None,
                None,
            ),
        ]
    );
}

#[test]
fn bounds_each_message_without_splitting_utf8() {
    let source = vec![
        turn_started("turn-1"),
        message("user", &"é".repeat(MAX_PORTABLE_TEXT_BYTES)),
        message("assistant", &"a".repeat(MAX_PORTABLE_TEXT_BYTES * 2)),
        turn_completed("turn-1"),
    ];

    let projected = portable_handoff_history(&source);
    let text = projected
        .iter()
        .filter_map(|item| match item {
            RolloutItem::ResponseItem(envelope) => match &envelope.item {
                ResponseItem::Message { content, .. } => content.first(),
                _ => None,
            },
            _ => None,
        })
        .map(|content| match content {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => text,
            ContentItem::InputImage { .. } | ContentItem::InputAudio { .. } => {
                panic!("portable history retained non-text content")
            }
        })
        .collect::<Vec<_>>();

    assert_eq!(text.len(), 2);
    assert_eq!(text[0].len(), MAX_PORTABLE_TEXT_BYTES);
    assert_eq!(text[1].len(), MAX_PORTABLE_TEXT_BYTES);
    assert!(text[0].is_char_boundary(text[0].len()));
}

#[test]
fn repeated_handoff_keeps_inherited_portable_turns_before_new_native_turns() {
    let inherited = portable_handoff_history(&[
        turn_started("source-turn"),
        message("user", "original user"),
        message("assistant", "original assistant"),
        turn_completed("source-turn"),
    ]);
    assert!(matches!(
        &inherited[0],
        RolloutItem::EventMsg(EventMsg::TurnStarted(event))
            if event.turn_id.starts_with(PORTABLE_TURN_ID_PREFIX)
    ));

    let mut mixed = inherited;
    mixed.extend([
        turn_started("native-turn"),
        RolloutItem::EventMsg(EventMsg::UserMessage(UserMessageEvent {
            message: "new user".to_string(),
            ..Default::default()
        })),
        message("user", "new user"),
        message("assistant", "new assistant"),
        turn_completed("native-turn"),
    ]);

    let projected = portable_handoff_history(&mixed);
    let visible_text = projected
        .iter()
        .filter_map(|item| match item {
            RolloutItem::ResponseItem(envelope) => match &envelope.item {
                ResponseItem::Message { content, .. } => content.first(),
                _ => None,
            },
            _ => None,
        })
        .map(|content| match content {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => text.as_str(),
            ContentItem::InputImage { .. } | ContentItem::InputAudio { .. } => {
                panic!("portable history retained non-text content")
            }
        })
        .collect::<Vec<_>>();

    assert_eq!(
        visible_text,
        vec![
            "original user",
            "original assistant",
            "new user",
            "new assistant",
        ]
    );
}
