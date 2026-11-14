/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Capture the default sink's monitor by running `pw-record` and reading raw
//! f32 stereo at 48 kHz from its stdout. A monitor capture is a passive tap: it
//! adds no delay to what the user hears.

use crate::config;
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdout, Command, Stdio};

pub struct Capture {
    child: Child,
    stdout: ChildStdout,
    /// Bytes of an incomplete stereo frame left over from the previous read.
    partial: Vec<u8>,
    raw: Vec<u8>,
}

const FRAME_BYTES: usize = 8;

impl Capture {
    pub fn spawn(cfg: &config::Capture) -> io::Result<Self> {
        let mut cmd = Command::new("pw-record");
        // No fixed target: WirePlumber links us to the default sink and moves us
        // when the default changes. node.passive lets an idle sink suspend.
        cmd.arg("-P").arg(
            "{ stream.capture.sink = true node.name = ds5-audio-haptics \
             node.description = \"DualSense audio haptics\" node.passive = true }",
        );
        if !cfg.target.is_empty() {
            cmd.arg("--target").arg(&cfg.target);
        }
        cmd.args(["--format", "f32", "--rate", "48000", "--channels", "2"])
            .arg("--latency")
            .arg(cfg.latency_frames.clamp(64, 8192).to_string())
            .args(["--raw", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped());
        // SAFETY: prctl is async-signal-safe; it makes pw-record die with us.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        Ok(Self { child, stdout, partial: Vec::new(), raw: vec![0; 1 << 16] })
    }

    /// Blocks for the next block of audio and replaces `out` with it (interleaved
    /// stereo). Ok(false) means pw-record exited.
    pub fn read(&mut self, out: &mut Vec<f32>) -> io::Result<bool> {
        out.clear();
        while out.is_empty() {
            let keep = self.partial.len();
            self.raw[..keep].copy_from_slice(&self.partial);
            let n = self.stdout.read(&mut self.raw[keep..])?;
            if n == 0 {
                return Ok(false);
            }
            let total = keep + n;
            let whole = total - total % FRAME_BYTES;
            out.extend(
                self.raw[..whole]
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            );
            self.partial.clear();
            self.partial.extend_from_slice(&self.raw[whole..total]);
        }
        Ok(true)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
