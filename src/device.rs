/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Find DualSense controllers by scanning sysfs. No udev, no root.

use std::fs;
use std::path::{Path, PathBuf};

pub const VENDOR_SONY: u32 = 0x054C;
pub const PRODUCT_DUALSENSE: u32 = 0x0CE6;
pub const PRODUCT_DUALSENSE_EDGE: u32 = 0x0DF2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    Usb,
    Bluetooth,
    Other(u32),
}

#[derive(Clone, Debug)]
pub struct Controller {
    /// e.g. /dev/hidraw8. Changes on every reconnect.
    pub hidraw: PathBuf,
    pub bus: Bus,
    pub product: u32,
    pub name: String,
    /// Controller MAC address; stable across reconnects.
    pub uniq: String,
}

impl Controller {
    pub fn is_edge(&self) -> bool {
        self.product == PRODUCT_DUALSENSE_EDGE
    }
}

/// Parse a HID uevent file. Returns None when it is not a DualSense.
fn parse_uevent(text: &str, hidraw: PathBuf) -> Option<Controller> {
    let mut id = None;
    let mut name = String::new();
    let mut uniq = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("HID_ID=") {
            // BUS:VENDOR:PRODUCT, hex, e.g. 0005:0000054C:00000CE6
            let mut parts = v.split(':').map(|p| u32::from_str_radix(p, 16).ok());
            id = Some((parts.next()??, parts.next()??, parts.next()??));
        } else if let Some(v) = line.strip_prefix("HID_NAME=") {
            name = v.to_string();
        } else if let Some(v) = line.strip_prefix("HID_UNIQ=") {
            uniq = v.to_string();
        }
    }
    let (bus, vendor, product) = id?;
    if vendor != VENDOR_SONY
        || (product != PRODUCT_DUALSENSE && product != PRODUCT_DUALSENSE_EDGE)
    {
        return None;
    }
    let bus = match bus {
        0x0003 => Bus::Usb,
        0x0005 => Bus::Bluetooth,
        other => Bus::Other(other),
    };
    Some(Controller { hidraw, bus, product, name, uniq })
}

/// All connected DualSense / DualSense Edge controllers, sorted by hidraw path.
pub fn scan() -> Vec<Controller> {
    scan_in(Path::new("/sys/class/hidraw"))
}

fn scan_in(sys: &Path) -> Vec<Controller> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(sys) else { return found };
    for e in entries.flatten() {
        let node = e.file_name();
        let Ok(text) = fs::read_to_string(e.path().join("device/uevent")) else { continue };
        if let Some(c) = parse_uevent(&text, Path::new("/dev").join(&node)) {
            found.push(c);
        }
    }
    found.sort_by(|a, b| a.hidraw.cmp(&b.hidraw));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const DECK: &str = "DRIVER=playstation\nHID_ID=0005:0000054C:00000CE6\n\
        HID_NAME=DualSense Wireless Controller\nHID_PHYS=aa:bb:cc:dd:ee:ff\n\
        HID_UNIQ=11:22:33:44:55:66\nMODALIAS=hid:b0005g0000v0000054Cp00000CE6\n";

    #[test]
    fn parses_bluetooth_dualsense() {
        let c = parse_uevent(DECK, "/dev/hidraw8".into()).unwrap();
        assert_eq!(c.bus, Bus::Bluetooth);
        assert_eq!(c.product, PRODUCT_DUALSENSE);
        assert_eq!(c.uniq, "11:22:33:44:55:66");
        assert!(!c.is_edge());
    }

    #[test]
    fn parses_usb_edge() {
        let c = parse_uevent("HID_ID=0003:0000054C:00000DF2\n", "/dev/hidraw1".into()).unwrap();
        assert_eq!(c.bus, Bus::Usb);
        assert!(c.is_edge());
    }

    #[test]
    fn ignores_other_devices() {
        assert!(parse_uevent("HID_ID=0003:000028DE:00001205\n", "/dev/hidraw0".into()).is_none());
        assert!(parse_uevent("HID_ID=0005:0000054C:000009CC\n", "/dev/hidraw0".into()).is_none());
        assert!(parse_uevent("garbage", "/dev/hidraw0".into()).is_none());
    }
}
