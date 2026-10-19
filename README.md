# FreeDSP Studio

An EQ editor for USB-C DSP cables and dongles — Moondrop's DUSK-SP, MAY and FreeDSP, the newer FreeDSP Mini, Walkplay-chipset devices sold under other brands such as the CrinEar Protocol Micro, FiiO's dongles, amps and receivers (BTR17, KA15 and the rest of FiiO's HID range, plus JadeAudio and SnowSky), KT Micro-chipset IEMs and cables, and the Fosi Audio DS3. It reads whatever EQ is on the cable, lets you edit the parametric bands by dragging points on the response curve, imports from squig.link, and writes it back. Built in Rust with [egui](https://github.com/emilk/egui): one native program that draws its own interface, with no browser engine and no GUI toolkit behind it. Runs on Windows, macOS and Linux (Linux not yet tested with a cable).

I made this because the stock app is clunky, locked to whole-dB steps, and it turns out the cable can do WAY more than the app lets on.

> Independent project. Nothing to do with Moondrop, CrinEar, FiiO, Fosi Audio or any other manufacturer here — not affiliated, not endorsed. For cables you own.

![FreeDSP Studio — frequency-response view with the EQ auto-fitted to a target](assets/screenshot.png)

## Download

From the [latest release](https://github.com/EffectiveEquivalent/FreeDSP-Studio/releases):

| | | |
|---|---|---|
| **Windows** | `FreeDSP-Studio.exe` | Single portable exe, nothing to install |
| **macOS** | `FreeDSP-Studio-macOS.dmg` | Universal, runs on Apple Silicon and Intel |
| **Linux** | `FreeDSP-Studio-linux-amd64.deb` / `.rpm` | Debian/Ubuntu or Fedora. Untested with a real cable, see below |
| | `FreeDSP-Studio-linux-x86_64.tar.gz` | Any other distro: run it from the folder, or `./install.sh` |

**Heads up: neither build is signed, so your OS will warn you about it.** Signing costs money per year on both platforms and this is a free project.

**Windows** shows a SmartScreen "unknown publisher" warning for any exe it hasn't seen before. That's a reputation check, not a virus detection. Click More info, then Run anyway. Signed releases via [SignPath](https://signpath.org) are in the works.

**macOS** shows "Apple could not verify FreeDSP Studio is free of malware". The app *is* ad-hoc signed, which is what lets it run on Apple Silicon at all, but it isn't notarised, and only notarisation clears that message. To get past it: try to open the app, dismiss the refusal, then go to **System Settings > Privacy & Security**, scroll down, and click **Open Anyway**. Note the old right-click > Open shortcut no longer works on current macOS. If you prefer the terminal:

```bash
xattr -dr com.apple.quarantine "/Applications/FreeDSP Studio.app"
```

Either way you only do it once. Or run from source (below), which avoids the whole thing.

**Linux is untested with a cable.** The device code is the same hidapi path macOS uses, going through hidraw instead of IOKit, and the app runs, but nobody has driven a cable with it yet. If you try it, please [open an issue](https://github.com/EffectiveEquivalent/FreeDSP-Studio/issues) saying what worked. It needs kernel 5.11 or newer.

On Linux the app links only libc, ALSA (for the listening test's sound) and libudev. Wayland or X11 and OpenGL are loaded when it starts, so it runs on any desktop, GNOME or not. File dialogs go through the XDG desktop portal (`xdg-desktop-portal`, which every mainstream desktop ships); dropping files on the window works without it.

Linux won't let a normal user open a HID device without a udev rule. The `.deb` and `.rpm` install one for you (`/usr/lib/udev/rules.d/70-freedsp-studio.rules`). Running from source or the tarball (its `install.sh` does this), grab [`70-freedsp-studio.rules`](tools/app/packaging/linux/70-freedsp-studio.rules) and:

```bash
sudo cp 70-freedsp-studio.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Then unplug and replug the cable.

## Supported cables

Five protocols. The app tells them apart by vendor id and by asking, so a device not listed here will often still work: anything that answers the gen-2 handshake is driven as a gen-2 cable.

**Tested** means checked on real hardware with this app. Everything else is driven from the maker's own app (FiiO's fiiocontrol.fiio.com) or from [devicePEQ](https://github.com/jeromeof/devicePEQ), the open-source code behind graph.hangout.audio's device EQ, and the app shows "untested model" when one is connected. Every write is still read back and checked, so a model that behaves differently says so rather than failing silently.

**Gen 1** — HID report `0x01` over the control pipe, 9 bands. Tested:

| Cable | USB ID (VID:PID) | Firmware presets |
|---|---|---|
| Moondrop DUSK-SP | `35D8:1499` | Crinacle tunings |
| Moondrop MAY | `35D8:1497` | Standard / Bass Head / Reference / No Bass / Harman |
| Moondrop FreeDSP | `35D8:1496` | none (custom bank only) |

devicePEQ's "Conexant" handler is this same protocol, so the original FreeDSP and Moondrop ECHO-B are the same thing.

**Gen 2** — HID report `0x4B` over the interrupt endpoints, with a real preamp register. Tested:

| Cable | USB ID (VID:PID) | Bands | Gain | Filter types | Firmware presets |
|---|---|---|---|---|---|
| Moondrop FreeDSP Mini | `35D8:98D4` | 8 | ±12 dB | PK / LS / HS | Default, Bass Enhance ×2, Space, Treble Enhance |
| CrinEar Protocol Micro | `3302:C20F` | 8 | ±10 dB | PK / LS / LPF / HPF | none (custom bank only) |

The CrinEar Protocol Max (`3302:43CC`, 10 bands, ±10 dB, PK / LS / HS) has its own profile but hasn't been tested with this app; [Contour](https://github.com/Chronosauros/contour), an Android EQ app, has driven one on the same limits.

The TRN Black Pearl (`3302:43E8`, "TE-C" over USB, 10 bands, ±10 dB, PK / LS / HS / LPF / HPF) has its own profile as well and hasn't been tested with this app either. Contour ran this app's write sequence against one (firmware 0.6) and read every band register, every coefficient and the preamp back exactly as sent, low and high pass included, also after unplugging it. It only appears on USB with headphones plugged in, so plug them in before Detect.

Untested: Walkplay's chip firmware comes in "schemes" with a fixed band count, gain range and filter set, and a product id belongs to one scheme whatever the brand. The app carries Walkplay's catalogue of about 350 product ids, by way of devicePEQ, across 19 vendor ids (Moondrop, CrinEar, Tanchjim, Truthear, ddHiFi, LETSHUOER, NiceHCK, BGVP, Kiwi Ears and others), and only probes devices on the less specific vendor ids if their descriptor declares report `0x4B`.

| Scheme | Bands | Gain | Filter types |
|---|---|---|---|
| 10, 21 | 8 | ±10 dB | PK |
| 11 | 8 | ±10 dB | PK / LS / LPF / HPF |
| 13, 16, 18 | 10 | ±10 dB | PK / LS / HS |
| 15 | 8 | ±10 dB | PK / LS / HS |
| 17 | 5 | ±10 dB | PK / LS / HS |
| 19 | 6 | ±10 dB | PK / LS / HS |
| 20 | 10 | ±10 dB | PK |

Also known by name: Moondrop FreeDSP Pro, MOONRIVER 3, Rays, AG Rays, Marigold, DAWN PRO 2, Echo A, DHA15, Deco Audio System, INN Deco75-DH; Tanchjim Space Pro, OLA II DSP, Stargate II; NiceHCK Octave; Truthear KeyX; ddHiFi DSP Cable/IEM - Memory; BGVP MX1; LETSHUOER DT04; generic CS43131/CS43198 dongles.

**FiiO** (with JadeAudio and SnowSky) — HID report 7 (1 on the KA17, 2 on JadeAudio), framed with a CRC-8. The device designs its own filters from freq / gain / Q, and holds several writable USER banks, which the app switches between like the presets above:

| Device | Tested | Bands | Gain | Filter types | Writable banks |
|---|---|---|---|---|---|
| FiiO BTR17 | yes | 10 | −24 / +12 dB | PK / LS / HS | USER1–10 |
| FiiO KA15 | yes | 10 | ±12 dB | PK / LS / HS / LPF / HPF | USER1–3 |
| FiiO KA17 | | 10 | ±12 dB | PK / LS / HS / LPF / HPF | USER1–3 |
| FiiO BTR13 | | 10 | ±12 dB | PK / LS / HS / LPF / HPF | USER1–3 |
| FiiO QX13 | | 10 | −24 / +12 dB | PK / LS / HS / LPF / HPF | USER1–10 |
| FiiO QX11 | | 10 | ±12 dB | PK / LS / HS / LPF / HPF | USER1–10 |
| FiiO K13 R2R, BR15 R2R | | 10 | −24 / +12 dB | PK / LS / HS / LPF / HPF | USER1–10 |
| FiiO AIR AMP | | 10 | −24 / +12 dB | PK / LS / HS / LPF / HPF | USER1–10 |
| FiiO Air Link, BT11 | | 10 | −24 / +12 dB | PK / LS / HS / LPF / HPF | USER1–10 |
| FiiO FX17 | | 10 | −24 / +12 dB | PK / LS / HS / LPF / HPF | USER1 |
| FiiO FP3, OAK NANO | | 10 | −24 / +12 dB | PK / LS / HS | USER1 |
| FiiO LS-TC2 | | 10 | ±12 dB | PK / LS / HS | USER1 |
| FiiO FG3 | | 10 | ±12 dB | PK / LS / HS / LPF / HPF | USER1–3 (music voicings only) |
| RETRO NANO | yes | 10 | ±12 dB | PK / LS / HS / LPF / HPF | USER1–3 |
| SNOWSKY Melody | | 10 | −24 / +12 dB | PK / LS / HS / LPF / HPF | USER1–3 |
| SNOWSKY TINY A, TINY B | | 10 | ±12 dB | PK / LS / HS / LPF / HPF | USER1–3 |
| JadeAudio JIEZI | | 10 | ±12 dB | PK / LS / HS / LPF / HPF | USER1–3 |
| JadeAudio JA11 | | as reported | ±12 dB | PK / LS / HS | USER1 |

Bands are as the device reports them (10 on every model checked so far). FiiO devices only share their EQ with EQ switched on in their settings; the app tells you if it's off. The KA15 keeps its preamp in whole dB, so the app rounds headroom down to the next whole dB for it, and plays shelves and USER banks differently from what it stores; the app corrects for both (see the changelog). FiiO's K15, K17 and K19 talk over a USB serial port rather than HID, and aren't supported yet.

**KT Micro** — HID report `0x4B`, a register per band parameter, vendor id `31B2`. Untested, 5 bands, ±10 dB, Q up to 5, PK / LS / HS: Moondrop CDSP and Chu II DSP, Tanchjim One DSP, Bunny DSP and Fission, Kiwi Ears Allegro Mini, Allegro Pro, Chorus and S-Link, JCally JM12 (KT02H20, peaking only), and other KT Micro parts on devicePEQ's defaults.

**Fosi Audio** — feature report 1, float parameters. Untested: the Fosi Audio DS3, 8 bands, ±12 dB, five writable banks (Custom 1–5). It works out its own headroom, so there's no preamp to set. devicePEQ measured that it realises a peak's Q divided by the gain-dependent RBJ factor, and places shelves by the other corner-frequency convention; both are corrected for, as devicePEQ does.

Plug in more than one and the app asks which to drive.

Windows, macOS and (untested) Linux. The gen-1 HID transport is split per platform — Windows goes at `hid.dll` directly, macOS and Linux go through hidapi, onto IOKit and hidraw respectively. hidraw supports the same two calls (`HIDIOCSOUTPUT` / `HIDIOCGINPUT`, kernel 5.11+), so Linux shares the macOS backend unchanged. Gen 2 needs neither, being plain interrupt reads and writes, so it already runs through hidapi on every platform. Linux needs the udev rule granting access to the vendor ids above (`35d8`, `3302`, `2972`, `0a12`, `31b2`, `152a` and the Walkplay ones), and someone with a cable to test it.

## What it does

- Detects the cable and pulls in whatever EQ is loaded on it
- Parametric EQ — 9 bands on gen-1 cables, 8 on gen-2 — with the filter types and gain range each cable actually supports.... drag the numbered nodes straight on the graph, or type exact values (`8k`, `-3.5`, `0.71`; arrow keys nudge)
- Proper fractional-dB gain, not the whole numbers the stock app limits you to (see below)
- A real preamp, with an auto headroom button and a clip check before every write
- Switches the active firmware profile on the DUSK, MAY and FreeDSP Mini, and the active bank on FiiO and Fosi devices
- Bank sets on devices with several writable banks (FiiO, Fosi): name the banks, drag them into order, and read or write the whole set at once. Write all skips banks that already match, checks each one it writes, and keeps whatever it overwrote as a set you can write back
- Works without a device: design a bank set for any FiiO or Fosi model, or for a generic device with 1 to 10 bands, and write it when you next plug in
- squig.link / AutoEQ import and export (Equalizer APO parametric text): drop an EQ file on the window to apply it, or Export to save one
- Drop an FR file on the window to add it as a measurement or target, with its name and rig
- Frequency-response view: load your IEM's measurement and a target, see the EQ'd result
- Auto-fit the EQ to a target (based on AutoEq): either up to a frequency, leaving the rest of the bands free, or over the whole target using your treble correction
- Treble listening test: a listening test through the cable that finds the peaks and dips your ear hears above ~6 kHz, where a 711 can't be trusted. Two methods: "stands out" (does a band poke out of pink noise?) and "louder / quieter" (match neighbouring bands by level). Saved per IEM, tip and method, reused with any target, and switchable on and off in the FR view
- Saves your EQs as plain JSON files you can back up or share
- Nothing touches the cable until you hit Write

## The interesting bits

I reverse engineered the cable's HID protocol to build this, and a few genuinely cool things fell out of it:

**The EQ lives on the cable, not the app.** The DSP chip has its own memory. Whatever you write follows the cable to any phone or laptop with zero software running.

**The whole-dB limit isn't real.** Each band stores two separate things: a little metadata record where gain is a whole-dB integer, and the actual biquad filter coefficients that do the DSP. The metadata is basically a label for the app to display. The sound comes ENTIRELY from the coefficients, which are computed at full precision, so 2.3 dB really is 2.3 dB.... it just reads back as 2 because the readable field can't hold decimals. Proved on hardware by writing metadata that said 0 dB with coefficients for a -12 dB shelf: read-back said flat, the bass was audibly gone.

**The preamp is the bit I'm most chuffed with.** The cable has no preamp register at all. But scaling one band's biquad numerator by `k = 10^(preamp/20)` shifts that filter's response by a flat amount at every frequency, and because the 9 bands are cascaded, the WHOLE combined curve moves down with its shape intact. A mathematically exact preamp riding on a filter that's already there, costing nothing. It sits on the last active band so the rest of the chain runs hot (better signal-to-noise, thanks to oratory1990 for the nudge).

Nerdy footnote: a gen-1 cable stores a separate coefficient set per sample rate (44.1 / 48 / 96 / 192 / 384 kHz), fixed-point scaled by 2^22. Silicon is a Conexant CX2077x-class DSP; control is HID over USB.

**Gen 2 is a different animal.** Report `0x4B`, framed as `[OP, CMD, LEN, payload]`, and — unlike gen 1 — replies come back on the interrupt endpoint rather than the control pipe. That difference is why these cables can be driven from a browser where gen-1 ones cannot: WebHID can send an output report and listen for input reports, but cannot issue a control-pipe `GET_REPORT`, which is exactly what gen 1 needs and exactly why this app exists.

Each band carries both the coefficients it runs (one set at 96 kHz, fixed-point scaled by 2^30) and a freq/Q/gain/type record for read-back, written together in a single 33-byte payload. There is a genuine preamp register here, so none of the numerator trickery above is needed. Writes are latched with a coefficient-commit command and then persisted to flash, so they survive unplugging.

One wrinkle worth recording: replies are tagged only with the command they answer, and several different questions share command `0x09` — reading a band and reading the active EQ slot both use it. Leave a stale reply in the queue and the next request happily consumes it, after which every answer is off by one. Draining before each send, and checking the band index the cable echoes back, is what keeps that honest.

## Run from source

You'll need the [Rust toolchain](https://rustup.rs/), plus:

- **Windows**: the MSVC C++ build tools (Visual Studio or the standalone Build Tools).
- **macOS**: the Xcode Command Line Tools (`xcode-select --install`).
- **Linux**: ALSA and libudev headers. On Debian/Ubuntu: `sudo apt install build-essential libasound2-dev libudev-dev`. You'll also need the udev rule above.

```bash
git clone https://github.com/EffectiveEquivalent/FreeDSP-Studio.git
cd FreeDSP-Studio/tools/app
cargo run --release
```

`cargo test` checks the EQ maths, auto-fit and listening-test maths against results recorded from the previous (HTML) version, in `tests/port.rs`.

## Build a release

Windows, the portable exe at `tools/app/target/release/freedsp-studio.exe`:

```bash
cargo build --release
```

Linux, a .deb and an .rpm (with the udev rule) into `tools/app/target/debian/` and `tools/app/target/generate-rpm/`:

```bash
cargo install cargo-deb cargo-generate-rpm
cargo deb
cargo generate-rpm
```

macOS: a universal .app (both architectures joined with `lipo`), ad-hoc signed and put in a dmg. The steps are in [`.github/workflows/release.yml`](.github/workflows/release.yml). Ad-hoc signing is free, needs no Apple account, and is what lets the app launch on Apple Silicon. It is not notarisation, so the Gatekeeper warning above still applies to anything downloaded. Running from source avoids both platforms' warnings entirely.

## Safety

Nothing is written until you click Write, writes are reversible, and the clip check won't let you push a distorting EQ without sorting the headroom first. Still: use at your own risk.

## Reverse engineering

This is a clean, independent implementation of the wire protocols, written for interoperability with hardware I own. There is no manufacturer code, firmware or assets in here.

## Support

If this saved you from the stock app, you can [buy me a coffee](https://buymeacoffee.com/effectiveequivalent). No pressure.

## Changelog

### 0.8.3
- The TRN Black Pearl (`3302:43E8`, which calls itself "TE-C" over USB) shows by name. It was already driven as a 10-band Walkplay scheme 16 cable
- The FiiO RETRO NANO is confirmed working on real hardware, so it no longer shows as untested
### Unreleased
- TRN Black Pearl (`3302:43E8`) gets a named profile: 10 bands, ±10 dB, peak, both shelves and low / high pass. Checked by [Contour](https://github.com/Chronosauros/contour) with this app's write sequence: everything read back as sent and survived unplugging. Untested with this app. It doesn't show up on USB at all until headphones are plugged in
- Fix: FiiO KA15 shelves were too broad and its USER banks 12 dB quiet. Measured on a line input by [Contour](https://github.com/Chronosauros/contour), the KA15 plays a low or high shelf at the stored Q divided by √2, and a USER bank 12 dB below its stored preamp. FiiO's own app writes the raw values too. Shelf Q and preamp are now sent corrected and read back as heard, so a squig.link or AutoEQ profile plays as designed. Banks written before this read back with a lower shelf Q and a preamp 12 dB down, which is what you were actually hearing, and Write all will rewrite them. Rewritten banks come out up to 12 dB louder, at the same level as Close EQ
- CrinEar Protocol Max (`3302:43CC`) gets a named profile: 10 bands, ±10 dB, peak and both shelves. Untested here

### 0.8.2
- Linux: a .tar.gz with the binary, the udev rule, a desktop entry and an install script, for distros that take neither .deb nor .rpm

### 0.8.1
- Fix: Detect could do nothing on Windows. Release builds no longer abort on an internal error, so a device read that fails is handled as before instead of stopping the app
- The connect screen has a log link, shows what Detect found or why it failed, and says so if detecting takes longer than 10 seconds

### 0.8.0
- **New native interface.** The app is now one Rust program drawn with egui, replacing Tauri and the system webview. No browser engine, no Node at build time, one process instead of three. On Linux it no longer needs WebKitGTK, GTK or anything from GNOME: it links libc, ALSA and libudev and nothing else. In testing it draws its window in about 0.2 s, where the webview version took about 1 s
- Looks and works as before: the same screens, dialogs, keyboard shortcuts and listening test. Your saved EQs, curves, treble profiles and bank sets are the same files in the same folder, so they carry over. The theme and auto-fit settings lived in the webview's storage and now live in `settings.json` beside them, so they reset once
- Auto-fit, the treble correction and the listening-test maths are ported from JavaScript to Rust, and tested against results recorded from the previous version
- Linux: the title bar can be dragged and the window resized from its edges. The AppImage is gone; use the .deb or .rpm
- About shows the IBM Plex font licence

### 0.7.1
- Linux build, untested with a real cable: .deb, .rpm and AppImage, with a udev rule so the app can open the cable without root

### 0.7.0
- FiiO KA15: read and write its three USER banks, switch banks, preamp. Tested on hardware. It carries FiiO's report on the same Consumer collection as its media keys (next to a Walkplay report it doesn't use for its banks), 63 bytes rather than the BTR17's 32, and stores its preamp in whole dB
- Every other model FiiO's own web app drives over HID: KA17, BTR13, QX13, QX11, FX17, FP3, LS-TC2, OAK NANO, K13 R2R, BR15 R2R, AIR AMP, Air Link, BT11, FG3, RETRO NANO, SnowSky Melody / TINY A / TINY B, JadeAudio JIEZI and JA11, with each model's banks, gain range, filter types and write pacing taken from FiiO's app. Untested on hardware. devicePEQ measured that the KA17 and QX13 realise a peak's Q divided by the gain-dependent RBJ factor, so those two are sent a corrected Q
- The report id, report length and collection a FiiO device uses now come from its own HID descriptor, rather than being assumed
- KT Micro devices (Moondrop CDSP and Chu II DSP, Tanchjim One / Bunny / Fission, Kiwi Ears Allegro / Chorus / S-Link, JCally JM12 and others) and the Fosi Audio DS3, from devicePEQ. Untested on hardware
- Walkplay: the product-id catalogue and per-scheme band counts, gain ranges and filter types, so the many Walkplay-chipset devices beyond the CrinEar Protocol Micro get their real limits instead of a conservative 8 bands. Named Moondrop gen-2 parts get the right preamp register, or none where the device works out its own headroom
- Preamp rounds down to the step the device stores: whole dB on the KA15 and on gen-2 cables' whole-dB register, so Auto headroom never leaves a fraction of a dB of clipping
- Design offline for any FiiO or Fosi model's bank set, not only the BTR17's

### 0.6.3
- Auto-fit, "measurement only, up to a frequency": a sharp treble feature inside the range could pull every band onto itself, leaving the rest of the range uncorrected. Fitting a Xenns Top Pro to a Dusk up to 10 kHz used 5 of 10 bands and left the bass worse than no EQ. Bands no longer start on top of each other, and stacked bands that get merged are re-placed where error is left. On 39 measurement/target pairs fitted to 10 kHz, every one improved or stayed the same (average error 0.69 to 0.26 dB, bands used 5.7 to 9.9), and fits to 6 kHz and full range improved too
- Checked against squig.link's and graph.hangout.audio's own Auto EQ on the same curves: up to 6 kHz the fit is now the most accurate of the three, and to 10 kHz it matches them. `tools/fitcheck/squig-compare.js` repeats that comparison
- L/R averaging on drop averages amplitude, exactly as squig does

### 0.6.2
- Dropping both channels of a measurement together (squig's "Name [1].txt" and "Name [2].txt", or L/R) now adds one curve averaged the way squig.link plots it, so an auto-fit made in the app matches what squig shows. A single channel dropped alone is still added, with a note that it isn't the average

### 0.6.1
- Fix: BTR17 writes could lose bands at the next power-off. The BTR17 saves each band to flash as it arrives, but skips any that arrive less than about 50 ms after the previous one, and 0.6.0 sent them 20 ms apart, so bands 5 and 9 of every write were kept only until the device was switched off. Bands are now sent 150 ms apart, and every write is checked against what the device actually saved, by switching to another bank and back (which reloads the saved copy), instead of against its working copy. You'll hear the other bank for a moment. **If you wrote banks with 0.6.0, write them again**: Write all will find the ones that no longer match

### 0.6.0
- FiiO BTR17 support: read and write any of its ten USER banks, switch banks, and a per-bank preamp (its global gain). Bank switches wait for the device to settle, since for up to 0.7 s afterwards it still reports the previous bank's EQ, which is what makes FiiO's own web app show the wrong curve. The app also never sends the command FiiO's code calls factory reset, which another tool sends as "save" on this device
- Bank sets: name, order, sort, read all and write all for the BTR17's banks, with an automatic copy of whatever a write overwrote
- No-cable mode: design for a BTR17 bank set or a generic 1 to 10 band device without anything plugged in
- Drag and drop: a squig / AutoEQ / APO parametric EQ is applied to the current EQ; a frequency response asks for a name, rig (711 by default) and source or target
- Export the EQ as a parametric .txt
- The graph's floor follows the device, down to -24 dB on the BTR17
- Low-pass and high-pass filters survive a round trip through EQ text instead of turning into peaking filters

### 0.5.2
- Auto-fit now spends the bands you ask for. Since 0.4.0 it dropped any band that improved the overall error by less than 0.15 dB, and because that error is averaged over the whole fit range, bands fixing real 1 dB wiggles were thrown away: with 8 bands it typically used about 4. Bands are now only dropped if they do essentially nothing (under 0.02 dB). On 31 test IEMs this took the fit from about 4 to 7 bands used and cut the error by 20 to 30%. To keep bands free for hand tuning, lower the Bands count
- After a whole-target fit the FR view switches to the treble-corrected view, where the target is drawn the way the fit aims at it (its broad shape above ~6 kHz) with the raw target dotted behind it

### 0.5.1
- Auto-fit no longer puts two bands on top of each other (e.g. 7106 and 7120 Hz). The whole-target fit did this most, when a narrow treble feature was only half covered by the band placed on it; the range fit did it occasionally. Bands within a twelfth of an octave are now folded into one filter, and the whole-target fit re-places the freed band where the most error is left. On 32 test IEMs, stacked pairs went from 10 to 0 in each mode, with the whole-target fit slightly more accurate overall (0.325 to 0.312 dB RMS)

### 0.5.0
- **Treble listening test.** A 711 coupler can't be trusted above about 6 kHz, and everyone's ears differ up there anyway, so the treble can now be set by ear through the cable. Two methods: *stands out* (does a band poke out of pink noise?) and *louder / quieter* (match neighbouring bands by level, solved together so small errors don't pile up). Quick or thorough, keyboard driven (arrows, space, R / V, Backspace to undo). Results are kept per IEM and tip
- Use the result two ways: **Apply to EQ** turns it straight into treble bands in the EQ's flat slots (no measurement needed), or the **whole target + treble correction** auto-fit spends every band over the full range, using the smoothed rig level, the target's broad shape and your profile above ~6 kHz instead of the rig's treble detail, up to an upper limit you set. The FR view's TREBLE switch shows the measurement with and without the correction
- **New layout.** A datasheet-style header with the connected cable's details, the graph as Fig. 1 and the bands as a table (Table 1), and the workflow as numbered steps: measurement, fit, treble by ear, write. Auto-fit settings are on screen instead of in a dialog; saved EQs open from the header
- Band frequency, gain and Q, and the preamp, are typed instead of dragged on sliders: `8k`, `-3.5`, `0.71`; arrow keys nudge, Shift for bigger steps

### 0.4.0
- Auto-fit rewritten so it lands where squig.link's Auto EQ does. The old fit searched each band's frequency across the whole range with a method that assumes a single minimum, so bands teleported across the spectrum, piled up next to each other and cancelled out. A band now only moves within an octave of where it sits
- A gain-capped band can no longer drag every following band onto the same frequency, and bands start at or below 10 kHz
- Bands the fit doesn't need are left flat for you to use by hand, the way squig leaves filters unused
- Outside the configured fit range the EQ is kept flat rather than chasing a part of the measurement you have said not to trust
- On Super* Review's Chu 3 fitted to the Dusk 2, 20 Hz to 6 kHz with 8 bands, the error against the target fell from 0.74 dB to 0.08 dB RMS (squig.link: 0.15 dB)

### 0.3.0
- Support for a second generation of cable, on a different wire protocol: HID report `0x4B` over the interrupt endpoints, 8 bands, and a real preamp register instead of the coefficient trick gen-1 needs
- **Moondrop FreeDSP Mini** (`35D8:98D4`) — 8 bands, ±12 dB, peak / low shelf / high shelf, plus its five firmware voicings
- **CrinEar Protocol Micro** (`3302:C20F`) — 8 bands, ±10 dB, peak / low shelf / low pass / high pass
- Other cables on the same chipset family should work without being listed: the generation is settled by asking the cable, not by matching a product id
- Band count, gain range and filter types now follow whatever cable is connected, rather than assuming gen-1's fixed 9 bands
- Plug in more than one cable and the app asks which to drive
- Gen-2 writes are verified by reading the bank back, so the write count means bands the cable is actually holding — a write that doesn't land now says so instead of reporting success
- Read-back on gen-2 is full resolution, so the whole-dB rounding note doesn't apply to those cables
- Fix: a read could quietly return another request's answer. Replies carry only the command they answer, and reading a band and reading the active EQ slot share one — a stale reply left in the queue would satisfy the next request and shift every answer after it. The queue is now drained before each request and the echoed band index checked

### 0.2.6
- Fix: crash on macOS while talking to the cable. Every device operation ran on a fresh short-lived thread and re-enumerated the whole HID bus, which drives hidapi's process-global `IOHIDManager` far harder than it is designed for — device operations now run on one long-lived thread, and the bus is only re-enumerated when opening the cable actually fails
- Fix: the window couldn't be dragged by its title bar on macOS. The title bar drag relied on `-webkit-app-region`, which WebView2 honours on Windows but WKWebView ignores; macOS now uses Tauri's drag region, which also needed the `core:window:allow-start-dragging` permission it had never been granted
- Double-clicking the title bar now zooms the window on macOS

### 0.2.5
- macOS support: universal build for Apple Silicon and Intel
- HID layer split per platform — Win32 on Windows, hidapi over IOKit on macOS — with the wire protocol, biquad maths and preamp shared between them
- Native title bar and traffic lights on macOS
- macOS builds are ad-hoc signed automatically (free, not notarised, see Download)

### 0.2.0
- Frequency-response view (measurement + target + EQ'd result)
- Auto-fit EQ to a target, based on AutoEq, with configurable frequency range and band count
- Measurement / target library with rig lock (5128 / 711 / other) and bulk import
- Any measurement can be used as a target for another IEM
- Backup and restore all saved EQs and FR curves to a file
- Edit the EQ as squig / AutoEQ text

### 0.1.0
- Read, edit and write the cable's parametric EQ (9 bands on gen-1, 8 on gen-2)
- Live response graph with draggable band nodes
- Fractional-dB gain and a coefficient-based preamp with clip check
- squig.link / AutoEQ import and export
- Firmware profile switching (DUSK / MAY / FreeDSP Mini), save and recall EQs

## Credits

- Auto-fit is based on [AutoEq](https://github.com/jaakkopasanen/AutoEq) by Jaakko Pasanen (MIT).

## Built with

Built with [Claude](https://claude.com/claude-code). The reverse engineering, hardware testing and decisions were mine; Claude wrote most of the code. Took a good few hours....

## License

[MIT](LICENSE) © 2026 EffectiveEquivalent
