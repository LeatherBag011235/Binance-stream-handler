use std::{collections::{HashMap, VecDeque}, time::{Duration, Instant}};
use futures_util::{Stream, StreamExt};
use tokio::sync::{mpsc, oneshot};

use crate::order_book::{DepthUpdate, CombinedDepthUpdate};


pub enum RouterCmd {
    BeginCutover { drain_for: Duration },
}

pub struct RouterHandle {
    tx: mpsc::UnboundedSender<RouterCmd>
}

impl RouterHandle {
    pub fn begin_cutover(&self, drain_for: Duration) {
        let _=self.tx.send(RouterCmd::BeginCutover { drain_for });
    }
}

enum Mode {
    SecondaryWarm,
    DrainingPrimary { until: Instant },
    SecondaryOnly,
}

pub fn start_dual_router<S1, S2>(
    mut primary: S1,
    mut secondary: S2,
    symbols: &[&str],
    chan_cap: usize,
    park_cap: usize,
) -> (RouterHandle, HashMap<String, mpsc::Receiver<DepthUpdate>>)
where
    S1: Stream<Item = CombinedDepthUpdate> + Send + 'static + Unpin,
    S2: Stream<Item = CombinedDepthUpdate> + Send + 'static + Unpin,
{
    let mut out_map = HashMap::<String, mpsc::Sender<DepthUpdate>>::new();
    let mut rx_map  = HashMap::<String, mpsc::Receiver<DepthUpdate>>::new();
    for &sym in symbols {
        let (tx, rx) = mpsc::channel::<DepthUpdate>(chan_cap);
        out_map.insert(sym.to_string(), tx);
        rx_map.insert(sym.to_string(), rx);
    }

    let mut park: HashMap<String, VecDeque<DepthUpdate>> = symbols
        .iter()
        .map(|s| (s.to_string(), VecDeque::with_capacity(park_cap)))
        .collect();

    let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel::<RouterCmd>();
    let handle = RouterHandle { tx: ctrl_tx };

    tokio::spawn(async move {
        let mut mode = Mode::SecondaryWarm;

        loop {
            tokio::select! {
                cmd = ctrl_rx.recv() => {
                    match cmd {
                        Some(RouterCmd::BeginCutover { drain_for }) => {
                            let until = Instant::now() + drain_for;
                            mode = Mode::DrainingPrimary { until };
                        }
                        None => { /* no controller; keep current mode */ }
                    }
                }

                p = primary.next(), if !matches!(mode, Mode::SecondaryOnly) => {
                    match p {
                        Some(env) => {
                            // forward immediately
                            let sym = env.data.s.to_ascii_uppercase();
                            if let Some(tx) = out_map.get(&sym) {
                                let _ = tx.send(env.data).await;
                            }
                        
                            // if draining, check deadline and flip
                            if let Mode::DrainingPrimary { until } = mode {
                                if Instant::now() >= until {
                                    // flush parked secondary
                                    for (sym, buf) in park.iter_mut() {
                                        if let Some(tx) = out_map.get(sym) {
                                            while let Some(du) = buf.pop_front() {
                                                let _ = tx.send(du).await;
                                            }
                                        } else {
                                            buf.clear();
                                        }
                                    }
                                    mode = Mode::SecondaryOnly;
                                }
                            }
                        }
                        None => {
                            // primary ended → flush parked secondary and flip immediately
                            for (sym, buf) in park.iter_mut() {
                                if let Some(tx) = out_map.get(sym) {
                                    while let Some(du) = buf.pop_front() {
                                        let _ = tx.send(du).await;
                                    }
                                } else {
                                    buf.clear();
                                }
                            }
                            mode = Mode::SecondaryOnly;
                        }
                    }
                }
                s = secondary.next() => {
                    match s {
                        Some(env) => {
                            let sym = env.data.s.to_ascii_uppercase();
                            match mode {
                                Mode::SecondaryWarm | Mode::DrainingPrimary { .. } => {
                                    // park bounded; on overflow drop oldest
                                    if let Some(buf) = park.get_mut(&sym) {
                                        if buf.len() >= park_cap { buf.pop_front(); }
                                        buf.push_back(env.data);
                                    }
                                }
                                Mode::SecondaryOnly => {
                                    // after flip: forward directly
                                    if let Some(tx) = out_map.get(&sym) {
                                        let _ = tx.send(env.data).await;
                                    }
                                }
                            }
                        }
                        None => {
                            // secondary ended unexpectedly; minimal handling is to ignore here.
                            // reconnection policy can live outside.
                        }
                    }
                }
            }
        }
    });
    (handle, rx_map)
}