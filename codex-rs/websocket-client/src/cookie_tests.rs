//! Retired ChatGPT routing-cookie traffic cannot open a connection in this distribution.

use codex_http_client::HttpClientFactory;
use codex_http_client::NetworkPolicyDenied;
use codex_http_client::OutboundProxyPolicy;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::COOKIE;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::WebSocketConnector;
use crate::network_policy_denial;

#[tokio::test]
async fn copilot_distribution_blocks_chatgpt_routing_cookies_before_connecting() {
    let factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault);
    let connector = WebSocketConnector::new(&factory).unwrap();
    for url in [
        "wss://websocket-cookie-test.chatgpt.com/backend-api/codex/responses",
        "wss://websocket-cookie-test.chatgpt.com/backend-api/codex/guardian",
        "ws://websocket-cookie-test.chatgpt.com/backend-api/codex/responses",
        "wss://api.openai.com/v1/responses",
    ] {
        let mut request = url.into_client_request().unwrap();
        request.headers_mut().insert(
            COOKIE,
            HeaderValue::from_static("__oailb=west; __cflb=west; explicit=keep"),
        );
        let result = connector
            .connect_with_route(
                request,
                WebSocketConfig::default(),
                async { panic!("blocked cookie traffic must not resolve a route") },
                false,
            )
            .await;
        let Err(error) = result else {
            panic!("retired ChatGPT cookie traffic unexpectedly connected")
        };
        assert_eq!(
            network_policy_denial(&error),
            Some(NetworkPolicyDenied::Destination)
        );
    }
}
