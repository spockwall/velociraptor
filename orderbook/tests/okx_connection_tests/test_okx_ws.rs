use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing OKX WebSocket connection...");

    let url = "wss://ws.okx.com:8443/ws/v5/public";

    println!("Connecting to: {}", url);

    match connect_async(url).await {
        Ok((ws_stream, _)) => {
            println!("✅ Successfully connected to OKX WebSocket!");

            let (mut write, mut read) = ws_stream.split();

            // Send a simple ping
            let ping_msg = "ping";
            println!("Sending ping: {}", ping_msg);
            write
                .send(Message::Text(ping_msg.to_string().into()))
                .await?;

            // Wait for response
            match tokio::time::timeout(std::time::Duration::from_secs(5), read.next()).await {
                Ok(Some(Ok(msg))) => {
                    println!("✅ Received response: {:?}", msg);
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
        Err(e) => {
            println!("❌ Failed to connect: {}", e);
        }
    }

    Ok(())
}
