use futures_util::{StreamExt, SinkExt, Stream};

use binance_stream_handler::streaming::{streaming, create_ws_url, start_router};
use binance_stream_handler::order_book::{OrderBook, UpdateDecision};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let currency_pairs = vec!["adausdt", "dogeusdt"];
    let ws_url = create_ws_url(&currency_pairs);
    let stream = streaming(ws_url).await?;
    let (router, mut receivers) = start_router(stream, &currency_pairs, 1024);

    for pair in currency_pairs {

        let mut rx = receivers
        .remove(&pair.to_ascii_uppercase())
        .expect("router created a channel for every symbol");

        tokio::spawn(async move {
            let mut ob =  match OrderBook::init_ob(&pair).await {
                Ok(ob) => ob,
                Err(e) => {
                    eprintln!("[{pair}] snapshot init error: {e}");
                    return;
                }
            };

            while let Some(du) = rx.recv().await {
                match ob.continuity_check(&du) {
                    UpdateDecision::Drop => (),
                    UpdateDecision::Apply(du) => ob.apply_update(du),
                    UpdateDecision::Resync(info) => {
                        eprintln!(
                            "[{pair}] RESYNC: expected pu={:?}, got pu={}, u={}",
                            info.expected_pu, info.got_pu, info.got_u
                        );
                        if let Err(e) = ob.resync_ob().await {
                            eprintln!("[{}] resnapshot error: {e}", ob.symbol);
                        }
                    }
                }
            }
        });
    }

    futures_util::future::pending::<()>().await;
    Ok(())
}