// Gen-2 DSP wire protocol — platform-free.
//
// The newer Moondrop cables (FreeDSP Mini/Pro, MOONRIVER 3, Rays, DAWN PRO 2 …) and the
// Walkplay-chipset devices sold under other brands (CrinEar Protocol Micro/Max, Tanchjim,
// EPZ, KiwiEars …) share one protocol, which has nothing in common with the gen-1 cables in
// dsp.rs.
//
// Wire format — HID report 0x4B, 64-byte reports, on the *interrupt* endpoints (hid_write /
// hid_read), not the control pipe gen-1 needs:
//
//     byte 0   report id (0x4B)
//     byte 1   OP    — 0x01 write, 0x80 read
//     byte 2   CMD
//     byte 3   LEN   — payload length
//     byte 4.. payload
//
// Replies come back as input reports on the same report id, echoing OP and CMD. Offsets in
// this file are quoted against the report *without* its id byte, so a field at 27 here sits
// at 28 in a raw frame.
//
// The device stores, per band, both the IIR coefficients it runs and the freq/Q/gain/type
// that produced them — the latter purely so a host can read the EQ back. Both are written in
// one 33-byte payload.

use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::Duration;

use serde::Serialize;

use crate::dsp::{Band, BandIn, WriteResult};

pub const REPORT_ID: u8 = 0x4B;
pub const FRAME_LEN: usize = 64;

const OP_WRITE: u8 = 0x01;
const OP_READ: u8 = 0x80;

const CMD_EQ_SAVE_FLASH: u8 = 0x01;
const CMD_GLOBAL_GAIN: u8 = 0x03;
const CMD_OFFSET_SAVE_FLASH: u8 = 0x04;
const CMD_EQ_UPDATE: u8 = 0x09;
const CMD_EQ_COEFF: u8 = 0x0A;
const CMD_VERSION: u8 = 0x0C;
const CMD_EQ_ACTIVE: u8 = 0x0F;
const CMD_PRE_GAIN: u8 = 0x23;

// Field offsets within a report, report id excluded — OP at 0, CMD at 1, LEN at 2, payload
// from PAY on. Fields are named by their position in a reply, and the payload-relative
// positions derived from those rather than written out twice.
const PAY: usize = 3;
const OFF_INDEX: usize = 4;
const OFF_COEFFS: usize = 7;
const OFF_FREQ: usize = 27;
const OFF_Q: usize = 29;
const OFF_GAIN: usize = 31;
const OFF_TYPE: usize = 33;
const OFF_SLOT: usize = 35;

// The device runs its biquads at a fixed 96 kHz and quantises coefficients to Q2.30 — one
// coefficient set per band, where gen-1 needed five (one per sample rate).
const COEFF_SCALE: f64 = 1_073_741_824.0; // 2^30
const COEFF_FS: f64 = 96_000.0;

// Pacing. The cables drop frames if pushed harder than this.
const DELAY_BAND_MS: u64 = 25;
const DELAY_OP_MS: u64 = 50;
const DELAY_FLASH_MS: u64 = 200;
const DELAY_WRITE_EQ_MS: u64 = 150;

/// One gen-2 cable, opened. Unlike gen-1 this is a pair of interrupt transfers rather than a
/// control-pipe request/response, so the read has to be filtered and timed out by the caller.
pub trait Transport2 {
    /// Sends one 64-byte report. True if the write reached the driver.
    fn send(&self, frame: &[u8]) -> bool;
    /// Waits for an input report on REPORT_ID whose CMD byte matches, id byte stripped.
    fn recv(&self, cmd: u8, timeout_ms: u64) -> Option<Vec<u8>>;
    /// Throws away input reports still queued from an earlier command.
    fn drain(&self);
}

// ---- device profiles ----

/// What a given cable can actually do. Band counts and gain limits differ per model, and the
/// UI has to follow them rather than assume gen-1's fixed 9 bands.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub name: String,
    pub bands: usize,
    pub gain_min: f64,
    pub gain_max: f64,
    pub q_min: f64,
    pub q_max: f64,
    /// Filter types this cable accepts, in the order the UI should cycle them.
    pub types: Vec<String>,
    /// Preamp register: "pre" = PRE_GAIN (0x23, 0.1 dB steps), "global" = GLOBAL_GAIN (0x03,
    /// whole dB), "none" = no preamp support.
    pub preamp: String,
    /// Fixed attenuation the hardware already applies, which the preamp budget sits on top
    /// of. Walkplay parts reserve -5 dB; only the excess is written to the register.
    pub preamp_buffer: f64,
    /// Firmware voicings this cable can be switched to, keyed by EQ slot, plus the editable
    /// slot itself. Cables with nothing but the editable slot get a single entry.
    pub presets: BTreeMap<u8, String>,
    /// The slot the editable bank lives in — 7 on Moondrop parts, 101 on Walkplay ones.
    /// Band writes have to quote it, and switching away from it makes the EQ read-only.
    pub custom_slot: u8,
    /// "Moondrop" or "Walkplay", for display.
    pub family: String,
    /// Checked on real hardware with this app, rather than taken from devicePEQ's tables.
    pub tested: bool,
}

// Walkplay's chip firmware comes in "schemes", each with a fixed band count, gain range and set
// of filter types, and a product id belongs to one scheme whatever the vendor id. The table is
// devicePEQ's (github.com/jeromeof/devicePEQ, usbDeviceConfig.js), taken from Walkplay's own
// catalogue; where a product id is listed under two schemes the first listed wins, as there.
struct Scheme {
    no: u8,
    bands: usize,
    gain: f64,
    types: &'static [&'static str],
    pids: &'static [u16],
}

const PK: &[&str] = &["PK"];
const SHELVES: &[&str] = &["PK", "LSQ", "HSQ"];
const LS_PASS: &[&str] = &["PK", "LSQ", "LP", "HP"];
const SHELVES_PASS: &[&str] = &["PK", "LSQ", "HSQ", "LP", "HP"];

const SCHEMES: &[Scheme] = &[
    Scheme { no: 11, bands: 8, gain: 10.0, types: LS_PASS, pids: &[
        0xC20C, 0x0004, 0x00C0, 0x0104, 0x0880, 0x1230, 0x1231, 0x1233, 0x1237, 0x123F, 0x1240, 0x1241, 0x1243, 0x1244,
        0x1245, 0x1248, 0x1249, 0x124A, 0x124B, 0x124C, 0x124D, 0x124E, 0x1251, 0x1261, 0x1262, 0x1264, 0x1266, 0x1269,
        0x126A, 0x126B, 0x126C, 0x126D, 0x126E, 0x126F, 0x1272, 0x1278, 0x127A, 0x127D, 0x127E, 0x1281, 0x1282, 0x1283,
        0x1284, 0x1285, 0x1286, 0x1287, 0x1288, 0x1289, 0x128A, 0x128B, 0x128C, 0x128D, 0x128E, 0x128F, 0x1292, 0x1293,
        0x1294, 0x1295, 0x1296, 0x1297, 0x1298, 0x1299, 0x129A, 0x129B, 0x129C, 0x129D, 0x129F, 0x12B3, 0x12C0, 0x12C1,
        0x12C3, 0x12C4, 0x12C5, 0x12C6, 0x12C8, 0x12C9, 0x12CA, 0x12CB, 0x12CC, 0x12CD, 0x12CE, 0x12DB, 0x12E9, 0x132B,
        0x13A3, 0x13A4, 0x13A5, 0x13AB, 0x13C0, 0x13C1, 0x13D3, 0x13D4, 0x13D7, 0x13D9, 0x13DC, 0x3302, 0x4302, 0x43C1,
        0x43C3, 0x43D1, 0x43D5, 0x43DC, 0x43E7, 0x51C0, 0x60D2, 0x9121, 0x9123, 0x9124, 0x9125, 0x93C0, 0x93C1, 0x93D1,
        0x98C0, 0x98C1, 0x98C2, 0x98D1, 0x98D2, 0x98D5, 0xA862, 0xC204, 0xC207, 0xC208, 0xC209, 0xC20A, 0xC20E, 0xC20F,
        0xC211, 0xC212, 0xC213, 0xC214, 0xC215, 0xC217, 0xF806, 0xF807, 0xFF01] },
    Scheme { no: 16, bands: 10, gain: 10.0, types: SHELVES, pids: &[
        0x011D, 0x39E0, 0x4301, 0x4302, 0x4304, 0x4305, 0x4306, 0x430D, 0x430E, 0x430F, 0x4311, 0x4312, 0x4313, 0x4316,
        0x4318, 0x4319, 0x431D, 0x4321, 0x4322, 0x4323, 0x4324, 0x4325, 0x4326, 0x4327, 0x4329, 0x432A, 0x432C, 0x4351,
        0x4352, 0x4355, 0x4358, 0x4359, 0x435A, 0x435C, 0x435D, 0x435E, 0x4360, 0x4361, 0x4363, 0x4364, 0x4366, 0x4367,
        0x4380, 0x4381, 0x4382, 0x4383, 0x4386, 0x43B1, 0x43B6, 0x43B7, 0x43B8, 0x43BC, 0x43BE, 0x43BF, 0x43C0, 0x43C2,
        0x43C5, 0x43C6, 0x43C7, 0x43C8, 0x43C9, 0x43CA, 0x43CC, 0x43CD, 0x43CF, 0x43D7, 0x43D8, 0x43DA, 0x43DB, 0x43DE,
        0x43E1, 0x43E4, 0x43E6, 0x43E8, 0x43EC, 0x43EF, 0x9201, 0x98D4, 0xEE10, 0xEE20, 0xF808] },
    Scheme { no: 15, bands: 8, gain: 10.0, types: SHELVES, pids: &[
        0x012A, 0x35D8, 0x39C1, 0x4353, 0x4357, 0x4362, 0x4370, 0x43CB, 0x43D6, 0x43D9, 0x43DF, 0x43E2, 0x43E3, 0x43EA,
        0x43EB] },
    Scheme { no: 13, bands: 10, gain: 10.0, types: SHELVES, pids: &[
        0x011B, 0x0123, 0x120C, 0x1320, 0x1321, 0x1326, 0x1327, 0x1328, 0x1329, 0x132A, 0x1330, 0x1332, 0x1333, 0x1334,
        0x13A9, 0x13AC, 0x13AE, 0x13AF, 0x13B0, 0x13B1, 0x13B2, 0x13B4, 0x13B6, 0x13B9, 0x13BA, 0x13BB, 0x13BE, 0x13BF,
        0x13DF, 0x23C0, 0x23C1, 0x60C0, 0x60C1, 0x60C3, 0x60C6, 0x60C7, 0x60C9, 0x60CA, 0x60CB, 0x60D1, 0x60E1, 0xB301,
        0xB302] },
    Scheme { no: 17, bands: 5, gain: 10.0, types: SHELVES, pids: &[
        0x2010, 0x201D, 0x201E, 0x2030, 0x2036, 0x2038, 0x203A, 0x203E, 0x2040, 0x2043, 0x2044, 0x2048, 0x2049, 0x20E1,
        0x20E2, 0x20E3, 0x20E5, 0x20E7, 0x20E8, 0x20EA, 0x20EC, 0x20EE, 0x20EF, 0x20FF, 0x2DC1] },
    Scheme { no: 18, bands: 10, gain: 10.0, types: SHELVES, pids: &[
        0x39A0, 0x39A2, 0x39A3, 0x39C2, 0x39C3, 0x39C4, 0x39C6, 0x39C7, 0x39C8, 0x39C9, 0x39CD, 0x39CE, 0x39CF, 0x44D1,
        0x44D2, 0x44D3, 0x44D6, 0x44D7, 0x44D9] },
    Scheme { no: 10, bands: 8, gain: 10.0, types: PK, pids: &[0x0881, 0x0888] },
    Scheme { no: 19, bands: 6, gain: 10.0, types: SHELVES, pids: &[
        0x129F, 0x231E, 0x231F, 0x2320, 0x2321, 0x2323, 0x2326, 0x232B, 0x232C, 0x2334, 0x23E2, 0x23EE] },
    Scheme { no: 20, bands: 10, gain: 10.0, types: PK, pids: &[0x0883, 0x1323, 0x13B7] },
    Scheme { no: 21, bands: 8, gain: 10.0, types: PK, pids: &[
        0x0880, 0x08F2, 0x13F2, 0x3DB0, 0x3DC1, 0x3DC4, 0x3DC5, 0x3DC6, 0x3DC7, 0x3DC8, 0x3DC9, 0x3DCA, 0x3DCC, 0x3DCD] },
];

/// Models devicePEQ knows by product name, for parts whose product id isn't in a scheme or
/// whose name says more than the id does. (name, bands, gain min, gain max, q max, types,
/// preamp). Moondrop's own parts take the 0.1 dB PRE_GAIN register, except those that work out
/// their own headroom from the coefficients ("none"), where a preamp on top would be counted
/// twice. Walkplay parts take the whole-dB global register.
struct Named {
    name: &'static str,
    bands: usize,
    gain: (f64, f64),
    q_max: f64,
    types: &'static [&'static str],
    preamp: &'static str,
    family: &'static str,
}

const NAMED: &[Named] = &[
    Named { name: "FreeDSP Pro", bands: 8, gain: (-12.0, 12.0), q_max: 10.0, types: SHELVES, preamp: "none", family: "Moondrop" },
    Named { name: "MOONRIVER 3", bands: 8, gain: (-12.0, 12.0), q_max: 10.0, types: SHELVES, preamp: "none", family: "Moondrop" },
    Named { name: "AG Rays", bands: 8, gain: (-12.0, 12.0), q_max: 10.0, types: PK, preamp: "pre", family: "Moondrop" },
    Named { name: "Rays", bands: 8, gain: (-12.0, 12.0), q_max: 10.0, types: PK, preamp: "none", family: "Moondrop" },
    Named { name: "Marigold", bands: 8, gain: (-12.0, 6.0), q_max: 10.0, types: PK, preamp: "pre", family: "Moondrop" },
    Named { name: "DAWN PRO2", bands: 8, gain: (-12.0, 12.0), q_max: 10.0, types: SHELVES, preamp: "pre", family: "Moondrop" },
    Named { name: "Echo A", bands: 8, gain: (-12.0, 12.0), q_max: 10.0, types: PK, preamp: "pre", family: "Moondrop" },
    Named { name: "DHA15", bands: 8, gain: (-12.0, 12.0), q_max: 10.0, types: SHELVES, preamp: "pre", family: "Moondrop" },
    Named { name: "Deco Audio System", bands: 8, gain: (-12.0, 12.0), q_max: 10.0, types: SHELVES, preamp: "pre", family: "Moondrop" },
    Named { name: "INN Deco75-DH Audio", bands: 8, gain: (-12.0, 12.0), q_max: 10.0, types: SHELVES, preamp: "pre", family: "Moondrop" },
    Named { name: "TANCHJIM-SPACE PRO", bands: 10, gain: (-10.0, 10.0), q_max: 10.0, types: PK, preamp: "global", family: "Walkplay" },
    // Tanchjim's own app caps this one's Q at 5.
    Named { name: "TANCHJIM-OLA II DSP", bands: 8, gain: (-10.0, 10.0), q_max: 5.0, types: PK, preamp: "global", family: "Walkplay" },
    Named { name: "TANCHJIM-STARGATE II", bands: 8, gain: (-10.0, 10.0), q_max: 10.0, types: PK, preamp: "global", family: "Walkplay" },
    Named { name: "Protocol Max", bands: 10, gain: (-10.0, 10.0), q_max: 10.0, types: SHELVES, preamp: "global", family: "Walkplay" },
    Named { name: "Octave", bands: 10, gain: (-10.0, 10.0), q_max: 10.0, types: SHELVES, preamp: "global", family: "Walkplay" },
    Named { name: "CS43131 HiFi Audio DSP", bands: 8, gain: (-10.0, 10.0), q_max: 10.0, types: LS_PASS, preamp: "global", family: "Walkplay" },
    // devicePEQ: this one works out its own headroom.
    Named { name: "CS43198 HiFi DSP Audio", bands: 8, gain: (-10.0, 10.0), q_max: 10.0, types: LS_PASS, preamp: "none", family: "Walkplay" },
    Named { name: "DAWN PRO 2", bands: 8, gain: (-10.0, 10.0), q_max: 10.0, types: SHELVES, preamp: "global", family: "Walkplay" },
    Named { name: "TRUTHEAR KEYX", bands: 8, gain: (-10.0, 10.0), q_max: 10.0, types: SHELVES, preamp: "global", family: "Walkplay" },
    Named { name: "ddHiFi DSP Cable - Memory", bands: 8, gain: (-10.0, 10.0), q_max: 10.0, types: SHELVES, preamp: "global", family: "Walkplay" },
    Named { name: "ddHiFi DSP IEM - Memory", bands: 8, gain: (-12.0, 12.0), q_max: 10.0, types: SHELVES, preamp: "pre", family: "Moondrop" },
    Named { name: "BGVP MX1", bands: 8, gain: (-10.0, 10.0), q_max: 10.0, types: SHELVES, preamp: "global", family: "Walkplay" },
    Named { name: "DT04", bands: 8, gain: (-10.0, 10.0), q_max: 10.0, types: SHELVES, preamp: "global", family: "Walkplay" },
];

fn profile(vid: u16, pid: u16, product: &str) -> Profile {
    let types = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    let slots = |v: &[(u8, &str)]| {
        v.iter()
            .map(|(k, s)| (*k, s.to_string()))
            .collect::<BTreeMap<u8, String>>()
    };
    let name = if (vid, pid, product.trim()) == (0x3302, 0x43E8, "TE-C") {
        // TRN Black Pearl, which only gives the chip's generic name over USB.
        "TRN Black Pearl".to_string()
    } else if product.is_empty() {
        format!("{:04x}:{:04x}", vid, pid)
    } else {
        product.to_string()
    };
    match (vid, pid) {
        // Moondrop FreeDSP Mini. 8 bands, +/-12 dB, peaking + both shelves, and a real
        // 0.1 dB preamp register.
        (0x35D8, 0x98D4) => Profile {
            name,
            bands: 8,
            gain_min: -12.0,
            gain_max: 12.0,
            q_min: 0.1,
            q_max: 10.0,
            types: types(&["PK", "LSQ", "HSQ"]),
            preamp: "pre".into(),
            preamp_buffer: 0.0,
            // Readable names for the cable's five firmware voicings, in slot order.
            // Slot 7 is the editable bank.
            presets: slots(&[
                (0, "Default"),
                (1, "Bass Enhance"),
                (2, "Bass Enhance+"),
                (3, "Space"),
                (4, "Treble Enhance"),
                (7, "Custom EQ"),
            ]),
            custom_slot: 7,
            family: "Moondrop".into(),
            tested: true,
        },
        // CrinEar Protocol Micro. 8 bands, +/-10 dB, low shelf but no high shelf, and
        // low/high pass instead; preamp is the whole-dB global gain register.
        (0x3302, 0xC20F) => Profile {
            name,
            bands: 8,
            gain_min: -10.0,
            gain_max: 10.0,
            q_min: 0.1,
            q_max: 10.0,
            types: types(&["PK", "LSQ", "LP", "HP"]),
            preamp: "global".into(),
            preamp_buffer: -5.0,
            presets: slots(&[(101, "Custom EQ")]),
            custom_slot: 101,
            family: "Walkplay".into(),
            tested: true,
        },
        // CrinEar Protocol Max, Walkplay scheme 16. 10 bands, +/-10 dB, peaking and both
        // shelves on their native codes, with none of scheme 11's frequency or Q offset. Not
        // tested here; Contour (github.com/Chronosauros/contour) has driven one on these limits.
        // Its slot map is unconfirmed, so writes go to whatever slot the cable reports, and
        // the Micro's -5 dB buffer isn't assumed.
        (0x3302, 0x43CC) => Profile {
            name: if product.is_empty() { "CrinEar Protocol Max".into() } else { name },
            bands: 10,
            gain_min: -10.0,
            gain_max: 10.0,
            q_min: 0.1,
            q_max: 10.0,
            types: types(SHELVES),
            preamp: "global".into(),
            preamp_buffer: 0.0,
            presets: BTreeMap::new(),
            custom_slot: 0,
            family: "Walkplay (scheme 16)".into(),
            tested: false,
        },
        // TRN Black Pearl, Walkplay scheme 16, firmware 0.6. It only shows up on USB with
        // headphones plugged in. Contour (github.com/Chronosauros/contour) ran this file's write
        // sequence against one on 2026-10-19: every register and biquad byte read back as sent,
        // and the bank and the preamp survived unplugging. Low and high pass (codes 4 and 5)
        // were stored as sent rather than turned into peaks. The bank sits in slot 0, which is
        // what the cable reports, so it is read rather than assumed, as on the Max.
        (0x3302, 0x43E8) => Profile {
            name: if product.is_empty() { "TRN Black Pearl".into() } else { name },
            bands: 10,
            gain_min: -10.0,
            gain_max: 10.0,
            q_min: 0.1,
            q_max: 10.0,
            types: types(SHELVES_PASS),
            preamp: "global".into(),
            preamp_buffer: 0.0,
            presets: BTreeMap::new(),
            custom_slot: 0,
            family: "Walkplay (scheme 16)".into(),
            tested: false,
        },
        _ => {
            // Slot unknown, so the editable one is whatever the cable reports at open.
            let base = |bands, gmin, gmax, qmax, t: &[&str], pre: &str, family: &str| Profile {
                name: name.clone(),
                bands,
                gain_min: gmin,
                gain_max: gmax,
                q_min: 0.1,
                q_max: qmax,
                types: types(t),
                preamp: pre.into(),
                preamp_buffer: 0.0,
                presets: BTreeMap::new(),
                custom_slot: 0,
                family: family.into(),
                tested: false,
            };
            let lower = product.trim().to_lowercase();
            if let Some(n) = NAMED.iter().find(|n| n.name.to_lowercase() == lower) {
                return base(n.bands, n.gain.0, n.gain.1, n.q_max, n.types, n.preamp, n.family);
            }
            let family = if vid == crate::dsp::MOONDROP_VID { "Moondrop" } else { "Walkplay" };
            let pre = if vid == crate::dsp::MOONDROP_VID { "pre" } else { "global" };
            if let Some(sc) = SCHEMES.iter().find(|sc| sc.pids.contains(&pid)) {
                // The scheme number is what devicePEQ files Walkplay quirks under.
                let family = format!("{} (Walkplay scheme {})", family, sc.no);
                return base(sc.bands, -sc.gain, sc.gain, 10.0, sc.types, pre, &family);
            }
            // Anything else that answers the gen-2 handshake. Deliberately conservative: 8 bands
            // and +/-10 dB is the smallest of the common profiles, so a cable we haven't
            // characterised can't be driven past what it accepts.
            base(8, -10.0, 10.0, 10.0, SHELVES, "global", family)
        }
    }
}

pub fn cable_profile(vid: u16, pid: u16, product: &str) -> Profile {
    profile(vid, pid, product)
}

// ---- frames ----

fn frame(op: u8, cmd: u8, payload: &[u8]) -> [u8; FRAME_LEN] {
    let mut b = [0u8; FRAME_LEN];
    b[0] = REPORT_ID;
    b[1] = op;
    b[2] = cmd;
    b[3] = payload.len() as u8;
    b[4..4 + payload.len()].copy_from_slice(payload);
    b
}

fn type_code(t: &str) -> u8 {
    match t {
        "LSQ" => 1,
        "HSQ" => 3,
        "LP" => 4,
        "HP" => 5,
        _ => 2, // PK
    }
}

fn type_name(c: u8) -> &'static str {
    match c {
        1 => "LSQ",
        3 => "HSQ",
        4 => "LP",
        5 => "HP",
        _ => "PK",
    }
}

// ---- biquad ----

/// RBJ sections returned as (a, b), with a0 already normalised to 1.
///
/// The grouping below is not arbitrary. Algebraically equivalent rearrangements round
/// differently once quantised to Q2.30, and the cable stores exactly what it is given, so the
/// form matters for getting the same filter the hardware would otherwise compute.
fn biquad(typ: u8, f: f64, gain: f64, q: f64) -> ([f64; 3], [f64; 3]) {
    let w = f * std::f64::consts::PI * 2.0 / COEFF_FS;
    let (cs, sn) = (w.cos(), w.sin());
    match typ {
        1 | 3 => {
            let r = 10f64.powf(gain / 40.0);
            let i = sn / 2.0 * ((r + 1.0 / r) * (1.0 / q - 1.0) + 2.0).sqrt();
            let rs = 2.0 * r.sqrt() * i;
            if typ == 1 {
                let l = r + 1.0 + (r - 1.0) * cs + rs;
                (
                    [
                        1.0,
                        -2.0 * (r - 1.0 + (r + 1.0) * cs) / l,
                        (r + 1.0 + (r - 1.0) * cs - rs) / l,
                    ],
                    [
                        r * (r + 1.0 - (r - 1.0) * cs + rs) / l,
                        2.0 * r * (r - 1.0 - (r + 1.0) * cs) / l,
                        r * (r + 1.0 - (r - 1.0) * cs - rs) / l,
                    ],
                )
            } else {
                let l = r + 1.0 - (r - 1.0) * cs + rs;
                (
                    [
                        1.0,
                        2.0 * (r - 1.0 - (r + 1.0) * cs) / l,
                        (r + 1.0 - (r - 1.0) * cs - rs) / l,
                    ],
                    [
                        r * (r + 1.0 + (r - 1.0) * cs + rs) / l,
                        -2.0 * r * (r - 1.0 + (r + 1.0) * cs) / l,
                        r * (r + 1.0 + (r - 1.0) * cs - rs) / l,
                    ],
                )
            }
        }
        4 => {
            let al = sn / (2.0 * q);
            let d = al + 1.0;
            (
                [1.0, cs * -2.0 / d, (1.0 - al) / d],
                [(1.0 - cs) / 2.0 / d, (1.0 - cs) / d, (1.0 - cs) / 2.0 / d],
            )
        }
        5 => {
            let al = sn / (2.0 * q);
            let d = al + 1.0;
            (
                [1.0, cs * -2.0 / d, (1.0 - al) / d],
                [(1.0 + cs) / 2.0 / d, (-1.0 - cs) / d, (1.0 + cs) / 2.0 / d],
            )
        }
        _ => {
            let r = 10f64.powf(gain / 20.0).sqrt();
            let al = sn / (2.0 * q);
            let d = al / r + 1.0;
            (
                [1.0, cs * -2.0 / d, (1.0 - al / r) / d],
                [(al * r + 1.0) / d, cs * -2.0 / d, (1.0 - al * r) / d],
            )
        }
    }
}

/// The 20 coefficient bytes: b0, b1, b2, -a1, -a2 as little-endian i32 at Q2.30.
///
/// A coefficient past the i32 range wraps rather than saturating — which is what the cable
/// expects, and what the wrapping cast here gives. Saturating instead would change the filter
/// on wide low-frequency shelves, where b1 can exceed 2.0.
fn coeffs(typ: u8, f: f64, gain: f64, q: f64) -> [u8; 20] {
    let (a, b) = biquad(typ, f, gain, q);
    let q30 = |v: f64| ((v * COEFF_SCALE).round() as i64) as i32;
    let vals = [q30(b[0]), q30(b[1]), q30(b[2]), -q30(a[1]), -q30(a[2])];
    let mut out = [0u8; 20];
    for (i, v) in vals.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

// ---- command sequences ----

// Replies are only tagged with the command they answer, and several different questions
// share one command — a band read and a slot read are both 0x09. So a reply left over from
// an earlier request will satisfy the next one and every answer after it is off by one.
// Draining before each send is what keeps requests and replies paired up.
//
// A cable answers in about 30 ms, so this is an order of magnitude of headroom; the retry
// covers the occasional report that never arrives rather than one that is merely slow. It is
// also what a gen-1 cable costs during detection, since it answers report 0x4B with silence —
// keeping it tight keeps "Detect" snappy for those.
const REPLY_TIMEOUT_MS: u64 = 300;

fn request<T: Transport2 + ?Sized>(dev: &T, op: u8, cmd: u8, payload: &[u8]) -> Option<Vec<u8>> {
    for _ in 0..2 {
        dev.drain();
        if !dev.send(&frame(op, cmd, payload)) {
            continue;
        }
        if let Some(r) = dev.recv(cmd, REPLY_TIMEOUT_MS) {
            return Some(r);
        }
    }
    None
}

/// Firmware version string, e.g. "0.7". Doubles as the gen-2 handshake: a gen-1 cable has no
/// report 0x4B and never answers, so a reply here is what tells the two generations apart.
pub fn read_version<T: Transport2 + ?Sized>(dev: &T) -> Option<String> {
    let r = request(dev, OP_READ, CMD_VERSION, &[0x00])?;
    if r.len() < 6 {
        return None;
    }
    let s: String = r[3..6]
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as char)
        .filter(|c| c.is_ascii_graphic())
        .collect();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// The EQ slot the cable is currently running. Moondrop parts answer 7, Walkplay parts 101;
/// it has to be echoed back in every band write or the write lands nowhere.
///
/// Read from a band reply rather than the bare 0x09 query: both carry the slot, but only the
/// band reply echoes an index we can check the answer against.
pub fn read_slot<T: Transport2 + ?Sized>(dev: &T) -> Option<u8> {
    band_reply(dev, 0)?.get(OFF_SLOT).copied()
}

/// One band's reply, confirmed to be the answer to the band we asked about.
fn band_reply<T: Transport2 + ?Sized>(dev: &T, i: u8) -> Option<Vec<u8>> {
    for _ in 0..2 {
        if let Some(r) = request(dev, OP_READ, CMD_EQ_UPDATE, &[0x00, i, 0x00]) {
            if r.len() > OFF_SLOT && r[OFF_INDEX] == i {
                return Some(r);
            }
        }
    }
    None
}

/// Switches the cable to a firmware voicing, or back to the editable bank. The slot is held
/// in flash, so it survives unplugging.
pub fn set_preset<T: Transport2 + ?Sized>(dev: &T, slot: u8) -> Option<i32> {
    dev.send(&frame(OP_WRITE, CMD_EQ_ACTIVE, &[slot]));
    sleep(Duration::from_millis(DELAY_WRITE_EQ_MS));
    dev.send(&frame(OP_WRITE, CMD_EQ_SAVE_FLASH, &[]));
    sleep(Duration::from_millis(DELAY_FLASH_MS));
    read_slot(dev).map(|s| s as i32)
}

fn read_band<T: Transport2 + ?Sized>(dev: &T, i: u8) -> Option<(u16, f64, f64, u8)> {
    let r = band_reply(dev, i)?;
    Some((
        u16::from_le_bytes([r[OFF_FREQ], r[OFF_FREQ + 1]]),
        i16::from_le_bytes([r[OFF_Q], r[OFF_Q + 1]]) as f64 / 256.0,
        i16::from_le_bytes([r[OFF_GAIN], r[OFF_GAIN + 1]]) as f64 / 256.0,
        r[OFF_TYPE],
    ))
}

pub fn read_bank<T: Transport2 + ?Sized>(dev: &T, bands: usize) -> Vec<Band> {
    let mut out = Vec::with_capacity(bands);
    for i in 0..bands {
        match read_band(dev, i as u8) {
            Some((freq, q, gain, typ)) => out.push(Band {
                ok: true,
                band: (i + 1) as u8,
                freq: freq as i32,
                gain,
                q,
                typ: type_name(typ).to_string(),
            }),
            None => out.push(Band {
                ok: false,
                band: (i + 1) as u8,
                freq: 0,
                gain: 0.0,
                q: 0.0,
                typ: "PK".into(),
            }),
        }
        sleep(Duration::from_millis(DELAY_OP_MS));
    }
    out
}

fn write_band<T: Transport2 + ?Sized>(dev: &T, i: u8, b: &BandIn, slot: u8) -> bool {
    let typ = type_code(&b.typ);
    let freq = b.freq.round().clamp(0.0, 65535.0) as u16;
    let mut p = [0u8; 33];
    p[OFF_INDEX - PAY - 1] = 0x00;
    p[OFF_INDEX - PAY] = i;
    // The two bytes after the index stay zero.
    p[OFF_COEFFS - PAY..OFF_COEFFS - PAY + 20]
        .copy_from_slice(&coeffs(typ, freq as f64, b.gain, b.q));
    p[OFF_FREQ - PAY..OFF_FREQ - PAY + 2].copy_from_slice(&freq.to_le_bytes());
    p[OFF_Q - PAY..OFF_Q - PAY + 2].copy_from_slice(&fixed8_8(b.q).to_le_bytes());
    p[OFF_GAIN - PAY..OFF_GAIN - PAY + 2].copy_from_slice(&fixed8_8(b.gain).to_le_bytes());
    p[OFF_TYPE - PAY] = typ;
    p[OFF_SLOT - PAY] = slot;
    dev.send(&frame(OP_WRITE, CMD_EQ_UPDATE, &p))
}

fn fixed8_8(v: f64) -> i16 {
    (v * 256.0).round().clamp(-32768.0, 32767.0) as i16
}

/// Latches every band the cable has just been given. Without this the coefficients sit in
/// staging registers and nothing is audible.
fn commit<T: Transport2 + ?Sized>(dev: &T) -> bool {
    dev.send(&frame(
        OP_WRITE,
        CMD_EQ_COEFF,
        &[0x00, 0x00, 0xFF, 0xFF],
    ))
}

fn write_preamp<T: Transport2 + ?Sized>(dev: &T, p: &Profile, preamp: f64) -> bool {
    match p.preamp.as_str() {
        // 0.1 dB steps, signed 8.8 — the cable's own preamp register.
        "pre" => {
            let v = fixed8_8(preamp.clamp(-128.0, 0.0));
            dev.send(&frame(OP_WRITE, CMD_PRE_GAIN, &v.to_le_bytes()))
        }
        // Whole-dB DAC offset. The part already holds `preamp_buffer` of headroom, so only
        // the excess beyond it belongs in the register.
        "global" => {
            let want = if p.preamp_buffer != 0.0 {
                (preamp - p.preamp_buffer).min(0.0)
            } else {
                preamp
            };
            let v = want.round().clamp(-128.0, 0.0) as i8;
            dev.send(&frame(
                OP_WRITE,
                CMD_GLOBAL_GAIN,
                &[0x00, v as u8],
            ))
        }
        _ => true,
    }
}

pub fn read_preamp<T: Transport2 + ?Sized>(dev: &T, p: &Profile) -> Option<f64> {
    match p.preamp.as_str() {
        "pre" => {
            let r = request(dev, OP_READ, CMD_PRE_GAIN, &[])?;
            (r.len() >= 5).then(|| i16::from_le_bytes([r[3], r[4]]) as f64 / 256.0)
        }
        "global" => {
            let r = request(dev, OP_READ, CMD_GLOBAL_GAIN, &[0x00])?;
            (r.len() >= 5).then(|| (r[4] as i8) as f64 + p.preamp_buffer)
        }
        _ => None,
    }
}

/// Writes the whole bank, then reads it back and reports how many bands actually took.
///
/// Gen-2 band writes are one-way — there is no per-frame acknowledgement to count — so the
/// honest measure of success is what the cable says it is holding afterwards. That is also
/// why `acked` here means something the gen-1 path's frame count never did.
pub fn write_bank<T: Transport2 + ?Sized>(
    dev: &T,
    p: &Profile,
    bands: &[BandIn],
    preamp: f64,
) -> WriteResult {
    // Writes go to the editable slot, always. If the cable is sitting on a firmware voicing,
    // switch it back first — otherwise the bank would be addressed at a preset slot, where it
    // either does nothing or overwrites a factory curve, and either way the user's EQ is not
    // what comes out. For a cable whose slot map we don't know, take whatever it reports.
    let slot = if p.presets.len() > 1 || p.custom_slot != 0 {
        p.custom_slot
    } else {
        read_slot(dev).unwrap_or(0)
    };
    if read_slot(dev) != Some(slot) {
        set_preset(dev, slot);
    }
    sleep(Duration::from_millis(DELAY_OP_MS));

    for (i, b) in bands.iter().take(p.bands).enumerate() {
        write_band(dev, i as u8, b, slot);
        sleep(Duration::from_millis(DELAY_BAND_MS));
    }
    commit(dev);
    sleep(Duration::from_millis(DELAY_OP_MS));

    write_preamp(dev, p, preamp);
    sleep(Duration::from_millis(DELAY_OP_MS));

    // Persist, so a write survives unplugging the cable.
    dev.send(&frame(OP_WRITE, CMD_EQ_SAVE_FLASH, &[]));
    sleep(Duration::from_millis(DELAY_FLASH_MS));
    if p.preamp != "none" {
        dev.send(&frame(OP_WRITE, CMD_OFFSET_SAVE_FLASH, &[]));
        sleep(Duration::from_millis(DELAY_FLASH_MS));
    }

    // Verify against what the cable is now holding.
    let mut ok = 0u32;
    for (i, want) in bands.iter().take(p.bands).enumerate() {
        if let Some((freq, q, gain, typ)) = read_band(dev, i as u8) {
            let matches = freq == want.freq.round().clamp(0.0, 65535.0) as u16
                && (q - want.q).abs() <= 1.0 / 256.0
                && (gain - want.gain).abs() <= 1.0 / 256.0
                && typ == type_code(&want.typ);
            if matches {
                ok += 1;
            }
        }
        sleep(Duration::from_millis(DELAY_OP_MS));
    }

    WriteResult {
        acked: ok,
        active: Some(slot as i32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The packing the cable actually reads: index and slot in the fixed slots either side of
    // the coefficient block, and freq/Q/gain/type where the read path expects to find them.
    #[test]
    fn band_payload_is_laid_out_where_the_cable_reads_it() {
        let b = BandIn { freq: 1234.0, gain: -5.5, q: 3.45, typ: "PK".into() };
        let typ = type_code(&b.typ);
        let mut p = [0u8; 33];
        p[OFF_INDEX - PAY] = 3;
        p[OFF_COEFFS - PAY..OFF_COEFFS - PAY + 20]
            .copy_from_slice(&coeffs(typ, 1234.0, b.gain, b.q));
        p[OFF_FREQ - PAY..OFF_FREQ - PAY + 2].copy_from_slice(&1234u16.to_le_bytes());
        p[OFF_Q - PAY..OFF_Q - PAY + 2].copy_from_slice(&fixed8_8(b.q).to_le_bytes());
        p[OFF_GAIN - PAY..OFF_GAIN - PAY + 2].copy_from_slice(&fixed8_8(b.gain).to_le_bytes());
        p[OFF_TYPE - PAY] = typ;
        p[OFF_SLOT - PAY] = 7;

        let f = frame(OP_WRITE, CMD_EQ_UPDATE, &p);
        assert_eq!(f[0], REPORT_ID);
        assert_eq!(f[1], OP_WRITE);
        assert_eq!(f[2], CMD_EQ_UPDATE);
        // 33 is the payload length both cables accept. A shorter 0x18 covering only the
        // coefficients is taken by the Protocol Micro but rejected by the FreeDSP Mini.
        assert_eq!(f[3], 33);
        assert_eq!(f[1 + OFF_INDEX], 3);
        assert_eq!(u16::from_le_bytes([f[1 + OFF_FREQ], f[1 + OFF_FREQ + 1]]), 1234);
        assert_eq!(i16::from_le_bytes([f[1 + OFF_Q], f[1 + OFF_Q + 1]]), 883); // 3.45 * 256
        assert_eq!(i16::from_le_bytes([f[1 + OFF_GAIN], f[1 + OFF_GAIN + 1]]), -1408);
        assert_eq!(f[1 + OFF_TYPE], 2);
        assert_eq!(f[1 + OFF_SLOT], 7);
    }

    // A flat peaking band must be a pass-through. It is not the trivial [1,0,0,0,0]: at 0 dB
    // the numerator and denominator come out equal but not zero, so what the packing has to
    // show is b0 = 1 and the stored -a1/-a2 mirroring b1/b2 exactly. If the two halves ever
    // stop matching, a band at 0 dB starts colouring the signal.
    #[test]
    fn a_flat_peaking_band_passes_signal_through() {
        let c = coeffs(2, 1000.0, 0.0, 1.0);
        let at = |i: usize| i32::from_le_bytes([c[i * 4], c[i * 4 + 1], c[i * 4 + 2], c[i * 4 + 3]]);
        assert_eq!(at(0), 1 << 30, "b0 should be unity");
        assert_eq!(at(3), -at(1), "stored -a1 should cancel b1");
        assert_eq!(at(4), -at(2), "stored -a2 should cancel b2");
        assert_ne!(at(1), 0, "a real peaking section, not the trivial one");
    }

    // Type codes are not gen-1's: peaking is 2 here and 0 there, and the shelves move too.
    // Getting this wrong silently turns every band into the wrong filter.
    #[test]
    fn type_codes_round_trip() {
        for (name, code) in [("PK", 2u8), ("LSQ", 1), ("HSQ", 3), ("LP", 4), ("HP", 5)] {
            assert_eq!(type_code(name), code);
            assert_eq!(type_name(code), name);
        }
        // Anything unknown has to fall back to peaking rather than to "disabled" (0).
        assert_eq!(type_code("nonsense"), 2);
    }

    // Walkplay parts hold -5 dB of headroom in hardware, so only the excess is written; a
    // preamp shallower than the buffer must write 0 rather than a positive boost.
    #[test]
    fn global_preamp_accounts_for_the_hardware_buffer() {
        let p = profile(0x3302, 0xC20F, "Protocol Micro");
        assert_eq!(p.preamp_buffer, -5.0);
        let want = |preamp: f64| ((preamp - p.preamp_buffer).min(0.0)).round() as i8;
        assert_eq!(want(-7.0), -2);
        assert_eq!(want(-3.0), 0);
        assert_eq!(want(0.0), 0);
    }

    // The two cables differ in ways the UI has to respect — band gain range and which
    // filter types exist at all.
    #[test]
    fn profiles_differ_per_cable() {
        let mini = profile(0x35D8, 0x98D4, "FreeDSP Mini");
        let micro = profile(0x3302, 0xC20F, "Protocol Micro");
        assert_eq!((mini.bands, mini.gain_max), (8, 12.0));
        assert_eq!((micro.bands, micro.gain_max), (8, 10.0));
        assert!(mini.types.contains(&"HSQ".to_string()));
        assert!(!micro.types.contains(&"HSQ".to_string()));
        assert!(micro.types.contains(&"LP".to_string()));
        assert_eq!(mini.preamp, "pre");
        assert_eq!(micro.preamp, "global");
    }

    // The Max shares the Micro's vendor id but not its limits: ten bands, both shelves, no
    // pass filters and no assumed preamp buffer, whatever product string it reports.
    #[test]
    fn protocol_max_has_its_own_profile() {
        let max = profile(0x3302, 0x43CC, "");
        assert_eq!(max.name, "CrinEar Protocol Max");
        assert_eq!((max.bands, max.gain_min, max.gain_max), (10, -10.0, 10.0));
        assert_eq!(max.types, ["PK", "LSQ", "HSQ"]);
        assert_eq!((max.preamp.as_str(), max.preamp_buffer, max.custom_slot), ("global", 0.0, 0));
        assert!(!max.tested);
        assert_eq!(profile(0x3302, 0x43CC, "Protocol Max").name, "Protocol Max");
    }

    // The Black Pearl gets scheme 16's ten bands and both shelves, plus the pass filters it was
    // seen to store, and keeps its name with or without the "TE-C" product string.
    #[test]
    fn black_pearl_has_its_own_profile() {
        let bp = profile(0x3302, 0x43E8, "TE-C");
        assert_eq!(bp.name, "TRN Black Pearl");
        assert_eq!(profile(0x3302, 0x43E8, "").name, "TRN Black Pearl");
        assert_eq!((bp.bands, bp.gain_min, bp.gain_max), (10, -10.0, 10.0));
        assert_eq!(bp.types, ["PK", "LSQ", "HSQ", "LP", "HP"]);
        assert_eq!((bp.preamp.as_str(), bp.preamp_buffer, bp.custom_slot), ("global", 0.0, 0));
        assert!(!bp.tested);
    }

    // Coefficient bytes a TRN Black Pearl (firmware 0.6) held after a write, read back on
    // 2026-10-19 by Contour, against what `coeffs` makes of the same band. The cable held them
    // unchanged, also after a replug: a wrapped low-shelf b1, both pass filters, and a cut past
    // -10 dB.
    #[test]
    fn coefficients_match_what_a_black_pearl_stored() {
        let hex = |s: &str| -> Vec<u8> {
            s.split(' ').map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
        };
        let cases: [(u8, f64, f64, f64, &str); 10] = [
            (1, 105.0, 6.0, 0.707, "12 21 21 40 41 c5 a0 80 1e 30 3f 3f 31 80 5f 7f 42 f4 9f c0"),
            (2, 250.0, 10.0, 1.0, "78 3b a2 40 54 6c 9a 80 fa b5 c7 3e ac 93 65 7f 8e 0e 96 c0"),
            (2, 500.0, -10.4, 0.1, "5c 86 c0 35 a5 96 6a 9d 08 68 e2 2c 5b 69 95 62 9c 11 5d dd"),
            (2, 1000.0, -10.0, 10.0, "95 3a bf 3f a4 34 03 81 97 51 83 3f 5c cb fc 7e d4 73 bd c0"),
            (3, 8000.0, 4.0, 0.707, "ae 57 b0 5c 6f 7a 4a 88 93 05 80 29 88 06 dc 48 c8 21 a9 e8"),
            (4, 18000.0, 0.0, 0.707, "0f 9e f2 0b 1e 3c e5 17 0f 9e f2 0b f0 52 a0 1d d4 34 95 f2"),
            (5, 30.0, 0.0, 0.707, "ca 43 e9 3f 6c 78 2d 80 ca 43 e9 3f 81 7f d2 7f 5a 70 2d c0"),
            (2, 3000.0, 2.3, 2.0, "33 85 cb 40 85 75 9a 87 af d9 f5 39 7b 8a 65 78 1e a1 3e c5"),
            (2, 6000.0, -3.7, 4.0, "ea 69 c2 3e 92 15 5a 90 ce 6d 16 3a 6e ea a5 6f 47 28 27 c7"),
            (2, 12000.0, 0.0, 0.75, "00 00 00 40 5f d1 7c c2 ea e0 fd 16 a1 2e 83 3d 16 1f 02 e9"),
        ];
        for (typ, f, gain, q, stored) in cases {
            assert_eq!(coeffs(typ, f, gain, q).to_vec(), hex(stored), "type {typ} at {f} Hz");
        }
    }
}
