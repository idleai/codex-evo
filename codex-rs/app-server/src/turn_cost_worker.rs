//! Turn-cost enrichment is intentionally disabled.
//!
//! The former worker queried OpenAI-owned analytics endpoints after turns completed. Local logs
//! and explicitly configured OTLP exporters continue to receive the token usage already available
//! in-process, but this build never performs a secondary telemetry request.

use std::sync::Arc;

use codex_core::config::Config;
use codex_login::AuthManager;
use codex_otel::SessionTelemetry;
use codex_protocol::ThreadId;
use codex_protocol::protocol::Event;

pub(crate) struct TurnCostWorker;

#[derive(Clone)]
pub(crate) struct TurnCostWorkerHandle;

impl TurnCostWorker {
    pub(crate) fn spawn(_config: Arc<Config>, _auth_manager: Arc<AuthManager>) -> Option<Self> {
        None
    }

    pub(crate) fn handle(&self) -> TurnCostWorkerHandle {
        TurnCostWorkerHandle
    }

    pub(crate) fn shutdown(&self) {}
}

impl TurnCostWorkerHandle {
    pub(crate) fn observe_event(
        &self,
        _thread_id: ThreadId,
        _thread_config: &Config,
        _event: &Event,
        _session_telemetry: impl FnOnce() -> SessionTelemetry,
    ) {
    }
}
