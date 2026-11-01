use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Result;
use codex_core::TurnInputRequest;
use codex_core::config::Config;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistry;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ToolAdmissionContributor;
use codex_extension_api::ToolAdmissionInput;
use codex_extension_api::ToolCallOutcome;
use codex_extension_api::ToolFinishInput;
use codex_extension_api::ToolLifecycleContributor;
use codex_extension_api::ToolStartInput;
use codex_features::Feature;
use codex_mcp::CODEX_APPS_MCP_SERVER_NAME;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::apps_test_server::AppsTestServer;
use core_test_support::apps_test_server::SEARCH_CALENDAR_LIST_TOOL;
use core_test_support::apps_test_server::SEARCH_CALENDAR_NAMESPACE;
use core_test_support::apps_test_server::apps_enabled_builder;
use core_test_support::responses;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use core_test_support::wait_for_mcp_server;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use test_case::test_case;
use tokio::sync::Notify;
use tokio::time::timeout;

const DENIED: &str = "Workspace authorization has ended";

#[derive(Clone, Copy)]
enum Admission {
    Registered,
    Absent,
}

#[derive(Default)]
struct Gate {
    target: &'static str,
    denied: AtomicBool,
    pause: bool,
    entered: Notify,
    starts: Mutex<Vec<String>>,
    finishes: Mutex<Vec<(String, ToolCallOutcome)>>,
}

impl Gate {
    fn extensions(self: &Arc<Self>, admission: Admission) -> Arc<ExtensionRegistry<Config>> {
        let mut builder = ExtensionRegistryBuilder::new();
        if matches!(admission, Admission::Registered) {
            builder.tool_admission_contributor(self.clone());
        }
        builder.tool_lifecycle_contributor(self.clone());
        Arc::new(builder.build().to_builder().build())
    }
}

impl ToolAdmissionContributor for Gate {
    fn admit<'a>(
        &'a self,
        input: ToolAdmissionInput<'a>,
    ) -> ExtensionFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if input.tool_name.name != self.target {
                return Ok(());
            }
            self.entered.notify_one();
            if self.pause {
                std::future::pending::<()>().await;
            }
            if self.denied.load(Ordering::Acquire) {
                Err(DENIED.into())
            } else {
                Ok(())
            }
        })
    }
}

impl ToolLifecycleContributor for Gate {
    fn on_tool_start<'a>(&'a self, input: ToolStartInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            self.starts
                .lock()
                .expect("starts lock")
                .push(input.tool_name.name.clone());
        })
    }

    fn on_tool_finish<'a>(&'a self, input: ToolFinishInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            self.finishes
                .lock()
                .expect("finishes lock")
                .push((input.tool_name.name.clone(), input.outcome));
        })
    }
}

fn finished_response() -> String {
    responses::sse(vec![
        responses::ev_assistant_message("answer", "done"),
        responses::ev_completed("done"),
    ])
}

#[test_case(Admission::Registered; "live_authorization")]
#[test_case(Admission::Absent; "ordinary_codex")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authorization_changes_block_handler_effects(admission: Admission) -> Result<()> {
    let server = responses::start_mock_server().await;
    let plan = json!({"explanation": "admission check", "plan": []});
    let requests = responses::mount_sse_sequence(
        &server,
        ["before", "after"]
            .into_iter()
            .flat_map(|id| {
                [
                    responses::sse(vec![
                        responses::ev_function_call(id, "update_plan", &plan.to_string()),
                        responses::ev_completed(id),
                    ]),
                    finished_response(),
                ]
            })
            .collect(),
    )
    .await;
    let gate = Arc::new(Gate {
        target: "update_plan",
        ..Default::default()
    });
    let test = test_codex()
        .with_extensions(gate.extensions(admission))
        .with_config(|config| config.update_plan_enabled = true)
        .build_with_auto_env(&server)
        .await?;
    for changed in [false, true] {
        gate.denied.store(changed, Ordering::Release);
        test.codex
            .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
                text: "update the plan".into(),
                text_elements: vec![],
            }]))
            .await?;
        let mut updates = Vec::new();
        wait_for_event(&test.codex, |event| match event {
            EventMsg::PlanUpdate(update) => {
                updates.push(serde_json::to_value(update).expect("plan JSON"));
                false
            }
            EventMsg::TurnComplete(_) => true,
            _ => false,
        })
        .await;
        let denied = changed && matches!(admission, Admission::Registered);
        assert_eq!(updates, if denied { vec![] } else { vec![plan.clone()] });
        assert_eq!(
            std::mem::take(&mut *gate.starts.lock().expect("starts lock")),
            if denied {
                vec![]
            } else {
                vec!["update_plan".to_string()]
            }
        );
        assert_eq!(
            std::mem::take(&mut *gate.finishes.lock().expect("finishes lock")),
            vec![(
                "update_plan".to_string(),
                if denied {
                    ToolCallOutcome::Blocked
                } else {
                    ToolCallOutcome::Completed { success: true }
                }
            )]
        );
    }
    let requests = requests.requests();
    assert_eq!(requests.len(), 4);
    assert!(
        requests[1]
            .function_call_output("before")
            .to_string()
            .contains("Plan updated")
    );
    assert!(
        requests[3]
            .function_call_output("after")
            .to_string()
            .contains(if matches!(admission, Admission::Registered) {
                DENIED
            } else {
                "Plan updated"
            })
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_admission_can_be_cancelled() -> Result<()> {
    let server = responses::start_mock_server().await;
    let requests = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_function_call("pending", "update_plan", "{\"plan\":[]}"),
            responses::ev_completed("first"),
        ]),
    )
    .await;
    let gate = Arc::new(Gate {
        target: "update_plan",
        pause: true,
        ..Default::default()
    });
    let test = test_codex()
        .with_extensions(gate.extensions(Admission::Registered))
        .with_config(|config| config.update_plan_enabled = true)
        .build_with_auto_env(&server)
        .await?;
    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "update the plan".into(),
            text_elements: vec![],
        }]))
        .await?;
    timeout(Duration::from_secs(/*secs*/ 10), gate.entered.notified()).await?;
    test.codex.submit(Op::Interrupt).await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnAborted(_))
    })
    .await;
    assert_eq!(requests.requests().len(), 1);
    assert!(gate.starts.lock().expect("starts lock").is_empty());
    assert_eq!(
        *gate.finishes.lock().expect("finishes lock"),
        vec![("update_plan".to_string(), ToolCallOutcome::Aborted)]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_code_mode_calls_pass_through_admission() -> Result<()> {
    let server = responses::start_mock_server().await;
    let requests = responses::mount_sse_sequence(
        &server,
        vec![
            responses::sse(vec![
                responses::ev_custom_tool_call(
                    "exec",
                    "exec",
                    "text(await tools.test_sync_tool({sleep_after_ms: 1}));",
                ),
                responses::ev_completed("first"),
            ]),
            finished_response(),
        ],
    )
    .await;
    let gate = Arc::new(Gate {
        target: "test_sync_tool",
        denied: AtomicBool::new(/*v*/ true),
        ..Default::default()
    });
    let test = test_codex()
        .with_extensions(gate.extensions(Admission::Registered))
        .with_model("test-gpt-5.1-codex")
        .with_config(|config| {
            config
                .features
                .enable(Feature::CodeMode)
                .expect("enable code mode");
        })
        .build_with_auto_env(&server)
        .await?;
    test.submit_text_turn("run the nested tool").await?;
    assert_eq!(
        *gate.starts.lock().expect("starts lock"),
        vec!["exec".to_string()]
    );
    let nested = gate
        .finishes
        .lock()
        .expect("finishes lock")
        .iter()
        .filter(|(name, _)| name == "test_sync_tool")
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        nested,
        vec![("test_sync_tool".to_string(), ToolCallOutcome::Blocked)]
    );
    assert!(
        requests.requests()[1]
            .custom_tool_call_output("exec")
            .to_string()
            .contains(DENIED)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn denied_mcp_call_does_not_reach_the_server() -> Result<()> {
    let server = responses::start_mock_server().await;
    let apps = AppsTestServer::mount(&server).await?;
    let requests = responses::mount_sse_sequence(
        &server,
        vec![
            responses::sse(vec![
                responses::ev_function_call_with_namespace(
                    "calendar",
                    SEARCH_CALENDAR_NAMESPACE,
                    SEARCH_CALENDAR_LIST_TOOL,
                    "{}",
                ),
                responses::ev_completed("first"),
            ]),
            finished_response(),
        ],
    )
    .await;
    let gate = Arc::new(Gate {
        target: SEARCH_CALENDAR_LIST_TOOL,
        denied: AtomicBool::new(/*v*/ true),
        ..Default::default()
    });
    let test = apps_enabled_builder(apps.chatgpt_base_url)
        .with_extensions(gate.extensions(Admission::Registered))
        .build_with_auto_env(&server)
        .await?;
    wait_for_mcp_server(&test.codex, CODEX_APPS_MCP_SERVER_NAME).await?;
    test.submit_text_turn("list calendar events").await?;
    assert!(gate.starts.lock().expect("starts lock").is_empty());
    assert_eq!(
        *gate.finishes.lock().expect("finishes lock"),
        vec![(
            SEARCH_CALENDAR_LIST_TOOL.to_string(),
            ToolCallOutcome::Blocked
        )]
    );
    assert!(
        requests.requests()[1]
            .function_call_output("calendar")
            .to_string()
            .contains(DENIED)
    );
    assert!(
        !server
            .received_requests()
            .await
            .expect("recorded requests")
            .iter()
            .any(|request| {
                serde_json::from_slice::<Value>(&request.body)
                    .ok()
                    .is_some_and(|body| body["method"] == "tools/call")
            })
    );
    Ok(())
}
