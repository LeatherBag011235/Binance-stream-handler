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

        

        let currency_pairs = self.currency_pairs;
        let life_span_a = self.stream_a.life_span;
        let life_span_b = self.stream_b.life_span;



        tokio::spawn(async move {
            enum Active { A, B}
            let mut active: Option<Active> = None;

            // Lazily-opened runtime streams
            type DynDepth = Pin<Box<dyn Stream<Item = CombinedDepthUpdate> + Send>>;
            let mut stream_a: Option<DynDepth> = None;
            let mut stream_b: Option<DynDepth> = None;

            // capture the configured per-symbol bound
            let park_cap_local = park.values().next().map(|v| v.capacity()).unwrap_or(0);
            
            // helper: open A on demand
            let mut open_a = || async {
                if stream_a.is_none() {
                    let mut builder = TimedStream { currency_pairs, life_span: life_span_a};
                    if let Ok(s) = builder.init_stream().await {
                        stream_a = Some(Box::pin(s));
                    }
                }
            };
            // helper: open B on demand
            let mut open_b = || async {
                if stream_b.is_none() {
                    let mut builder = TimedStream { currency_pairs, life_span: life_span_b };
                    if let Ok(s) = builder.init_stream().await {
                        stream_b = Some(Box::pin(s));
                    }
                }
            };
            // helper: flush parked updates FIFO into out_map
            let mut flush_park = |
            out_map: &mut HashMap<String, mpsc::Sender<DepthUpdate>>,
            park: &mut HashMap<String, VecDeque<DepthUpdate>>| async {
                for (sym, buf) in park.iter_mut() {
                    if let Some(tx) = out_map.get(sym) {
                        while let Some(du) = buf.pop_front() {
                            let _ = tx.send(du).await;
                        }
                    } else {
                        buf.clear();
                    }
                }
            }
            // 5) align to current mode immediately
            let mut mode = *ctrl_rx.borrow();
            match mode {
                Mode::OnlyA => {
                    (open_a)().await;
                    stream_b = None;
                    active = Some(Active:A);
                    (flush_park)(&mut out_map, &mut park).await;
                }
                Mode::OnlyB => {
                    (open_b)().await;
                    stream_a = None;
                    active = Some(Active::B);
                    (flush_park)(&mut out_map, &mut park).await;
                }
                Mode::BothAB => {
                    (open_a)().await;
                    (open_b)().await;
                    active = Some(Active::A);
                }
            }

            // 6) main loop: react to mode changes and stream events
            loop {
                tokio::select! {
                    // 6a) time-based mode change
                    changed = ctrl_rx.changed() => {
                        if changed.is_err() { break; }
                        let new_mode = *ctrl_rx.borrow_and_update();

                        match (mode, new_mode) {
                            (Mode::OnlyA, Mode:: BothAB) => {
                                (open_b).await;
                                active = Some(Active::A);
                            }
                            (Mode::BothAB, Mode::OnlyB) => {
                                (flush_park)(&mut out_map, &mut park).await 
                                stream_a = None;
                                (open_b)().await;
                                active = Some(Active::B);
                            }
                            (Mode::OnlyB, Mode::BothAB) => {
                                (open_a)().await;
                                active = Some(Active::B)
                            }
                            (Mode::BothAB, Mode::OnlyA) => {
                                (flush_park)(&mut out_map, &mut park).await;
                                stream_b = None;
                                (open_a)().await;
                                active = Some(Active::A);
                            }
                            _ => {panick!("Unexpected Mode (stream line) switch from {} to {}", mode, new_mode);}

                        }
                        mode = new_mode;
                    }
                    // 6b) Stream A events
                    maybe_env = async {
                        if let Some(s) = &mut stream_a { s.next()}
                    }
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