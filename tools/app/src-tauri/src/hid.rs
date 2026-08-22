// Public HID API. Picks a platform transport, serialises access to the cable, and refuses
// to write to a cable that isn't the one the user connected. The protocol itself lives in
// dsp.rs; everything here is bookkeeping.

use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};

use crate::dsp;

pub use crate::dsp::{Band, BandIn, CableInfo, WriteResult};

#[cfg(windows)]
use crate::hid_win as backend;

#[cfg(not(windows))]
use crate::hid_hidapi as backend;

// ---- the HID thread ----
//
// Every device op runs on this one thread, for the life of the process. That's not just
// tidiness: hidapi's macOS backend binds its global IOHIDManager to whatever thread first
// calls hid_init(), by way of
//
//   IOHIDManagerScheduleWithRunLoop(hid_mgr, CFRunLoopGetCurrent(), ...)
//
// and the Rust wrapper inits once and never calls hid_exit(). Each later enumerate then
// schedules the devices it matches onto that same stored run loop. Run these ops on tokio's
// blocking pool and the binding lands on a pool thread that idles out and exits a few
// seconds later, taking its CFRunLoop with it — the next enumerate hands IOKit a dangling
// CFRunLoopRef and the process dies on a pointer-authentication trap in CFRunLoopAddSource.
// A thread that never exits keeps that run loop alive and valid.
//
// Serialising the ops is the other half of the job: the cable answers one request at a
// time, and the channel gives that for free.

type Job = Box<dyn FnOnce() + Send + 'static>;

fn hid_thread() -> &'static Sender<Job> {
    static TX: OnceLock<Sender<Job>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("hid".to_string())
            .spawn(move || {
                for job in rx {
                    job();
                }
            })
            .expect("could not start the HID thread");
        tx
    })
}

// Hands `f` to the HID thread and waits for it. A panic inside `f` drops the reply channel
// rather than killing the thread, so one bad op doesn't take the rest of the session down.
fn on_hid_thread<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (done, wait) = mpsc::channel();
    let job: Job = Box::new(move || {
        if let Ok(v) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            let _ = done.send(v);
        }
    });
    hid_thread()
        .send(job)
        .map_err(|_| "the HID thread has stopped".to_string())?;
    wait.recv()
        .map_err(|_| "the HID operation failed".to_string())
}

// Remembers which cable we opened. Read and written from the HID thread and from close(),
// so it keeps its own lock.
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
    on_hid_thread(|| {
        use dsp::Transport;
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
    })?
}

pub fn close() {
    *EXPECTED.lock().unwrap() = None;
}

pub fn read_mode() -> Result<Option<i32>, String> {
    on_hid_thread(|| {
        let dev = backend::open_best(expected_pid())?;
        Ok(dsp::read_mode(&dev))
    })?
}

pub fn read_bank() -> Result<Vec<Band>, String> {
    on_hid_thread(|| {
        let dev = backend::open_best(expected_pid())?;
        Ok(dsp::read_bank(&dev))
    })?
}

pub fn set_preset(idx: i32) -> Result<Option<i32>, String> {
    on_hid_thread(move || {
        let dev = backend::open_best(expected_pid())?;
        guard(&dev)?;
        Ok(dsp::set_preset(&dev, idx))
    })?
}

pub fn write_bank(bands: Vec<BandIn>, preamp: f64) -> Result<WriteResult, String> {
    on_hid_thread(move || {
        let dev = backend::open_best(expected_pid())?;
        guard(&dev)?;
        Ok(dsp::write_bank(&dev, &bands, preamp))
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every op must land on the one long-lived HID thread — see the note above on why the
    // macOS backend can't be driven from a churn of short-lived threads.
    #[test]
    fn ops_share_one_permanent_thread() {
        let name = || {
            on_hid_thread(|| std::thread::current().name().map(str::to_string)).unwrap()
        };
        let first = name();
        assert_eq!(first.as_deref(), Some("hid"));

        // Called from other threads, and after a gap long enough for a tokio blocking-pool
        // thread to have idled out and taken its CFRunLoop with it.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let from_elsewhere = std::thread::spawn(name).join().unwrap();
        assert_eq!(from_elsewhere, first);
    }

    // A panicking op must not take the HID thread — and so every later op — down with it.
    #[test]
    fn a_panicking_op_leaves_the_thread_usable() {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let boom = on_hid_thread(|| panic!("boom"));
        std::panic::set_hook(prev);

        assert!(boom.is_err());
        assert_eq!(on_hid_thread(|| 42).unwrap(), 42);
    }

    // Repeated real device ops: the genuine open path, over and over, with gaps. This is the
    // shape that killed the app on macOS 27, so it's worth running for real rather than
    // against a stub. Passes whether or not a cable is attached — it's the crash we're
    // testing for, not the cable.
    #[test]
    fn repeated_ops_stay_alive() {
        for _ in 0..10 {
            if let Err(e) = read_bank() {
                assert!(
                    e.contains("no Moondrop cable found") || e.contains("could not open"),
                    "unexpected error: {e}"
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(120));
        }
    }

    // The pre-fix shape, kept as a stress harness: a fresh short-lived thread per operation,
    // which is what tauri's spawn_blocking gave us. Worth knowing that this did *not*
    // reproduce the field crash on the bench — 300 rounds against a live cable under
    // MallocScribble came back clean — so it stands as a regression harness for the HID path
    // rather than as proof of the cause. Ignored by default: it needs a cable, it's slow, and
    // if it ever does fail the way the wild reports do it takes the test process down with
    // SIGTRAP rather than failing an assertion. Run it with:
    //   HID_ROUNDS=300 cargo test --release -- --ignored --nocapture old_shape
    #[test]
    #[ignore = "slow, needs a cable attached; run explicitly"]
    fn old_shape_churns_threads() {
        let rounds: usize = std::env::var("HID_ROUNDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        for i in 0..rounds {
            let r = std::thread::spawn(|| {
                let dev = backend::open_best(None)?;
                Ok::<_, String>(dsp::read_bank(&dev).len())
            })
            .join()
            .expect("worker thread panicked");
            if i % 10 == 0 {
                println!("pass {i}: {r:?}");
            }
            // Recycle the freed run loops' memory.
            let mut keep: Vec<Vec<u8>> = Vec::new();
            for k in 0..2000 {
                let v = vec![0xABu8; 64 + (k % 1024)];
                if k % 3 == 0 {
                    keep.push(v);
                }
            }
            drop(keep);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
