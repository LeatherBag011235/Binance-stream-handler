use reqwest::Client;
use serde::Deserialize;
use std::collections::BTreeMap;
use ordered_float::OrderedFloat as OF;

type Price = OF<f64>;
type Qty   = f64;

#[derive(Debug, Deserialize)]
struct DepthSnapshot {
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
        symbol: &str,
        limit: u16,
    ) -> Result<DepthSnapshot, Box<dyn std::error::Error>> {
        let sym = symbol.to_ascii_uppercase();
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
    pub fn from_snapshot(mut self, snap: &DepthSnapshot) -> Self {
        self.bids.clear()
        self.asks.clear()

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
    pub fn is_mine(&self, resived: CombinedDepthUpdate) -> DepthUpdate {
        if resived.data.s.eq_ignore_ascii_case(&self.symbol):
            
    }

    fn parse_f64(s: &str) -> f64 {
        // Binance sends clean numeric strings; unwrap is fine for now.
        s.parse::<f64>().unwrap()
    }

}