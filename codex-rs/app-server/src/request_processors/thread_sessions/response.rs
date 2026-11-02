use crate::error_code::internal_error;
use codex_app_server_protocol::IdleSessionCallResponse;
use codex_app_server_protocol::JSONRPCErrorError;
use serde_json::Value;
use serde_json::json;

// Leave space in the one-MiB relay frame for the RPC and relay envelopes.
const MAX_RESPONSE_BYTES: usize = 960 * 1024;
const MAX_TURN_CHARS: usize = 4096;

pub(super) fn transcript(session: &str, turn: &Value) -> Value {
    let mut chars = turn
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) != Some("userMessage"))
        .filter_map(|item| {
            item.get("text")
                .or_else(|| item.get("aggregatedOutput"))
                .and_then(Value::as_str)
        })
        .enumerate()
        .flat_map(|(index, text)| (index > 0).then_some('\n').into_iter().chain(text.chars()));
    let text = chars.by_ref().take(MAX_TURN_CHARS).collect::<String>();
    let truncated = chars.next().is_some();
    json!({"session_id":session,"turn_id":turn["id"],"status":turn["status"],"text":text,"truncated":truncated})
}

pub(super) fn snapshot(
    mut result: Value,
    mut transcripts: Vec<Value>,
    readiness: Option<String>,
) -> Value {
    let Some(sessions) = result
        .pointer("/snapshot/sessions")
        .and_then(Value::as_array)
    else {
        return result;
    };
    transcripts.retain(|transcript| {
        let id = transcript.get("session_id").and_then(Value::as_str);
        id.is_some()
            && sessions
                .iter()
                .any(|session| session.pointer("/value/id").and_then(Value::as_str) == id)
    });
    result["transcripts"] = json!(transcripts);
    result["readiness"] = json!(readiness);
    result
}

pub(super) fn encode(mut result: Value) -> Result<IdleSessionCallResponse, JSONRPCErrorError> {
    loop {
        let response = serde_json::to_string(&result)
            .map_err(|_| internal_error("Invalid live session response"))?;
        // The response is itself a JSON string inside the RPC result. Count its
        // escaped UTF-8 bytes, including control characters and quotation marks.
        let bytes = serde_json::to_vec(&response)
            .map_err(|_| internal_error("Invalid live session response"))?;
        if bytes.len() <= MAX_RESPONSE_BYTES {
            return Ok(IdleSessionCallResponse { response });
        }
        if !shorten_transcripts(&mut result) {
            return Err(internal_error(
                "Live session directory exceeds the transport response limit",
            ));
        }
    }
}

fn shorten_transcripts(result: &mut Value) -> bool {
    let Some(transcripts) = result.get_mut("transcripts").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut changed = false;
    for transcript in transcripts {
        let Some(text) = transcript.get("text").and_then(Value::as_str) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        let text = text
            .chars()
            .take(text.chars().count() / 2)
            .collect::<String>();
        transcript["text"] = json!(text);
        transcript["truncated"] = json!(true);
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn encoded_output_fits_the_relay_with_unicode_and_json_escapes() {
        for character in ['🌍', '\0', '\n', '"', '\\'] {
            let text = character.to_string().repeat(MAX_TURN_CHARS);
            let transcripts: Vec<_> = (0..64).map(|index| {
                transcript("session", &json!({"id":index.to_string(),"status":"completed","items":[{"type":"agentMessage","text":text}]}))
            }).collect();
            let result =
                json!({"snapshot":{},"inputs":"p".repeat(192 * 1024),"transcripts":transcripts});
            let response = encode(result).expect("bounded session response");
            let result: Value = serde_json::from_str(&response.response).expect("session result");
            let frame =
                json!({"kind":"send","connectionId":1,"message":{"id":100,"result":response}});
            assert!(serde_json::to_vec(&frame).expect("relay frame").len() < 1024 * 1024);
            let transcripts = result["transcripts"].as_array().expect("output records");
            assert_eq!(transcripts.len(), 64);
            for turn in transcripts {
                let output = turn["text"].as_str().expect("output text");
                assert!(text.starts_with(output));
                assert!(!output.is_empty());
                assert_eq!(turn["truncated"], output != text);
            }
        }
    }

    #[test]
    fn transcript_bounds_output_without_copying_the_complete_turn() {
        let turn = json!({"id":"turn","status":"completed","items":[
            {"type":"userMessage","text":"private input"},
            {"type":"agentMessage","text":"hello"},
            {"type":"commandExecution","aggregatedOutput":"🌍".repeat(8192)}
        ]});
        let output = transcript("session", &turn);
        assert_eq!(
            output["text"].as_str().expect("text").chars().count(),
            MAX_TURN_CHARS
        );
        assert!(
            output["text"]
                .as_str()
                .expect("text")
                .starts_with("hello\n🌍")
        );
        assert_eq!(output["truncated"], true);
    }

    #[test]
    fn latest_snapshot_removes_output_for_revoked_sessions() {
        let current = json!({"snapshot":{"sessions":[{"value":{"id":"allowed"}}]}});
        let result = snapshot(
            current,
            vec![
                json!({"session_id":"revoked","text":"private output"}),
                json!({"session_id":"allowed","text":"visible output"}),
            ],
            None,
        );
        assert_eq!(
            result["transcripts"],
            json!([{"session_id":"allowed","text":"visible output"}])
        );
        let denied = json!({"error":{"code":"forbidden"}});
        assert_eq!(
            snapshot(denied.clone(), vec![json!({"text":"private output"})], None),
            denied
        );
    }

    #[test]
    fn metadata_overflow_returns_an_rpc_error_instead_of_breaking_the_relay() {
        assert!(encode(json!({"snapshot":"x".repeat(1024 * 1024)})).is_err());
    }
}
