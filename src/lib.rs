use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::protocol::Message;

#[tokio::main]
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
    