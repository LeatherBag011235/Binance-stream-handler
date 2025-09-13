use futures_util::{StreamExt, SinkExt, Stream, future};
use tokio::sync::mpsc;
use std::collections::HashMap;
use tracing_subscriber::{fmt, EnvFilter};
use tracing::{info, debug, error, warn, trace};
use chrono::NaiveTime;

use binance_stream_handler::{init_order_books, };
use binance_stream_handler::streaming::{TimedStream};
use binance_stream_handler::order_book::{OrderBook, UpdateDecision, DepthUpdate, CombinedDepthUpdate};
use binance_stream_handler::router::{DualRouter};

pub static CURRENCY_PAIRS: &[&str] = &["ADAUSDT", "DOGEUSDT"];


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    info!("App starting");

    let switch_cutoffs = (
        NaiveTime::from_hms_opt(2, 0, 0).unwrap(),   // 02:00
        NaiveTime::from_hms_opt(14, 0, 0).unwrap(),  // 14:00
    );
    let dual_router = DualRouter::new(switch_cutoffs, CURRENCY_PAIRS);
    let mut receivers = dual_router.start_dual_router(1024, 512);
    
    let ob_streams = init_order_books(CURRENCY_PAIRS, receivers);

    let mut ada_rx = ob_streams.get("ADAUSDT").unwrap().clone();

    tokio::spawn(async move {
        loop {
            if ada_rx.changed().await.is_err() {break;}

            let latest = ada_rx.borrow().clone();

            println!(
                "[{}] best bid={:?}, best ask={:?}, last_u={:?}",
                latest.symbol,
                latest.bids.keys().last(),
                latest.asks.keys().next(),
                latest.last_u,
            );

        }
    });

    futures_util::future::pending::<()>().await;
    Ok(())
}



