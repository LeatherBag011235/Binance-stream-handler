use futures_util::{StreamExt, SinkExt, Stream};

use binance_stream_handler::{streaming, create_ws_url, OrderBook};


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let currency_pairs = vec!["adausdt", "dogeusdt"];
    let ws_url = create_ws_url(&currency_pairs);
    let mut stream = streaming(ws_url).await?;

    let mut orderbook = OrderBook::new(&currency_pairs[0]);
    let snapshot = orderbook.get_depth_snapshot(1000).await?;
    orderbook.from_snapshot(&snapshot);
    println!("{:?}", orderbook.last_u);

    let mut stream_checked = orderbook.filter_stream(stream).boxed();

    while let Some(item) = stream_checked.next().await {

        match item {
            Ok(du) => {
                println!("{}", du.s);
                orderbook.apply_update(&du);
            }
            Err(resync) => println!("resync needed for {}", resync.symbol),
        }
    }
    
    Ok(())
}