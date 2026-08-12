# FreeDSP Studio

An EQ editor for Moondrop's DSP USB-C cables (DUSK-SP, MAY and FreeDSP). It reads whatever EQ is on the cable, lets you edit the 9 parametric bands by dragging points on the response curve, imports from squig.link, and writes it back. Built in Rust with Tauri, so it's a few MB of native app rather than a bundled browser.

I made this because the stock app is clunky, locked to whole-dB steps, and it turns out the cable can do WAY more than the app lets on.

> Independent project. Nothing to do with Moondrop, not affiliated, not endorsed. For cables you own.

*(Screenshot goes here: the response graph with draggable band nodes.)*

## Download

Grab `FreeDSP-Studio.exe` from the [latest release](https://github.com/EffectiveEquivalent/FreeDSP-Studio/releases). It's a single portable exe, nothing to install.

**Heads up: Windows will warn you about it.** The build isn't code-signed yet, so SmartScreen shows "unknown publisher" for any exe it hasn't seen before. That's a reputation check, not a virus detection. Click More info, then Run anyway. If you'd rather not, run it from source instead (below), or wait: signed releases via [SignPath](https://signpath.org) are in the works.

## Supported cables

| Cable | USB ID (VID:PID) | Firmware presets |
|---|---|---|
| Moondrop DUSK-SP | `35D8:1499` | Crinacle tunings |
| Moondrop MAY | `35D8:1497` | Standard / Bass Head / Reference / No Bass / Harman |
| Moondrop FreeDSP | `35D8:1496` | none (custom bank only) |

Windows only for now (the HID layer talks to `hid.dll` directly). A Mac port is planned.

## What it does

- Detects the cable and pulls in whatever EQ is loaded on it
- 9-band parametric EQ (peak / low shelf / high shelf).... drag the numbered nodes straight on the graph, or use the sliders
- Proper fractional-dB gain, not the whole numbers the stock app limits you to (see below)
- A real preamp, with an auto headroom button and a clip check before every write
- Switches the active firmware profile on the DUSK and MAY
- squig.link / AutoEQ import and export (Equalizer APO parametric text)
- Saves your EQs as plain JSON files you can back up or share
- Nothing touches the cable until you hit Write

## The interesting bits

I reverse engineered the cable's HID protocol to build this, and a few genuinely cool things fell out of it:

**The EQ lives on the cable, not the app.** The DSP chip has its own memory. Whatever you write follows the cable to any phone or laptop with zero software running.

**The whole-dB limit isn't real.** Each band stores two separate things: a little metadata record where gain is a whole-dB integer, and the actual biquad filter coefficients that do the DSP. The metadata is basically a label for the app to display. The sound comes ENTIRELY from the coefficients, which are computed at full precision, so 2.3 dB really is 2.3 dB.... it just reads back as 2 because the readable field can't hold decimals. Proved on hardware by writing metadata that said 0 dB with coefficients for a -12 dB shelf: read-back said flat, the bass was audibly gone.

**The preamp is the bit I'm most chuffed with.** The cable has no preamp register at all. But scaling one band's biquad numerator by `k = 10^(preamp/20)` shifts that filter's response by a flat amount at every frequency, and because the 9 bands are cascaded, the WHOLE combined curve moves down with its shape intact. A mathematically exact preamp riding on a filter that's already there, costing nothing. It sits on the last active band so the rest of the chain runs hot (better signal-to-noise, thanks to oratory1990 for the nudge).

Nerdy footnote: the cable stores a separate coefficient set per sample rate (44.1 / 48 / 96 / 192 / 384 kHz), fixed-point scaled by 2^22. Silicon is a Conexant CX2077x-class DSP; control is HID over USB.

## Run from source

You'll need [Node.js](https://nodejs.org/), the [Rust toolchain](https://rustup.rs/), and on Windows the MSVC C++ build tools (Visual Studio or the standalone Build Tools). WebView2 is already on any normal Windows 11 install.

```bash
git clone https://github.com/EffectiveEquivalent/FreeDSP-Studio.git
cd FreeDSP-Studio/tools/app
npm install
npx tauri dev
```

## Build a release

```bash
cd tools/app
npx tauri build
```

Output lands in `tools/app/src-tauri/target/release/`. Builds are unsigned for now, so SmartScreen will moan about an unknown publisher. Running from source avoids that entirely.

## Safety

Nothing is written until you click Write, writes are reversible, and the clip check won't let you push a distorting EQ without sorting the headroom first. Still: use at your own risk.

## Reverse engineering

This is a clean, independent implementation of the wire protocol, written for interoperability with my own hardware. There is no Moondrop code, firmware or assets in here.

## Support

If this saved you from the stock app, you can [buy me a coffee](https://buymeacoffee.com/effectiveequivalent). No pressure.

## Built with

Built with [Claude](https://claude.com/claude-code). The reverse engineering, hardware testing and decisions were mine; Claude wrote most of the code. Took a good few hours....

## License

[MIT](LICENSE) © 2026 EffectiveEquivalent
