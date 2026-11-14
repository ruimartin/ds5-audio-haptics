/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! DualSense Bluetooth HD haptics.
//!
//! The haptics report format is based on Sdore's SAxense research:
//! <https://apps.sdore.me/SAxense>. See docs/PROTOCOL.md.

pub mod capture;
pub mod config;
pub mod control;
pub mod device;
pub mod dsp;
pub mod protocol;
pub mod rumble;
pub mod service;
