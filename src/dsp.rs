/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Audio to haptics. Input: interleaved stereo f32 at 48 kHz. Output: interleaved
//! stereo signed 8-bit at 3 kHz.
//!
//! At 48 kHz: high-pass, 4th-order low-pass, onset detection on the high band.
//! Then decimate 16:1 (the low-pass already removed everything near 1.5 kHz).
//! At 3 kHz: stereo blend, gain, one dynamics stage (gate, level curve, compressor),
//! transient pulses, soft limiter.
//!
//! The general shape (low-pass, envelope, waveform shaping) follows the "Audio Auto
//! Haptics" design in loteran/DS5Dongle (MIT); the code is independent.

use crate::config::Params;
use std::f32::consts::{PI, TAU};

pub const INPUT_RATE: f32 = 48_000.0;
pub const OUTPUT_RATE: f32 = 3_000.0;
pub const DECIMATION: usize = 16;

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// RBJ cookbook low-pass or high-pass. Keeps the filter state.
    fn set(&mut self, highpass: bool, hz: f32, q: f32, rate: f32) {
        let w = TAU * hz / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        let (b0, b1) = if highpass {
            ((1.0 + cos) / 2.0, -(1.0 + cos))
        } else {
            ((1.0 - cos) / 2.0, 1.0 - cos)
        };
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b0 / a0;
        self.a1 = -2.0 * cos / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    /// RBJ cookbook low shelf (slope 1): `gain_db` below `hz`, unity above.
    fn set_low_shelf(&mut self, hz: f32, gain_db: f32, rate: f32) {
        let a = 10f32.powf(gain_db / 40.0);
        let w = TAU * hz / rate;
        let (sin, cos) = w.sin_cos();
        let beta = 2.0 * a.sqrt() * sin / 2.0 * 2f32.sqrt();
        let a0 = (a + 1.0) + (a - 1.0) * cos + beta;
        self.b0 = a * ((a + 1.0) - (a - 1.0) * cos + beta) / a0;
        self.b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos) / a0;
        self.b2 = a * ((a + 1.0) - (a - 1.0) * cos - beta) / a0;
        self.a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos) / a0;
        self.a2 = ((a + 1.0) + (a - 1.0) * cos - beta) / a0;
    }

    /// Transposed direct form II.
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// One-pole smoothing coefficient for a time constant in milliseconds.
fn coeff(ms: f32, rate: f32) -> f32 {
    1.0 - (-1.0 / (ms * 0.001 * rate)).exp()
}

/// High-pass followed by a 4th-order Butterworth low-pass.
#[derive(Clone, Copy, Default)]
struct BandFilter {
    hp: Biquad,
    shelf: Biquad,
    lp: [Biquad; 2],
}

impl BandFilter {
    fn set(&mut self, p: &Params) {
        let (highpass_hz, lowpass_hz) = (p.highpass_hz, p.lowpass_hz);
        self.hp.set(true, highpass_hz, 0.7071, INPUT_RATE);
        self.shelf.set_low_shelf(p.bass_boost_hz, p.bass_boost_db, INPUT_RATE);
        // Butterworth 4th order = two sections with these Q values.
        self.lp[0].set(false, lowpass_hz, 0.5412, INPUT_RATE);
        self.lp[1].set(false, lowpass_hz, 1.3066, INPUT_RATE);
    }

    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.hp.run(x);
        let y = self.shelf.run(y);
        let y = self.lp[0].run(y);
        self.lp[1].run(y)
    }
}

/// Finds sudden onsets (gunshots, impacts, menu blips) and how bright they are.
#[derive(Default)]
struct OnsetDetector {
    hp: Biquad,
    /// Second high-pass at 2 kHz, to tell a bright tick from a dull thud.
    hp_bright: Biquad,
    fast: f32,
    slow: f32,
    /// Average level over the last few seconds: how loud the music and ambience are.
    bed: f32,
    bright: f32,
    /// Samples left in the current measuring window; 0 when idle.
    window: u32,
    peak: f32,
    peak_bright: f32,
    /// Background level when the current onset began.
    base: f32,
    refractory: u32,
    /// Strongest onset since the last `take`: (strength 0..1, brightness 0..1).
    pending: (f32, f32),
}

impl OnsetDetector {
    /// After an onset starts, its peak is measured for this long before the pulse
    /// fires, so the pulse strength reflects the hit and not its first sample.
    const WINDOW: u32 = (INPUT_RATE * 0.003) as u32;

    #[inline]
    fn run(&mut self, x: f32, sensitivity: f32) {
        let band = self.hp.run(x);
        let v = band.abs();
        let vb = self.hp_bright.run(band).abs();
        // Fast followers: about 0.5 ms up, 15 ms down. Slow follower: about 60 ms.
        self.fast += if v > self.fast { 0.04 } else { 0.0014 } * (v - self.fast);
        self.bright += if vb > self.bright { 0.04 } else { 0.0014 } * (vb - self.bright);
        self.slow += 0.00035 * (v - self.slow);
        // The bed follows quiet quickly (about 1.5 s) and loudness slowly (about 8 s),
        // so a long burst of gunfire does not become the bed and keeps registering.
        self.bed += if v < self.bed { 0.000014 } else { 0.0000026 } * (v - self.bed);
        if self.window > 0 {
            self.peak = self.peak.max(self.fast - self.bed);
            self.peak_bright = self.peak_bright.max(self.bright);
            self.window -= 1;
            if self.window == 0 {
                // Loud hits (gunshots) are scored by level alone and reach full strength
                // at a quarter of full scale. Quieter ones (menu blips) are lifted by a
                // square root, but only as far as they stand clear of what was playing
                // just before, so ambient crackle stays faint.
                let level = (self.peak * 4.0).min(1.0);
                let standout = ((self.peak / self.base.max(1e-4) - 2.5) / 6.0).clamp(0.0, 1.0);
                let strength = level.max(level.sqrt() * standout) * sensitivity;
                if strength > self.pending.0 {
                    let brightness = (self.peak_bright / self.peak.max(1e-6)).clamp(0.0, 1.0);
                    self.pending = (strength.min(1.0), brightness);
                }
                self.refractory = (INPUT_RATE * 0.07) as u32;
            }
        } else if self.refractory > 0 {
            self.refractory -= 1;
        } else if self.fast > 0.004 && self.fast > self.bed * 3.5 && self.fast > self.slow * 1.5 {
            // An onset is a rise (clear of the last 60 ms) that also towers over the
            // bed. Drum hits sit about 8 dB above their own music; a gunshot over the
            // same music sits well beyond that.
            self.window = Self::WINDOW;
            self.base = self.bed;
            self.peak = 0.0;
            self.peak_bright = 0.0;
        }
    }

    fn take(&mut self) -> (f32, f32) {
        std::mem::take(&mut self.pending)
    }
}

/// Decaying sine burst added on each onset. Bright onsets get a higher, shorter
/// burst (a tick); dull ones a lower, longer one (a thump).
#[derive(Default)]
struct Pulse {
    amp: f32,
    phase: f32,
    step: f32,
    decay: f32,
}

impl Pulse {
    #[inline]
    fn run(&mut self, (strength, brightness): (f32, f32), p: &Params) -> f32 {
        if strength > self.amp {
            self.amp = strength;
            self.phase = 0.0;
            let hz = p.transient_pulse_hz + brightness * (p.transient_click_hz - p.transient_pulse_hz);
            let ms = p.transient_decay_ms * (1.0 - 0.5 * brightness);
            self.step = hz / OUTPUT_RATE;
            self.decay = (-1.0 / (ms * 0.001 * OUTPUT_RATE)).exp();
        }
        if self.amp < 0.002 {
            self.amp = 0.0;
            return 0.0;
        }
        let y = self.amp * (TAU * self.phase).sin();
        self.phase = (self.phase + self.step).fract();
        self.amp *= self.decay;
        y
    }
}

/// Static response of the dynamics stage: output level in dB for a bass-band level
/// of `db`, before intensity and the limiter. `None` below the gate.
pub fn response_db(db: f32, p: &Params) -> Option<f32> {
    if db < p.gate_db {
        return None;
    }
    let mut out = db * p.curve;
    if out > p.comp_threshold_db {
        out = p.comp_threshold_db + (out - p.comp_threshold_db) / p.comp_ratio;
    }
    // Never lift by more than 30 dB, so noise just above the gate stays modest.
    Some((out + p.makeup_db).min(db + 30.0))
}

/// Final strength 0..1 for a steady bass-band level, as the settings page plots it.
pub fn response_curve(p: &Params) -> Vec<(f32, f32)> {
    (-60..=0)
        .map(|db| {
            let db = db as f32;
            let out = response_db(db, p).map_or(0.0, |o| 10f32.powf(o / 20.0) * p.intensity);
            (db, p.ceiling * (out / p.ceiling).tanh())
        })
        .collect()
}

/// What the settings page shows live. Peaks hold until the next `take_meter`.
#[derive(Clone, Copy, Default, Debug)]
pub struct Meter {
    /// Overall volume of the mix, linear peak.
    pub volume: f32,
    /// Bass-band level entering the dynamics stage, linear peak.
    pub bass: f32,
    /// Background estimate, dB.
    pub floor_db: f32,
    /// Haptic output, 0..1 peak.
    pub output: f32,
    /// Strongest onset, 0..1.
    pub onset: f32,
}

/// Gate, level curve and compressor as one gain computer on a shared envelope.
struct Dynamics {
    env: f32,
    gain: f32,
    open: bool,
    /// Running estimate of the steady (background) level, in dB.
    floor_db: f32,
    /// 0..1, set by onsets and decaying: how far background rejection is lifted.
    open_by_onset: f32,
}

/// Onsets at least this strong hold the background estimate still. Weaker ones
/// (ambience, soft drum hits) must not, or music would never settle as background.
const FREEZE: f32 = 0.25;

impl Dynamics {
    #[inline]
    fn run(&mut self, level: f32, onset: f32, p: &Params) -> f32 {
        // Peak envelope: instant attack, 60 ms release, so one cycle of a low tone holds.
        self.env = level.max(self.env * (1.0 - coeff(60.0, OUTPUT_RATE)));
        let db = 20.0 * self.env.max(1e-6).log10();

        // 4 dB of hysteresis stops the gate chattering around the threshold.
        self.open = db > if self.open { p.gate_db - 4.0 } else { p.gate_db };
        // Onsets lift background rejection, and decay over transient_open_ms.
        self.open_by_onset = onset.max(self.open_by_onset * (1.0 - coeff(p.transient_open_ms.max(1.0), OUTPUT_RATE)));

        // Background estimate: drops quickly to the quietest recent level and climbs
        // at a fixed slow rate, so a steady bed of music settles at the floor. It does
        // not climb while hits keep landing: sustained gunfire must never turn into
        // "background" and fade.
        let level = db.max(p.gate_db);
        if level < self.floor_db {
            self.floor_db += coeff(p.background_fall_ms, OUTPUT_RATE) * (level - self.floor_db);
        } else if self.open_by_onset < FREEZE {
            self.floor_db = (self.floor_db + p.background_rise_db_s / OUTPUT_RATE).min(level);
        }
        // Full strength when 9 dB or more above the floor, up to 30 dB down at the floor.
        let above = ((db - self.floor_db) / 9.0).clamp(0.0, 1.0);
        let reject_db = -30.0 * p.background_reject * (1.0 - above) * (1.0 - self.open_by_onset);

        let target = match response_db(db, p) {
            Some(out) if self.open => 10f32.powf((out + reject_db * p.curve - db) / 20.0),
            // Inside the hysteresis band the gate is still open but the curve says shut.
            None if self.open => 10f32.powf((db * p.curve + p.makeup_db + reject_db - db).min(30.0) / 20.0),
            _ => 0.0,
        };
        // Gain falling = compressor attack; gain rising = release.
        let ms = if target < self.gain { p.attack_ms } else { p.release_ms };
        self.gain += coeff(ms, OUTPUT_RATE) * (target - self.gain);
        self.gain
    }
}

/// Where the sound sits between left and right, judged on the whole audible band.
/// Bass is nearly always mixed to the centre, so the low band alone cannot tell;
/// the mids and highs of the same sound can.
#[derive(Default)]
struct Panner {
    /// 4th-order high-pass per side, so centred bass does not dilute the reading.
    hp: [[Biquad; 2]; 2],
    env: [f32; 2],
}

impl Panner {
    #[inline]
    fn run(&mut self, l: f32, r: f32) {
        for (i, x) in [l, r].into_iter().enumerate() {
            let y = self.hp[i][0].run(x);
            let v = self.hp[i][1].run(y).abs();
            // About 2 ms up, 80 ms down.
            self.env[i] += if v > self.env[i] { 0.01 } else { 0.00026 } * (v - self.env[i]);
        }
    }

    /// Gains for the left and right grip. Equal when the sound is centred or too
    /// quiet to judge.
    fn gains(&self, width: f32) -> (f32, f32) {
        let (l, r) = (self.env[0], self.env[1]);
        if l + r < 1e-4 {
            return (1.0, 1.0);
        }
        let pan = (r - l) / (l + r); // -1 left .. +1 right
        ((1.0 - width * pan).clamp(0.0, 1.0), (1.0 + width * pan).clamp(0.0, 1.0))
    }
}

/// Envelope of one band, for the rumble fallback.
#[derive(Default)]
struct BandLevel {
    filter: [Biquad; 2],
    env: f32,
}

impl BandLevel {
    #[inline]
    fn run(&mut self, x: f32) {
        let y = self.filter[0].run(x);
        let v = self.filter[1].run(y).abs();
        self.env += if v > self.env { 0.01 } else { 0.0004 } * (v - self.env);
    }
}

pub struct Processor {
    params: Params,
    band: [BandFilter; 2],
    panner: Panner,
    onset: OnsetDetector,
    pulse: Pulse,
    dynamics: Dynamics,
    sub: BandLevel,
    mid: BandLevel,
    phase: usize,
    /// Peak envelope of the full mix, for the volume threshold.
    volume: f32,
    volume_gain: f32,
    meter: Meter,
}

impl Processor {
    pub fn new(params: &Params) -> Self {
        let mut p = Self {
            params: params.clone(),
            band: Default::default(),
            panner: Default::default(),
            onset: Default::default(),
            pulse: Default::default(),
            dynamics: Dynamics { env: 0.0, gain: 0.0, open: false, floor_db: params.gate_db, open_by_onset: 0.0 },
            sub: Default::default(),
            mid: Default::default(),
            phase: 0,
            volume: 0.0,
            volume_gain: 1.0,
            meter: Meter::default(),
        };
        p.set_params(params);
        p
    }

    /// Recomputes coefficients; filter state is kept so changes do not click.
    pub fn set_params(&mut self, params: &Params) {
        self.params = params.clone();
        for b in &mut self.band {
            b.set(params);
        }
        self.onset.hp.set(true, params.transient_highpass_hz, 0.7071, INPUT_RATE);
        self.onset.hp_bright.set(true, 2000.0, 0.7071, INPUT_RATE);
        for side in &mut self.panner.hp {
            side[0].set(true, 300.0, 0.5412, INPUT_RATE);
            side[1].set(true, 300.0, 1.3066, INPUT_RATE);
        }
        // Rumble fallback bands: sub-bass below 80 Hz, low-mids 80..250 Hz.
        self.sub.filter[0].set(true, 20.0, 0.7071, INPUT_RATE);
        self.sub.filter[1].set(false, 80.0, 0.7071, INPUT_RATE);
        self.mid.filter[0].set(true, 80.0, 0.7071, INPUT_RATE);
        self.mid.filter[1].set(false, 250.0, 0.7071, INPUT_RATE);
    }

    /// Consumes interleaved stereo input and appends interleaved stereo output.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<i8>) {
        let p = self.params.clone();
        for frame in input.chunks_exact(2) {
            // Non-finite input would poison the filter state for good.
            let l = if frame[0].is_finite() { frame[0] } else { 0.0 };
            let r = if frame[1].is_finite() { frame[1] } else { 0.0 };
            let mono = 0.5 * (l + r);
            let low_l = self.band[0].run(l);
            let low_r = self.band[1].run(r);
            if p.transient {
                self.onset.run(mono, p.transient_sensitivity);
            }
            if p.stereo {
                self.panner.run(l, r);
            }
            self.sub.run(mono);
            self.mid.run(mono);
            // Instant attack, about 150 ms release: the tail of a loud sound still counts.
            self.volume = l.abs().max(r.abs()).max(self.volume * 0.99986);

            self.phase += 1;
            if self.phase < DECIMATION {
                continue;
            }
            self.phase = 0;

            let low = 0.5 * (low_l + low_r);
            let trigger = if p.transient { self.onset.take() } else { (0.0, 0.0) };
            let open = if p.transient_open_ms > 0.0 { trigger.0 } else { 0.0 };
            let gain = self.dynamics.run(low.abs(), open, &p);
            let pulse = self.pulse.run(trigger, &p) * p.transient_gain;

            // Volume threshold: fully ignored below it, fully felt 6 dB above it.
            let over = 20.0 * self.volume.max(1e-6).log10() - p.volume_threshold_db;
            let t = if p.volume_threshold_db <= -89.0 { 1.0 } else { (over / 6.0).clamp(0.0, 1.0) };
            // Opens within a millisecond so attacks survive, closes gently.
            let ms = if t * t > self.volume_gain { 1.0 } else { 20.0 };
            self.volume_gain += coeff(ms, OUTPUT_RATE) * (t * t - self.volume_gain);

            let v = (low * gain + pulse) * p.intensity * self.volume_gain;

            self.meter.volume = self.meter.volume.max(self.volume);
            self.meter.bass = self.meter.bass.max(self.dynamics.env);
            self.meter.floor_db = self.dynamics.floor_db;
            self.meter.onset = self.meter.onset.max(trigger.0);
            self.meter.output = self.meter.output.max((p.ceiling * (v / p.ceiling).tanh()).abs());

            let (gl, gr) = if p.stereo { self.panner.gains(p.stereo_width) } else { (1.0, 1.0) };
            for v in [v * gl, v * gr] {
                // tanh keeps the output under the ceiling without hard edges.
                let y = p.ceiling * (v / p.ceiling).tanh();
                out.push((y * 127.0).round().clamp(-127.0, 127.0) as i8);
            }
        }
    }

    /// Levels since the previous call.
    pub fn take_meter(&mut self) -> Meter {
        std::mem::take(&mut self.meter)
    }

    /// (sub-bass, low-mid) levels 0..1 for the rumble fallback.
    pub fn rumble_levels(&self) -> (f32, f32) {
        let p = &self.params;
        let shape = |env: f32| {
            let db = 20.0 * env.max(1e-6).log10();
            if db < p.gate_db {
                0.0
            } else {
                // Same level curve as the HD path, plus 12 dB because band envelopes
                // of real audio sit far below full scale.
                (10f32.powf((db * p.curve + 12.0) / 20.0) * p.intensity * self.volume_gain).min(1.0)
            }
        };
        (shape(self.sub.env), shape(self.mid.env))
    }
}

/// Group delay of the low-pass near DC, in milliseconds (for latency reporting).
pub fn lowpass_delay_ms(lowpass_hz: f32) -> f32 {
    // 4th-order Butterworth: sum of 1/(Q*w0) over both sections.
    (1.0 / 0.5412 + 1.0 / 1.3066) / (2.0 * PI * lowpass_hz) * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Preset;

    fn sine(hz: f32, amp: f32, secs: f32) -> Vec<f32> {
        (0..(INPUT_RATE * secs) as usize)
            .flat_map(|n| {
                let v = amp * (TAU * hz * n as f32 / INPUT_RATE).sin();
                [v, v]
            })
            .collect()
    }

    /// Peak of the left channel over the last half of the output.
    fn settled_peak(out: &[i8]) -> i32 {
        out[out.len() / 2..].chunks(2).map(|f| (f[0] as i32).abs()).max().unwrap()
    }

    fn run(p: &Params, input: &[f32]) -> Vec<i8> {
        let mut out = Vec::new();
        Processor::new(p).process(input, &mut out);
        out
    }

    fn plain() -> Params {
        // Linear, no dynamics, no pulses: isolates the filters.
        Params {
            curve: 1.0,
            comp_ratio: 1.0,
            transient: false,
            ceiling: 1.0,
            gate_db: -90.0,
            bass_boost_db: 0.0,
            background_reject: 0.0,
            stereo: false,
            makeup_db: 0.0,
            ..Preset::Game.params()
        }
    }

    #[test]
    fn output_rate_is_one_sixteenth() {
        let out = run(&plain(), &sine(60.0, 0.5, 1.0));
        assert_eq!(out.len(), 3000 * 2);
    }

    #[test]
    fn passes_bass_and_rejects_treble() {
        let p = plain();
        let bass = settled_peak(&run(&p, &sine(60.0, 0.5, 1.0)));
        let treble = settled_peak(&run(&p, &sine(1000.0, 0.5, 1.0)));
        // 0.5 * 127 = 63 for the passband; tanh at 0.5 costs about 8 %.
        assert!((55..=64).contains(&bass), "bass {bass}");
        // 1 kHz is 2.7 octaves above 150 Hz: about -66 dB with a 4th-order filter.
        assert!(treble <= 1, "treble {treble}");
    }

    #[test]
    fn cutoff_is_minus_three_db() {
        let p = Params { lowpass_hz: 150.0, ..plain() };
        let at_cutoff = settled_peak(&run(&p, &sine(150.0, 0.25, 1.0))) as f32;
        let reference = settled_peak(&run(&p, &sine(50.0, 0.25, 1.0))) as f32;
        let db = 20.0 * (at_cutoff / reference).log10();
        assert!((-3.8..=-2.2).contains(&db), "{db} dB");
    }

    #[test]
    fn silence_in_silence_out() {
        let out = run(&Preset::Game.params(), &vec![0.0; 48_000 * 2]);
        assert!(out.iter().all(|&s| s == 0));
    }

    #[test]
    fn gate_mutes_low_level_noise() {
        // -70 dBFS hum is under the -48 dB gate.
        let out = run(&Preset::Game.params(), &sine(60.0, 0.0003, 1.0));
        assert!(out.iter().all(|&s| s == 0));
    }

    #[test]
    fn curve_lifts_quiet_content() {
        let quiet = sine(60.0, 0.02, 1.0);
        let linear = settled_peak(&run(&plain(), &quiet));
        let lifted = settled_peak(&run(&Params { curve: 0.7, ..plain() }, &quiet));
        assert!(lifted > linear * 2, "lifted {lifted} linear {linear}");
    }

    #[test]
    fn bass_boost_lifts_sub_bass_only() {
        let p = Params { bass_boost_db: 6.0, bass_boost_hz: 70.0, ..plain() };
        let db = |hz: f32| {
            let a = settled_peak(&run(&p, &sine(hz, 0.1, 1.0))) as f32;
            let b = settled_peak(&run(&plain(), &sine(hz, 0.1, 1.0))) as f32;
            20.0 * (a / b).log10()
        };
        assert!((4.5..=6.5).contains(&db(30.0)), "30 Hz {}", db(30.0));
        assert!(db(140.0) < 1.5, "140 Hz {}", db(140.0));
    }

    #[test]
    fn background_reject_ducks_steady_sound_but_not_its_onset() {
        // 1 s of silence, then a steady 60 Hz tone for 5 s.
        let mut input = vec![0.0f32; 48_000 * 2];
        input.extend(sine(60.0, 0.2, 5.0));
        let p = Params {
            background_reject: 1.0,
            background_rise_db_s: 20.0,
            gate_db: -48.0,
            ..plain()
        };
        let out = run(&p, &input);
        let peak = |from: f32, to: f32| {
            let r = (from * 3000.0) as usize * 2..(to * 3000.0) as usize * 2;
            out[r].chunks(2).map(|f| (f[0] as i32).abs()).max().unwrap()
        };
        let onset = peak(1.05, 1.25);
        let settled = peak(5.0, 6.0);
        assert!(onset >= 18, "onset {onset}"); // near the unducked 25
        assert!(settled * 5 < onset, "settled {settled} onset {onset}");
        // With rejection off the tone stays put.
        let flat = run(&plain(), &input);
        let late = flat[flat.len() - 6000..].chunks(2).map(|f| (f[0] as i32).abs()).max().unwrap();
        assert!(late >= 22, "late {late}");
    }

    #[test]
    fn volume_threshold_ignores_quiet_sounds() {
        let p = Params { volume_threshold_db: -20.0, ..plain() };
        // -30 dBFS: under the threshold, nothing. -10 dBFS: over it, untouched.
        assert!(run(&p, &sine(60.0, 0.03, 1.0)).iter().all(|&s| s == 0));
        let loud = settled_peak(&run(&p, &sine(60.0, 0.3, 1.0)));
        let reference = settled_peak(&run(&plain(), &sine(60.0, 0.3, 1.0)));
        assert_eq!(loud, reference);
        // Off by default.
        assert!(settled_peak(&run(&plain(), &sine(60.0, 0.03, 1.0))) >= 3);
    }

    #[test]
    fn response_curve_matches_processing() {
        // The plotted curve must be what a steady tone actually produces.
        let p = Params { stereo: false, background_reject: 0.0, bass_boost_db: 0.0, transient: false, ..Preset::Game.params() };
        let curve = response_curve(&p);
        for db in [-40.0f32, -24.0, -12.0, -3.0] {
            let amp = 10f32.powf(db / 20.0);
            let got = settled_peak(&run(&p, &sine(50.0, amp, 1.5))) as f32 / 127.0;
            let want = curve.iter().find(|c| c.0 == db).unwrap().1;
            assert!((got - want).abs() < 0.06, "{db} dB: got {got} want {want}");
        }
        assert_eq!(curve[0].1, 0.0); // -60 dB is under the gate
    }

    #[test]
    fn never_exceeds_ceiling() {
        let p = Params { intensity: 4.0, ceiling: 0.5, ..Preset::Game.params() };
        let out = run(&p, &sine(60.0, 1.0, 1.0));
        let peak = out.iter().map(|&s| (s as i32).abs()).max().unwrap();
        assert!(peak <= 64, "peak {peak}");
    }

    #[test]
    fn click_triggers_pulse_only_when_enabled() {
        // A 2 ms burst of 4 kHz: nothing for the low-pass, an onset for the detector.
        let mut input = vec![0.0f32; 48_000 * 2];
        for n in 0..96 {
            let v = 0.8 * (TAU * 4000.0 * n as f32 / INPUT_RATE).sin();
            input[(24_000 + n) * 2] = v;
            input[(24_000 + n) * 2 + 1] = v;
        }
        let on = run(&Preset::Game.params(), &input);
        let off = run(&Params { transient: false, ..Preset::Game.params() }, &input);
        let peak = |o: &[i8]| o.iter().map(|&s| (s as i32).abs()).max().unwrap();
        assert!(peak(&on) > 20, "pulse peak {}", peak(&on));
        assert!(peak(&off) <= 1, "leak {}", peak(&off));
        // The pulse starts within 5 ms of the click (1500 output frames = 0.5 s).
        let first = on.chunks(2).position(|f| f[0] != 0).unwrap();
        assert!((1500..1516).contains(&first), "first {first}");
    }

    #[test]
    fn quiet_blip_gets_a_firm_bright_tick() {
        // 30 ms menu blip at 3 kHz, only 10 % of full scale, after silence.
        let blip = |hz: f32, amp: f32| -> Vec<f32> {
            let mut input = vec![0.0f32; 48_000 * 2];
            for n in 0..1440 {
                let v = amp * (TAU * hz * n as f32 / INPUT_RATE).sin();
                input[(24_000 + n) * 2] = v;
                input[(24_000 + n) * 2 + 1] = v;
            }
            input
        };
        let p = Params { stereo: false, ..Preset::Game.params() };
        let out = run(&p, &blip(3000.0, 0.1));
        let peak = out.iter().map(|&s| (s as i32).abs()).max().unwrap();
        assert!(peak >= 50, "peak {peak}"); // was about 30 with linear scaling

        // Bright blip ticks faster than a dull one: count zero crossings in 20 ms.
        let crossings = |o: &[i8]| {
            let first = o.chunks(2).position(|f| f[0] != 0).unwrap();
            let mono: Vec<i8> = o.chunks(2).skip(first).take(60).map(|f| f[0]).collect();
            mono.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count()
        };
        let bright = crossings(&out);
        let dull = crossings(&run(&p, &blip(500.0, 0.1)));
        assert!(bright > dull + 2, "bright {bright} dull {dull}");
    }

    #[test]
    fn stereo_pans_by_the_broadband_image() {
        // Centred 60 Hz bass plus a 1 kHz component that is `l` / `r` loud per side:
        // the bass has no direction of its own, the 1 kHz part gives it one.
        let mix = |l: f32, r: f32| -> Vec<f32> {
            (0..48_000)
                .flat_map(|n| {
                    let t = n as f32 / INPUT_RATE;
                    let bass = 0.4 * (TAU * 60.0 * t).sin();
                    let cue = (TAU * 1000.0 * t).sin();
                    [bass + l * cue, bass + r * cue]
                })
                .collect()
        };
        let side = |o: &[i8], i: usize| {
            o[o.len() / 2..].chunks(2).map(|f| (f[i] as i32).abs()).max().unwrap()
        };
        let st = |width: f32| Params { stereo: true, stereo_width: width, ..plain() };

        let mono = run(&plain(), &mix(0.3, 0.0));
        assert_eq!(side(&mono, 0), side(&mono, 1));

        let hard_left = run(&st(1.0), &mix(0.3, 0.0));
        assert!(side(&hard_left, 0) > 40 && side(&hard_left, 1) <= 1);

        let centred = run(&st(1.5), &mix(0.2, 0.2));
        assert!((side(&centred, 0) - side(&centred, 1)).abs() <= 1);

        // 3:1 towards the right is pan +0.5: half strength on the left at width 1,
        // nothing on the left at width 2.
        let w1 = run(&st(1.0), &mix(0.1, 0.3));
        let ratio = side(&w1, 0) as f32 / side(&w1, 1) as f32;
        assert!((0.4..0.6).contains(&ratio), "ratio {ratio}");
        let w2 = run(&st(2.0), &mix(0.1, 0.3));
        assert!(side(&w2, 0) <= 1 && side(&w2, 1) > 40);
    }

    #[test]
    fn rumble_bands_split() {
        let level = |hz: f32| {
            let mut p = Processor::new(&Preset::Game.params());
            p.process(&sine(hz, 0.02, 1.0), &mut Vec::new());
            p.rumble_levels()
        };
        let (sub, mid) = level(45.0);
        assert!(sub > mid * 2.0, "45 Hz: sub {sub} mid {mid}");
        let (sub, mid) = level(160.0);
        assert!(mid > sub * 2.0, "160 Hz: sub {sub} mid {mid}");
    }

    #[test]
    fn non_finite_input_does_not_stick() {
        let mut p = Processor::new(&plain());
        let mut out = Vec::new();
        p.process(&[f32::NAN, f32::INFINITY], &mut out);
        out.clear();
        p.process(&sine(60.0, 0.5, 1.0), &mut out);
        assert!(settled_peak(&out) > 50);
    }

    #[test]
    fn default_lowpass_delay_is_a_few_ms() {
        let d = lowpass_delay_ms(150.0);
        assert!((2.0..4.0).contains(&d), "{d}");
    }
}
