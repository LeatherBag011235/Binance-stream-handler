use futures_util::{StreamExt, SinkExt, Stream, pin_mut};
use serde::Deserialize;
use tokio_tungstenite::{connect_async};
use tokio_tungstenite::tungstenite::Message;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use futures_util::future::ready;
use std::collections::HashMap;

pub mod order_book;
pub mod streaming;
pub mod router;

use crate::order_book::{OrderBook, DepthUpdate, CombinedDepthUpdate, UpdateDecision};
use crate::streaming::{create_ws_url, streaming};
use crate::router::{};

pub async fn init_stream(
    currency_pairs: &[&str]
) -> Result<impl futures_util::Stream<Item = CombinedDepthUpdate>, Box<dyn std::error::Error>> {
    let lower: Vec<String> = currency_pairs.iter().map(|s| s.to_lowercase()).collect();
    let currency_lower: Vec<&str> = lower.iter().map(|s| s.as_str()).collect();
    
    let ws_url = create_ws_url(&currency_lower);
    let stream = streaming(ws_url).await?;
    Ok(stream)
}







pub fn init_order_books(
    currency_pairs: &'static [&'static str], 
    mut receivers: HashMap<String, mpsc::Receiver<DepthUpdate>>,
) {
    for &pair in currency_pairs {
        let mut rx = receivers
        .remove(&pair.to_ascii_uppercase())
        .expect("router created a channel for every symbol");

        tokio::spawn(async move {
            let mut ob =  match OrderBook::init_ob(pair).await {
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
}



