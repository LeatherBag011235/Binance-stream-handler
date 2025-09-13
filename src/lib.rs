use futures_util::{StreamExt, SinkExt, Stream, pin_mut};
use serde::Deserialize;
use tokio_tungstenite::{connect_async};
use tokio_tungstenite::tungstenite::Message;
use tokio_stream::wrappers::ReceiverStream;
use futures_util::future::ready;
use std::collections::HashMap;
use tokio::sync::{mpsc, watch};
use tracing::{info, debug, error, warn, trace};
use tracing::Instrument;
use tracing::{info_span}; 

pub mod order_book;
pub mod streaming;
pub mod router;

use crate::order_book::{OrderBook, DepthUpdate, CombinedDepthUpdate, UpdateDecision};
//use crate::streaming::{create_ws_url, streaming};
use crate::router::{};

pub fn init_order_books(
    currency_pairs: &'static [&'static str], 
    mut receivers: HashMap<String, mpsc::Receiver<DepthUpdate>>,
) -> HashMap<String, watch::Receiver<OrderBook>> {
    let mut ob_streams: HashMap<String, watch::Receiver<OrderBook>> = HashMap::new();

    for &pair in currency_pairs {
        let pair_up = pair.to_ascii_uppercase();
        let mut rx = receivers
        .remove(&pair_up)
        .expect("router created a channel for every symbol");

        let (tx_ob, rx_ob) = watch::channel(OrderBook::new(pair));
        ob_streams.insert(pair_up.clone(), rx_ob);

        tokio::spawn(async move {
            let mut ob =  match OrderBook::init_ob(pair).await {
                Ok(ob) => {
                    debug!("{} orderbook is initiated, last Id: {:?}", ob.symbol, ob.snapshot_id);
                    ob
                },
                Err(e) => {
                    eprintln!("[{pair}] snapshot init error: {e}");
                    return;
                }
            };
            let mut need_resync = false;
            let _ = tx_ob.send_replace(ob);

            while let Some(du) = rx.recv().await {

                if need_resync {                    
                    let fresh_ob = match OrderBook::init_ob(pair).await {
                        Ok(ob) => ob,
                        Err(e) => {
                            eprintln!("[{pair}] snapshot init error: {e}");
                            return;
                        }
                    };
                    let _ = tx_ob.send_replace(fresh_ob);
                    need_resync = false;
                } else {
                    let _ = tx_ob.send_modify(|book| {
                        match book.continuity_check(&du) {
                            UpdateDecision::Drop => {trace!("Update dropped");},
                            UpdateDecision::Apply(du) => {
                                trace!("Update applied");
                                book.apply_update(du);
                            }
                            UpdateDecision::Resync(info) => {
                                trace!("Resync required");
                                eprintln!(
                                    "[{pair}] RESYNC: expected pu={:?}, got pu={}, u={}",
                                    info.expected_pu, info.got_pu, info.got_u
                                );
                                need_resync = true;
                            }
                        }
                    });
                }
            }

        }.instrument(info_span!("orderbook_task", symbol = %pair)));
    }
    ob_streams
}



