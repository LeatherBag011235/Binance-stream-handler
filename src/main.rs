use futures_util::{StreamExt, SinkExt, Stream};

use binance_stream_handler::{streaming, create_ws_url};


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let currency_pairs = vec!["adausdt", "dogeusdt"];
    let ws_url = create_ws_url(currency_pairs);
    let mut stream = streaming(ws_url).await?;

    while let Some(update) = stream.next().await {
        println!("stream: {}", update.stream);
        println!("symbol: {} U:{} u:{}", update.data.s, update.data.U, update.data.u);
    }
    Ok(())
    //get_depth_snapshot("BTCUSDT", 1000).await;
}