// Moondrop DSP wire protocol — platform-free. Frame building, the RBJ biquad maths and
// the command sequences all live here; a platform backend only has to move 62 bytes in
// each direction (see `Transport`).

use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const MOONDROP_VID: u16 = 0x35D8;
const BQ_SCALE: f64 = 4_194_304.0; // 2^22
const SAMPLE_RATES: [(i32, f64); 5] =
    [(4, 44100.0), (5, 48000.0), (6, 96000.0), (7, 192000.0), (8, 384000.0)];

// A frame is one HID report: byte 0 is the report ID, the remaining 61 are payload.
pub const FRAME_LEN: usize = 62;
pub const REPORT_ID: u8 = 0x01;

/// One cable, opened. Both directions go over the HID *control* pipe — GET_REPORT on an
/// input report and SET_REPORT on an output report — which is what the stock app does and
/// what node-hid / WebHID cannot express.
pub trait Transport {
    /// SET_REPORT(Output). `frame` is FRAME_LEN bytes, report ID first. True on success.
    fn set_output(&self, frame: &[u8]) -> bool;
    /// GET_REPORT(Input). Returns (ok, report) with the report ID still in byte 0.
    fn get_input(&self) -> (bool, Vec<u8>);
    fn pid(&self) -> u16;
    fn product(&self) -> &str;
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CableInfo {
    pub name: String,
    pub vid: u32,
    pub pid: u32,
    pub has_presets: Option<bool>,
    pub presets: BTreeMap<u8, String>,
}

#[derive(Serialize)]
pub struct Band {
    pub ok: bool,
    pub band: u8,
    pub freq: i32,
    pub gain: f64,
    pub q: f64,
    #[serde(rename = "type")]
    pub typ: String,
}

#[derive(Deserialize)]
pub struct BandIn {
    pub freq: f64,
    pub gain: f64,
    pub q: f64,
    #[serde(rename = "type")]
    pub typ: String,
}

#[derive(Serialize)]
pub struct WriteResult {
    pub acked: u32,
    pub active: Option<i32>,
}

fn ftype(code: i32) -> &'static str {
    match code {
        0 => "PK",
        1 => "LSQ",
        2 => "HSQ",
        _ => "?",
    }
}

fn type_code(t: &str) -> i32 {
    match t {
        "LSQ" => 1,
        "HSQ" => 2,
        _ => 0,
    }
}

pub fn cable_profile(pid: u16, product: &str) -> (String, Option<bool>, BTreeMap<u8, String>) {
    let map = |pairs: &[(u8, &str)]| {
        pairs
            .iter()
            .map(|(k, v)| (*k, v.to_string()))
            .collect::<BTreeMap<u8, String>>()
    };
    match pid {
        0x1499 => (
            "DUSK-SP".into(),
            Some(true),
            map(&[
                (0, "Custom EQ"),
                (1, "DUSK-Default"),
                (2, "DUSK-V"),
                (3, "DUSK-Harman"),
                (4, "DUSK-Bass+"),
                (5, "DUSK-Diffuse-Tilted"),
            ]),
        ),
        0x1497 => (
            "MAY".into(),
            Some(true),
            map(&[
                (0, "Custom EQ"),
                (1, "Standard"),
                (2, "Bass Head"),
                (3, "Reference"),
                (4, "No Bass"),
                (5, "Harman Style"),
            ]),
        ),
        0x1496 => ("FreeDSP".into(), Some(false), map(&[(0, "Custom EQ")])),
        _ => (
            if product.is_empty() {
                format!("Moondrop 0x{:x}", pid)
            } else {
                product.to_string()
            },
            None,
            map(&[(0, "Custom EQ")]),
        ),
    }
}

fn build_frame(cmd: u8, flag: u8, dwords: &[i32]) -> [u8; FRAME_LEN] {
    let mut b = [0u8; FRAME_LEN];
    for (i, v) in [REPORT_ID, 0, 0x0d, 0, cmd, flag, 0, 0x23, 0x2d, 0xb3]
        .iter()
        .enumerate()
    {
        b[i] = *v;
    }
    let mut o = 10;
    for d in dwords {
        b[o..o + 4].copy_from_slice(&d.to_le_bytes());
        o += 4;
    }
    b
}

fn encode_gain24(g: f64) -> i32 {
    let g = g.round() as i32;
    if g < 0 {
        0x1000000 + g
    } else {
        g
    }
}

fn rbj(t: &str, f: f64, gain: f64, q: f64, fs: f64) -> [f64; 6] {
    let a = 10f64.powf(gain / 40.0);
    let w0 = 2.0 * std::f64::consts::PI * f / fs;
    let cw = w0.cos();
    let sw = w0.sin();
    let alpha = sw / (2.0 * q);
    let (b0, b1, b2, a0, a1, a2);
    if t == "LSQ" || t == "HSQ" {
        let sa = (sw / 2.0) * ((a + 1.0 / a) * (1.0 / q - 1.0) + 2.0).sqrt();
        let tsa = 2.0 * a.sqrt() * sa;
        if t == "LSQ" {
            b0 = a * ((a + 1.0) - (a - 1.0) * cw + tsa);
            b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cw);
            b2 = a * ((a + 1.0) - (a - 1.0) * cw - tsa);
            a0 = (a + 1.0) + (a - 1.0) * cw + tsa;
            a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cw);
            a2 = (a + 1.0) + (a - 1.0) * cw - tsa;
        } else {
            b0 = a * ((a + 1.0) + (a - 1.0) * cw + tsa);
            b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cw);
            b2 = a * ((a + 1.0) + (a - 1.0) * cw - tsa);
            a0 = (a + 1.0) - (a - 1.0) * cw + tsa;
            a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cw);
            a2 = (a + 1.0) - (a - 1.0) * cw - tsa;
        }
    } else {
        b0 = 1.0 + alpha * a;
        b1 = -2.0 * cw;
        b2 = 1.0 - alpha * a;
        a0 = 1.0 + alpha / a;
        a1 = -2.0 * cw;
        a2 = 1.0 - alpha / a;
    }
    [b0, b1, b2, a0, a1, a2]
}

fn compute_biquad(t: &str, f: f64, gain: f64, q: f64, fs: f64) -> [i32; 5] {
    let c = rbj(t, f, gain, q, fs);
    let a0 = c[3];
    [c[0], c[1], c[2], -c[4], -c[5]].map(|x| (x / a0 * BQ_SCALE).round() as i32)
}

fn send_ack<T: Transport + ?Sized>(dev: &T, frame: &[u8]) -> (bool, Vec<u8>) {
    dev.set_output(frame);
    dev.get_input()
}

// ---- command sequences ----

pub fn read_mode<T: Transport + ?Sized>(dev: &T) -> Option<i32> {
    dev.set_output(&build_frame(0x5a, 0x01, &[0x5a]));
    sleep(Duration::from_millis(10));
    let (ok, d) = dev.get_input();
    if ok && d.get(4) == Some(&0x5a) && d.len() >= 18 {
        Some(i32::from_le_bytes([d[14], d[15], d[16], d[17]]))
    } else {
        None
    }
}

pub fn read_bank<T: Transport + ?Sized>(dev: &T) -> Vec<Band> {
    let mut bands = Vec::with_capacity(9);
    for n in 1..=9i32 {
        dev.set_output(&build_frame(0xdd, 0x01, &[n]));
        sleep(Duration::from_millis(12));
        let (ok, d) = dev.get_input();
        if !ok || d.len() < 34 {
            bands.push(Band {
                ok: false,
                band: n as u8,
                freq: 0,
                gain: 0.0,
                q: 0.0,
                typ: "PK".into(),
            });
            continue;
        }
        let mut g = (d[30] as i32) | ((d[31] as i32) << 8) | ((d[32] as i32) << 16);
        if g & 0x800000 != 0 {
            g -= 0x1000000;
        }
        let freq = i32::from_le_bytes([d[18], d[19], d[20], d[21]]);
        let qraw = i32::from_le_bytes([d[22], d[23], d[24], d[25]]);
        let tcode = i32::from_le_bytes([d[26], d[27], d[28], d[29]]);
        bands.push(Band {
            ok: d[4] == 0xdd && (d[5] & 0x80) != 0,
            band: n as u8,
            freq,
            gain: g as f64,
            q: qraw as f64 / 256.0,
            typ: ftype(tcode).to_string(),
        });
    }
    bands
}

pub fn set_preset<T: Transport + ?Sized>(dev: &T, idx: i32) -> Option<i32> {
    send_ack(dev, &build_frame(0x5a, 0x00, &[0x5a, idx]));
    sleep(Duration::from_millis(12));
    send_ack(dev, &build_frame(0xdc, 0x00, &[0xff]));
    sleep(Duration::from_millis(12));
    read_mode(dev)
}

pub fn write_bank<T: Transport + ?Sized>(dev: &T, bands: &[BandIn], preamp: f64) -> WriteResult {
    let mut ok = 0u32;
    // Preamp = flat gain baked into the LAST ACTIVE band's numerator, so the rest of
    // the cascade runs at full level ahead of the attenuation (best SNR).
    let k = 10f64.powf(preamp / 20.0);
    let mut preamp_band: i32 = -1;
    if k != 1.0 {
        for i in (0..9).rev() {
            if bands.get(i).map(|b| b.gain != 0.0).unwrap_or(false) {
                preamp_band = i as i32;
                break;
            }
        }
        if preamp_band < 0 {
            preamp_band = 8;
        }
    }
    send_ack(dev, &build_frame(0x5a, 0x00, &[0x5a, 0]));
    sleep(Duration::from_millis(6));
    for i in 0..9usize {
        let e = &bands[i];
        let f = build_frame(
            0xdc,
            0x00,
            &[
                0,
                (i as i32) + 1,
                e.freq.round() as i32,
                (e.q * 256.0).round() as i32,
                type_code(&e.typ),
                encode_gain24(e.gain),
            ],
        );
        if send_ack(dev, &f).0 {
            ok += 1;
        }
        sleep(Duration::from_millis(6));
    }
    for i in 0..9usize {
        let e = &bands[i];
        for (sr, fs) in SAMPLE_RATES {
            let mut cf = compute_biquad(&e.typ, e.freq, e.gain, e.q, fs);
            if i as i32 == preamp_band {
                cf[0] = (cf[0] as f64 * k).round() as i32;
                cf[1] = (cf[1] as f64 * k).round() as i32;
                cf[2] = (cf[2] as f64 * k).round() as i32;
            }
            let f = build_frame(
                0xdc,
                0x00,
                &[sr, (i as i32) + 1, 3, cf[0], cf[1], cf[2], cf[3], cf[4]],
            );
            if send_ack(dev, &f).0 {
                ok += 1;
            }
            sleep(Duration::from_millis(6));
        }
    }
    send_ack(dev, &build_frame(0xdc, 0x00, &[0xff]));
    sleep(Duration::from_millis(10));
    WriteResult {
        acked: ok,
        active: read_mode(dev),
    }
}
