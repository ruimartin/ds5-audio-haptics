/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Control socket: one JSON object per line in, one JSON object per line out.
//!
//! ```text
//! {"cmd":"status"}
//! {"cmd":"set","enabled":true,"preset":"music","mode":"auto","dsp":{"intensity":0.8,"lowpass_hz":null}}
//! {"cmd":"save"}
//! ```
//! In `dsp`, a value overrides the preset and `null` returns that key to the preset.

use crate::config::{Config, DspOverrides};
use crate::dsp::lowpass_delay_ms;
use crate::service::Shared;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;

pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| "/tmp".into());
    dir.join("ds5-audio-haptics.sock")
}

pub fn status(shared: &Shared) -> Value {
    let cfg = shared.config();
    let params = cfg.params();
    json!({
        "ok": true,
        "enabled": cfg.enabled,
        "mode": cfg.mode,
        "preset": cfg.preset,
        "overrides": cfg.dsp,
        "filter_delay_ms": lowpass_delay_ms(params.lowpass_hz),
        "params": params,
        "capture": *shared.capture.lock().unwrap(),
        "output": *shared.sink.lock().unwrap(),
        "reports_sent": shared.reports_sent.load(Relaxed),
        "reports_dropped": shared.reports_dropped.load(Relaxed),
        "write_errors": shared.write_errors.load(Relaxed),
        "process_latency_ms": {
            "avg": shared.latency_avg_us.load(Relaxed) as f64 / 1000.0,
            "max": shared.latency_max_us.load(Relaxed) as f64 / 1000.0,
        },
    })
}

/// Applies a `set` request to a copy of the config; nothing changes on error.
fn apply_set(cfg: &Config, req: &Value) -> Result<Config, String> {
    let mut new = cfg.clone();
    let obj = req.as_object().ok_or("request must be an object")?;
    for (key, value) in obj {
        let bad = |e: serde_json::Error| format!("{key}: {e}");
        match key.as_str() {
            "cmd" => {}
            "enabled" => new.enabled = serde_json::from_value(value.clone()).map_err(bad)?,
            "preset" => new.preset = serde_json::from_value(value.clone()).map_err(bad)?,
            "mode" => new.mode = serde_json::from_value(value.clone()).map_err(bad)?,
            "dsp" => {
                let mut merged = serde_json::to_value(&new.dsp).map_err(bad)?;
                let changes = value.as_object().ok_or("dsp must be an object")?;
                for (k, v) in changes {
                    if v.is_null() {
                        merged.as_object_mut().unwrap().remove(k);
                    } else {
                        merged[k] = v.clone();
                    }
                }
                // Unknown keys and wrong types are rejected here.
                new.dsp = serde_json::from_value::<DspOverrides>(merged).map_err(bad)?;
            }
            other => return Err(format!("unknown key '{other}'")),
        }
    }
    Ok(new)
}

fn handle(shared: &Shared, line: &str) -> Value {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return json!({"ok": false, "error": format!("bad JSON: {e}")}),
    };
    let result = match req.get("cmd").and_then(Value::as_str) {
        Some("status") => return status(shared),
        Some("set") => {
            let mut cfg = shared.config.lock().unwrap();
            apply_set(&cfg, &req).map(|new| {
                *cfg = new;
                shared.generation.fetch_add(1, Relaxed);
            })
        }
        Some("save") => shared.config().save(&shared.config_path),
        Some("reset_stats") => {
            shared.latency_max_us.store(0, Relaxed);
            shared.latency_avg_us.store(0, Relaxed);
            Ok(())
        }
        _ => Err("cmd must be status, set, save or reset_stats".to_string()),
    };
    match result {
        Ok(()) => status(shared),
        Err(e) => json!({"ok": false, "error": e}),
    }
}

fn serve(shared: &Shared, stream: UnixStream) {
    let Ok(mut writer) = stream.try_clone() else { return };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let mut reply = handle(shared, &line).to_string();
        reply.push('\n');
        if writer.write_all(reply.as_bytes()).is_err() {
            return;
        }
    }
}

/// Binds the socket and serves clients on background threads.
pub fn start(shared: Arc<Shared>) -> Result<(), String> {
    let path = socket_path();
    // A live service answers on the socket; only a stale file may be replaced.
    if UnixStream::connect(&path).is_ok() {
        return Err(format!("another instance is running ({})", path.display()));
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    std::thread::Builder::new()
        .name("control".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = shared.clone();
                let _ = std::thread::Builder::new()
                    .name("client".into())
                    .spawn(move || serve(&shared, stream));
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Client side: send one request, return the reply.
pub fn request(req: &Value) -> Result<Value, String> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path)
        .map_err(|e| format!("service not reachable at {}: {e}", path.display()))?;
    stream.write_all(format!("{req}\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).map_err(|e| e.to_string())?;
    serde_json::from_str(&line).map_err(|e| format!("bad reply: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Preset;

    #[test]
    fn set_merges_and_null_resets() {
        let cfg = Config::default();
        let a = apply_set(&cfg, &json!({"cmd":"set","preset":"music","dsp":{"intensity":0.5,"lowpass_hz":90}})).unwrap();
        assert_eq!(a.preset, Preset::Music);
        assert_eq!(a.params().intensity, 0.5);
        assert_eq!(a.params().lowpass_hz, 90.0);
        let b = apply_set(&a, &json!({"cmd":"set","dsp":{"lowpass_hz":null}})).unwrap();
        assert_eq!(b.params().lowpass_hz, Preset::Music.params().lowpass_hz);
        assert_eq!(b.params().intensity, 0.5);
    }

    #[test]
    fn set_rejects_bad_input() {
        let cfg = Config::default();
        assert!(apply_set(&cfg, &json!({"dsp":{"nope":1}})).is_err());
        assert!(apply_set(&cfg, &json!({"dsp":{"intensity":"loud"}})).is_err());
        assert!(apply_set(&cfg, &json!({"preset":"disco"})).is_err());
        assert!(apply_set(&cfg, &json!({"volume":1})).is_err());
    }
}
