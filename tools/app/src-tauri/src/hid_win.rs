// Windows HID transport. Goes at hid.dll directly rather than through hidapi, because
// HidD_GetInputReport / HidD_SetOutputReport are the control-pipe calls the cable needs
// and hidapi's Windows backend routes hid_write down the interrupt OUT pipe instead.

use std::ffi::{c_void, OsStr};
use std::iter::once;
use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetInputReport, HidD_GetPreparsedData, HidD_SetOutputReport,
    HidP_GetCaps, HIDP_CAPS, PHIDP_PREPARSED_DATA,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

use crate::dsp::{Transport, MOONDROP_VID, REPORT_ID};

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

// RAII wrapper: closes the HID handle when dropped.
struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub struct Dev {
    handle: OwnedHandle,
    in_len: usize,
    pid: u16,
    product: String,
}

impl Transport for Dev {
    fn set_output(&self, frame: &[u8]) -> bool {
        unsafe {
            HidD_SetOutputReport(
                self.handle.0,
                frame.as_ptr() as *mut c_void,
                frame.len() as u32,
            )
            .0 != 0
        }
    }

    fn get_input(&self) -> (bool, Vec<u8>) {
        let n = self.in_len.max(2);
        let mut buf = vec![0u8; n];
        buf[0] = REPORT_ID;
        let ok = unsafe {
            HidD_GetInputReport(self.handle.0, buf.as_mut_ptr() as *mut c_void, n as u32).0 != 0
        };
        (ok, buf)
    }

    fn pid(&self) -> u16 {
        self.pid
    }

    fn product(&self) -> &str {
        &self.product
    }
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

/// Open the Moondrop collection with the largest input report — that's the vendor one.
pub fn open_best(pref_pid: Option<u16>) -> Result<Dev, String> {
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
    let mut best: Option<Dev> = None;
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
                best = Some(Dev {
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
