use futures_util::{StreamExt, SinkExt, Stream, pin_mut};
use serde::Deserialize;
use tokio_tungstenite::{connect_async};
use tokio_tungstenite::tungstenite::Message;
use tokio_stream::wrappers::ReceiverStream;
use futures_util::future::ready;
use std::collections::HashMap;
use tokio::sync::{mpsc, watch};
use tracing::{info, debug, error, warn, trace};
use tracing::Instrument;
use tracing::{info_span}; 
use chrono::NaiveTime;

pub mod ob_manager;
pub mod router;

use crate::ob_manager::init_order_books;
use crate::ob_manager::order_book::{OrderBook};
use crate::router::{DualRouter};

pub fn generate_orderbooks(
    currency_pairs: &'static [&'static str],
    chan_cap: usize,
    park_cap: usize,
    switch_cutoffs: (NaiveTime, NaiveTime),
) -> HashMap<String, watch::Receiver<OrderBook>> {

    let dual_router = DualRouter::new(switch_cutoffs, currency_pairs);
    let receivers = dual_router.start_dual_router(chan_cap, park_cap);

    let ob_streams = init_order_books(currency_pairs, receivers);

    ob_streams
}