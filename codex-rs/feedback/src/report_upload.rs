use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use codex_http_client::HttpClientFactory;
use codex_protocol::protocol::SessionSource;
use sentry::protocol::Attachment;
use sentry::protocol::Envelope;
use sentry::protocol::EnvelopeHeaders;
use sentry::protocol::EnvelopeItem;
use sentry::types::Uuid;

use crate::FeedbackAttachment;
use crate::FeedbackSnapshot;
use crate::MAX_DECODED_UPLOAD_BYTES;
use crate::MAX_EVENT_BYTES;
use crate::MAX_UPLOAD_BYTES;
use crate::upload;

/// The upstream HTTP result, not proof of durable storage in Sentry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedbackDelivery {
    Accepted {
        retry_after: Option<Duration>,
    },
    Rejected {
        status: u16,
        retry_after: Option<Duration>,
    },
    /// No response was received. The upstream may still have accepted the envelope.
    Unconfirmed,
}

/// Sends persisted report parts without retries or request diagnostics.
pub struct FeedbackTransport;

impl FeedbackTransport {
    pub fn new(_http_client_factory: HttpClientFactory) -> Result<Self> {
        Err(anyhow!(
            "remote OpenAI feedback delivery is disabled in this build"
        ))
    }

    /// This cannot be reached through [`Self::new`]; retained for API compatibility.
    pub async fn send(&self, _envelope: Vec<u8>) -> FeedbackDelivery {
        FeedbackDelivery::Unconfirmed
    }
}

impl FeedbackSnapshot {
    /// Prepare only the core report, using the caller's stable UUID as its Sentry event ID.
    pub fn prepare_report_event(
        &self,
        report_id: &str,
        classification: &str,
        reason: Option<&str>,
        tags: Option<&BTreeMap<String, String>>,
        session_source: Option<&SessionSource>,
    ) -> Result<Vec<u8>> {
        let mut event = self.feedback_event(classification, reason, tags, session_source);
        // Auth tags must come from this report's caller, not metadata retained
        // from an earlier account in the process-wide tracing layer.
        event.tags.retain(|key, _| {
            !matches!(key.as_str(), "account_id" | "chatgpt_user_id")
                || tags.is_some_and(|tags| tags.contains_key(key))
        });
        event.event_id =
            Uuid::parse_str(report_id).map_err(|_| anyhow!("invalid feedback report ID"))?;
        let mut envelope = Envelope::new();
        envelope.add_item(EnvelopeItem::Event(event));
        prepare_envelope(envelope, MAX_EVENT_BYTES)
    }
}

/// Prepare one whole attachment linked to an existing report, without replaying its event.
/// The caller must apply consent and the report's cumulative attachment budget first.
pub fn prepare_report_attachment(
    report_id: &str,
    attachment: FeedbackAttachment,
) -> Result<Vec<u8>> {
    let event_id = Uuid::parse_str(report_id).map_err(|_| anyhow!("invalid feedback report ID"))?;
    let mut envelope = Envelope::new().with_headers(EnvelopeHeaders::new().with_event_id(event_id));
    envelope.add_item(EnvelopeItem::Attachment(Attachment {
        buffer: attachment.buffer,
        filename: attachment.filename,
        content_type: attachment.content_type,
        ty: None,
    }));
    prepare_envelope(envelope, MAX_DECODED_UPLOAD_BYTES)
}

fn prepare_envelope(envelope: Envelope, max_decoded_bytes: usize) -> Result<Vec<u8>> {
    let (bytes, decoded_bytes) =
        upload::gzip_envelope(&envelope).context("failed to serialize feedback envelope")?;
    anyhow::ensure!(
        bytes.len() <= MAX_UPLOAD_BYTES && decoded_bytes <= max_decoded_bytes,
        "feedback envelope exceeds the size limit"
    );
    Ok(bytes)
}
