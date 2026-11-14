# DualSense Bluetooth HD-haptics protocol

Status: Phase 0 research. Nothing here has been sent to a controller by this project yet.
Each claim is tagged with how it was established:

- **[src]** read in reference source code
- **[cap]** seen in Nielk1's PS5 Bluetooth capture (SAxense issue #1, `packet_samples.txt`, 1444 reports)
- **[dev]** read from the user's own hardware on 2026-11-14 (read-only)
- **[hyp]** inference, to be confirmed in Phase 1

## Sources

| Source | License | Used for |
|---|---|---|
| egormanga/SAxense `SAxense.c` | MPL-2.0 | Original report layout, CRC, pacing |
| SAxense issue #1 (Nielk1) + `packet_samples.txt` | n/a (facts) | Sub-packet "doubled" bit, real PS5 traffic |
| awalol/DS5Dongle `src/audio.cpp`, `bt.cpp`, `utils.h` | MPL-2.0 | Haptics-only report variant, counters, sample format |
| loteran/DS5Dongle `src/audio.cpp` | MIT | Auto-haptics DSP design |
| kungaa/ds5-linux-bridge | **GPL-3.0** | Facts only (rumble-mode handling). No code may be copied. |
| torvalds/linux `drivers/hid/hid-playstation.c` | GPL-2.0 | What the kernel sends on report 0x31 (facts only) |
| libsdl-org/SDL `SDL_hidapi_ps5.c` | zlib | What Steam-lineage code sends; rumble vs audio-haptics switching |
| ValveSoftware/steam-devices `60-steam-input.rules` | MIT | hidraw permissions |

Credit required by SAxense's README: the haptics format is Sdore's research,
<https://apps.sdore.me/SAxense>. This project will carry that credit and be MPL-2.0.

## 1. Transport

We `write()` a complete output report, starting with the report ID, to the controller's
`/dev/hidrawN`. Over Bluetooth the HID device is a `uhid` device created by BlueZ
(`/sys/devices/virtual/misc/uhid/0005:054C:0CE6.*` **[dev]**, Deck and Bazzite both BlueZ 5.87,
Bazzite sets `UserspaceHID=true`). The kernel hands the report to BlueZ, which sends it on the
HIDP interrupt channel prefixed with the transaction byte `0xA2` (DATA | OUTPUT).
We never write `0xA2` ourselves, but it is part of the CRC (section 5).

hidraw does not check the write length against the descriptor, so a wrong size is not rejected
locally; the controller just ignores it.

Output reports declared by the controller's BT descriptor **[dev]** (320-byte descriptor read from
the Deck; sizes exclude the report-ID byte):

| ID | Payload | Total written | Use |
|---|---|---|---|
| 0x31 | 77 | 78 | State (rumble, LEDs, triggers). Used by the kernel and Steam. |
| 0x32 | 141 | **142** | Haptics only. **This is the one we send.** |
| 0x33 | 205 | 206 | unused here |
| 0x34 | 269 | 270 | unused here |
| 0x35 | 333 | 334 | unused here |
| 0x36 | 397 | 398 | 1 haptics frame + 1 Opus speaker frame [src awalol] |
| 0x37 | 461 | 462 | unused here |
| 0x38 | 525 | 526 | unused here |
| 0x39 | 546 | 547 | 2 haptics frames + 2 Opus speaker frames [src awalol] |

All of 0x32..0x39 share one container format; the ID only selects the size.

## 2. Report container

```
offset  size  field
0       1     report ID (0x32)
1       1     low nibble: tag (0), high nibble: report sequence 0..15
2       N     sub-packets, back to back
...           zero padding
len-4   4     CRC32, little-endian
```

For 0x32: bytes 0..137 are covered by the CRC, bytes 138..141 hold it.

**Report sequence (byte 1, high nibble).** awalol increments it per report, shared between its
0x31 and 0x32 reports **[src]**. SAxense never initialises or increments it and still works
**[src]**. SDL writes a constant `0x02` in this byte of 0x31 reports **[src]**. So the controller
does not enforce continuity **[hyp, strong]**. We will increment it anyway.

## 3. Sub-packet header

Each sub-packet is `header(1) length(1) data(...)` **[src SAxense, cap]**:

```
header bit 7    "sized": always 1 in everything observed
header bit 6    "doubled" (SAxense's `unk`): data is 2 x length bytes
header bits 5:0 packet ID
length          size of ONE unit of data
```

The capture confirms issue #1: the PS5 sends `0x15` and `0x12` with bit 6 set and twice the
stated length (`d5 c8` followed by 400 bytes, `d2 40` followed by 128 bytes) **[cap]**.

Packet IDs:

| ID | Header byte | Length | Content | Evidence |
|---|---|---|---|---|
| 0x10 | 0x90 | 63 | SetStateData, same 47-byte struct as report 0x31, zero padded | cap, src awalol |
| 0x11 | 0x91 | 1..7 | Audio/haptics stream control (section 4) | cap, src |
| 0x12 | 0x92 / 0xD2 | 64 | Haptics PCM (section 6) | cap, src |
| 0x13 | 0x93 / 0xD3 | 200 | Opus, controller speaker | src awalol |
| 0x15 | 0x95 / 0xD5 | 200 | Opus, as sent by a real PS5 | cap |
| 0x16 | 0x96 / 0xD6 | 200 | Opus, headphone jack | src awalol |
| 0x17 | 0x97 | 11 | unknown, 3 occurrences | cap |

We send only 0x11 and 0x12. **We never send 0x10**, which is the only sub-packet that carries
lightbar, player LED, trigger, rumble or volume state.

## 4. Sub-packet 0x11 (stream control)

`91 <len> <flags> <values...> <packet counter>`

Observed forms:

| Sender | Bytes after `91` | Notes |
|---|---|---|
| PS5 [cap], 1421 reports | `06 7f 51 1f 62 28 <ctr>` | first four values never change |
| SAxense [src] | `07 fe 00 00 00 00 ff <ctr>` | |
| awalol 0x36 [src] | `07 fe/ff B B B B B <ctr>` | B = buffer length, default 48 |
| awalol 0x39 [src] | `06 7e/7f B B B B <ctr>` | |
| awalol 0x32 haptics-only [src] | `04 66/67 B B <ctr>` | **our default** |
| awalol/PS5 mic status [src, cap] | `01 02` / `01 03` / `02 0b 1f` | no counter |

What is understood:

- **flags bit 0** = enable controller microphone streaming **[src awalol]**. We keep it 0.
- **Length** matches `1 + popcount(flags & 0xBE)` for every form that carries a counter: bits 0 and
  6 do not consume a value byte **[hyp, fits cap and src]**.
- awalol labels the value bytes headphone volume, mic volume, speaker volume, audio buffer length
  (x2), and notes the volume ones are "guess but no work". The PS5's `51 1f 62 28` is plausible as
  three volumes plus a buffer length of 40 **[hyp]**.
- awalol's haptics-only form (flags `0x66`) drops the three volume-like bytes and keeps two
  buffer-length bytes. Comment in source: "bit 5 - buf_len adjust".
- **Packet counter** (last byte): 8-bit, wraps. +1 per report carrying one haptics frame, +2 per
  report carrying a doubled frame **[src awalol; cap shows deltas of 2 dominating at 760 of 1420]**.
  SAxense also increments it by 1 per report. Gaps are normal in the capture.

Decision: default to awalol's `04 66 B B <ctr>` with `B = 48`, because it is the form built for
"haptics, no speaker" and carries no volume-like bytes that could touch audio state. Keep
SAxense's `07 fe 00 00 00 00 ff <ctr>` as a selectable alternative in the Phase 1 tool.
`B` is exposed as a setting; its unit and its effect on latency are unknown **[hyp]**.

## 5. CRC

Standard reflected CRC-32 (polynomial `0xEDB88320`, init `0xFFFFFFFF`, final XOR `0xFFFFFFFF`)
computed over `0xA2` followed by report bytes `0 .. len-5`, stored little-endian in the last four
bytes **[src SAxense, awalol, kernel]**. Equivalent shortcut: start the register at
`~0xEADA2D49` and feed only the report bytes, since `crc32([0xA2]) == 0xEADA2D49` (checked).

Test vector (computed with zlib, used as a unit test in Phase 1):

```
32 00 91 04 66 30 30 00 92 40 <64 x 00> <64 x 00 padding>   ->  CRC 0x935D9C35  ->  35 9c 5d 93
```

A report with a bad CRC is silently dropped by the controller.

## 6. Sub-packet 0x12 (haptics PCM)

`92 40 <64 bytes>`: 32 stereo frames, interleaved left, right.

- **Rate 3000 Hz** **[src SAxense, awalol]**. One report = 32 frames = 10.667 ms, so 93.75
  reports per second. SAxense's timer is `64 / (3000 * 2)` s, which is the same number: its "64
  samples" are 64 bytes, i.e. 32 frames.
- **Format is signed 8-bit, silence = 0x00. Not u8.** Evidence: all 168,192 haptic bytes in the PS5
  capture are `0x00` during silence **[cap]**; awalol and loteran both convert to `int8_t` with
  clamp to -128..127 **[src]**; SAxense's working example is `ffmpeg ... -f s8`. (SAxense's
  pipe-tunnel example says `audio.format=u8`; that looks like a README slip. Sending u8 would put a
  constant DC offset of -128 on both actuators.) Phase 1 confirms by feel: a wrong format is
  obvious.
- **Left/right mapping**: first byte of each pair is the left actuator **[hyp]**, to be checked
  with a left-only test tone.
- **No anti-alias filter in the controller** **[src awalol comment]**: tones above 1500 Hz fold
  back. Irrelevant for us since we low-pass far below that, but the resampler must still filter.
- Doubled form `d2 40 <128 bytes>` puts 21.3 ms in one report and exactly fills a 0x32 report.
  It halves the report rate at the cost of 10.7 ms latency. Not used by default.

## 7. The exact report we will send

```
off  bytes
0    32                 report ID
1    S0                 S = report sequence 0..15
2    91 04 66 30 30 CC  control, CC = packet counter
8    92 40              haptics header
10   <64 bytes>         32 stereo s8 frames
74   <64 x 00>          padding
138  <crc32 LE>
```

Sent once every 10.667 ms from an absolute-deadline timer (`clock_nanosleep` with
`TIMER_ABSTIME`), never "sleep then send", so jitter does not accumulate.

## 8. Coexistence with Steam Input and the kernel driver

Facts:

- On the Deck the DualSense's hidraw node is currently held open by the `steam` process **[dev]**.
  hidraw allows any number of writers; each `write()` is one whole report, so reports from
  different processes cannot interleave at byte level.
- Steam and the kernel `playstation` driver send report **0x31**, whose body is a SetStateData
  with "valid" flag bits that select which fields apply **[src kernel, SDL]**. We send **0x32**
  with no SetStateData at all. There is no field of ours that can overwrite lightbar, player LEDs,
  mute LED, triggers or volumes.
- The three writers use independent sequence nibbles already (kernel counts, SDL sends a constant),
  so a fourth uncoordinated writer is consistent with what the controller already tolerates.

The one real interaction is the actuator **mode**:

- SetStateData flag0 bit 1 (kernel: `HAPTICS_SELECT`, SDL: "Disable audio haptics") switches the
  voice coils from PCM haptics to emulated rumble. SDL sets it while a rumble is non-zero and
  comments "Leaving emulated rumble bits off will restore audio haptics" **[src]**. Steam's own
  driver is closed but shares this lineage **[hyp]**.
- The kernel driver sets the bit on every rumble update, including the one that sets both motors
  to zero **[src]**, and only sends rumble when something uses the evdev force-feedback device.

Consequences and design:

1. While a game rumbles through Steam Input, our haptics are muted by the controller and come
   back when the rumble ends. We do not fight this. Per-game, the user can turn rumble off in
   Steam Input if they prefer audio haptics only.
2. We never send 0x31 in normal operation. An opt-in "reclaim haptics mode" (a 0x31 with all
   valid flags clear) is only added if Phase 1 shows the controller stays latched in rumble mode.
3. Fallback mode (evdev `FF_RUMBLE` via the kernel) necessarily puts the controller in rumble
   mode. HD mode and fallback mode are therefore exclusive, chosen at start or on failure, never
   mixed per frame.

## 9. Device discovery and permissions

- Find the node by scanning `/sys/class/hidraw/*/device/uevent` for
  `HID_ID=0005:0000054C:00000CE6` or `...00000DF2`. Bus `0005` is Bluetooth, `0003` is USB.
  `HID_UNIQ` is the controller's MAC and identifies it across reconnects; the hidraw number
  changes on every reconnect **[dev: hidraw8 on the Deck today]**.
- Reconnect handling: `write()` returning `ENODEV`/`EIO` or the sysfs node vanishing means
  disconnect; rescan on a timer or an inotify watch on `/dev`.
- Permissions, no root needed:
  - Deck (SteamOS 3.9.6) **[dev]**: `/dev/hidraw8` is `crw-rw----+` with ACL `user:deck:rw-`.
    `60-steam-input.rules` covers 054C:0CE6 only, but SteamOS also ships a blanket
    `KERNEL=="hidraw*", TAG+="uaccess"`, so the Edge (0DF2) is covered too.
  - Bazzite 45 **[dev]**: rules for both 0CE6 and 0DF2, USB and BT, `MODE=0660, TAG+=uaccess`.
  - `uaccess` grants the ACL to the user of the active local seat, which is what a
    `systemd --user` service of the logged-in user runs as.

## 10. Phase 1 experiments (what the 60 Hz tool must answer)

| # | Question | How |
|---|---|---|
| E1 | Does the 0x66 control form produce haptics? If not, does SAxense's 0xFE form? | `--ctl awalol|saxense` |
| E2 | s8 confirmed? | 60 Hz sine at low amplitude should feel clean, not a thud plus buzz |
| E3 | Left/right order | left-only then right-only tone |
| E4 | With Steam running: lightbar, player LEDs and triggers unchanged while we play? | observe |
| E5 | After a game rumble through Steam, do haptics resume by themselves? After a kernel `fftest` rumble? | play tone, trigger rumble, observe |
| E6 | Effect of buffer-length byte B (16, 48, 127) on smoothness and delay | `--buflen` |
| E7 | Write-timing jitter and dropouts over 5 minutes, WiFi busy | tool prints interval histogram |
| E8 | DualSense Edge behaves the same | repeat E1 on 0DF2 |

## 11. Phase 1 results (2026-11-14, Deck, DualSense 0CE6 over BT, Steam running)

Tested with `ds5-tone`; "felt" results are the user's reports.

| # | Result |
|---|---|
| E1 | **Confirmed.** The awalol `91 04 66 30 30 ctr` form produces haptics. SAxense form not needed, not tested. |
| E2 | **Confirmed s8.** 60 Hz at amplitude 0.25 felt as a smooth hum. |
| E3 | **Confirmed.** First byte of each pair is the left grip. Separation is weak at 60 Hz (whole shell moves) and clear at 160 Hz, with some blending. |
| E4 | **Confirmed.** 20 s tone with Steam holding the node: no change to lightbar, player LEDs or mute light, hum steady. |
| E5 | **Steam rumble: confirmed benign.** During a 40 s tone, Steam's rumble test played its own rumble and the hum resumed instantly each time, repeatedly. No latching, so no "reclaim" report is needed. Kernel evdev rumble (fallback path only) still untested. |
| E6 | open: buffer-length byte, to be measured with the Phase 2 latency test |
| E7 | Partial: over 20 s (1885 reports) worst wake-up lateness 0.40 ms, no late frames, slowest write 0.27 ms. A 5-minute run under WiFi load is still to do. |
| E8 | open: DualSense Edge |

Design note from E3: stereo haptics only carry useful left/right information above roughly
100 Hz. The sub-bass band should be sent to both sides; any stereo split belongs in the
low-mid and transient bands.
