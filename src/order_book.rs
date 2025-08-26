use reqwest::Client;
use std::collections::BTreeMap;
use ordered_float::OrderedFloat as OF;
use serde::Deserialize;

type Price = OF<f64>;
type Qty   = f64;

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

#[derive(Debug, Clone)]
pub struct ResyncNeeded {
    pub symbol: String,
    pub expected_pu: Option<u64>, // what we expected (prev u)
    pub got_pu: u64,              // the pu we received
    pub got_u: u64,               // the u we received
}

#[derive(Debug, Deserialize)]
pub struct DepthSnapshot {
    #[serde(rename = "lastUpdateId")]
    last_update_id: u64,
    E: u64, // event time (ms)
    T: u64, // transaction time (ms)
    bids: Vec<[String; 2]>, // [price, qty]
    asks: Vec<[String; 2]>,
}

#[derive(Debug)]
pub enum UpdateDecision<'a>{
    Drop,                       // ignore this event
    Apply(&'a DepthUpdate),         // apply to book
    Resync(ResyncNeeded),       // trigger re-snapshot
}


#[derive(Debug, Clone)]
pub struct OrderBook {
    pub symbol: String,
    // Sorted by price ascending (wrapped so it implements Ord)
    bids: BTreeMap<Price, Qty>,
    asks: BTreeMap<Price, Qty>,
    pub last_u: Option<u64>,
    snapshot_id: Option<u64>,
    depth: u16
}

impl OrderBook {
    pub fn new(symbol: &str) -> Self {
        Self {
            symbol: symbol.to_ascii_uppercase(),
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            last_u: None,
            snapshot_id: None,
            depth: 1000,
        }
    }

    pub async fn init_ob(
        symbol: &str
    ) -> Result<OrderBook, Box<dyn std::error::Error>> {
        let mut ob = OrderBook::new(symbol);
        let snapshot = ob.get_depth_snapshot(ob.depth).await?;
        ob.from_snapshot(&snapshot);
        Ok(ob)
    }

    pub async fn resync_ob(
        &mut self
    ) -> Result<(), Box<dyn std::error::Error>>{
        let new_snapshot = self.get_depth_snapshot(self.depth).await?;
        self.from_snapshot(&new_snapshot);
        Ok(())
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

        self.snapshot_id = Some(snap.last_update_id);

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

    pub fn continuity_check<'a>(
        &mut self, 
        du: &'a DepthUpdate
    ) -> UpdateDecision<'a> {

        let snapshot_id = match self.snapshot_id {
            None => {
                self.last_u = None;
                return UpdateDecision::Resync(ResyncNeeded {
                    symbol: self.symbol.clone(), 
                    expected_pu: None, 
                    got_pu: du.pu, 
                    got_u: du.u,
                });
                
            }
            Some(s) => s,            
        };

        match self.last_u {
            None => {
                if du.U <= snapshot_id && snapshot_id <= du.u {
                    self.last_u = Some(du.u);
                    return UpdateDecision::Apply(&du);

                } else if snapshot_id <= du.U  {
                    self.last_u = None;
                    return UpdateDecision::Resync(ResyncNeeded {
                        symbol: self.symbol.clone(),
                        expected_pu: None,
                        got_pu: du.pu,
                        got_u: du.u,
                    })
                } else {
                    return UpdateDecision::Drop;
                }
            }

            Some(pu) => {
                if pu == du.pu {
                    self.last_u = Some(du.u);
                    return UpdateDecision::Apply(&du);
                } else {
                    self.last_u = None;
                    return UpdateDecision::Resync(ResyncNeeded {
                        symbol: self.symbol.clone(),
                        expected_pu: Some(pu),
                        got_pu: du.pu,
                        got_u: du.u,
                    })
                }
            }
        }
    }

    fn parse_f64(s: &str) -> f64 {
        // Binance sends clean numeric strings; unwrap is fine for now.
        s.parse::<f64>().unwrap()
    }

}