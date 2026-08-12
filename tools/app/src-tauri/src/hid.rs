// HID + Moondrop DSP wire protocol — Rust port of the Electron `dsp-core.js`.
// Windows-only: reads/writes go over the HID control pipe via HidD_GetInputReport /
// HidD_SetOutputReport (what node-hid / WebHID cannot do), same as the old winhid.js.
#![cfg(windows)]

use std::collections::BTreeMap;
use std::ffi::{c_void, OsStr};
use std::iter::once;
use std::os::windows::ffi::OsStrExt;
use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use windows::core::PCWSTR;
use windows::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetInputReport, HidD_GetPreparsedData, HidD_SetOutputReport,
    HidP_GetCaps, HIDP_CAPS, PHIDP_PREPARSED_DATA,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

const MOONDROP_VID: u16 = 0x35D8;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const BQ_SCALE: f64 = 4_194_304.0; // 2^22
const SAMPLE_RATES: [(i32, f64); 5] =
    [(4, 44100.0), (5, 48000.0), (6, 96000.0), (7, 192000.0), (8, 384000.0)];

// Serialises all device ops (one at a time), and remembers which cable we opened.
static IO: Mutex<()> = Mutex::new(());
static EXPECTED: Mutex<Option<Expected>> = Mutex::new(None);

#[derive(Clone)]
struct Expected {
    pid: u16,
    name: String,
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

// RAII wrapper: closes the HID handle when dropped.
struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct OpenDev {
    handle: OwnedHandle,
    in_len: usize,
    pid: u16,
    product: String,
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

fn cable_profile(pid: u16, product: &str) -> (String, Option<bool>, BTreeMap<u8, String>) {
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

fn build_frame(cmd: u8, flag: u8, dwords: &[i32]) -> [u8; 62] {
    let mut b = [0u8; 62];
    for (i, v) in [0x01u8, 0, 0x0d, 0, cmd, flag, 0, 0x23, 0x2d, 0xb3]
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

fn create_file(path: &str) -> Result<HANDLE, String> {
    let wide: Vec<u16> = OsStr::new(path).encode_wide().chain(once(0)).collect();
    unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .map_err(|e| e.to_string())
}

fn input_len(h: HANDLE) -> Option<usize> {
    unsafe {
        let mut ppd = PHIDP_PREPARSED_DATA::default();
        if HidD_GetPreparsedData(h, &mut ppd).0 == 0 {
            return None;
        }
        let mut caps = HIDP_CAPS::default();
        let _ = HidP_GetCaps(ppd, &mut caps);
        let _ = HidD_FreePreparsedData(ppd);
        let n = caps.InputReportByteLength as usize;
        if n > 0 {
            Some(n)
        } else {
            None
        }
    }
}

fn set_output(h: HANDLE, frame: &[u8]) -> bool {
    unsafe { HidD_SetOutputReport(h, frame.as_ptr() as *mut c_void, frame.len() as u32).0 != 0 }
}

fn get_input(h: HANDLE, in_len: usize) -> (bool, Vec<u8>) {
    let n = in_len.max(2);
    let mut buf = vec![0u8; n];
    buf[0] = 0x01;
    let ok = unsafe { HidD_GetInputReport(h, buf.as_mut_ptr() as *mut c_void, n as u32).0 != 0 };
    (ok, buf)
}

fn send_ack(h: HANDLE, in_len: usize, frame: &[u8]) -> (bool, Vec<u8>) {
    set_output(h, frame);
    get_input(h, in_len)
}

fn open_best(pref_pid: Option<u16>) -> Result<OpenDev, String> {
    let api = hidapi::HidApi::new().map_err(|e| e.to_string())?;
    let mut list: Vec<&hidapi::DeviceInfo> = api
        .device_list()
        .filter(|d| d.vendor_id() == MOONDROP_VID && !d.path().to_bytes().is_empty())
        .collect();
    // Prefer the cable we originally opened, if it's still present.
    if let Some(pp) = pref_pid {
        if list.iter().any(|d| d.product_id() == pp) {
            list.retain(|d| d.product_id() == pp);
        }
    }
    let mut best: Option<OpenDev> = None;
    for d in list {
        let path = match d.path().to_str() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let h = match create_file(path) {
            Ok(h) => h,
            Err(_) => continue,
        };
        match input_len(h) {
            Some(n) if best.as_ref().map(|b| n > b.in_len).unwrap_or(true) => {
                best = Some(OpenDev {
                    handle: OwnedHandle(h),
                    in_len: n,
                    pid: d.product_id(),
                    product: d.product_string().unwrap_or("").to_string(),
                });
            }
            _ => unsafe {
                let _ = CloseHandle(h);
            },
        }
    }
    best.ok_or_else(|| "no Moondrop cable found".to_string())
}

fn do_read_mode(h: HANDLE, in_len: usize) -> Option<i32> {
    set_output(h, &build_frame(0x5a, 0x01, &[0x5a]));
    sleep(Duration::from_millis(10));
    let (ok, d) = get_input(h, in_len);
    if ok && d.get(4) == Some(&0x5a) && d.len() >= 18 {
        Some(i32::from_le_bytes([d[14], d[15], d[16], d[17]]))
    } else {
        None
    }
}

fn do_read_bank(h: HANDLE, in_len: usize) -> Vec<Band> {
    let mut bands = Vec::with_capacity(9);
    for n in 1..=9i32 {
        set_output(h, &build_frame(0xdd, 0x01, &[n]));
        sleep(Duration::from_millis(12));
        let (ok, d) = get_input(h, in_len);
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

fn do_set_preset(h: HANDLE, in_len: usize, idx: i32) -> Option<i32> {
    send_ack(h, in_len, &build_frame(0x5a, 0x00, &[0x5a, idx]));
    sleep(Duration::from_millis(12));
    send_ack(h, in_len, &build_frame(0xdc, 0x00, &[0xff]));
    sleep(Duration::from_millis(12));
    do_read_mode(h, in_len)
}

fn do_write_bank(h: HANDLE, in_len: usize, bands: &[BandIn], preamp: f64) -> WriteResult {
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
    send_ack(h, in_len, &build_frame(0x5a, 0x00, &[0x5a, 0]));
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
        if send_ack(h, in_len, &f).0 {
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
            if send_ack(h, in_len, &f).0 {
                ok += 1;
            }
            sleep(Duration::from_millis(6));
        }
    }
    send_ack(h, in_len, &build_frame(0xdc, 0x00, &[0xff]));
    sleep(Duration::from_millis(10));
    WriteResult {
        acked: ok,
        active: do_read_mode(h, in_len),
    }
}

fn expected_pid() -> Option<u16> {
    EXPECTED.lock().unwrap().as_ref().map(|e| e.pid)
}

fn guard(dev: &OpenDev) -> Result<(), String> {
    if let Some(exp) = EXPECTED.lock().unwrap().as_ref() {
        if dev.pid != exp.pid {
            let found = cable_profile(dev.pid, &dev.product).0;
            return Err(format!(
                "Wrong cable connected: expected {}, found {}. Nothing was written.",
                exp.name, found
            ));
        }
    }
    Ok(())
}

// ---- public API (each opens/closes its own handle, serialised) ----
pub fn open() -> Result<CableInfo, String> {
    let _io = IO.lock().unwrap();
    let dev = open_best(None)?;
    let (name, has, presets) = cable_profile(dev.pid, &dev.product);
    *EXPECTED.lock().unwrap() = Some(Expected {
        pid: dev.pid,
        name: name.clone(),
    });
    Ok(CableInfo {
        name,
        vid: MOONDROP_VID as u32,
        pid: dev.pid as u32,
        has_presets: has,
        presets,
    })
}

pub fn close() {
    *EXPECTED.lock().unwrap() = None;
}

pub fn read_mode() -> Result<Option<i32>, String> {
    let _io = IO.lock().unwrap();
    let dev = open_best(expected_pid())?;
    Ok(do_read_mode(dev.handle.0, dev.in_len))
}

pub fn read_bank() -> Result<Vec<Band>, String> {
    let _io = IO.lock().unwrap();
    let dev = open_best(expected_pid())?;
    Ok(do_read_bank(dev.handle.0, dev.in_len))
}

pub fn set_preset(idx: i32) -> Result<Option<i32>, String> {
    let _io = IO.lock().unwrap();
    let dev = open_best(expected_pid())?;
    guard(&dev)?;
    Ok(do_set_preset(dev.handle.0, dev.in_len, idx))
}

pub fn write_bank(bands: Vec<BandIn>, preamp: f64) -> Result<WriteResult, String> {
    let _io = IO.lock().unwrap();
    let dev = open_best(expected_pid())?;
    guard(&dev)?;
    Ok(do_write_bank(dev.handle.0, dev.in_len, &bands, preamp))
}
