use codex_extension_api::ToolAdmissionInput;

use crate::tools::context::ToolInvocation;
use crate::tools::lifecycle::extension_tool_call_source;

pub(crate) async fn admit_tool_call(invocation: &ToolInvocation) -> Result<(), String> {
    for contributor in invocation
        .session
        .services
        .extensions
        .tool_admission_contributors()
    {
        contributor
            .admit(ToolAdmissionInput {
                session_store: &invocation.session.services.session_extension_data,
                thread_store: &invocation.session.services.thread_extension_data,
                turn_store: invocation.turn.extension_data.as_ref(),
                turn_id: &invocation.turn.sub_id,
                call_id: &invocation.call_id,
                tool_name: &invocation.tool_name,
                payload: &invocation.payload,
                source: extension_tool_call_source(invocation.source.clone()),
            })
            .await?;
    }
    Ok(())
}
