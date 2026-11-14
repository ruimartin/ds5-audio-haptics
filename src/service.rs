/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The running service: capture -> DSP on one thread, paced output on another.

use crate::capture::Capture;
use crate::config::{Config, Mode};
use crate::device::{self, Bus};
use crate::dsp::{Meter, Processor};
use crate::protocol::{trigger_report, ControlForm, ReportBuilder, TriggerEffect, PAYLOAD, REPORT_PERIOD_NS};
use crate::rumble::{self, Rumble};
use crate::triggers;
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// State shared between the DSP thread, the output thread and the control socket.
pub struct Shared {
    pub config: Mutex<Config>,
    pub config_path: PathBuf,
    /// Bumped on every config change so threads know to re-read it.
    pub generation: AtomicU64,
    pub sink: Mutex<String>,
    /// False while no controller is usable; audio capture is stopped meanwhile.
    pub sink_present: AtomicBool,
    pub capture: Mutex<String>,
    /// Steam app ID of the running game, 0 when none.
    pub game: AtomicU32,
    started: Instant,
    /// When the settings page last asked for levels (ms since start, 0 = never).
    meter_seen_ms: AtomicU64,
    /// Levels since the settings page last asked.
    pub meter: Mutex<Meter>,
    pub reports_sent: AtomicU64,
    pub reports_dropped: AtomicU64,
    pub write_errors: AtomicU64,
    /// Microseconds from audio arriving in this process to the hidraw write.
    pub latency_avg_us: AtomicU64,
    pub latency_max_us: AtomicU64,
}

impl Shared {
    pub fn new(config: Config, config_path: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            config: Mutex::new(config),
            config_path,
            generation: AtomicU64::new(1),
            sink: Mutex::new("none".into()),
            sink_present: AtomicBool::new(false),
            capture: Mutex::new("stopped".into()),
            game: AtomicU32::new(0),
            started: Instant::now(),
            meter_seen_ms: AtomicU64::new(0),
            meter: Mutex::new(Meter { floor_db: -90.0, ..Default::default() }),
            reports_sent: AtomicU64::new(0),
            reports_dropped: AtomicU64::new(0),
            write_errors: AtomicU64::new(0),
            latency_avg_us: AtomicU64::new(0),
            latency_max_us: AtomicU64::new(0),
        })
    }

    pub fn config(&self) -> Config {
        self.config.lock().unwrap().clone()
    }

    /// Called when someone reads the meters: keeps capture alive for the live display.
    pub fn meter_watched(&self) {
        self.meter_seen_ms.store(self.started.elapsed().as_millis() as u64 + 1, Relaxed);
    }

    /// Capture runs while a controller is connected, or while the settings page is
    /// open, so the levels can be watched and tuned without one.
    fn capture_wanted(&self) -> bool {
        let seen = self.meter_seen_ms.load(Relaxed);
        self.sink_present.load(Relaxed)
            || (seen > 0 && self.started.elapsed().as_millis() as u64 <= seen + 3000)
    }

    fn set_sink(&self, text: String) {
        self.sink_present.store(!text.starts_with("none"), Relaxed);
        let mut s = self.sink.lock().unwrap();
        if *s != text {
            eprintln!("output: {text}");
            *s = text;
        }
    }

    fn set_capture(&self, text: &str) {
        let mut s = self.capture.lock().unwrap();
        if *s != text {
            eprintln!("capture: {text}");
            *s = text.into();
        }
    }
}

/// 10.67 ms of haptics plus what the rumble fallback needs.
struct Frame {
    samples: [i8; PAYLOAD],
    rumble: (f32, f32),
    /// Strongest impact detected in this stretch of audio, 0..1.
    onset: f32,
    /// When the audio that completed this frame reached us.
    born: Instant,
}

/// Runs forever. Spawns the output thread and does capture + DSP on the caller's.
pub fn run(shared: Arc<Shared>) -> ! {
    let (tx, rx) = mpsc::sync_channel::<Frame>(16);
    {
        let shared = shared.clone();
        std::thread::Builder::new()
            .name("output".into())
            .spawn(move || output_loop(&shared, rx))
            .expect("spawn output thread");
    }
    {
        // Notice which game is running, for the adaptive-trigger profiles.
        let shared = shared.clone();
        let _ = std::thread::Builder::new().name("games".into()).spawn(move || loop {
            shared.game.store(crate::games::running_appid().unwrap_or(0), Relaxed);
            std::thread::sleep(Duration::from_secs(3));
        });
    }
    loop {
        capture_loop(&shared, &tx);
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// One capture session. Returns when it ends, fails or the service is disabled.
fn capture_loop(shared: &Shared, tx: &SyncSender<Frame>) {
    let mut generation = shared.generation.load(Relaxed);
    let mut cfg = shared.config();
    if !cfg.enabled {
        shared.set_capture("disabled");
        while shared.generation.load(Relaxed) == generation {
            std::thread::sleep(Duration::from_millis(200));
        }
        return;
    }
    if !shared.capture_wanted() {
        // No controller: do not keep an audio stream open for nothing.
        shared.set_capture("waiting for a controller");
        while !shared.capture_wanted() && shared.generation.load(Relaxed) == generation {
            std::thread::sleep(Duration::from_millis(250));
        }
        return;
    }
    let mut capture = match Capture::spawn(&cfg.capture) {
        Ok(c) => c,
        Err(e) => {
            shared.set_capture(&format!("cannot run pw-record: {e}"));
            return;
        }
    };
    shared.set_capture("running");

    let mut dsp = Processor::new(&cfg.params());
    let mut audio = Vec::new();
    let mut pending: Vec<i8> = Vec::new();
    loop {
        match capture.read(&mut audio) {
            Ok(true) => {}
            Ok(false) => return shared.set_capture("pw-record exited, restarting"),
            Err(e) => return shared.set_capture(&format!("read error: {e}")),
        }
        let born = Instant::now();
        if !shared.capture_wanted() {
            return;
        }

        let now = shared.generation.load(Relaxed);
        if now != generation {
            generation = now;
            let new = shared.config();
            if !new.enabled || new.capture.target != cfg.capture.target
                || new.capture.latency_frames != cfg.capture.latency_frames
            {
                return; // restart with the new capture settings
            }
            dsp.set_params(&new.params());
            cfg = new;
        }

        dsp.process(&audio, &mut pending);
        let mut chunk_onset;
        {
            let new = dsp.take_meter();
            chunk_onset = new.onset;
            let mut m = shared.meter.lock().unwrap();
            m.volume = m.volume.max(new.volume);
            m.bass = m.bass.max(new.bass);
            m.output = m.output.max(new.output);
            m.onset = m.onset.max(new.onset);
            m.floor_db = new.floor_db;
        }
        let mut used = 0;
        for chunk in pending.chunks_exact(PAYLOAD) {
            let frame = Frame {
                samples: chunk.try_into().unwrap(),
                rumble: dsp.rumble_levels(),
                // The impact belongs to the first frame cut from this chunk.
                onset: std::mem::take(&mut chunk_onset),
                born,
            };
            // A full channel means the output thread is stuck; dropping is right.
            if tx.try_send(frame).is_err() {
                shared.reports_dropped.fetch_add(1, Relaxed);
            }
            used += PAYLOAD;
        }
        pending.drain(..used);
    }
}

enum Sink {
    None,
    Hd { file: File, builder: ReportBuilder },
    Rumble { dev: Rumble, last: Instant },
}

fn open_hd(cfg: &Config) -> Result<(File, String), String> {
    let all = device::scan();
    let c = all
        .iter()
        .filter(|c| c.bus == Bus::Bluetooth)
        .find(|c| cfg.device.is_empty() || c.uniq.eq_ignore_ascii_case(&cfg.device))
        .ok_or("no Bluetooth DualSense")?;
    let file = OpenOptions::new()
        .write(true)
        .open(&c.hidraw)
        .map_err(|e| format!("{}: {e}", c.hidraw.display()))?;
    Ok((file, format!("hd {} {} ({})", c.hidraw.display(), c.name, c.uniq)))
}

fn open_sink(shared: &Shared, cfg: &Config) -> Sink {
    let mut why = String::new();
    if cfg.mode != Mode::Rumble {
        match open_hd(cfg) {
            Ok((file, text)) => {
                shared.set_sink(text);
                let builder = ReportBuilder::new(ControlForm::Awalol, cfg.output.buffer_len.clamp(16, 127));
                return Sink::Hd { file, builder };
            }
            Err(e) => why = e,
        }
    }
    if cfg.mode != Mode::Hd {
        for path in rumble::scan() {
            match Rumble::open(path.clone()) {
                Ok(dev) => {
                    shared.set_sink(format!("rumble {} (fallback: {why})", path.display()));
                    return Sink::Rumble { dev, last: Instant::now() };
                }
                Err(e) => why = format!("{why}; {}: {e}", path.display()),
            }
        }
    }
    shared.set_sink(format!("none ({})", if why.is_empty() { "no controller" } else { &why }));
    Sink::None
}

fn output_loop(shared: &Shared, rx: Receiver<Frame>) {
    let period = Duration::from_nanos(REPORT_PERIOD_NS);
    let mut cfg = shared.config();
    let mut generation = shared.generation.load(Relaxed);
    let mut sink = Sink::None;
    let mut next_probe = Instant::now();
    let mut last_send = Instant::now() - period;
    let mut last_active = Instant::now();
    let mut queue: VecDeque<Frame> = VecDeque::new();
    let mut params = cfg.params();
    // Adaptive-trigger kick in progress: when to switch it off.
    let mut kick: Option<Instant> = None;
    let mut state_seq = 0u8;
    // What was last decided for the triggers at rest (inner None = left alone), and when.
    let mut resting: Option<Option<(TriggerEffect, TriggerEffect)>> = None;
    let mut resting_at = Instant::now();
    let mut kick_ended = Instant::now();

    loop {
        if queue.is_empty() {
            // While a kick is on, wake up in time to end it even if audio stops.
            let wait = match kick {
                Some(until) => until.saturating_duration_since(Instant::now()).max(Duration::from_millis(1)),
                None => Duration::from_secs(2),
            };
            match rx.recv_timeout(wait) {
                Ok(f) => queue.push_back(f),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        queue.extend(rx.try_iter());
        // Bound the delay: when behind, skip to the newest audio.
        while queue.len() > cfg.output.max_backlog.max(1) {
            queue.pop_front();
            shared.reports_dropped.fetch_add(1, Relaxed);
        }

        let now = shared.generation.load(Relaxed);
        if now != generation {
            generation = now;
            let new = shared.config();
            if new.mode != cfg.mode || new.device != cfg.device
                || new.output.buffer_len != cfg.output.buffer_len
            {
                sink = Sink::None;
                next_probe = Instant::now();
            }
            cfg = new;
            params = cfg.params();
        }

        // Look for a controller when we have none, and for a way back from the
        // rumble fallback to HD.
        let fallback = matches!(sink, Sink::Rumble { .. }) && cfg.mode == Mode::Auto;
        if (matches!(sink, Sink::None) || fallback) && Instant::now() >= next_probe {
            next_probe = Instant::now() + Duration::from_secs(2);
            if !fallback || open_hd(&cfg).is_ok() {
                sink = open_sink(shared, &cfg);
            }
        }

        // Adaptive triggers: a resting feel per trigger, and a short kick on each
        // strong impact (the controller only makes that felt while the trigger is held).
        if let Sink::Hd { file, .. } = &mut sink {
            let mut set = |right: TriggerEffect, left: TriggerEffect| {
                state_seq = state_seq.wrapping_add(1);
                let _ = file.write(&trigger_report(state_seq, Some(right), Some(left)));
            };
            let appid = Some(shared.game.load(Relaxed)).filter(|&id| id != 0);
            let profile = triggers::resolve(params.trigger_profile, appid, &cfg.games);
            let plan = triggers::plan(profile, &params).filter(|_| cfg.enabled);

            if kick.is_some_and(|until| Instant::now() >= until) || (kick.is_some() && plan.is_none()) {
                kick = None;
                kick_ended = Instant::now();
                resting = None; // fall through to restoring the resting feel
            }
            if let (Some(plan), Some(frame)) = (plan, queue.front()) {
                // One kick per impact, always the same strength and length, with a
                // pause before the next: separate thumps instead of a wavering buzz.
                let rested = kick_ended.elapsed() >= Duration::from_millis(40);
                let wanted = plan.right.kick > 0 || plan.left.kick > 0;
                if wanted && kick.is_none() && rested && frame.onset >= params.trigger_threshold.max(0.01) {
                    let pulse = |side: triggers::Side| match (side.kick, params.trigger_freq_hz as u8) {
                        (0, _) => side.rest,
                        (level, 0) => TriggerEffect::feedback(0, level), // a single push back
                        (level, hz) => TriggerEffect::vibration(0, level, hz),
                    };
                    set(pulse(plan.right), pulse(plan.left));
                    kick = Some(Instant::now() + Duration::from_millis(params.trigger_ms as u64));
                }
            }
            // Write the resting feel when it changed, after a kick, and now and then
            // in case something else reset the triggers. With the feature off they
            // are released once and then never touched.
            let rest = plan.map(|p| (p.right.rest, p.left.rest));
            let stale = rest.is_some() && resting_at.elapsed() > Duration::from_secs(5);
            if kick.is_none() && (resting != Some(rest) || stale) {
                if rest.is_some() || resting.is_some_and(|r| r.is_some()) {
                    let (right, left) = rest.unwrap_or((TriggerEffect::OFF, TriggerEffect::OFF));
                    set(right, left);
                }
                resting = Some(rest);
                resting_at = Instant::now();
            }
        } else {
            kick = None;
            resting = None;
        }

        let Some(frame) = queue.pop_front() else { continue };

        // After a stretch of silence stop transmitting: saves battery and radio time.
        let silent = frame.samples.iter().all(|&s| s == 0);
        if !silent {
            last_active = Instant::now();
        }
        if last_active.elapsed().as_secs_f32() > cfg.output.idle_stop_secs {
            continue;
        }

        match &mut sink {
            Sink::None => {}
            Sink::Hd { file, builder } => {
                // Audio arrives in bursts when the graph quantum exceeds one report;
                // space the writes so the controller gets an even stream.
                let earliest = last_send + period - Duration::from_millis(1);
                let wait = earliest.saturating_duration_since(Instant::now());
                if !wait.is_zero() {
                    std::thread::sleep(wait);
                }
                let report = builder.build(&frame.samples);
                match file.write(&report) {
                    Ok(n) if n == report.len() => {
                        last_send = Instant::now();
                        shared.reports_sent.fetch_add(1, Relaxed);
                        let us = frame.born.elapsed().as_micros() as u64;
                        let avg = shared.latency_avg_us.load(Relaxed);
                        // Exponential average over about 100 reports (1 s).
                        let avg = if avg == 0 { us } else { (avg * 99 + us) / 100 };
                        shared.latency_avg_us.store(avg, Relaxed);
                        shared.latency_max_us.fetch_max(us, Relaxed);
                    }
                    other => {
                        shared.write_errors.fetch_add(1, Relaxed);
                        shared.set_sink(format!("none (write failed: {other:?})"));
                        sink = Sink::None;
                        next_probe = Instant::now() + Duration::from_secs(1);
                    }
                }
            }
            Sink::Rumble { dev, last } => {
                let interval = Duration::from_secs_f32(1.0 / cfg.output.rumble_rate_hz.clamp(5.0, 90.0));
                if last.elapsed() >= interval {
                    *last = Instant::now();
                    let (sub, mid) = if silent { (0.0, 0.0) } else { frame.rumble };
                    // Left (heavy) motor takes sub-bass, right (light) motor low-mids.
                    if let Err(e) = dev.set(sub, mid) {
                        shared.write_errors.fetch_add(1, Relaxed);
                        shared.set_sink(format!("none (rumble failed: {e})"));
                        sink = Sink::None;
                        next_probe = Instant::now() + Duration::from_secs(1);
                    }
                }
            }
        }
    }
}
