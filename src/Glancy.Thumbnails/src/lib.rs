//! Glancy's thumbnail engine: an `IThumbnailProvider` for PSD/PSB, AI, EPS, PDF and SVG/SVGZ that reads its
//! settings from `HKCU\Software\Glancy\Thumbnails` (docs/registry-contract.md) and logs every thumbnail to
//! `%LOCALAPPDATA%\Glancy\Logs\history.log`. Design notes: docs/architecture.md.

pub mod bitmap;
pub mod catalog;
pub mod com;
pub mod compose;
pub mod delegate;
pub mod error;
pub mod formats;
pub mod image;
pub mod log;
pub mod pipeline;
pub mod reg;
pub mod registration;
pub mod resize;
pub mod settings;
pub mod source;
pub mod wic;

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use windows::Win32::Foundation::{BOOL, E_FAIL, E_INVALIDARG, S_FALSE, S_OK};
use windows::core::{GUID, HRESULT, PCWSTR};

/// # Safety
/// Called by COM with valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(rclsid: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
    if rclsid.is_null() || riid.is_null() {
        return E_INVALIDARG;
    }
    std::panic::catch_unwind(|| unsafe { com::get_class_object(&*rclsid, &*riid, ppv) }).unwrap_or(E_FAIL)
}

#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    // PDFium cannot be reinitialized in the same process: once it is loaded, the engine stays loaded.
    let busy = com::OBJECTS.load(Ordering::SeqCst) > 0 || com::LOCKS.load(Ordering::SeqCst) > 0;
    if busy || formats::pdf::is_loaded() { S_FALSE } else { S_OK }
}

fn hresult(r: error::Result<()>) -> HRESULT {
    match r {
        Ok(()) => S_OK,
        Err(e) => {
            log::debug(&format!("Registration: {e}"));
            E_FAIL
        }
    }
}

/// `regsvr32 Glancy.Thumbnails.dll`: registers for the current user (no administrator rights needed).
#[unsafe(no_mangle)]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    std::panic::catch_unwind(|| match registration::module_path() {
        Some(p) => hresult(registration::register(&p)),
        None => E_FAIL,
    })
    .unwrap_or(E_FAIL)
}

/// `regsvr32 /u Glancy.Thumbnails.dll`: removes the registration and restores the previous providers.
#[unsafe(no_mangle)]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    std::panic::catch_unwind(|| hresult(registration::unregister())).unwrap_or(E_FAIL)
}

/// `regsvr32 /n /i:sync Glancy.Thumbnails.dll`: applies `Treatment`/`TypeOverlay` right away from the current
/// settings (useful right after the app applies changes). Without `sync`, it is the same as registering or
/// unregistering.
///
/// # Safety
/// `cmdline` is null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllInstall(install: BOOL, cmdline: PCWSTR) -> HRESULT {
    let cmd = if cmdline.is_null() { String::new() } else { unsafe { cmdline.to_string() }.unwrap_or_default() };
    std::panic::catch_unwind(|| {
        if cmd.trim().eq_ignore_ascii_case("sync") {
            let snap = settings::Snapshot::capture(&settings::RegistrySource::new(settings::SETTINGS_KEY));
            if registration::sync_shell_values(&snap) {
                registration::notify_association_change();
            }
            S_OK
        } else if install.as_bool() {
            DllRegisterServer()
        } else {
            DllUnregisterServer()
        }
    })
    .unwrap_or(E_FAIL)
}
