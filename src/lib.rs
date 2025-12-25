//! # binance-stream-handler
//!
//! Produce live Binance order books as Tokio `watch::Receiver<OrderBook>` streams.
//!
//! ## Quick start
//!
//! ```no_run
//! use binance_stream_handler::generate_orderbooks;
//! use chrono::NaiveTime;
//!
//! // Currency pairs must be defined as a 'static slice.
//! pub static CURRENCY_PAIRS: &[&str] = &["ADAUSDT", "DOGEUSDT"];
//!
//! #[tokio::main(flavor = "multi_thread")]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let cutoffs = (NaiveTime::from_hms_opt(2,0,0).unwrap(),
//!                    NaiveTime::from_hms_opt(18,42,0).unwrap());
//!
//!     let streams = generate_orderbooks(CURRENCY_PAIRS, 1024, 512, cutoffs);
//!
//!     let mut ada = streams["ADAUSDT"].clone();
//!     tokio::spawn(async move {
//!         while ada.changed().await.is_ok() {
//!             let ob = ada.borrow().clone();
//!             println!("{} best bid={:?} ask={:?}",
//!                      ob.symbol, ob.bids.keys().last(), ob.asks.keys().next());
//!         }
//!     });
//!
//!     futures_util::future::pending::<()>().await;
//!     Ok(())
//! }
//! ```

//! ## The `OrderBook` type
//!
//! Each stream yields an [`OrderBook`], which contains the current snapshot
//! of bids and asks for a symbol.
//!
//! ```text
//! struct OrderBook {
//!     symbol: String,                     // e.g. "ADAUSDT"
//!     bids: BTreeMap<Price, Qty>,         // sorted descending
//!     asks: BTreeMap<Price, Qty>,         // sorted ascending
//!     last_u: Option<u64>,                // last update ID applied
//!     snapshot_id: Option<u64>,           // REST snapshot ID
//!     depth: u16                          // snapshot depth (default 1000)
//! }
//! ```
//!
//! - **`bids`**: map from price → quantity, sorted by price descending  
//! - **`asks`**: map from price → quantity, sorted by price ascending  
//! - **`last_u`**: last WebSocket update sequence number applied  
//! - **`snapshot_id`**: ID of the REST snapshot used to initialize the book  
//! - **`depth`**: the configured maximum depth (default: 1000)  
//!
//! You normally just clone the latest `OrderBook` from a `watch::Receiver` and
//! inspect the maps to get the best bid/ask or traverse the book.

use chrono::NaiveTime;


use std::collections::HashMap;
use tokio::sync::{watch};

mod ob_manager;
mod router;

pub use crate::ob_manager::init_order_books;
pub use crate::ob_manager::order_book::OrderBook;
use crate::router::DualRouter;

pub async fn generate_orderbooks(
    currency_pairs: &'static [&'static str],
    chan_cap: usize,
    park_cap: usize,
    switch_cutoffs: (NaiveTime, NaiveTime),
) -> HashMap<String, watch::Receiver<OrderBook>> {
    let dual_router = DualRouter::new(switch_cutoffs, currency_pairs);
    let (receivers, connected) = dual_router.start_dual_router(chan_cap, park_cap);

    connected.notified().await;
    let ob_streams = init_order_books(currency_pairs, receivers);

    ob_streams
}
