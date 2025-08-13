use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::protocol::Message;
use reqwest::Client;
use serde::Deserialize;
use std::collections::BTreeMap;
use ordered_float::OrderedFloat as OF;


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
    #[serde(rename = "lastUpdateId")]
    pub last_update_id: u64,
    pub E: u64,
    pub T: u64,
    pub bids: Vec<[String; 2]>,
    pub asks: Vec<[String; 2]>,
}

#[derive(Debug, Deserialize)]
pub struct DepthUpdate {
    pub e: String,    // "depthUpdate"
    pub E: u64,
    pub T: u64,
    pub s: String,
    pub U: u64,
    pub u: u64,
    pub pu: u64,
    pub b: Vec<[String; 2]>,   // bids updates [price, qty]
    pub a: Vec<[String; 2]>,   // asks updates
}

pub type Price = OF<f64>;
pub type Qty   = f64;

#[derive(Debug, Clone)]
pub struct OrderBook {
    // Sorted by price ascending (wrapped so it implements Ord)
    bids: BTreeMap<Price, Qty>,
    asks: BTreeMap<Price, Qty>,
    pub last_u: Option<u64>,
}

impl OrderBook {
    pub fn new() -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            last_u: None,
        }
    }

    /// Build a sorted book directly from a REST snapshot.
    pub fn from_snapshot(snap: &DepthSnapshot) -> Self {
        let mut ob = Self::new();

        for [p, q] in &snap.bids {
            let (p, q) = (Self::parse_f64(p), Self::parse_f64(q));
            if q != 0.0 {
                ob.bids.insert(OF(p), q);
            }
        }
        for [p, q] in &snap.asks {
            let (p, q) = (Self::parse_f64(p), Self::parse_f64(q));
            if q != 0.0 {
                ob.asks.insert(OF(p), q);
            }
        }
        ob
    }

    /// Apply one WS depth update (absolute quantities).
    /// Assumes you've enforced U/u/pu sequencing outside.
    pub fn apply_update(&mut self, ev: &DepthUpdate) {
        // bids
        for [p, q] in &ev.b {
            let (p, q) = (Self::parse_f64(p), Self::parse_f64(q));
            if q == 0.0 {
                self.bids.remove(&OF(p));
            } else {
                self.bids.insert(OF(p), q);
            }
        }
        // asks
        for [p, q] in &ev.a {
            let (p, q) = (Self::parse_f64(p), Self::parse_f64(q));
            if q == 0.0 {
                self.asks.remove(&OF(p));
            } else {
                self.asks.insert(OF(p), q);
            }
        }
        self.last_u = Some(ev.u);
    }

    fn parse_f64(s: &str) -> f64 {
        // Binance sends clean numeric strings; unwrap is fine for now.
        s.parse::<f64>().unwrap()
    }

}