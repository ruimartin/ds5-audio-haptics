/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Test tool: pulse the adaptive triggers of a Bluetooth DualSense.

use ds5_audio_haptics::device::{self, Bus};
use ds5_audio_haptics::protocol::{trigger_report, TriggerEffect};
use std::fs::OpenOptions;
use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

const USAGE: &str = "\
ds5-trigger: pulse the adaptive triggers of a Bluetooth DualSense

  --side S        right | left | both (default right)
  --effect E      vibration | weapon | feedback | off (default vibration)
  --pos N         start zone 0..9 (default 0)
  --amp N         strength 1..8 (default 8)
  --freq HZ       vibration frequency (default 40)
  --ms N          how long each pulse stays on (default 1000)
  --count N       number of pulses (default 1)
  --gap-ms N      pause between pulses (default 400)

The triggers are always switched off again at the end.
";

fn run() -> Result<(), String> {
    let (mut side, mut effect) = ("right".to_string(), "vibration".to_string());
    let (mut pos, mut amp, mut freq, mut ms, mut count, mut gap) = (0u8, 8u8, 40u8, 1000u64, 1u32, 400u64);
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| if flag == "-h" || flag == "--help" { String::new() } else { format!("{flag} needs a value") })?;
        let bad = |_| format!("bad value for {flag}");
        match flag.as_str() {
            "--side" => side = value,
            "--effect" => effect = value,
            "--pos" => pos = value.parse().map_err(bad)?,
            "--amp" => amp = value.parse().map_err(bad)?,
            "--freq" => freq = value.parse().map_err(bad)?,
            "--ms" => ms = value.parse::<u64>().map_err(bad)?.min(20_000),
            "--count" => count = value.parse::<u32>().map_err(bad)?.min(200),
            "--gap-ms" => gap = value.parse::<u64>().map_err(bad)?.min(10_000),
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    let on = match effect.as_str() {
        "vibration" => TriggerEffect::vibration(pos, amp, freq),
        "weapon" => TriggerEffect::weapon(pos.max(2), pos.max(2) + 2, amp),
        "feedback" => TriggerEffect::feedback(pos, amp),
        "off" => TriggerEffect::OFF,
        other => return Err(format!("unknown effect '{other}'")),
    };
    let pick = |e: TriggerEffect| match side.as_str() {
        "right" => Ok((Some(e), None)),
        "left" => Ok((None, Some(e))),
        "both" => Ok((Some(e), Some(e))),
        other => Err(format!("unknown side '{other}'")),
    };
    let (on_r, on_l) = pick(on)?;
    let (off_r, off_l) = pick(TriggerEffect::OFF)?;

    let all = device::scan();
    let c = all.iter().find(|c| c.bus == Bus::Bluetooth).ok_or("no Bluetooth DualSense found")?;
    let mut dev = OpenOptions::new()
        .write(true)
        .open(&c.hidraw)
        .map_err(|e| format!("{}: {e}", c.hidraw.display()))?;
    let mut seq = 0u8;
    let mut send = |r: Option<TriggerEffect>, l: Option<TriggerEffect>| -> Result<(), String> {
        seq = seq.wrapping_add(1);
        dev.write_all(&trigger_report(seq, r, l)).map_err(|e| format!("write failed: {e}"))
    };
    println!("{effect} on {side}: {count} x {ms} ms, gap {gap} ms (pos {pos}, amp {amp}, {freq} Hz)");
    let mut result = Ok(());
    for i in 0..count {
        result = send(on_r, on_l);
        if result.is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(ms));
        result = send(off_r, off_l);
        if result.is_err() || i + 1 == count {
            break;
        }
        std::thread::sleep(Duration::from_millis(gap));
    }
    // Whatever happened, leave the triggers free.
    let _ = send(off_r, off_l);
    result
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.is_empty() => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("ds5-trigger: {e}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}
