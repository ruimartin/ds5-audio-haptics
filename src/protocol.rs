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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn period_is_ten_and_two_thirds_ms() {
        assert_eq!(REPORT_PERIOD_NS, 10_666_666);
    }
}
