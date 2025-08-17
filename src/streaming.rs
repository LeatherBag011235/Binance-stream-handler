use futures_util::{StreamExt, SinkExt, Stream};
use serde::Deserialize;
use tokio_tungstenite::{connect_async};
use tokio_tungstenite::tungstenite::Message;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;


#[derive(Debug, Deserialize)]
pub struct DepthUpdate {
    pub e: String,    // Event type: "depthUpdate"
    pub E: u64,       // Event time 
    pub T: u64,       // Transaction time 
    pub s: String,    // Symbol
    pub U: u64,       // First update ID in event
    pub u: u64,       // Final update ID in event
    pub pu: u64,      // Final update Id in last stream(ie `u` in last stream)
    pub b: Vec<[String; 2]>,   // bids updates [price, qty]
    pub a: Vec<[String; 2]>,   // asks updates
}

#[derive(Debug, Deserialize)]
pub struct CombinedDepthUpdate {
    // e.g. "adausdt@depth@100ms"
    pub stream: String,
    pub data: DepthUpdate,
}


pub async fn streaming(
    url: String,
) -> Result<impl Stream<Item = CombinedDepthUpdate>, Box<dyn std::error::Error>> {
    
    println!("Connecting to {url} ...");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url).await?;
    println!("Connected. Waiting for messages...");

    let (tx, rx) = mpsc::channel::<CombinedDepthUpdate>(1024);

    tokio::spawn(async move {
        while let Some(msg_res) = ws.next().await {
            match msg_res {
                Ok(Message::Text(txt)) => {
                    if let Ok(env) = serde_json::from_str::<CombinedDepthUpdate>(&txt) {
                        let _ = tx.send(env).await;
                    }
                }
                Ok(Message::Ping(payload)) => {
                    let _ = ws.send(Message::Pong(payload)).await;
                }
                Ok(Message::Close(_)) => break,
                _ => (), 
            }
        }
    });

    Ok(ReceiverStream::new(rx))
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