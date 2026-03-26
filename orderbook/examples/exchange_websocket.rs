use futures_util::{SinkExt, StreamExt};
use orderbook::types::endpoints::{binance, okx};
use serde_json::Value;
use std::{path::Path, time::Duration};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};

async fn test_websocket_connection(
    url: &str,
    subscribe_msg: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing websocket connection to: {url}");

    let (ws_stream, _) = connect_async(url).await?;
    let (mut write, mut read) = ws_stream.split();

    // Send subscription message if provided
    if let Some(msg) = subscribe_msg {
        write.send(Message::Text(msg.into())).await?;
        println!("Sent subscription message: {msg}");
    }

    // Read messages for 10 seconds
    let mut message_count = 0;
    let timeout_duration = Duration::from_secs(10);

    loop {
        match timeout(timeout_duration, read.next()).await {
            Ok(Some(Ok(message))) => {
                message_count += 1;
                match message {
                    Message::Text(text) => {
                        println!("Message #{message_count}: {text}");

                        // Try to parse as JSON to show structure
                        if let Ok(json_value) = serde_json::from_str::<Value>(&text) {
                            println!("JSON structure: {json_value:#}");
                            // store the message in a file
                            let file_name = format!("message_{message_count}.json");
                            let file_path = Path::new("./messages/");
                            let file_path = file_path.join(file_name);
                            if !file_path.exists() {
                                std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
                            }
                            // create the file if it doesn't exist
                            std::fs::write(file_path, json_value.to_string()).unwrap();
                        }

                        // Stop after 5 messages to avoid spam
                        if message_count >= 20 {
                            break;
                        }
                    }
                    Message::Binary(data) => {
                        println!("Binary message #{message_count}: {} bytes", data.len());
                    }
                    Message::Close(_) => {
                        println!("Connection closed by server");
                        break;
                    }
                    _ => {}
                }
            }
            Ok(Some(Err(e))) => {
                println!("Error receiving message: {e}");
                break;
            }
            Ok(None) => {
                println!("Connection closed");
                break;
            }
            Err(_) => {
                println!("Timeout after {} seconds", timeout_duration.as_secs());
                break;
            }
        }
    }

    Ok(())
}

async fn test_okx_websocket() {
    println!("\n=== Testing OKX WebSocket ===");

    let subscribe_msg =
        r#"{"op": "subscribe", "args": [{"channel": "books", "instId": "BTC-USDT"}]}"#;

    match test_websocket_connection(okx::ws::PUBLIC_STREAM, Some(subscribe_msg)).await {
        Ok(()) => println!("OKX test completed successfully"),
        Err(e) => println!("OKX test failed: {e}"),
    }
}

async fn test_binance_websocket() {
    println!("\n=== Testing Binance WebSocket ===");

    // Binance uses URL parameters for subscription
    let binance_ws_url = binance::ws::PUBLIC_STREAM;
    let binance_url = format!("{binance_ws_url}/btcusdt@depth20@100ms");

    match test_websocket_connection(&binance_url, None).await {
        Ok(()) => println!("Binance test completed successfully"),
        Err(e) => println!("Binance test failed: {e}"),
    }
}

#[tokio::main]
async fn main() {
    test_okx_websocket().await;
    test_binance_websocket().await;
}
