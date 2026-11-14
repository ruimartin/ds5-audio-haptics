/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use ds5_audio_haptics::config::Config;
use ds5_audio_haptics::{control, service, web};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
ds5-audio-haptics: DualSense HD haptics from system audio

  ds5-audio-haptics run [--config PATH]   run the service (what the systemd unit does)
  ds5-audio-haptics ui                    open the settings page in your browser
  ds5-audio-haptics status                show state, settings and timing
  ds5-audio-haptics set KEY=VALUE ...     change settings on the running service
  ds5-audio-haptics save                  write the running settings to the config file
  ds5-audio-haptics default-config        print a commented config file
  ds5-audio-haptics render [--config PATH] <in.f32 >out.s8
                                          offline: 48 kHz stereo f32 in, 3 kHz stereo s8 out

Keys for `set`: enabled, preset (game|music), mode (auto|hd|rumble), lan (true|false:
settings page reachable from other devices on your network), and any [dsp]
key such as intensity, lowpass_hz, stereo, transient. VALUE `default` returns a
[dsp] key to the preset.
";

const DEFAULT_CONFIG: &str = include_str!("../../config.example.toml");

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("run") => {
            let path = match args.get(1).map(String::as_str) {
                Some("--config") => PathBuf::from(args.get(2).ok_or("--config needs a path")?),
                Some(other) => return Err(format!("unknown option '{other}'")),
                None => Config::default_path(),
            };
            let config = Config::load(&path)?;
            eprintln!("config: {} (preset {:?}, mode {:?})", path.display(), config.preset, config.mode);
            let shared = service::Shared::new(config, path);
            control::start(shared.clone())?;
            web::start(shared.clone());
            service::run(shared)
        }
        Some("ui") => {
            let status = control::request(&json!({"cmd": "status"}))?;
            let url = status["ui"].as_str().ok_or("the settings page is disabled ([ui] enabled = false)")?;
            println!("{url}");
            open_ui(url)
        }
        Some("status") => print(control::request(&json!({"cmd": "status"}))?),
        Some("save") => print(control::request(&json!({"cmd": "save"}))?),
        Some("set") => {
            let mut req = json!({"cmd": "set"});
            let mut dsp = serde_json::Map::new();
            for pair in &args[1..] {
                let (key, text) = pair.split_once('=').ok_or(format!("expected KEY=VALUE, got '{pair}'"))?;
                // Numbers and booleans parse as JSON; anything else is a string.
                let value = match text {
                    "default" => Value::Null,
                    _ => serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.into())),
                };
                match key {
                    "enabled" | "preset" | "mode" | "lan" => req[key] = value,
                    _ => drop(dsp.insert(key.into(), value)),
                }
            }
            if !dsp.is_empty() {
                req["dsp"] = Value::Object(dsp);
            }
            print(control::request(&req)?)
        }
        Some("render") => {
            use std::io::{Read, Write};
            let config = match args.get(1).map(String::as_str) {
                Some("--config") => Config::load(&PathBuf::from(args.get(2).ok_or("--config needs a path")?))?,
                _ => Config::default(),
            };
            let mut raw = Vec::new();
            std::io::stdin().read_to_end(&mut raw).map_err(|e| e.to_string())?;
            let audio: Vec<f32> =
                raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
            let mut out = Vec::new();
            ds5_audio_haptics::dsp::Processor::new(&config.params()).process(&audio, &mut out);
            let bytes: Vec<u8> = out.iter().map(|&s| s as u8).collect();
            std::io::stdout().write_all(&bytes).map_err(|e| e.to_string())
        }
        Some("default-config") => {
            print!("{DEFAULT_CONFIG}");
            Ok(())
        }
        _ => Err(String::new()),
    }
}

/// Opens the settings page. On a desktop this hands the URL to the default browser.
/// Inside Steam's Gaming Mode (gamescope) it runs a browser full screen and waits
/// for it, so the page can be added to Steam as a non-Steam game.
fn open_ui(url: &str) -> Result<(), String> {
    use std::process::Command;
    if std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_none() {
        // Best effort; the printed URL is there to copy if no browser opens.
        let _ = Command::new("xdg-open").arg(url).status();
        return Ok(());
    }
    let home = std::env::var("HOME").map_err(|_| "HOME is not set")?;
    // (flatpak id or program, is_flatpak, is_firefox)
    let browsers = [
        ("org.mozilla.firefox", true, true),
        ("com.google.Chrome", true, false),
        ("org.chromium.Chromium", true, false),
        ("com.brave.Browser", true, false),
        ("firefox", false, true),
        ("chromium", false, false),
        ("google-chrome", false, false),
    ];
    for (name, flatpak, firefox) in browsers {
        let present = if flatpak {
            Command::new("flatpak").args(["info", name]).output().map(|o| o.status.success()).unwrap_or(false)
        } else {
            Command::new("sh").args(["-c", &format!("command -v {name}")]).output().map(|o| o.status.success()).unwrap_or(false)
        };
        if !present {
            continue;
        }
        // A profile of its own: the browser must not hand the page to an instance
        // that is already running and exit, or Steam would think the "game" quit.
        // For a flatpak it has to live where the sandbox can see it.
        let profile = if flatpak {
            format!("{home}/.var/app/{name}/ds5-audio-haptics")
        } else {
            format!("{home}/.cache/ds5-audio-haptics/browser")
        };
        std::fs::create_dir_all(&profile).map_err(|e| format!("{profile}: {e}"))?;
        let mut cmd = if flatpak {
            let mut c = Command::new("flatpak");
            c.args(["run", name]);
            c
        } else {
            Command::new(name)
        };
        if firefox {
            // Skip the first-run pages and prompts a fresh profile would show.
            let prefs = "user_pref(\"browser.aboutwelcome.enabled\", false);\n\
                user_pref(\"browser.shell.checkDefaultBrowser\", false);\n\
                user_pref(\"datareporting.policy.dataSubmissionEnabled\", false);\n\
                user_pref(\"browser.startup.homepage_override.mstone\", \"ignore\");\n\
                user_pref(\"toolkit.telemetry.reportingpolicy.firstRun\", false);\n";
            std::fs::write(format!("{profile}/user.js"), prefs).map_err(|e| format!("{profile}: {e}"))?;
            cmd.args(["--kiosk", "--new-instance", "--profile", &profile, url]);
        } else {
            cmd.args(["--kiosk", "--no-first-run", "--no-default-browser-check"])
                .arg(format!("--user-data-dir={profile}"))
                .arg(url);
        }
        return cmd.status().map(|_| ()).map_err(|e| format!("{name}: {e}"));
    }
    Err("no browser found: install Firefox from Discover in Desktop Mode".into())
}

fn print(reply: Value) -> Result<(), String> {
    if reply["ok"] == true {
        println!("{}", serde_json::to_string_pretty(&reply).unwrap());
        Ok(())
    } else {
        Err(reply["error"].as_str().unwrap_or("request failed").to_string())
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.is_empty() => {
            print!("{USAGE}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("ds5-audio-haptics: {e}");
            ExitCode::FAILURE
        }
    }
}
