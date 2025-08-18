use futures_util::{StreamExt, SinkExt, Stream};
use serde::Deserialize;
use tokio_tungstenite::{connect_async};
use tokio_tungstenite::tungstenite::Message;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use futures_util::future::ready;



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

pub fn create_ws_url(currency_pairs: &Vec<&str>,) -> String {
    let stream_spec = "@depth@100ms";
    let base_url = "wss://fstream.binance.com/stream?streams=";

    let mut url = String::from(base_url);
    for (i, pair) in currency_pairs.iter().enumerate() {
        let insert_str = format!("{}{}", pair, stream_spec);
        if i > 0 {
            url.push_str("/")
        }
        url.push_str(&insert_str);
    }

    url
}

//////////////////////////////////////////////////////////////////////////////////////


use reqwest::Client;
use std::collections::BTreeMap;
use ordered_float::OrderedFloat as OF;

type Price = OF<f64>;
type Qty   = f64;

#[derive(Debug, Clone)]
pub struct ResyncNeeded {
    pub symbol: String,
    pub expected_pu: Option<u64>, // what we expected (prev u)
    pub got_pu: u64,              // the pu we received
    pub got_u: u64,               // the u we received
}

#[derive(Debug, Deserialize)]
pub struct DepthSnapshot {
    lastUpdateId: u64,
    E: u64, // event time (ms)
    T: u64, // transaction time (ms)
    bids: Vec<[String; 2]>, // [price, qty]
    asks: Vec<[String; 2]>,
}


#[derive(Debug, Clone)]
pub struct OrderBook {
    pub symbol: String,
    // Sorted by price ascending (wrapped so it implements Ord)
    bids: BTreeMap<Price, Qty>,
    asks: BTreeMap<Price, Qty>,
    pub last_u: Option<u64>,
}

impl OrderBook {
    pub fn new(symbol: &str) -> Self {
        Self {
            symbol: symbol.to_ascii_uppercase(),
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            last_u: None,
        }
    }

    pub async fn get_depth_snapshot(
        &self,
        limit: u16,
    ) -> Result<DepthSnapshot, Box<dyn std::error::Error>> {
        let sym = self.symbol.to_ascii_uppercase();
        let url = format!(
            "https://fapi.binance.com/fapi/v1/depth?symbol={sym}&limit={limit}"
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

    /// Build a sorted book directly from a REST snapshot.
    pub fn from_snapshot(&mut self, snap: &DepthSnapshot) {
        self.bids.clear();
        self.asks.clear();

        self.last_u = Some(snap.lastUpdateId);

        for [p, q] in &snap.bids {
            let (p, q) = (Self::parse_f64(p), Self::parse_f64(q));
            if q != 0.0 {
                self.bids.insert(OF(p), q);
            }
        }
        for [p, q] in &snap.asks {
            let (p, q) = (Self::parse_f64(p), Self::parse_f64(q));
            if q != 0.0 {
                self.asks.insert(OF(p), q);
            }
        }
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
    
    pub fn filter_stream<'a, S>(
        &self,
        src: S,
    ) -> impl futures_util::Stream<Item = Result<DepthUpdate, ResyncNeeded>> + 'a
    where
        S: futures_util::Stream<Item = CombinedDepthUpdate> + 'a,
    {
        let sym_for_filter = self.symbol.clone();
        let sym_for_err = self.symbol.clone();

        // 1) keep only our symbol, unwrap to DepthUpdate
        src.filter_map(move |env| {
                let ok = env.data.s.eq_ignore_ascii_case(&sym_for_filter);
                async move { if ok { Some(env.data) } else { None } }
            })
            // 2) carry `prev_u` as stream state and validate `pu == prev_u`
            .scan(None, move |prev_u: &mut Option<u64>, du: DepthUpdate| {
                let symbol = sym_for_err.clone();
            
                // Decide the result synchronously, then return a ready future.
                let res = match *prev_u {
                    // If we already have a previous u, enforce continuity
                    Some(prev) if du.pu != prev => {
                        Some(Err(ResyncNeeded {
                            symbol,
                            expected_pu: Some(prev),
                            got_pu: du.pu,
                            got_u: du.u,
                        }))
                    }
                    // First message OR continuity is OK → pass it through and advance prev_u
                    _ => {
                        *prev_u = Some(du.u);
                        Some(Ok(du))
                    }
                };
            
                // Return a future that's immediately ready, avoiding any async/await here.
                ready(res)
            })

    }

    fn parse_f64(s: &str) -> f64 {
        // Binance sends clean numeric strings; unwrap is fine for now.
        s.parse::<f64>().unwrap()
    }

}