//! Live checks against a real server. Opt-in: `cargo test -- --ignored`.
//! Override the endpoint with BACKROOM_URL.

use backroom_core::proto::Message;
use backroom_core::proto::caps::CapSet;
use backroom_core::transport::{Connector, WebSocketConnector};

/// WebSocket handshake + `CAP LS 302`, no authentication.
#[tokio::test]
#[ignore = "needs network access to the soju server"]
async fn websocket_cap_ls() {
    let url =
        std::env::var("BACKROOM_URL").unwrap_or_else(|_| "wss://irc.chillincool.net/socket".into());
    let mut transport = WebSocketConnector::new(&url)
        .unwrap()
        .connect()
        .await
        .unwrap();
    transport.send("CAP LS 302").await.unwrap();

    let mut caps = CapSet::default();
    loop {
        let line = transport
            .recv()
            .await
            .unwrap()
            .expect("server closed connection");
        let msg = Message::parse(&line).unwrap();
        if msg.command == "CAP" && msg.param(1) == Some("LS") {
            caps.extend_from_list(msg.params.last().unwrap());
            if msg.param(2) != Some("*") {
                break;
            }
        }
    }
    transport.close().await.ok();

    println!("caps: {:?}", caps.names().collect::<Vec<_>>());
    println!("sasl: {:?}", caps.value("sasl"));
    assert!(caps.contains("soju.im/bouncer-networks"));
    assert!(caps.sasl_mechanisms().contains(&"OAUTHBEARER"));
}
