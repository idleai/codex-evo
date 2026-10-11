use crate::error_code::invalid_request;
use codex_app_server_protocol::JSONRPCErrorError;
use serde_json::Value;

pub(crate) fn validate(method: &str, params: &Value) -> Result<(), JSONRPCErrorError> {
    let keys: &[&str] = match method {
        "thread/start" => &[
            "model",
            "modelProvider",
            "serviceTier",
            "historyMode",
            "experimentalRawEvents",
        ],
        "thread/resume" => &[
            "threadId",
            "model",
            "modelProvider",
            "serviceTier",
            "excludeTurns",
            "initialTurnsPage",
        ],
        "thread/fork" => &["threadId", "excludeTurns", "lastTurnId", "beforeTurnId"],
        "thread/read" => &["threadId", "includeTurns"],
        "thread/unsubscribe" => &["threadId"],
        "thread/turns/list" | "thread/items/list" => {
            &["threadId", "turnId", "cursor", "limit", "sortDirection"]
        }
        "turn/start" => &[
            "threadId",
            "input",
            "model",
            "effort",
            "summary",
            "serviceTier",
            "clientUserMessageId",
        ],
        "turn/steer" => &["threadId", "input", "expectedTurnId"],
        "turn/interrupt" => &["threadId", "turnId"],
        _ => {
            return Err(invalid_request(
                "This operation is outside the runner grant",
            ));
        }
    };
    let params = params
        .as_object()
        .ok_or_else(|| invalid_request("Invalid runner request"))?;
    if params
        .iter()
        .any(|(key, value)| !value.is_null() && !keys.contains(&key.as_str()))
    {
        return Err(invalid_request(
            "Runner requests cannot replace their saved paths, environment, tools or approval policy",
        ));
    }
    if let Some(input) = params.get("input") {
        let input = input
            .as_array()
            .ok_or_else(|| invalid_request("Runner input must be text"))?;
        if input.iter().any(|item| {
            item.get("type").and_then(Value::as_str) != Some("text")
                || item.get("textElements").is_some_and(|elements| {
                    !elements.is_null()
                        && elements
                            .as_array()
                            .is_none_or(|elements| !elements.is_empty())
                })
        }) {
            return Err(invalid_request("Runner input must be plain text"));
        }
    }
    Ok(())
}
