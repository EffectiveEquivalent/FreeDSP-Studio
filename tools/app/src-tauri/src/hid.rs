// Public HID API. Picks a platform transport, serialises access to the cable, and refuses
// to write to a cable that isn't the one the user connected. The protocol itself lives in
// dsp.rs; everything here is bookkeeping.

use std::sync::Mutex;

use crate::dsp;

pub use crate::dsp::{Band, BandIn, CableInfo, WriteResult};

#[cfg(windows)]
use crate::hid_win as backend;

#[cfg(not(windows))]
use crate::hid_hidapi as backend;

// Serialises all device ops (one at a time), and remembers which cable we opened.
static IO: Mutex<()> = Mutex::new(());
static EXPECTED: Mutex<Option<Expected>> = Mutex::new(None);

#[derive(Clone)]
struct Expected {
    pid: u16,
    name: String,
}

fn expected_pid() -> Option<u16> {
    EXPECTED.lock().unwrap().as_ref().map(|e| e.pid)
}

fn guard(dev: &backend::Dev) -> Result<(), String> {
    use dsp::Transport;
    if let Some(exp) = EXPECTED.lock().unwrap().as_ref() {
        if dev.pid() != exp.pid {
            let found = dsp::cable_profile(dev.pid(), dev.product()).0;
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
    use dsp::Transport;
    let _io = IO.lock().unwrap();
    let dev = backend::open_best(None)?;
    let (name, has, presets) = dsp::cable_profile(dev.pid(), dev.product());
    *EXPECTED.lock().unwrap() = Some(Expected {
        pid: dev.pid(),
        name: name.clone(),
    });
    Ok(CableInfo {
        name,
        vid: dsp::MOONDROP_VID as u32,
        pid: dev.pid() as u32,
        has_presets: has,
        presets,
    })
}

pub fn close() {
    *EXPECTED.lock().unwrap() = None;
}

pub fn read_mode() -> Result<Option<i32>, String> {
    let _io = IO.lock().unwrap();
    let dev = backend::open_best(expected_pid())?;
    Ok(dsp::read_mode(&dev))
}

pub fn read_bank() -> Result<Vec<Band>, String> {
    let _io = IO.lock().unwrap();
    let dev = backend::open_best(expected_pid())?;
    Ok(dsp::read_bank(&dev))
}

pub fn set_preset(idx: i32) -> Result<Option<i32>, String> {
    let _io = IO.lock().unwrap();
    let dev = backend::open_best(expected_pid())?;
    guard(&dev)?;
    Ok(dsp::set_preset(&dev, idx))
}

pub fn write_bank(bands: Vec<BandIn>, preamp: f64) -> Result<WriteResult, String> {
    let _io = IO.lock().unwrap();
    let dev = backend::open_best(expected_pid())?;
    guard(&dev)?;
    Ok(dsp::write_bank(&dev, &bands, preamp))
}
