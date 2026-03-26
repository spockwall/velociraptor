use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::{
    connect_async, tungstenite::Message, tungstenite::handshake::client::Request,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 Testing OKX WebSocket with robust connection...");

    let url = "wss://ws.okx.com:8443/ws/v5/public";

    // Create request with proper headers
    let request = Request::builder()
        .uri(url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
        )
        .header("Accept", "*/*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Accept-Encoding", "gzip, deflate, br")
        .header("Cache-Control", "no-cache")
        .header("Pragma", "no-cache")
        .header("Sec-WebSocket-Protocol", "websocket")
        .body(())?;

    println!("📡 Attempting connection...");

    match tokio::time::timeout(Duration::from_secs(15), connect_async(request)).await {
        Ok(Ok((ws_stream, response))) => {
            println!("✅ WebSocket connection established!");
            println!("📋 Response status: {}", response.status());
            println!("📋 Response headers: {:?}", response.headers());

            let (mut write, mut read) = ws_stream.split();

            // Wait a moment before sending ping
            tokio::time::sleep(Duration::from_millis(100)).await;

            println!("📤 Sending ping...");
            write.send(Message::Text("ping".into())).await?;

            // Wait for response
            match tokio::time::timeout(Duration::from_secs(5), read.next()).await {
                Ok(Some(Ok(msg))) => {
                    println!("✅ Received response: {:?}", msg);

                    // Try to send a subscription message
                    println!("📤 Sending subscription...");
                    let sub_msg = r#"{"op": "subscribe", "args": [{"channel": "books", "instId": "BTC-USDT"}]}"#;
                    write.send(Message::Text(sub_msg.into())).await?;

                    // Wait for subscription response
                    match tokio::time::timeout(Duration::from_secs(5), read.next()).await {
                        Ok(Some(Ok(msg))) => {
                            println!("✅ Subscription response: {:?}", msg);
                        }
                        Ok(Some(Err(e))) => {
                            println!("❌ Subscription error: {}", e);
                        }
                        Ok(None) => {
                            println!("❌ WebSocket closed after subscription");
                        }
                        Err(_) => {
                            println!("❌ Timeout waiting for subscription response");
                        }
                    }
                }
                Ok(Some(Err(e))) => {
                    println!("❌ WebSocket error: {}", e);
                }
                Ok(None) => {
                    println!("❌ WebSocket closed");
                }
                Err(_) => {
                    println!("❌ Timeout waiting for ping response");
                }
            }
        }
        Ok(Err(e)) => {
            println!("❌ Connection failed: {}", e);
            println!("🔍 Error details: {:?}", e);
        }
        Err(_) => {
            println!("❌ Connection timeout after 15 seconds");
        }
    }

    Ok(())
}
