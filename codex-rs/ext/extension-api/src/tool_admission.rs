use crate::ExtensionData;
use crate::ExtensionFuture;
use crate::ToolCallSource;
use crate::ToolName;
use crate::ToolPayload;

/// Finalized invocation offered to an extension before the tool handler runs.
pub struct ToolAdmissionInput<'a> {
    /// Store scoped to the host session runtime.
    pub session_store: &'a ExtensionData,
    /// Store scoped to this thread runtime.
    pub thread_store: &'a ExtensionData,
    /// Store scoped to this turn runtime.
    pub turn_store: &'a ExtensionData,
    /// Current turn submission id.
    pub turn_id: &'a str,
    /// Model-visible tool call id.
    pub call_id: &'a str,
    /// Tool name as routed by the host.
    pub tool_name: &'a ToolName,
    /// Arguments after pre-tool hooks. Sensitive plaintext must not be logged.
    pub payload: &'a ToolPayload,
    /// Source that issued the call, including nested Code Mode invocations.
    pub source: ToolCallSource,
}

/// Adds a live restriction after pre-tool hooks and before tool-handler execution.
///
/// Contributors run in registration order; the first error blocks the handler
/// and reports a `ToolCallOutcome::Blocked` result. Return a concise, model-safe
/// reason. `Ok(())` leaves subsequent native approval and sandbox checks intact.
/// Hosts without contributors retain their normal dispatch behavior.
///
/// Checks should be bounded and free of side effects. The host may drop the
/// future on cancellation. Admission does not reserve authorization across later
/// approval waits or the lifetime of a running process.
pub trait ToolAdmissionContributor: Send + Sync {
    fn admit<'a>(
        &'a self,
        input: ToolAdmissionInput<'a>,
    ) -> ExtensionFuture<'a, Result<(), String>>;
}
