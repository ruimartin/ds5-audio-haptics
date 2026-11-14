/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Bluetooth output report 0x32 (haptics only). Layout in docs/PROTOCOL.md section 7.

pub const REPORT_ID: u8 = 0x32;
/// Bytes written to hidraw, report ID included.
pub const REPORT_LEN: usize = 142;
pub const SAMPLE_RATE: u32 = 3000;
/// Stereo frames per report.
pub const FRAMES: usize = 32;
/// Haptic payload bytes per report (interleaved left, right, signed 8-bit).
pub const PAYLOAD: usize = FRAMES * 2;
/// One report covers 32 / 3000 s.
pub const REPORT_PERIOD_NS: u64 = 1_000_000_000 * FRAMES as u64 / SAMPLE_RATE as u64;

const SUB_SIZED: u8 = 0x80;
const SUB_CONTROL: u8 = 0x11 | SUB_SIZED;
const SUB_HAPTICS: u8 = 0x12 | SUB_SIZED;

/// Which form of the 0x11 control sub-packet to send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlForm {
    /// `91 04 66 B B ctr`: DS5Dongle's haptics-only form. Default.
    Awalol,
    /// `91 07 fe 00 00 00 00 ff ctr`: the original SAxense form.
    Saxense,
}

/// CRC-32 (reflected, poly 0xEDB88320) over 0xA2 followed by `data`.
/// 0xA2 is the HIDP "DATA | OUTPUT" header BlueZ puts on the wire.
pub fn crc32_bt_output(data: &[u8]) -> u32 {
    // Register state after feeding 0xA2 to a fresh CRC-32.
    let mut crc: u32 = !0xEADA_2D49;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

/// Builds consecutive haptics reports, keeping the two counters.
pub struct ReportBuilder {
    form: ControlForm,
    buffer_len: u8,
    seq: u8,
    counter: u8,
}

impl ReportBuilder {
    pub fn new(form: ControlForm, buffer_len: u8) -> Self {
        Self { form, buffer_len, seq: 0, counter: 0 }
    }

    /// One report carrying 32 stereo frames.
    pub fn build(&mut self, samples: &[i8; PAYLOAD]) -> [u8; REPORT_LEN] {
        let mut r = [0u8; REPORT_LEN];
        r[0] = REPORT_ID;
        r[1] = self.seq << 4;
        self.seq = (self.seq + 1) & 0x0F;

        let mut i = 2;
        r[i] = SUB_CONTROL;
        match self.form {
            ControlForm::Awalol => {
                r[i + 1] = 4;
                r[i + 2] = 0x66; // bit 0 (controller mic) stays off
                r[i + 3] = self.buffer_len;
                r[i + 4] = self.buffer_len;
                r[i + 5] = self.counter;
                i += 6;
            }
            ControlForm::Saxense => {
                r[i + 1] = 7;
                r[i + 2] = 0xFE;
                r[i + 7] = 0xFF;
                r[i + 8] = self.counter;
                i += 9;
            }
        }
        self.counter = self.counter.wrapping_add(1);

        r[i] = SUB_HAPTICS;
        r[i + 1] = PAYLOAD as u8;
        for (dst, &s) in r[i + 2..i + 2 + PAYLOAD].iter_mut().zip(samples) {
            *dst = s as u8;
        }

        let crc = crc32_bt_output(&r[..REPORT_LEN - 4]);
        r[REPORT_LEN - 4..].copy_from_slice(&crc.to_le_bytes());
        r
    }
}

/// Bluetooth output report 0x31: controller state. We use it for the adaptive
/// triggers only; every other field is left untouched by keeping its flag clear.
pub const STATE_REPORT_ID: u8 = 0x31;
pub const STATE_REPORT_LEN: usize = 78;
const STATE_TAG: u8 = 0x10;
const FLAG0_RIGHT_TRIGGER: u8 = 1 << 2;
const FLAG0_LEFT_TRIGGER: u8 = 1 << 3;

/// One trigger's effect: a mode byte and ten parameters.
/// Encodings follow Nielk1's DualSense trigger effect documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriggerEffect(pub [u8; 11]);

impl TriggerEffect {
    pub const OFF: Self = Self([0x05, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);

    /// Vibrates from zone `position` (0..9) to the end of travel.
    /// `amplitude` 1..8, `frequency` in Hz.
    pub fn vibration(position: u8, amplitude: u8, frequency: u8) -> Self {
        let (position, level) = (position.min(9) as u32, (amplitude.clamp(1, 8) - 1) as u64);
        let (mut zones, mut amplitudes) = (0u16, 0u64);
        for zone in position..10 {
            zones |= 1 << zone;
            amplitudes |= level << (3 * zone);
        }
        let (z, a) = (zones.to_le_bytes(), (amplitudes as u32).to_le_bytes());
        Self([0x26, z[0], z[1], a[0], a[1], a[2], a[3], 0, 0, frequency.max(1), 0])
    }

    /// Steady resistance from zone `position` (0..9) to the end of travel, `strength` 1..8.
    pub fn feedback(position: u8, strength: u8) -> Self {
        let (position, level) = (position.min(9) as u32, (strength.clamp(1, 8) - 1) as u64);
        let (mut zones, mut forces) = (0u16, 0u64);
        for zone in position..10 {
            zones |= 1 << zone;
            forces |= level << (3 * zone);
        }
        let (z, f) = (zones.to_le_bytes(), (forces as u32).to_le_bytes());
        Self([0x21, z[0], z[1], f[0], f[1], f[2], f[3], 0, 0, 0, 0])
    }

    /// Resistance that gives way: a click between zones `start` (2..7) and `end` (..8).
    pub fn weapon(start: u8, end: u8, strength: u8) -> Self {
        let start = start.clamp(2, 7);
        let end = end.clamp(start + 1, 8);
        let zones = ((1u16 << start) | (1u16 << end)).to_le_bytes();
        Self([0x25, zones[0], zones[1], strength.clamp(1, 8) - 1, 0, 0, 0, 0, 0, 0, 0])
    }
}

/// A state report that sets the given triggers and nothing else.
pub fn trigger_report(seq: u8, right: Option<TriggerEffect>, left: Option<TriggerEffect>) -> [u8; STATE_REPORT_LEN] {
    let mut r = [0u8; STATE_REPORT_LEN];
    r[0] = STATE_REPORT_ID;
    r[1] = (seq & 0x0F) << 4;
    r[2] = STATE_TAG;
    // The state block starts at byte 3: flags, then fields at fixed offsets.
    if let Some(effect) = right {
        r[3] |= FLAG0_RIGHT_TRIGGER;
        r[3 + 10..3 + 21].copy_from_slice(&effect.0);
    }
    if let Some(effect) = left {
        r[3] |= FLAG0_LEFT_TRIGGER;
        r[3 + 21..3 + 32].copy_from_slice(&effect.0);
    }
    let crc = crc32_bt_output(&r[..STATE_REPORT_LEN - 4]);
    r[STATE_REPORT_LEN - 4..].copy_from_slice(&crc.to_le_bytes());
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRC_RIGHT_OFF_SEQ3: [u8; 4] = [0xee, 0x14, 0xdc, 0x28];

    #[test]
    fn crc_seed_matches_a2_prefix() {
        // crc32([0xA2]) is 0xEADA2D49, so an empty tail must return it.
        assert_eq!(crc32_bt_output(&[]), 0xEADA_2D49);
    }

    #[test]
    fn silence_report_matches_reference() {
        // Vector computed independently with zlib (docs/PROTOCOL.md section 5).
        let mut b = ReportBuilder::new(ControlForm::Awalol, 0x30);
        let r = b.build(&[0; PAYLOAD]);
        assert_eq!(&r[..10], &[0x32, 0x00, 0x91, 0x04, 0x66, 0x30, 0x30, 0x00, 0x92, 0x40]);
        assert_eq!(&r[138..], &[0x35, 0x9c, 0x5d, 0x93]);
    }

    #[test]
    fn saxense_layout() {
        let mut b = ReportBuilder::new(ControlForm::Saxense, 0);
        let mut s = [0i8; PAYLOAD];
        s[0] = -1;
        s[63] = 127;
        let r = b.build(&s);
        assert_eq!(&r[2..13], &[0x91, 7, 0xFE, 0, 0, 0, 0, 0xFF, 0, 0x92, 0x40]);
        assert_eq!(r[13], 0xFF);
        assert_eq!(r[76], 0x7F);
        assert!(r[77..138].iter().all(|&x| x == 0));
    }

    #[test]
    fn counters_advance_and_wrap() {
        let mut b = ReportBuilder::new(ControlForm::Awalol, 48);
        let mut last = [0u8; REPORT_LEN];
        for _ in 0..258 {
            last = b.build(&[0; PAYLOAD]);
        }
        assert_eq!(last[1], ((257u32 % 16) as u8) << 4);
        assert_eq!(last[7], 1); // 257 mod 256
    }

    #[test]
    fn vibration_effect_encoding() {
        // Zones 0..9 active, amplitude 8 (stored as 7 in three bits per zone), 40 Hz.
        assert_eq!(TriggerEffect::vibration(0, 8, 40).0, [0x26, 0xFF, 0x03, 0xFF, 0xFF, 0xFF, 0x3F, 0, 0, 40, 0]);
        // From zone 5 at amplitude 1: zones set, all amplitude bits zero.
        assert_eq!(TriggerEffect::vibration(5, 1, 10).0, [0x26, 0xE0, 0x03, 0, 0, 0, 0, 0, 0, 10, 0]);
        assert_eq!(TriggerEffect::weapon(2, 5, 8).0, [0x25, 0x24, 0, 7, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(TriggerEffect::feedback(0, 8).0, [0x21, 0xFF, 0x03, 0xFF, 0xFF, 0xFF, 0x3F, 0, 0, 0, 0]);
    }

    #[test]
    fn trigger_report_sets_only_trigger_flags() {
        let r = trigger_report(3, Some(TriggerEffect::OFF), None);
        assert_eq!(&r[..5], &[0x31, 0x30, 0x10, 0x04, 0x00]);
        assert_eq!(r[13], 0x05);
        // Everything except the right trigger's mode byte is zero up to the CRC.
        assert!(r[5..74].iter().enumerate().all(|(i, &b)| b == 0 || i + 5 == 13));
        let both = trigger_report(0, Some(TriggerEffect::OFF), Some(TriggerEffect::vibration(0, 8, 40)));
        assert_eq!(both[3], 0x0C);
        assert_eq!(both[24], 0x26);
        assert_eq!(both[33], 40);
        // CRC checked against zlib: crc32(b"\xa2" + report[:74]).
        assert_eq!(&r[74..], &CRC_RIGHT_OFF_SEQ3);
    }

    #[test]
    fn period_is_ten_and_two_thirds_ms() {
        assert_eq!(REPORT_PERIOD_NS, 10_666_666);
    }
}
