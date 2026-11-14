# Risks and open decisions

Ordered by how likely each is to change the design. See PROTOCOL.md for the evidence tags.

## Protocol and hardware

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | Game rumble through Steam Input switches the actuators to rumble mode and mutes our haptics while it lasts; the controller may stay latched afterwards | High that it mutes, unknown that it latches | Haptics drop out in games that rumble | Accept muting. Test latching in Phase 1 (E5). If it latches, add an opt-in "reclaim" 0x31 report with no valid flags, sent only after silence from Steam. Document turning rumble off per game. |
| R2 | The 0x11 control bytes are only partly understood (buffer-length unit, which bits are mandatory) | Medium | No output, or extra latency | Two known-good forms selectable (awalol 0x66, SAxense 0xFE). Buffer length is a setting. |
| R3 | Sample format: the brief said u8, evidence says signed 8-bit | Low that s8 is wrong | Constant offset, weak or harsh output | Confirm by feel in Phase 1 (E2). Format is one line of code. |
| R4 | Controller firmware differences (old firmware, DualSense Edge) | Low to medium | No output on some units | Log firmware version from feature report 0x20. Test the Edge that is paired to the Deck. |
| R5 | Sustained strong low-frequency drive: heat, battery drain, actuator wear | Medium for battery | Shorter battery life | Limiter with a ceiling below full scale, noise gate so silence sends silence, stop sending reports after N seconds of silence. |
| R6 | A report with no valid 0x10 state could still change something in the controller (mic, speaker path) | Low | Unwanted side effect | Mic bit kept 0, no volume-like bytes in the default form. Checked in E4. |

## Bluetooth and timing

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R7 | 94 reports/s of 142 bytes alongside Steam's traffic and 2.4 GHz WiFi on the Deck causes jitter or dropouts | Medium | Stutter, added delay | Absolute-deadline timer, drop late frames instead of queuing, optional doubled-frame mode (47 reports/s), measure in E7. |
| R8 | 40 ms end-to-end target | Medium | Target missed | Budget: capture quantum 5 ms + frame accumulation 10.7 ms + filter group delay about 3 ms + BT 5 to 15 ms + controller buffer (unknown). Software part is measurable exactly; the physical part needs a microphone test (click in audio vs. recorded actuator buzz). The controller buffer is the unknown that decides this. |
| R9 | Input lag on the controller's own buttons from the extra BT traffic | Low | Worse gameplay | loteran saw this on a Pico, caused by CPU not radio. Check input report rate with and without haptics. |

## PipeWire capture

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R10 | `pw-record` through a pipe adds latency (SAxense's README reports up to seconds with pipe-tunnel) | Medium | Latency target missed | Request a small `--latency`, read the pipe eagerly, discard backlog when behind. Plan B: link libpipewire dynamically (not static, see below). |
| R11 | Asking for a low-latency capture lowers the whole graph's quantum, raising CPU load and xrun risk for game audio on the Deck | Medium | Audio crackle in heavy games | Quantum request is a setting, default moderate (256 at 48 kHz), never forced. It does not add latency to the user's output; a monitor capture is a passive tap. |
| R12 | Following the default sink: Deck switches between speaker, headphones, HDMI (HDMI is default right now), Bazzite between devices | Certain to occur | Haptics stop after a switch | Capture with `stream.capture.sink=true` and no fixed target so WirePlumber re-links; restart the capture child if it exits. Verify on both machines. |
| R13 | Monitor level may or may not follow sink volume and mute | Unknown | Haptics change with volume, or play while muted | Measure. Offer "follow volume" as a setting if feasible. |
| R14 | Sound played on the controller itself (if the DualSense is ever the audio sink) feeding haptics | Low over BT | Feedback-like buzz | Not possible over BT today (no BT audio device). Exclude the DualSense sink by name for the USB case. |

## Platform and packaging

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R15 | libpipewire cannot be linked into a static binary (it loads plugins at run time) | Certain | Conflicts with "static binary" | Static musl binary that spawns the system's `pw-record` (present on both machines, PipeWire 1.6.x). Only plan B uses libpipewire. |
| R16 | SteamOS update changes udev rules or PipeWire tools | Low | Service fails after update | Nothing installed outside `~/.local` and `~/.config`. Start-up self-check prints the exact reason (no node, no permission, no pw-record). |
| R17 | hidraw ACL exists only while the user's local session is active | Low | Permission denied at boot or over SSH-only sessions | Service is `WantedBy=default.target` of the logged-in user; retry with backoff on `EACCES`. |
| R18 | No Rust toolchain on the Bazzite PC (`cargo` absent), `/usr` is read-only on both | Certain | Build friction | Build in a podman container targeting `x86_64-unknown-linux-musl`; copy one binary to the Deck. No host changes. |
| R19 | Decky Loader is not installed on the Deck (`~/homebrew` absent) | Certain today | Phase 3 blocked until installed | User installs Decky before Phase 3. The service's socket API is designed in Phase 2 so the plugin is a thin client. |
| R20 | Licensing: ds5-linux-bridge is GPL-3.0 | n/a | Copying from it would force GPL | Facts only from that repo. Project is MPL-2.0; awalol (MPL-2.0), loteran (MIT), SDL (zlib) are compatible. Credit SAxense as its README asks. |

## Decisions I am recommending

1. **Rust**, one static musl binary for capture control, DSP, hidraw output, evdev fallback and the
   control socket. Reasons: the musl target gives a static binary with no extra work; TOML, the
   socket protocol and reconnect logic are much less code than in C; a long-running daemon that
   parses external input benefits from memory safety. The DSP is a few biquads at 48 kHz and costs
   nothing in either language. C would only win if we needed libpipewire in-process, which a static
   binary rules out anyway.
2. **Capture by spawning `pw-record`** on the default sink monitor, raw f32 at 48 kHz on stdout.
   Low-pass at 48 kHz, then decimate 16:1 to 3000 Hz (exact integer ratio, as loteran does).
3. **Signed 8-bit output**, not u8.
4. **Never send report 0x31** in HD mode. Game rumble wins over audio haptics while it is active.
5. **MPL-2.0** for the whole project.
