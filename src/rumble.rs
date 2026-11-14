/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Fallback: classic rumble through the kernel's evdev force-feedback interface.
//! The kernel driver switches the controller to rumble mode, so this is never
//! mixed with HD haptics.

use crate::device::{PRODUCT_DUALSENSE, PRODUCT_DUALSENSE_EDGE, VENDOR_SONY};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

const EV_FF: u16 = 0x15;
const FF_RUMBLE: u16 = 0x50;
/// _IOW('E', 0x80, struct ff_effect), 48 bytes on 64-bit.
const EVIOCSFF: u32 = 0x4030_4580;
/// Each upload plays this long, so the motors stop by themselves if we die.
const EFFECT_MS: u16 = 250;

/// struct ff_effect with the rumble member of its union.
#[repr(C)]
struct FfEffect {
    kind: u16,
    id: i16,
    direction: u16,
    trigger_button: u16,
    trigger_interval: u16,
    replay_length: u16,
    replay_delay: u16,
    _pad: u16,
    strong: u16,
    weak: u16,
    _rest: [u8; 28],
}

const _: () = assert!(std::mem::size_of::<FfEffect>() == 48);

pub struct Rumble {
    file: File,
    id: i16,
    pub path: PathBuf,
}

/// evdev nodes of DualSense controllers that support force feedback.
pub fn scan() -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/input") else { return found };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str().filter(|n| n.starts_with("event")) else { continue };
        let dev = e.path().join("device");
        let hex = |f: &str| {
            fs::read_to_string(dev.join(f)).ok().and_then(|s| u32::from_str_radix(s.trim(), 16).ok())
        };
        let product = hex("id/product");
        let has_ff = fs::read_to_string(dev.join("capabilities/ff"))
            .map(|s| s.split_whitespace().any(|w| w != "0"))
            .unwrap_or(false);
        if hex("id/vendor") == Some(VENDOR_SONY)
            && matches!(product, Some(PRODUCT_DUALSENSE) | Some(PRODUCT_DUALSENSE_EDGE))
            && has_ff
        {
            found.push(PathBuf::from("/dev/input").join(name));
        }
    }
    found.sort();
    found
}

impl Rumble {
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        Ok(Self { file, id: -1, path })
    }

    /// Sets both motors, 0..1. `strong` is the heavy (left) motor.
    pub fn set(&mut self, strong: f32, weak: f32) -> io::Result<()> {
        let scale = |v: f32| (v.clamp(0.0, 1.0) * 65535.0) as u16;
        let mut effect = FfEffect {
            kind: FF_RUMBLE,
            id: self.id,
            direction: 0,
            trigger_button: 0,
            trigger_interval: 0,
            replay_length: EFFECT_MS,
            replay_delay: 0,
            _pad: 0,
            strong: scale(strong),
            weak: scale(weak),
            _rest: [0; 28],
        };
        // SAFETY: EVIOCSFF reads and writes exactly one ff_effect at this pointer.
        if unsafe { libc::ioctl(self.file.as_raw_fd(), EVIOCSFF as _, &mut effect) } < 0 {
            return Err(io::Error::last_os_error());
        }
        self.id = effect.id;
        // (Re)start playback: struct input_event { timeval, type, code, value }.
        let mut ev = [0u8; 24];
        ev[16..18].copy_from_slice(&EV_FF.to_ne_bytes());
        ev[18..20].copy_from_slice(&(self.id as u16).to_ne_bytes());
        ev[20..24].copy_from_slice(&1i32.to_ne_bytes());
        self.file.write_all(&ev)
    }
}

impl Drop for Rumble {
    fn drop(&mut self) {
        let _ = self.set(0.0, 0.0);
    }
}
