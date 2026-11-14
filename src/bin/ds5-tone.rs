/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 1 test tool: play a sine wave on a Bluetooth DualSense's haptic actuators.

use ds5_audio_haptics::device::{self, Bus, Controller};
use ds5_audio_haptics::protocol::{
    ControlForm, ReportBuilder, FRAMES, PAYLOAD, REPORT_PERIOD_NS, SAMPLE_RATE,
};
use std::f64::consts::TAU;
use std::fs::OpenOptions;
use std::io::Write;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const USAGE: &str = "\
ds5-tone: play a test tone on a Bluetooth DualSense's HD haptics

  --list              list detected controllers and exit
  --device PATH       hidraw node (default: first Bluetooth DualSense)
  --freq HZ           tone frequency, 10..1400 (default 60)
  --amp A             amplitude 0..1 (default 0.25)
  --secs S            duration, max 60 (default 3)
  --channel C         both | left | right (default both)
  --ctl FORM          awalol | saxense (default awalol)
  --buflen N          buffer-length byte for the awalol form, 16..127 (default 48)
  --dry-run           build and print the first report, write nothing
";

struct Args {
    list: bool,
    device: Option<String>,
    freq: f64,
    amp: f64,
    secs: f64,
    left: bool,
    right: bool,
    form: ControlForm,
    buflen: u8,
    dry_run: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        list: false,
        device: None,
        freq: 60.0,
        amp: 0.25,
        secs: 3.0,
        left: true,
        right: true,
        form: ControlForm::Awalol,
        buflen: 48,
        dry_run: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--list" => a.list = true,
            "--dry-run" => a.dry_run = true,
            "--device" => a.device = Some(value()?),
            "--freq" => a.freq = value()?.parse().map_err(|_| "bad --freq")?,
            "--amp" => a.amp = value()?.parse().map_err(|_| "bad --amp")?,
            "--secs" => a.secs = value()?.parse().map_err(|_| "bad --secs")?,
            "--buflen" => a.buflen = value()?.parse().map_err(|_| "bad --buflen")?,
            "--channel" => match value()?.as_str() {
                "both" => (a.left, a.right) = (true, true),
                "left" => (a.left, a.right) = (true, false),
                "right" => (a.left, a.right) = (false, true),
                other => return Err(format!("unknown channel '{other}'")),
            },
            "--ctl" => match value()?.as_str() {
                "awalol" => a.form = ControlForm::Awalol,
                "saxense" => a.form = ControlForm::Saxense,
                other => return Err(format!("unknown control form '{other}'")),
            },
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    if !(10.0..=1400.0).contains(&a.freq) {
        return Err("--freq must be 10..1400".into());
    }
    if !(0.0..=1.0).contains(&a.amp) {
        return Err("--amp must be 0..1".into());
    }
    if !(0.1..=60.0).contains(&a.secs) {
        return Err("--secs must be 0.1..60".into());
    }
    if !(16..=127).contains(&a.buflen) {
        return Err("--buflen must be 16..127".into());
    }
    Ok(a)
}

fn describe(c: &Controller) -> String {
    format!(
        "{}  {:?}  {}{}  {}",
        c.hidraw.display(),
        c.bus,
        c.name,
        if c.is_edge() { " (Edge)" } else { "" },
        c.uniq
    )
}

/// Fills one report's worth of samples starting at frame index `n0`.
fn fill(args: &Args, n0: u64, total: u64, out: &mut [i8; PAYLOAD]) {
    // 50 ms raised-cosine fade at both ends so the tone does not click.
    let fade = (SAMPLE_RATE as f64 * 0.05) as u64;
    for i in 0..FRAMES {
        let n = n0 + i as u64;
        let edge = n.min(total.saturating_sub(n));
        let env = if n >= total {
            0.0
        } else if edge < fade {
            0.5 - 0.5 * (TAU * 0.5 * edge as f64 / fade as f64).cos()
        } else {
            1.0
        };
        let v = (TAU * args.freq * n as f64 / SAMPLE_RATE as f64).sin() * args.amp * env;
        let s = (v * 127.0).round().clamp(-127.0, 127.0) as i8;
        out[i * 2] = if args.left { s } else { 0 };
        out[i * 2 + 1] = if args.right { s } else { 0 };
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let found = device::scan();

    if args.list {
        if found.is_empty() {
            println!("no DualSense found");
        }
        for c in &found {
            println!("{}", describe(c));
        }
        return Ok(());
    }

    let mut builder = ReportBuilder::new(args.form, args.buflen);
    let total = (args.secs * SAMPLE_RATE as f64) as u64;
    let mut samples = [0i8; PAYLOAD];

    if args.dry_run {
        fill(&args, total / 2, total, &mut samples);
        let r = builder.build(&samples);
        let hex: Vec<String> = r.iter().map(|b| format!("{b:02x}")).collect();
        println!("{}", hex.join(" "));
        return Ok(());
    }

    let path = match &args.device {
        Some(p) => p.clone(),
        None => {
            let c = found
                .iter()
                .find(|c| c.bus == Bus::Bluetooth)
                .ok_or("no Bluetooth DualSense found (try --list)")?;
            println!("controller: {}", describe(c));
            c.hidraw.to_string_lossy().into_owned()
        }
    };
    let mut dev = OpenOptions::new()
        .write(true)
        .open(&path)
        .map_err(|e| format!("cannot open {path} for writing: {e}"))?;

    println!(
        "tone: {} Hz, amplitude {}, {} s, form {:?}",
        args.freq, args.amp, args.secs, args.form
    );

    // Tone, then 10 reports of silence so the controller's buffer ends on zero.
    let reports = total.div_ceil(FRAMES as u64) + 10;
    let period = Duration::from_nanos(REPORT_PERIOD_NS);
    let start = Instant::now();
    let (mut worst_late, mut late, mut worst_write) = (Duration::ZERO, 0u64, Duration::ZERO);

    for k in 0..reports {
        // Absolute deadlines: a late wake-up does not shift the following ones.
        let deadline = start + Duration::from_nanos(REPORT_PERIOD_NS * k);
        let now = Instant::now();
        if deadline > now {
            std::thread::sleep(deadline - now);
        }
        let lateness = Instant::now().saturating_duration_since(deadline);
        worst_late = worst_late.max(lateness);
        if lateness > period / 2 {
            late += 1;
        }

        fill(&args, k * FRAMES as u64, total, &mut samples);
        let report = builder.build(&samples);
        let t = Instant::now();
        let n = dev.write(&report).map_err(|e| format!("write failed at report {k}: {e}"))?;
        worst_write = worst_write.max(t.elapsed());
        if n != report.len() {
            return Err(format!("short write: {n} of {} bytes", report.len()));
        }
    }

    println!(
        "sent {reports} reports in {:.2} s; worst wake-up lateness {:.2} ms, \
         {late} later than half a period, slowest write {:.2} ms",
        start.elapsed().as_secs_f64(),
        worst_late.as_secs_f64() * 1e3,
        worst_write.as_secs_f64() * 1e3
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.is_empty() => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("ds5-tone: {e}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}
