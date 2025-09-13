use std::collections::{HashMap, VecDeque};
use std::time::{Duration as StdDur, Instant};
use std::pin::Pin;
use futures_util::{Stream, StreamExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::sleep;
use futures_util::pin_mut;
use chrono::{NaiveTime, Timelike, Utc, Duration as ChronoDur};
use tracing::{info, debug, error, warn, trace};

use crate::order_book::{DepthUpdate, CombinedDepthUpdate};
use crate::streaming::{TimedStream};

type DynDepth = Pin<Box<dyn Stream<Item = CombinedDepthUpdate> + Send>>;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    OnlyA,
    BothAB,
    OnlyB,
}

#[derive(PartialEq)]
enum Active { A, B}

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
    ) -> HashMap::<String, mpsc::Receiver<DepthUpdate>> {
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

        let (ctrl_tx, mut ctrl_rx) = watch::channel(Mode::OnlyA);

        let switch_cutoff = self.switch_cutoff;
        tokio::spawn(rout_mode(switch_cutoff, ctrl_tx));

        let currency_pairs = self.currency_pairs;
        let life_span_a = self.stream_a.life_span;
        let life_span_b = self.stream_b.life_span;

        tokio::spawn(async move {
            
            let mut active: Option<Active> = None;

            // Lazily-opened runtime streams
            let mut stream_a: Option<DynDepth> = None;
            let mut stream_b: Option<DynDepth> = None;

            // capture the configured per-symbol bound
            let park_cap_local = park.values().next().map(|v| v.capacity()).unwrap_or(0);
            

            let mut pending_mode: Option<Mode> = None; 
            let mut mode = *ctrl_rx.borrow();

            match mode {
                Mode::OnlyA => {
                    open_stream(&mut stream_a, currency_pairs, life_span_a).await;
                    stream_b = None;
                    active = Some(Active::A);
                    flush_park(&mut out_map, &mut park).await;
                }
                Mode::OnlyB => {
                    open_stream(&mut stream_b, currency_pairs, life_span_b).await;
                    stream_a = None;
                    active = Some(Active::B);
                    flush_park(&mut out_map, &mut park).await;
                }
                Mode::BothAB => {
                    open_stream(&mut stream_a, currency_pairs, life_span_a).await;
                    open_stream(&mut stream_b, currency_pairs, life_span_b).await;
                    active = Some(Active::A);
                }
            }

            // 6) main loop: react to mode changes and stream events
            loop {
                if let Some(new_mode) = pending_mode.take() {
                    mode = apply_transition(
                        mode, new_mode,
                        &mut active,
                        &mut stream_a, &mut stream_b,
                        currency_pairs, life_span_a, life_span_b,
                        &mut out_map, &mut park,
                    ).await;
                }

                let a_open = stream_a.is_some();
                let b_open = stream_b.is_some();


                tokio::select! {
                    // 6a) time-based mode change
                    changed = ctrl_rx.changed() => {
                        if changed.is_err() { break; }
                        pending_mode = Some(*ctrl_rx.borrow_and_update());
                    }
                    // 6b) Stream A events
                    maybe_env = async {
                        if let Some(s) = &mut stream_a { s.next().await } else {None}
                    }, if a_open => {
                        match maybe_env {
                            Some(env) => {
                                let CombinedDepthUpdate {data: du, ..} = env;
                                let sym = du.s.to_ascii_uppercase();

                                match mode {
                                    Mode::OnlyA => {
                                        if let Some(tx) = out_map.get(&sym) {
                                            let _ = tx.send(du).await;
                                        }
                                    }
                                    Mode::BothAB => {
                                        if active == Some(Active::A) {
                                            if let Some(tx) = out_map.get(&sym) {
                                                let _ = tx.send(du).await; 
                                            }
                                        } else {
                                            if let Some(buf) = park.get_mut(&sym) {
                                                if buf.len() >= park_cap_local { buf.pop_front(); }
                                                buf.push_back(du);
                                            }
                                        }
                                    }
                                    Mode::OnlyB => {
                                        // A is closed 
                                    }
                                }
                            }
                            None => {
                                //stream_a = None;
                            }
                        }
                    }
                    maybe_env = async {
                        if let Some(s) = &mut stream_b { s.next().await } else { None }
                    }, if b_open => {
                        match maybe_env {
                            Some(env) => {
                                let CombinedDepthUpdate { data: du, .. } = env; 
                                let sym = du.s.to_ascii_uppercase();

                                match mode {
                                    Mode::OnlyB => {
                                        if let Some(tx) = out_map.get(&sym) {
                                            let _ = tx.send(du).await;
                                        }
                                    }
                                    Mode::BothAB => {
                                        if active == Some(Active::B) {
                                            if let Some(tx) = out_map.get(&sym) {
                                                let _ = tx.send(du).await;
                                            }    
                                        } else {
                                            if let Some(buf) = park.get_mut(&sym) {
                                                if buf.len() >= park_cap_local { buf.pop_front(); }
                                                buf.push_back(du);
                                            }
                                        }
                                    }
                                    Mode::OnlyA => {
                                        // B is closed
                                    }
                                }
                            }
                            None => {
                                //stream_b = None;
                            }
                        }
                    }
                }
            }
        });
        rx_map
    }
}

async fn apply_transition(
    old: Mode, 
    new: Mode,
    active: &mut Option<Active>,
    stream_a: &mut Option<DynDepth>,
    stream_b: &mut Option<DynDepth>,
    currency_pairs: &'static [&'static str], 
    life_span_a: (NaiveTime, NaiveTime),
    life_span_b: (NaiveTime, NaiveTime),
    out_map: &mut HashMap::<String, mpsc::Sender<DepthUpdate>>,
    park: &mut HashMap<String, VecDeque<DepthUpdate>>,
) -> Mode {

    match (old, new) {
        (o, n) if o == n => {
            trace!("Pseudo mode flip occured");
            return n 
        },
        // OnlyA → BothAB: keep A primary, open B (start parking B)
        (Mode::OnlyA, Mode::BothAB) => {
            open_stream(stream_b, currency_pairs, life_span_b).await;
            *active = Some(Active::A);
        }
        // BothAB → OnlyB: flush parked (B), close A, A→None, B becomes primary
        (Mode::BothAB, Mode::OnlyB) => {
            flush_park(out_map, park).await;
            *stream_a = None;
            open_stream(stream_b, currency_pairs, life_span_b).await;
            *active = Some(Active::B);
        }
        // OnlyB → BothAB: keep B primary, open A (start parking A)
        (Mode::OnlyB, Mode::BothAB) => {
            open_stream(stream_a, currency_pairs, life_span_a).await;
            *active = Some(Active::B);
        }
        // BothAB → OnlyA: flush parked (A), close B, B→None, A becomes primary
        (Mode::BothAB, Mode::OnlyA) => {
            flush_park(out_map, park).await;
            *stream_b = None;
            open_stream(stream_a, currency_pairs, life_span_a).await;
            *active = Some(Active::A);
        }
        _ => { panic!("Unexpected channel/mode switch from {:?} to {:?}", old, new); }
    }
    
    new
}

async fn open_stream(
    stream: &mut Option<DynDepth>,
    currency_pairs: &'static [&'static str],
    life_span: (NaiveTime, NaiveTime),
) {
    if stream.is_none() {
        let mut builder = TimedStream { currency_pairs, life_span };
        if let Ok(s) = builder.init_stream().await {
            *stream = Some(Box::pin(s));
        }
    }
}

async fn flush_park(
    out_map: &mut HashMap<String, mpsc::Sender<DepthUpdate>>,
    park: &mut HashMap<String, VecDeque<DepthUpdate>>
) {
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

async fn rout_mode(
    switch_cutoff: (NaiveTime, NaiveTime),
    mut ctrl_tx: tokio::sync::watch::Sender<Mode>,
) {
    let (cut_a, cut_b) = switch_cutoff;
    let win_a_start= sub_secs_wrap(cut_a, 3);
    let win_b_start= sub_secs_wrap(cut_b, 3);

    let mut last_sent: Option<Mode> = Some(Mode::OnlyA);

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

        let new_mode = if in_double_a || in_double_b {
            Mode::BothAB
        } else if in_window (now_tod, cut_a, cut_b) {
            Mode::OnlyA
        } else {
            Mode::OnlyB
        };

        if last_sent != Some(new_mode) {
            let _ = ctrl_tx.send_replace(new_mode);
            last_sent = Some(new_mode);
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
//pub fn start_dual_router<S1, S2>(
//        mut primary: S1,
//        mut secondary: S2,
//        symbols: &[&str],
//        chan_cap: usize,
//        park_cap: usize,
//    ) -> (RouterHandle, HashMap<String, mpsc::Receiver<DepthUpdate>>)
//    where
//        S1: Stream<Item = CombinedDepthUpdate> + Send + 'static + Unpin,
//        S2: Stream<Item = CombinedDepthUpdate> + Send + 'static + Unpin,
//    {
//        let mut out_map = HashMap::<String, mpsc::Sender<DepthUpdate>>::new();
//        let mut rx_map  = HashMap::<String, mpsc::Receiver<DepthUpdate>>::new();
//
//        for &sym in symbols {
//            let (tx, rx) = mpsc::channel::<DepthUpdate>(chan_cap);
//            out_map.insert(sym.to_string(), tx);
//            rx_map.insert(sym.to_string(), rx);
//        }
//
//        let mut park: HashMap<String, VecDeque<DepthUpdate>> = symbols
//            .iter()
//            .map(|s| (s.to_string(), VecDeque::with_capacity(park_cap)))
//            .collect();
//
//        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel::<RouterCmd>();
//        let handle = RouterHandle { tx: ctrl_tx };
//
//        tokio::spawn(async move {
//            let mut mode = Mode::SecondaryWarm;
//
//            loop {
//                tokio::select! {
//                    cmd = ctrl_rx.recv() => {
//                        match cmd {
//                            Some(RouterCmd::BeginCutover { drain_for }) => {
//                                let until = Instant::now() + drain_for;
//                                mode = Mode::DrainingPrimary { until };
//                            }
//                            None => { /* no controller; keep current mode */ }
//                        }
//                    }
//
//                    p = primary.next(), if !matches!(mode, Mode::SecondaryOnly) => {
//                        match p {
//                            Some(env) => {
//                                // forward immediately
//                                let sym = env.data.s.to_ascii_uppercase();
//                                if let Some(tx) = out_map.get(&sym) {
//                                    let _ = tx.send(env.data).await;
//                                }
//                            
//                                // if draining, check deadline and flip
//                                if let Mode::DrainingPrimary { until } = mode {
//                                    if Instant::now() >= until {
//                                        // flush parked secondary
//                                        for (sym, buf) in park.iter_mut() {
//                                            if let Some(tx) = out_map.get(sym) {
//                                                while let Some(du) = buf.pop_front() {
//                                                    let _ = tx.send(du).await;
//                                                }
//                                            } else {
//                                                buf.clear();
//                                            }
//                                        }
//                                        mode = Mode::SecondaryOnly;
//                                    }
//                                }
//                            }
//                            None => {
//                                // primary ended → flush parked secondary and flip immediately
//                                for (sym, buf) in park.iter_mut() {
//                                    if let Some(tx) = out_map.get(sym) {
//                                        while let Some(du) = buf.pop_front() {
//                                            let _ = tx.send(du).await;
//                                        }
//                                    } else {
//                                        buf.clear();
//                                    }
//                                }
//                                mode = Mode::SecondaryOnly;
//                            }
//                        }
//                    }
//                    s = secondary.next() => {
//                        match s {
//                            Some(env) => {
//                                let sym = env.data.s.to_ascii_uppercase();
//                                match mode {
//                                    Mode::SecondaryWarm | Mode::DrainingPrimary { .. } => {
//                                        // park bounded; on overflow drop oldest
//                                        if let Some(buf) = park.get_mut(&sym) {
//                                            if buf.len() >= park_cap { buf.pop_front(); }
//                                            buf.push_back(env.data);
//                                        }
//                                    }
//                                    Mode::SecondaryOnly => {
//                                        // after flip: forward directly
//                                        if let Some(tx) = out_map.get(&sym) {
//                                            let _ = tx.send(env.data).await;
//                                        }
//                                    }
//                                }
//                            }
//                            None => {
//                                // secondary ended unexpectedly; minimal handling is to ignore here.
//                                // reconnection policy can live outside.
//                            }
//                        }
//                    }
//                }
//            }
//        });
//        (handle, rx_map)
//    }