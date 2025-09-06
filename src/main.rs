use futures_util::{StreamExt, SinkExt, Stream, future};
use tokio::sync::mpsc;
use std::collections::HashMap;
use tracing_subscriber::{fmt, EnvFilter};
use tracing::{info, debug, error, warn, trace};
use chrono::NaiveTime;

use binance_stream_handler::{init_order_books, };
use binance_stream_handler::streaming::{TimedStream};
use binance_stream_handler::order_book::{OrderBook, UpdateDecision, DepthUpdate, CombinedDepthUpdate};
use binance_stream_handler::router::{start_dual_router};

pub static CURRENCY_PAIRS: &[&str] = &["ADAUSDT", "DOGEUSDT"];


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    info!("App starting");

    let mut cs = TimedStream {
        currency_pairs: CURRENCY_PAIRS,
        life_span: (
            NaiveTime::from_hms_opt(2, 0, 0).unwrap(),   // 02:00
            NaiveTime::from_hms_opt(14, 0, 0).unwrap(),  // 14:00
        )
    };

    let mut stream_1 = cs.init_stream().await?;
    let mut stream_2 = cs.init_stream().await?;

    let (router, mut receivers) = start_dual_router(stream_1, stream_2, CURRENCY_PAIRS, 1024, 512);
    let ob_streams = init_order_books(CURRENCY_PAIRS, receivers);

    tokio::spawn({
        //let router = router.clone();
        async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            router.begin_cutover(std::time::Duration::from_millis(200));
        }
    });

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



