//! Runs a render on its own thread, with a large stack and a time limit.
//!
//! The engine runs inside Explorer (`DisableProcessIsolation`), whose threads have 512 KB of stack and must not hang.
//! A decoder that recurses deeply gets 16 MB here instead, and one that never finishes (PDFium, resvg and the system
//! codecs cannot be interrupted) is abandoned after the limit: Explorer gets an error at once, and the thread keeps
//! running until it ends on its own. While too many abandoned threads are still running, new renders are refused, so
//! a folder full of hostile files cannot pile up threads and memory.

use crate::error::{Error, Result};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;

/// Stack of the render thread.
pub const STACK_SIZE: usize = 16 * 1024 * 1024;
/// Time Explorer waits for a thumbnail before the render is abandoned.
pub const DEADLINE: Duration = Duration::from_secs(30);
/// Abandoned renders still running beyond which new renders are refused.
pub const MAX_ABANDONED: usize = 4;

/// Abandoned renders that are still running, for the engine's renders.
pub static ABANDONED: AtomicUsize = AtomicUsize::new(0);

/// Runs `f` on a new thread and waits up to `deadline` for it. `abandoned` counts the threads given up on that are
/// still running; with `max_abandoned` of them, `f` is not run.
pub fn run<T: Send + 'static>(
    abandoned: &'static AtomicUsize,
    max_abandoned: usize,
    deadline: Duration,
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    if abandoned.load(Ordering::SeqCst) >= max_abandoned {
        return Err(Error::new("Engine busy: earlier thumbnails are still being drawn"));
    }
    // RUNNING → DONE (the thread finished in time) or RUNNING → GIVEN_UP (the caller gave up). Whoever loses the
    // race knows the other side's state: the counter is raised before giving up and lowered by the thread that ends
    // after being abandoned, so it never goes below zero.
    let state = Arc::new(AtomicU8::new(RUNNING));
    let thread_state = state.clone();
    let (tx, rx) = mpsc::channel();
    keep_module_loaded();
    let spawned = std::thread::Builder::new().name("glancy-render".into()).stack_size(STACK_SIZE).spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
            .unwrap_or_else(|_| Err(Error::new("Internal engine failure")));
        let _ = tx.send(r);
        if thread_state.compare_exchange(RUNNING, DONE, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            abandoned.fetch_sub(1, Ordering::SeqCst);
        }
    });
    if let Err(e) = spawned {
        return Err(Error(format!("Couldn't start the render thread: {e}")));
    }
    match rx.recv_timeout(deadline) {
        Ok(r) => r,
        Err(RecvTimeoutError::Disconnected) => Err(Error::new("Internal engine failure")),
        Err(RecvTimeoutError::Timeout) => {
            abandoned.fetch_add(1, Ordering::SeqCst);
            if state.compare_exchange(RUNNING, GIVEN_UP, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                return Err(Error(format!("Took more than {} s to draw", deadline.as_secs_f32())));
            }
            // It finished just now: its result is already in the channel.
            abandoned.fetch_sub(1, Ordering::SeqCst);
            rx.recv().unwrap_or_else(|_| Err(Error::new("Internal engine failure")))
        }
    }
}

static STARTED: AtomicBool = AtomicBool::new(false);

/// `true` once a render thread has been started in this process.
pub fn started() -> bool {
    STARTED.load(Ordering::SeqCst)
}

/// An abandoned thread keeps running this module's code, so the module must never be unloaded after the first one
/// starts: it is pinned (FreeLibrary no longer unloads it) and `DllCanUnloadNow` says no.
fn keep_module_loaded() {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW,
    };
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut h = HMODULE::default();
    let _ = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
            windows::core::PCWSTR(keep_module_loaded as *const u16),
            &mut h,
        )
    };
}

const RUNNING: u8 = 0;
const DONE: u8 = 1;
const GIVEN_UP: u8 = 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_result() {
        static A: AtomicUsize = AtomicUsize::new(0);
        assert_eq!(run(&A, 4, Duration::from_secs(5), || Ok(42)), Ok(42));
        assert_eq!(run::<()>(&A, 4, Duration::from_secs(5), || Err(Error::new("bad"))), Err(Error::new("bad")));
    }

    #[test]
    fn gives_up_after_the_deadline() {
        static A: AtomicUsize = AtomicUsize::new(0);
        let (release, blocked) = mpsc::channel::<()>();
        let start = std::time::Instant::now();
        let r = run(&A, 4, Duration::from_millis(100), move || {
            let _ = blocked.recv();
            Ok(())
        });
        assert!(r.unwrap_err().0.contains("more than"), "the error says it took too long");
        // Generous for shared CI machines: what matters is that it does not wait for the render, which never ends
        // until it is released.
        assert!(start.elapsed() < Duration::from_secs(10));
        assert_eq!(A.load(Ordering::SeqCst), 1, "the abandoned thread is still running");
        release.send(()).unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(10);
        while A.load(Ordering::SeqCst) != 0 && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(A.load(Ordering::SeqCst), 0, "it is no longer counted once it ends");
    }

    #[test]
    fn refuses_while_too_many_are_abandoned() {
        static A: AtomicUsize = AtomicUsize::new(0);
        A.store(2, Ordering::SeqCst);
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = ran.clone();
        let r = run(&A, 2, Duration::from_secs(5), move || {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        });
        assert!(r.unwrap_err().0.contains("busy"));
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[test]
    fn a_panic_is_an_error() {
        static A: AtomicUsize = AtomicUsize::new(0);
        let r = run::<()>(&A, 4, Duration::from_secs(5), || panic!("boom"));
        assert!(r.unwrap_err().0.contains("Internal"));
    }

    #[test]
    fn the_module_stays_loaded_once_a_thread_has_started() {
        // An abandoned thread still runs this module's code: unloading it would crash the host.
        static A: AtomicUsize = AtomicUsize::new(0);
        run(&A, 4, Duration::from_secs(5), || Ok(())).unwrap();
        assert!(started());
        assert_eq!(crate::DllCanUnloadNow(), windows::Win32::Foundation::S_FALSE);
    }

    #[test]
    fn deep_recursion_fits_in_the_stack() {
        // About 8 MB of stack: more than Explorer's threads (512 KB) and the test threads (2 MB) have.
        fn depth(n: u32) -> u64 {
            let pad = std::hint::black_box([n as u8; 1024]);
            if n == 0 { pad[0] as u64 } else { depth(n - 1) + pad[1] as u64 }
        }
        static A: AtomicUsize = AtomicUsize::new(0);
        assert!(run(&A, 4, Duration::from_secs(10), || Ok(depth(8000))).is_ok());
    }
}
