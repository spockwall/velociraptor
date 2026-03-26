use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::{
    connect_async, tungstenite::Message, tungstenite::handshake::client::Request,
};

const ENDPOINTS: &[&str] = &[
    "wss://ws.okx.com:8443/ws/v5/public",
    "wss://ws.okx.com/ws/v5/public",
    "wss://ws.okx.com:443/ws/v5/public",
    "wss://ws.okx.com:8443/ws/v5/public",
];

async fn test_endpoint(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n🔍 Testing endpoint: {}", url);

    // Try with custom headers
    let mut request = Request::builder()
        .uri(url)
        .header("User-Agent", "Mozilla/5.0 (compatible; OrderbookBot/1.0)")
        .body(())?;

    match tokio::time::timeout(Duration::from_secs(5), connect_async(request)).await {
        Ok(Ok((ws_stream, _))) => {
            println!("✅ Connected successfully!");

            let (mut write, mut read) = ws_stream.split();

            // Send ping
            println!("📤 Sending ping...");
            write.send(Message::Text("ping".into())).await?;

            // Wait for response
            match tokio::time::timeout(Duration::from_secs(3), read.next()).await {
                Ok(Some(Ok(msg))) => {
                    println!("✅ Received response: {:?}", msg);
                    return Ok(());
                }
                Ok(Some(Err(e))) => {
                    println!("❌ WebSocket error: {}", e);
                }
                Ok(None) => {
                    println!("❌ WebSocket closed");
                }
                Err(_) => {
                    println!("❌ Timeout waiting for response");
                }
            }
        }
        Ok(Err(e)) => {
            println!("❌ Connection failed: {}", e);
        }
        Err(_) => {
            println!("❌ Connection timeout");
        }
    }

    Err("Endpoint test failed".into())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Testing OKX WebSocket endpoints...");

    let mut working_endpoint = None;

    for endpoint in ENDPOINTS {
        if let Ok(()) = test_endpoint(endpoint).await {
            working_endpoint = Some(endpoint);
            break;
        }
    }

    match working_endpoint {
        Some(endpoint) => {
            println!("\n🎉 Found working endpoint: {}", endpoint);
            println!("Update your configuration to use this endpoint.");
        }
        None => {
            println!("\n💥 All endpoints failed. Possible issues:");
            println!("   - Network/firewall blocking connections");
            println!("   - OKX WebSocket service temporarily down");
            println!("   - Rate limiting from your IP");
            println!("   - SSL/TLS certificate issues");
        }
    }

    Ok(())
}
