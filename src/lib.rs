use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::protocol::Message;
use reqwest::Client;
use serde::Deserialize;


pub async fn streaming(url: String) -> Result<(), Box<dyn std::error::Error>> {
    
    println!("Connecting to {url} ...");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url).await?;
    println!("Connected. Waiting for messages...");

    while let Some(msg_result) = ws.next().await {
        // If the stream yields an error, bail out
        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                eprintln!("WebSocket error: {e}");
                break;
            }
        };
            
        match msg {
        Message::Text(txt) => {
                println!("{txt}");
            }
            Message::Binary(bin) => {
                println!("(binary) {} bytes", bin.len());
            }
            Message::Ping(payload) => {
                // Reply to keepalive
                ws.send(Message::Pong(payload)).await?;
            }
            Message::Close(frame) => {
                println!("Server closed: {:?}", frame);
                break;
            }
            _ => {()}
        }
    }

    Ok(())
}

pub fn create_ws_url(currency_pairs: Vec<&str>,) -> String {
    let stream_spec = "@depth@100ms";
    let base_url = "wss://fstream.binance.com/stream?streams=";

    let mut url = String::from(base_url);
    for (i, pair) in currency_pairs.iter().enumerate() {
        let insert_str = format!("{}{}", pair, stream_spec);
        if i > 0 {
            url.push_str("/")
        }
        println!("{}", i);
        url.push_str(&insert_str);
    }

    url
}

pub async fn get_depth_snapshot(
    symbol: &str,
    limit: u16,
) -> Result<DepthSnapshot, Box<dyn std::error::Error>> {
    // Binance expects uppercase symbols.
    let sym = symbol.to_ascii_uppercase();
    let url = format!(
        "https://fapi.binance.com/fapi/v1/depth?symbol={}&limit={}",
        sym, limit
    );

    let client = Client::builder()
        .user_agent("binance-stream-handler/0.1")
        .build()?;

    let resp = client.get(&url).send().await?;
   
    if !resp.status().is_success() {
        return Err(format!("Snapshot HTTP error: {}", resp.status()).into());
    }

    let snapshot: DepthSnapshot = resp.json().await?;


    Ok(snapshot)
}

#[derive(Debug, Deserialize)]
pub struct DepthSnapshot {
    lastUpdateId: u64,
    E: u64, // Event time (ms since epoch)
    T: u64, // Transaction time (ms since epoch)
    bids: Vec<[String; 2]>, // [price, quantity] as strings
    asks: Vec<[String; 2]>,
}

    