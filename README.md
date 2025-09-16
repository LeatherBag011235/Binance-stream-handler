# binance-stream-handler

Async order book streams for Binance futures.  
This crate spawns background tasks that keep [`OrderBook`]s updated via REST snapshots and WebSocket deltas, and exposes them as Tokio `watch::Receiver`s.

## Install
```sh
cargo add binance-stream-handler
