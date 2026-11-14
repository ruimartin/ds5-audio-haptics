/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! TOML configuration. Every key is optional; `[dsp]` keys override the preset.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// HD haptics when a Bluetooth DualSense is writable, otherwise rumble.
    #[default]
    Auto,
    Hd,
    Rumble,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    #[default]
    Game,
    Music,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub mode: Mode,
    pub preset: Preset,
    /// Controller MAC address; empty picks the first Bluetooth DualSense.
    pub device: String,
    pub capture: Capture,
    pub dsp: DspOverrides,
    pub output: Output,
    pub ui: Ui,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: Mode::Auto,
            preset: Preset::Game,
            device: String::new(),
            capture: Capture::default(),
            dsp: DspOverrides::default(),
            output: Output::default(),
            ui: Ui::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Capture {
    /// PipeWire node name or serial to capture; empty follows the default sink.
    pub target: String,
    /// Requested latency in frames at 48 kHz. Smaller means lower delay and more CPU wake-ups.
    pub latency_frames: u32,
}

impl Default for Capture {
    fn default() -> Self {
        Self { target: String::new(), latency_frames: 256 }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Output {
    /// Buffer-length byte of the control sub-packet (16..127).
    pub buffer_len: u8,
    /// Stop sending reports after this many seconds of silence.
    pub idle_stop_secs: f32,
    /// Reports allowed to queue before the oldest are dropped.
    pub max_backlog: usize,
    /// Rumble fallback update rate.
    pub rumble_rate_hz: f32,
}

impl Default for Output {
    fn default() -> Self {
        Self { buffer_len: 48, idle_stop_secs: 2.0, max_backlog: 3, rumble_rate_hz: 30.0 }
    }
}

/// The settings page.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ui {
    pub enabled: bool,
    pub port: u16,
    /// false: reachable from this machine only. true: also from other devices on the
    /// local network (phone, laptop), without a password.
    pub lan: bool,
    /// Name under which the page is announced on the local network (without
    /// `.local`). Empty picks `dsconfig-<hostname>`, unique per machine.
    pub mdns_name: String,
}

impl Default for Ui {
    fn default() -> Self {
        Self { enabled: true, port: 5757, lan: false, mdns_name: String::new() }
    }
}

impl Ui {
    /// The full `.local` host name, lower case, safe as a DNS label.
    pub fn mdns_host(&self) -> String {
        let clean = |text: &str| -> String {
            let s: String = text
                .trim()
                .trim_end_matches(".local")
                .to_ascii_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            s.trim_matches('-').chars().take(60).collect()
        };
        let mut label = clean(&self.mdns_name);
        if label.is_empty() {
            let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
            label = match clean(&host).as_str() {
                "" => "dsconfig".to_string(),
                h => format!("dsconfig-{h}"),
            };
        }
        format!("{label}.local")
    }
}

/// Resolved DSP settings. See `Preset::params` for the defaults.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Params {
    /// Master gain applied before the dynamics stage.
    pub intensity: f32,
    /// Pan the haptics between the grips following where the sound is in the stereo image.
    pub stereo: bool,
    /// How far a given left/right imbalance moves the haptics: 1 is proportional,
    /// 2 puts a sound that is 75/25 to one side entirely in that grip.
    pub stereo_width: f32,
    pub lowpass_hz: f32,
    pub highpass_hz: f32,
    /// Low-shelf boost below `bass_boost_hz`, to make sub-bass felt as strongly as mid-bass.
    pub bass_boost_db: f32,
    pub bass_boost_hz: f32,
    /// 0..1: how strongly steady content (background music, engine drone) is turned
    /// down relative to sounds that stand out from it.
    pub background_reject: f32,
    /// How fast the background estimate may climb, in dB per second. Low values let a
    /// long firefight stay strong; high values make sustained sounds fade sooner.
    pub background_rise_db_s: f32,
    /// How quickly the estimate drops back when things go quiet again.
    pub background_fall_ms: f32,
    /// Sounds whose overall volume is below this are ignored entirely (dBFS of the
    /// full mix, not just the bass). -90 turns it off. Useful when background music
    /// is quieter than the effects.
    pub volume_threshold_db: f32,
    /// Noise gate on the bass band: below this level the output is muted.
    pub gate_db: f32,
    /// Level exponent: below 1 lifts quiet content, 1 is linear.
    pub curve: f32,
    pub comp_threshold_db: f32,
    pub comp_ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_db: f32,
    /// Output ceiling, 0..1 of full scale.
    pub ceiling: f32,
    pub transient: bool,
    /// Onsets are detected above this frequency.
    pub transient_highpass_hz: f32,
    pub transient_sensitivity: f32,
    pub transient_gain: f32,
    /// Pulse frequency for dull hits (thuds, explosions).
    pub transient_pulse_hz: f32,
    /// Pulse frequency for bright hits (menu blips, clicks, glass): felt as a sharp tick.
    pub transient_click_hz: f32,
    pub transient_decay_ms: f32,
    /// After an onset, background rejection is lifted for about this long, so the
    /// bass body of a gunshot or explosion comes through even over loud music.
    pub transient_open_ms: f32,
}

macro_rules! overrides {
    ($($field:ident: $ty:ty),* $(,)?) => {
        /// Optional per-key overrides of the preset.
        #[derive(Clone, Debug, Default, Deserialize, Serialize)]
        #[serde(default, deny_unknown_fields)]
        pub struct DspOverrides {
            $(#[serde(skip_serializing_if = "Option::is_none")] pub $field: Option<$ty>,)*
        }
        impl DspOverrides {
            fn apply(&self, p: &mut Params) {
                $(if let Some(v) = self.$field { p.$field = v; })*
            }
        }
    };
}

overrides! {
    intensity: f32, stereo: bool, stereo_width: f32, lowpass_hz: f32, highpass_hz: f32,
    bass_boost_db: f32, bass_boost_hz: f32, background_reject: f32, background_rise_db_s: f32,
    background_fall_ms: f32,
    volume_threshold_db: f32, gate_db: f32, curve: f32, comp_threshold_db: f32, comp_ratio: f32, attack_ms: f32,
    release_ms: f32, makeup_db: f32, ceiling: f32, transient: bool,
    transient_highpass_hz: f32, transient_sensitivity: f32, transient_gain: f32,
    transient_pulse_hz: f32, transient_click_hz: f32, transient_decay_ms: f32, transient_open_ms: f32,
}

impl Preset {
    pub fn params(self) -> Params {
        // Game: punchy. Quiet rumble is lifted, impacts get an added pulse.
        let game = Params {
            intensity: 1.0,
            stereo: true,
            stereo_width: 1.5,
            lowpass_hz: 150.0,
            highpass_hz: 20.0,
            bass_boost_db: 6.0,
            bass_boost_hz: 70.0,
            background_reject: 0.9,
            background_rise_db_s: 3.0,
            background_fall_ms: 1000.0,
            volume_threshold_db: -90.0,
            gate_db: -48.0,
            curve: 0.85,
            comp_threshold_db: -6.0,
            comp_ratio: 2.0,
            attack_ms: 4.0,
            release_ms: 120.0,
            makeup_db: 8.0,
            ceiling: 1.0,
            transient: true,
            transient_highpass_hz: 300.0,
            transient_sensitivity: 1.0,
            transient_gain: 0.85,
            transient_pulse_hz: 90.0,
            transient_click_hz: 200.0,
            transient_decay_ms: 35.0,
            transient_open_ms: 250.0,
        };
        match self {
            Preset::Game => game,
            // Music: follows the bass line faithfully, gentler dynamics, lighter pulses.
            Preset::Music => Params {
                stereo: false,
                lowpass_hz: 110.0,
                highpass_hz: 25.0,
                bass_boost_db: 3.0,
                background_reject: 0.0,
                gate_db: -54.0,
                curve: 0.9,
                comp_threshold_db: -10.0,
                comp_ratio: 2.0,
                release_ms: 180.0,
                transient_sensitivity: 0.6,
                transient_gain: 0.3,
                ..game
            },
        }
    }
}

impl Config {
    /// Preset with overrides applied and every value forced into a safe range.
    pub fn params(&self) -> Params {
        let mut p = self.preset.params();
        self.dsp.apply(&mut p);
        p.intensity = p.intensity.clamp(0.0, 4.0);
        p.stereo_width = p.stereo_width.clamp(0.0, 3.0);
        p.lowpass_hz = p.lowpass_hz.clamp(30.0, 800.0);
        p.highpass_hz = p.highpass_hz.clamp(5.0, p.lowpass_hz * 0.5);
        p.bass_boost_db = p.bass_boost_db.clamp(0.0, 18.0);
        p.bass_boost_hz = p.bass_boost_hz.clamp(30.0, 200.0);
        p.background_reject = p.background_reject.clamp(0.0, 1.0);
        p.background_rise_db_s = p.background_rise_db_s.clamp(0.2, 60.0);
        p.background_fall_ms = p.background_fall_ms.clamp(200.0, 30_000.0);
        p.volume_threshold_db = p.volume_threshold_db.clamp(-90.0, 0.0);
        p.gate_db = p.gate_db.clamp(-90.0, -10.0);
        p.curve = p.curve.clamp(0.3, 1.5);
        p.comp_threshold_db = p.comp_threshold_db.clamp(-60.0, 0.0);
        p.comp_ratio = p.comp_ratio.clamp(1.0, 20.0);
        p.attack_ms = p.attack_ms.clamp(0.5, 100.0);
        p.release_ms = p.release_ms.clamp(10.0, 2000.0);
        p.makeup_db = p.makeup_db.clamp(-24.0, 24.0);
        p.ceiling = p.ceiling.clamp(0.05, 1.0);
        p.transient_highpass_hz = p.transient_highpass_hz.clamp(50.0, 8000.0);
        p.transient_sensitivity = p.transient_sensitivity.clamp(0.0, 4.0);
        p.transient_gain = p.transient_gain.clamp(0.0, 1.0);
        p.transient_pulse_hz = p.transient_pulse_hz.clamp(30.0, 400.0);
        p.transient_click_hz = p.transient_click_hz.clamp(60.0, 600.0);
        p.transient_decay_ms = p.transient_decay_ms.clamp(5.0, 200.0);
        p.transient_open_ms = p.transient_open_ms.clamp(0.0, 2000.0);
        p
    }

    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("ds5-audio-haptics/config.toml")
    }

    /// A missing file yields the defaults; a malformed one is an error.
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_defaults() {
        let c: Config = toml::from_str("").unwrap();
        assert!(c.enabled);
        assert_eq!(c.params(), Preset::Game.params());
    }

    #[test]
    fn overrides_win_over_preset_and_are_clamped() {
        let c: Config =
            toml::from_str("preset = \"music\"\n[dsp]\nlowpass_hz = 5000\nstereo = true\n")
                .unwrap();
        let p = c.params();
        assert_eq!(p.lowpass_hz, 800.0);
        assert!(p.stereo); // preset says false, the override wins
        assert_eq!(p.curve, Preset::Music.params().curve);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(toml::from_str::<Config>("[dsp]\nlowpas_hz = 100\n").is_err());
    }

    #[test]
    fn mdns_name_is_sanitised() {
        let ui = |name: &str| Ui { mdns_name: name.into(), ..Ui::default() }.mdns_host();
        assert_eq!(ui("dsconfig"), "dsconfig.local");
        assert_eq!(ui("DSConfig.local"), "dsconfig.local");
        assert_eq!(ui(" my deck! "), "my-deck.local");
        assert!(ui("").starts_with("dsconfig"));
        assert!(ui("").ends_with(".local"));
    }

    #[test]
    fn round_trips() {
        let mut c = Config::default();
        c.dsp.intensity = Some(0.5);
        let back: Config = toml::from_str(&toml::to_string_pretty(&c).unwrap()).unwrap();
        assert_eq!(back.params(), c.params());
    }
}
