use binance_stream_handler::{streaming, create_ws_url};

fn main() {
    let currency_pairs = vec!["adausdt", "dogeusdt"];
    let ws_url = create_ws_url(currency_pairs);
    streaming(ws_url);
}