use std::collections::{HashMap, VecDeque};
use std::time::{Duration as StdDur, Instant};
use futures_util::{Stream, StreamExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::sleep;
use futures_util::pin_mut;
use chrono::{NaiveTime, Timelike, Utc, Duration as ChronoDur};
use tracing::{info, debug, error, warn, trace};

use crate::order_book::{DepthUpdate, CombinedDepthUpdate};
use crate::streaming::{TimedStream};


#[derive(Clone, Copy, Debug)]
enum Mode {
    OnlyA,
    BothAB,
    OnlyB,
}

pub struct DualRouter {
    pub switch_cutoff: (NaiveTime, NaiveTime),
    pub currency_pairs: &'static [&'static str],
    stream_a: TimedStream,
    stream_b: TimedStream,
}


impl DualRouter {

    pub fn new(
        switch_cutoff: (NaiveTime, NaiveTime),
        currency_pairs: &'static [&'static str],
    ) -> Self {
        let life_span_a = switch_cutoff;
        let life_span_b = (switch_cutoff.1, switch_cutoff.0);

        let stream_a = TimedStream { 
            currency_pairs: currency_pairs, 
            life_span: (life_span_a) 
        };
        let stream_b = TimedStream { 
            currency_pairs: currency_pairs, 
            life_span: (life_span_b) 
        };

        Self {
            switch_cutoff,
            currency_pairs,
            stream_a,
            stream_b,
        }
    }

    pub fn start_dual_router(
        &self,         
        chan_cap: usize,
        park_cap: usize,
    ) {
        let mut out_map = HashMap::<String, mpsc::Sender<DepthUpdate>>::new();
        let mut rx_map  = HashMap::<String, mpsc::Receiver<DepthUpdate>>::new();

        for &sym in self.currency_pairs {
            let (tx, rx) = mpsc::channel::<DepthUpdate>(chan_cap);
            out_map.insert(sym.to_string(), tx);
            rx_map.insert(sym.to_string(), rx);
        }

        let mut park: HashMap<String, VecDeque<DepthUpdate>> = self.currency_pairs
            .iter()
            .map(|s| (s.to_string(), VecDeque::with_capacity(park_cap)))
            .collect();

        let (ctrl_tx, ctrl_rx) = watch::channel(Mode::OnlyA);

        let switch_cutoff = self.switch_cutoff;
        tokio::spawn(rout_mode(switch_cutoff, ctrl_tx));

        tokio::spawn(async move {
            loop {
                let current_stream = match ctrl {
                    Mode::OnlyA => {
                        match self.stream_a.init_stream().await {
                            Ok(stream) => {stream}
                            Err(e) => {error!("{e}");}
                        }
                    }                   
                    Mode::OnlyB => {
                        match self.stream_b.init_stream().await {
                            Ok(stream) => {stream}
                            Err(e) => {error!("{e}");}
                        }
                    }
                    Mode::BothAB=> {
                        match self.stream_a.init_stream().await {
                            Ok(stream) => {stream}
                            Err(e) => {error!("{e}");}
                        }
                        match self.stream_b.init_stream().await {
                            Ok(stream) => {stream}
                            Err(e) => {error!("{e}");}
                        }
                    };
                }
            }
        });




    }
}

async fn rout_mode(
    switch_cutoff: (NaiveTime, NaiveTime),
    mut ctrl_tx: tokio::sync::watch::Sender<Mode>,
) {
    let (cut_a, cut_b) = switch_cutoff;
    let win_a_start= sub_secs_wrap(cut_a, 3);
    let win_b_start= sub_secs_wrap(cut_b, 3);

    loop {
        let now_tod: NaiveTime = Utc::now().time();
        let in_double_a = in_window(
            now_tod, 
            win_a_start, 
            cut_a,
        );
        let in_double_b = in_window(
            now_tod, 
            win_b_start, 
            cut_b,
        );

        if in_double_a || in_double_b {
            let _ = ctrl_tx.send_replace(Mode::BothAB);
        } else if in_window(now_tod, cut_a, cut_b) {
            let _ = ctrl_tx.send_replace(Mode::OnlyA);
        } else {
            let _ = ctrl_tx.send_replace(Mode::OnlyB);
        }
        // Tick roughly once per second; adjust as needed
        sleep(StdDur::from_millis(250)).await;
    }
}

#[inline]
fn in_window(t: NaiveTime, start: NaiveTime, end: NaiveTime) -> bool {
    if start <= end {
        t >= start && t < end
    } else {
        // wraps midnight
        t >= start || t < end
    }
}

#[inline]
fn sub_secs_wrap(t: NaiveTime, secs: i64) -> NaiveTime {
    const DAY: i64 = 24 * 60 * 60;
    let cur = t.num_seconds_from_midnight() as i64;
    let mut s = (cur - secs) % DAY;
    if s < 0 { s += DAY; }
    NaiveTime::from_num_seconds_from_midnight_opt(s as u32, 0).unwrap()
}

////////////////////////////////////////////////////////////////////////////////////////
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