use tokio_tungstenite::tungstenite::Message;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use futures_util::Stream;
use futures_util::StreamExt;
use futures_util::SinkExt;
use tracing::{info, debug, error, warn, trace};
use chrono::NaiveTime;

use crate::ob_manager::order_book::{CombinedDepthUpdate};

pub struct TimedStream  {
    pub currency_pairs: &'static [&'static str],
    pub life_span: (NaiveTime, NaiveTime),
}

impl TimedStream { 
    pub async fn init_stream(
        &self
    ) -> Result<impl Stream<Item = CombinedDepthUpdate> + Send + 'static, Box<dyn std::error::Error>> {
        let lower: Vec<String> = self.currency_pairs.iter().map(|s| s.to_lowercase()).collect();
        let currency_lower: Vec<&str> = lower.iter().map(|s| s.as_str()).collect();

        let ws_url = Self::create_ws_url(&currency_lower);
        let stream = Self::streaming(ws_url).await?;
        Ok(stream)
    }

    pub async fn streaming(
        url: String,
    ) -> Result<impl Stream<Item = CombinedDepthUpdate> + Send + 'static, Box<dyn std::error::Error>> {

        info!("Connecting to {url} ...");
        let (mut ws, _resp) = tokio_tungstenite::connect_async(&url).await?;
        info!("Connected. Waiting for messages...");

        let (tx, rx) = mpsc::channel::<CombinedDepthUpdate>(1024);

        tokio::spawn(async move {
            while let Some(msg_res) = ws.next().await {
                match msg_res {
                    Ok(Message::Text(txt)) => {
                        if let Ok(env) = serde_json::from_str::<CombinedDepthUpdate>(&txt) {
                            let _ = tx.send(env).await;
                        }
                    }
                    Ok(Message::Ping(payload)) => {
                        let _ = ws.send(Message::Pong(payload)).await;
                    }
                    Ok(Message::Close(_)) => break,
                    _ => (), 
                }
            }
        });

        Ok(ReceiverStream::new(rx))
    }

    pub fn create_ws_url(currency_pairs: &Vec<&str>,) -> String {
        let stream_spec = "@depth@100ms";
        let base_url = "wss://fstream.binance.com/stream?streams=";

        let mut url = String::from(base_url);
        for (i, pair) in currency_pairs.iter().enumerate() {
            let insert_str = format!("{}{}", pair, stream_spec);
            if i > 0 {
                url.push_str("/")
            }
            url.push_str(&insert_str);
        }

        url
    }
}