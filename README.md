# binance-stream-handler

Async order book streams for Binance futures.  
This crate spawns background tasks that keep [`OrderBook`]s updated via REST snapshots and WebSocket deltas, and exposes them as Tokio `watch::Receiver`s.

## Add this crate, plus Tokio (needed to run the async runtime):
```sh
cargo add binance-stream-handler
cargo add tokio --features full
