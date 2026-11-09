//! Split readers keep their revocation wakeup and report denial even when writers fail first.

use crate::*;
use codex_http_client::DestinationPolicy;
use codex_http_client::NetworkPolicyController;
use codex_http_client::OutboundProxyPolicy::ReqwestDefault;
use futures::SinkExt;
use futures::StreamExt;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[tokio::test]
async fn copilot_distribution_blocks_openai_websocket_before_proxy_or_socket_work() {
    let factory = HttpClientFactory::new(ReqwestDefault);
    let connector = WebSocketConnector::new(&factory).unwrap();
    let request = "wss://chatgpt.com/backend-api/codex/responses"
        .into_client_request()
        .unwrap();
    let result = connector
        .connect_with_route(
            request,
            WebSocketConfig::default(),
            async { panic!("OpenAI WebSocket must be denied before resolving its proxy") },
            false,
        )
        .await;
    let Err(error) = result else {
        panic!("OpenAI WebSocket unexpectedly connected")
    };
    assert_eq!(
        network_policy_denial(&error),
        Some(codex_http_client::NetworkPolicyDenied::Destination)
    );
}

struct WakeFlag(AtomicBool);

impl futures::task::ArcWake for WakeFlag {
    fn wake_by_ref(waker: &Arc<Self>) {
        waker.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn revocation_wakes_split_reader_and_survives_writer_observing_it_first() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = websocket.next().await;
    });
    let controller = NetworkPolicyController::default();
    let policy = controller.policy();
    assert!(controller.publish(policy.revision(), DestinationPolicy::Unrestricted));
    let factory = HttpClientFactory::new(ReqwestDefault).with_network_policy(policy.clone());
    let connector = WebSocketConnector::new(&factory).unwrap();
    let request = url.into_client_request().unwrap();
    let (connection, _) = connector
        .connect(request, WebSocketConfig::default())
        .await
        .unwrap();
    let (mut writer, mut reader) = connection.split();
    let wake = Arc::new(WakeFlag(AtomicBool::new(false)));
    let waker = futures::task::waker(Arc::clone(&wake));
    let mut context = Context::from_waker(&waker);
    assert!(Pin::new(&mut reader).poll_next(&mut context).is_pending());
    writer.flush().await.unwrap();
    policy.invalidate();
    assert!(wake.0.load(Ordering::SeqCst));
    let error = writer.flush().await.unwrap_err();
    assert!(network_policy_denial(&error).is_some());
    let error = reader.next().await.unwrap().unwrap_err();
    assert!(network_policy_denial(&error).is_some());
    assert!(reader.next().await.is_none());
    server.abort();
}
