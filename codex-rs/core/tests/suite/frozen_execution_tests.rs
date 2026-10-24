use anyhow::Result;
use codex_core::TurnInputRequest;
use codex_core::config::Constrained;
use codex_extension_api::ToolPolicy;
use codex_features::Feature;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::skip_if_wine_exec;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::json;
use test_case::test_case;

#[derive(Clone, Copy)]
enum Execution {
    Frozen,
    Ordinary,
}

#[derive(Clone, Copy)]
enum Approval {
    Prompt,
    AllowRule,
    ExplicitEscalation,
    Never,
}

#[test_case(Execution::Frozen, Approval::Prompt; "approval_keeps_sandbox")]
#[test_case(Execution::Ordinary, Approval::ExplicitEscalation; "ordinary_explicit_approval")]
#[test_case(Execution::Frozen, Approval::AllowRule; "allow_rule_keeps_sandbox")]
#[test_case(Execution::Ordinary, Approval::AllowRule; "ordinary_allow_rule")]
#[test_case(Execution::Frozen, Approval::ExplicitEscalation; "explicit_escalation_rejected")]
#[test_case(Execution::Frozen, Approval::Never; "denial_without_approval")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execution_permission_ceiling(execution: Execution, approval: Approval) -> Result<()> {
    skip_if_wine_exec!(
        Ok(()),
        "Wine does not emulate Windows restricted-token and ACL sandbox semantics"
    );

    let server = responses::start_mock_server().await;
    let mut builder = test_codex()
        .with_model("gpt-5.2")
        .with_config(move |config| {
            config
                .features
                .enable(Feature::UnifiedExec)
                .expect("enable unified execution");
            config
                .features
                .disable(Feature::CodeMode)
                .expect("disable code mode");
            config
                .features
                .disable(Feature::ShellZshFork)
                .expect("disable shell bridge");
            config.permissions.approval_policy = Constrained::allow_any(match approval {
                Approval::Prompt | Approval::AllowRule => AskForApproval::UnlessTrusted,
                Approval::ExplicitEscalation => AskForApproval::OnRequest,
                Approval::Never => AskForApproval::Never,
            });
            config.approvals_reviewer = ApprovalsReviewer::User;
            config.set_windows_sandbox_enabled(/*value*/ true);
            config
                .permissions
                .set_permission_profile(PermissionProfile::read_only())
                .expect("read-only permission profile");
        });
    if matches!(approval, Approval::AllowRule) {
        builder = builder.with_pre_build_hook(|home| {
            std::fs::create_dir_all(home.join("rules")).expect("create rules directory");
            std::fs::write(
                home.join("rules/default.rules"),
                "prefix_rule(pattern=[\"mkdir\"], decision=\"allow\")\n",
            )
            .expect("write command rule");
        });
    }
    let fixture = builder.build_with_auto_env(&server).await?;
    let mut options = fixture.start_thread_options().await;
    if matches!(execution, Execution::Frozen) {
        options
            .thread_extension_init
            .insert(codex_extension_api::SessionIsolation::Isolated);
    }
    options.thread_extension_init.insert(ToolPolicy {
        freeze_execution_permissions: matches!(execution, Execution::Frozen),
        ..Default::default()
    });
    let agent = fixture.thread_manager.start_thread(options).await?;
    let mut args = json!({"cmd":"mkdir runner-output","yield_time_ms":1000});
    if matches!(approval, Approval::ExplicitEscalation) {
        args["sandbox_permissions"] = json!("require_escalated");
        args["justification"] = json!("test the fixed permission ceiling");
    }
    let requests = responses::mount_sse_sequence(
        &server,
        vec![
            responses::sse(vec![
                responses::ev_function_call("write", "exec_command", &args.to_string()),
                responses::ev_completed("command"),
            ]),
            responses::sse(vec![responses::ev_completed("done")]),
        ],
    )
    .await;
    agent
        .thread
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "Try to create the directory".into(),
            text_elements: Vec::new(),
        }]))
        .await?;
    let mut approvals = 0;
    let mut command_finished = false;
    loop {
        match wait_for_event(&agent.thread, |event| {
            matches!(
                event,
                EventMsg::ExecApprovalRequest(_)
                    | EventMsg::ExecCommandEnd(_)
                    | EventMsg::TurnComplete(_)
            )
        })
        .await
        {
            EventMsg::ExecApprovalRequest(request) => {
                approvals += 1;
                assert!(approvals <= 2, "unexpected approval loop");
                agent
                    .thread
                    .submit(Op::ExecApproval {
                        id: request.effective_approval_id(),
                        turn_id: None,
                        decision: ReviewDecision::Approved,
                    })
                    .await?;
            }
            EventMsg::ExecCommandEnd(event) => {
                command_finished |= event.call_id == "write";
            }
            EventMsg::TurnComplete(_) => break,
            _ => unreachable!("event filter"),
        }
    }
    assert_eq!(
        approvals,
        usize::from(
            matches!(approval, Approval::Prompt)
                || matches!(
                    (execution, approval),
                    (Execution::Ordinary, Approval::ExplicitEscalation)
                )
        )
    );
    let output = requests.requests()[1]
        .function_call_output("write")
        .to_string();
    // A yielded process can finish after the model's turn; inspect its final filesystem state.
    if output.contains("Process running with session ID ") && !command_finished {
        wait_for_event(
            &agent.thread,
            |event| matches!(event, EventMsg::ExecCommandEnd(event) if event.call_id == "write"),
        )
        .await;
    }
    let written = fixture
        .fs()
        .get_metadata(
            &fixture.workspace_path_uri("runner-output")?,
            Default::default(),
            /*sandbox*/ None,
        )
        .await;
    assert_eq!(
        written.is_ok(),
        matches!(execution, Execution::Ordinary),
        "{output}"
    );
    if matches!(
        (execution, approval),
        (Execution::Frozen, Approval::ExplicitEscalation)
    ) {
        assert!(output.contains("cannot be escalated"), "{output}");
    }
    agent.thread.shutdown_and_wait().await?;
    fixture.codex.shutdown_and_wait().await?;
    Ok(())
}
