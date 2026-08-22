// HID transport for everything that isn't Windows, via hidapi.
//
// macOS today, Linux (hidraw) with the same code path once a udev rule is in place.
// The two calls that matter map cleanly onto the Win32 ones this app was built around:
//
//   HidD_SetOutputReport  ->  hid_send_output_report  (SET_REPORT over the control pipe)
//   HidD_GetInputReport   ->  hid_get_input_report    (GET_REPORT on an input report)
//
// Note this is *not* hid_write/hid_read: those use the interrupt endpoints, which the
// cable does not answer on. Report ID 0x01 sits in byte 0 of the buffer in both
// directions, exactly as on Windows, so the frame layout in dsp.rs is untouched.

use std::cell::RefCell;

use hidapi::{HidApi, HidDevice};

use crate::dsp::{Transport, FRAME_LEN, MOONDROP_VID, REPORT_ID};

// Windows sizes the read buffer from HIDP_CAPS.InputReportByteLength; hidapi's DeviceInfo
// carries no equivalent, but the descriptor pins it anyway — report ID 1 is 61 bytes in
// and 61 out, so a frame is FRAME_LEN in both directions and HIDP_CAPS reports the same
// 62 on Windows. Asking IOHIDDeviceGetReport for more than the report holds is rejected
// by some drivers, so ask for exactly the frame.
const IN_BUF: usize = FRAME_LEN;

pub struct Dev {
    dev: HidDevice,
    pid: u16,
    product: String,
}

impl Transport for Dev {
    fn set_output(&self, frame: &[u8]) -> bool {
        self.dev.send_output_report(frame).is_ok()
    }

    fn get_input(&self) -> (bool, Vec<u8>) {
        let mut buf = vec![0u8; IN_BUF];
        buf[0] = REPORT_ID;
        match self.dev.get_input_report(&mut buf) {
            // Truncate to what the device actually returned so short replies still fail
            // the length checks in dsp.rs rather than reading zero padding as data.
            Ok(n) if n >= 2 => {
                buf.truncate(n);
                (true, buf)
            }
            _ => (false, buf),
        }
    }

    fn pid(&self) -> u16 {
        self.pid
    }

    fn product(&self) -> &str {
        &self.product
    }
}

#[cfg(target_os = "macos")]
fn permission_hint() -> &'static str {
    // The cable's HID collection is Consumer Control (it carries the inline volume keys on
    // report 2), which macOS treats as a protected device: opening it needs Input
    // Monitoring consent, and IOHIDDeviceOpen just fails until it's granted.
    "\n\nmacOS is most likely blocking HID access. Open System Settings > Privacy & \
     Security > Input Monitoring, allow FreeDSP Studio, then try again."
}

#[cfg(not(target_os = "macos"))]
fn permission_hint() -> &'static str {
    "\n\nIf the cable is plugged in, you may not have permission to open the hidraw device. \
     A udev rule granting access to VID 35d8 is needed."
}

// The hidapi context, kept for the life of the HID thread.
//
// HidApi::new() enumerates the whole HID bus, and on macOS that means
// IOHIDManagerSetDeviceMatching on hidapi's process-global manager, which reschedules every
// matched device onto the manager's run loop. Those IOHIDDeviceRefs are cached by IOKit per
// service, so they're the same objects hid_close() has just rescheduled onto the main run
// loop — enumerating on every operation runs that collision over and over, and it is where
// the cable crashed the app on macOS 27 (a pointer-authentication trap on a CFRunLoop inside
// CFRunLoopAddSource).
//
// Nothing here needs a fresh list that often. Device paths stay valid while the cable stays
// plugged in, so hold the context and re-enumerate only when opening from the list we have
// fails — which is exactly the case where the cable has been unplugged or re-plugged and the
// paths really have changed.
thread_local! {
    static API: RefCell<Option<HidApi>> = const { RefCell::new(None) };
}

pub fn open_best(pref_pid: Option<u16>) -> Result<Dev, String> {
    API.with(|cell| {
        let mut slot = cell.borrow_mut();

        // A context we've just built carries a fresh list; there's nothing to refresh yet.
        let fresh = slot.is_none();
        if fresh {
            let api = HidApi::new().map_err(|e| e.to_string())?;

            // Don't seize the device: the cable's inline controls stay working while we're
            // open, and we only ever touch the vendor reports.
            #[cfg(target_os = "macos")]
            api.set_open_exclusive(false);

            *slot = Some(api);
        }
        let api = slot.as_mut().expect("context was just placed");

        match open_from_list(api, pref_pid) {
            Ok(dev) => Ok(dev),
            Err(e) if fresh => Err(e),
            Err(_) => {
                api.refresh_devices().map_err(|e| e.to_string())?;
                open_from_list(api, pref_pid)
            }
        }
    })
}

fn open_from_list(api: &HidApi, pref_pid: Option<u16>) -> Result<Dev, String> {
    let mut list: Vec<&hidapi::DeviceInfo> = api
        .device_list()
        .filter(|d| d.vendor_id() == MOONDROP_VID && !d.path().to_bytes().is_empty())
        .collect();
    if list.is_empty() {
        return Err("no Moondrop cable found".to_string());
    }

    // Prefer the cable we originally opened, if it's still present.
    if let Some(pp) = pref_pid {
        if list.iter().any(|d| d.product_id() == pp) {
            list.retain(|d| d.product_id() == pp);
        }
    }

    // Windows picks the collection by report size. There's no such handle here, but there
    // doesn't need to be: these cables put the DSP on report ID 1 of a Consumer Control
    // collection (0x0C) — not a vendor-defined page — and every collection of one device
    // shares an IOHIDDevice, so opening any of them and addressing report 1 routes right.
    // The 0xFF00 preference below is only a tiebreak for cables that do use a vendor page.
    list.sort_by_key(|d| if d.usage_page() >= 0xFF00 { 0 } else { 1 });

    let mut last_err = String::new();
    let mut tried: Vec<&std::ffi::CStr> = Vec::new();
    for d in list {
        // Collections of one device share a path on macOS; opening it twice is pointless.
        if tried.contains(&d.path()) {
            continue;
        }
        tried.push(d.path());
        match api.open_path(d.path()) {
            Ok(dev) => {
                return Ok(Dev {
                    dev,
                    pid: d.product_id(),
                    product: d.product_string().unwrap_or("").to_string(),
                })
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    Err(format!(
        "could not open the Moondrop cable: {}{}",
        last_err,
        permission_hint()
    ))
}
