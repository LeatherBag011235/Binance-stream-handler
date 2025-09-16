use chrono::NaiveTime;
use tracing::info;
use tracing_subscriber::EnvFilter;

use binance_stream_handler::generate_orderbooks;

pub static CURRENCY_PAIRS: &[&str] = &["ADAUSDT", "DOGEUSDT"];

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    info!("App starting");

    let switch_cutoffs = (
        NaiveTime::from_hms_opt(2, 0, 0).unwrap(),   // 02:00
        NaiveTime::from_hms_opt(18, 42, 0).unwrap(), // 14:00
    );
    let chan_cap = 1024;
    let park_cap = 512;

    let ob_streams = generate_orderbooks(CURRENCY_PAIRS, chan_cap, park_cap, switch_cutoffs);

    let mut ada_rx = ob_streams.get("ADAUSDT").unwrap().clone();

    tokio::spawn(async move {
        loop {
            if ada_rx.changed().await.is_err() {
                break;
            }

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
