use futures_util::{StreamExt, SinkExt, Stream, future};
use tokio::sync::mpsc;
use std::collections::HashMap;

use binance_stream_handler::{init_order_books, init_stream};
use binance_stream_handler::streaming::{streaming, create_ws_url, start_router};
use binance_stream_handler::order_book::{OrderBook, UpdateDecision, DepthUpdate, CombinedDepthUpdate};

pub static CURRENCY_PAIRS: &[&str] = &["ADAUSDT", "DOGEUSDT"];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {

    let mut stream_1 = init_stream(CURRENCY_PAIRS).await?;

    let mut stream_2 = init_stream(CURRENCY_PAIRS).await?;

    loop {
        let (a_opt, b_opt) = future::join(stream_1.next(), stream_2.next()).await;

        match (a_opt, b_opt) {
            (Some(a), Some(b)) => {
                if a != b {
                    println!("Missmatch:\n  s1: 
                    Symbole {:?}; U: {:?} \n s2: {:?}", a.data, b);
                } else {
                    println!("True");
                } 
            }
            _ => println!("Ether stream is ended"),
        }
    }

    //let (_router, receivers) = start_router(stream, CURRENCY_PAIRS, 1024);

    //init_order_books(CURRENCY_PAIRS, receivers);

    futures_util::future::pending::<()>().await;
    Ok(())
}



