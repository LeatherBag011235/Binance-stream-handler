use std::{collections::{HashMap, VecDeque}, time::{Duration, Instant}};
use futures_util::{Stream, StreamExt};
use tokio::sync::{mpsc, oneshot, watch};

use futures_util::pin_mut;
use chrono::{NaiveTime, Timelike, Utc, Duration};

use crate::order_book::{DepthUpdate, CombinedDepthUpdate};
use crate::streaming::{TimedStream};
use tokio::time::{sleep, Duration as StdDur};


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
    OnlyA,
    BothAB,
    OnlyB,
}

pub struct DualRouter<'a> {
    pub switch_cutoff: (NaiveTime, NaiveTime),
    pub currency_pairs: &'a [&'a str],
    stream_a: TimedStream<'a>,
    stream_b: TimedStream<'a>,
}


impl<'a> DualRouter<'a> {

    pub fn new(
        switch_cutoff: (NaiveTime, NaiveTime),
        currency_pairs: &'a [&'a str],
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

    fn in_window(t: NaiveTime, start: NaiveTime, end: NaiveTime) -> bool {
        if start <= end {
            t >= start && t < end
        } else {
            // wraps midnight
            t >= start || t < end
        }
    }

    async fn rout_mode(
        &self,
        ctrl_tx: &tokio::sync::watch::Sender<Mode>,
    ) {
        let (cut_a, cut_b) = self.switch_cutoff;
        let win_a_start = cut_a
            .checked_sub_signed(Duration::seconds(3))
            .unwrap_or(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        let win_b_start = cut_b
            .checked_sub_signed(Duration::seconds(3))
            .unwrap_or(NaiveTime::from_hms_opt(0, 0, 0).unwrap());

        loop {
            let now_tod: NaiveTime = Utc::now().time();

            let in_double_a = in_window(now_tod, win_a_start, cut_a.saturating_add_signed(Duration::seconds(1)));
            let in_double_b = in_window(now_tod, win_b_start, cut_b.saturating_add_signed(Duration::seconds(1)));

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

        rout_mode(ctrl_tx);

        loop {
            match ctrl_rx {...}
        }

    }

    
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