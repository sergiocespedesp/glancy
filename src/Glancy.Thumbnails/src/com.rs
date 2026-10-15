//! COM objects: the thumbnail provider and its class factory.
//!
//! The provider is initialized with `IInitializeWithItem` or `IInitializeWithFile`. It is read-only: any open mode
//! that asks for write access is rejected with `STG_E_ACCESSDENIED`.
//!
//! It does not expose `IInitializeWithStream`: the thumbnail cache prefers it, and the stream only carries the file
//! name, not the path (verified with `glancy-thumb shell`). Without a path, excluded folders, desktop transparency
//! and cloud-file protection would not work, and the history would lack the full path the contract requires. For
//! items with no file path (for example, inside a ZIP) the item itself is asked for a stream.

use crate::delegate;
use crate::error::Error;
use crate::formats;
use crate::log;
use crate::pipeline::{self, Decision, Request};
use crate::settings;
use crate::source::{FileSource, Source, StreamSource};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Instant;
use windows::Win32::Foundation::{
    BOOL, CLASS_E_NOAGGREGATION, E_FAIL, E_POINTER, E_UNEXPECTED, ERROR_ALREADY_INITIALIZED, STG_E_ACCESSDENIED,
};
use windows::Win32::Graphics::Gdi::HBITMAP;
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl, IStream};
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithFile_Impl};
use windows::Win32::UI::Shell::{
    IInitializeWithItem, IInitializeWithItem_Impl, IShellItem, IThumbnailProvider, IThumbnailProvider_Impl,
    SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY, WTS_ALPHATYPE, WTSAT_ARGB,
};
use windows::core::{GUID, HRESULT, IUnknown, Interface, PCWSTR, implement};

/// Live objects and server locks, for `DllCanUnloadNow`.
pub static OBJECTS: AtomicI32 = AtomicI32::new(0);
pub static LOCKS: AtomicI32 = AtomicI32::new(0);

enum Input {
    Path(PathBuf),
    /// `name` is what `IStream::Stat` returns (sometimes the full path, sometimes just the name).
    Stream { stream: IStream, name: Option<String> },
}

#[implement(IThumbnailProvider, IInitializeWithItem, IInitializeWithFile)]
pub struct Provider {
    input: Mutex<Option<Input>>,
}

impl Provider {
    pub fn new() -> Self {
        OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self { input: Mutex::new(None) }
    }

    fn set(&self, input: Input) -> windows::core::Result<()> {
        let mut slot = self.input.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return Err(ERROR_ALREADY_INITIALIZED.to_hresult().into());
        }
        *slot = Some(input);
        Ok(())
    }
}

impl Default for Provider {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Read-only: rejects STGM_WRITE (1) or STGM_READWRITE (2).
fn check_mode(grfmode: u32) -> windows::core::Result<()> {
    if grfmode & 3 != 0 { Err(STG_E_ACCESSDENIED.into()) } else { Ok(()) }
}

impl IInitializeWithItem_Impl for Provider_Impl {
    fn Initialize(&self, psi: Option<&IShellItem>, grfmode: u32) -> windows::core::Result<()> {
        check_mode(grfmode)?;
        let item = psi.ok_or(windows::core::Error::from(E_POINTER))?;
        let name = |sigdn| unsafe {
            let p = item.GetDisplayName(sigdn).ok()?;
            let s = p.to_string().ok();
            windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const c_void));
            s
        };
        if let Some(path) = name(SIGDN_FILESYSPATH) {
            log::debug(&format!("IInitializeWithItem: {path}"));
            return self.set(Input::Path(PathBuf::from(path)));
        }
        // Item with no file path (ZIP, portable device…): read it as a stream.
        let stream: IStream = unsafe { item.BindToHandler(None, &windows::Win32::UI::Shell::BHID_Stream)? };
        let display = name(windows::Win32::UI::Shell::SIGDN_PARENTRELATIVEPARSING).or_else(|| name(SIGDN_NORMALDISPLAY));
        log::debug(&format!("IInitializeWithItem (stream): {}", display.as_deref().unwrap_or("?")));
        self.set(Input::Stream { stream, name: display })
    }
}

impl IInitializeWithFile_Impl for Provider_Impl {
    fn Initialize(&self, pszfilepath: &PCWSTR, grfmode: u32) -> windows::core::Result<()> {
        check_mode(grfmode)?;
        if pszfilepath.is_null() {
            return Err(E_POINTER.into());
        }
        let path = unsafe { pszfilepath.to_string() }.map_err(|_| windows::core::Error::from(E_FAIL))?;
        log::debug(&format!("IInitializeWithFile: {path}"));
        self.set(Input::Path(PathBuf::from(path)))
    }
}

// The signature is fixed by the COM interface: the code checks the pointer and the caller guarantees it.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
impl IThumbnailProvider_Impl for Provider_Impl {
    fn GetThumbnail(&self, cx: u32, phbmp: *mut HBITMAP, pdwalpha: *mut WTS_ALPHATYPE) -> windows::core::Result<()> {
        if phbmp.is_null() {
            return Err(E_POINTER.into());
        }
        let guard = self.input.lock().unwrap_or_else(|e| e.into_inner());
        let input = guard.as_ref().ok_or(windows::core::Error::from(E_UNEXPECTED))?;
        // No panic may cross the COM boundary: it would bring down Explorer.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| thumbnail(input, cx)));
        let (hbmp, alpha) = match result {
            Ok(r) => r?,
            Err(_) => {
                log::history(false, 0, &display_name(input), Some("Internal engine failure"));
                return Err(E_FAIL.into());
            }
        };
        unsafe {
            *phbmp = hbmp;
            if !pdwalpha.is_null() {
                *pdwalpha = alpha;
            }
        }
        Ok(())
    }
}

fn display_name(input: &Input) -> String {
    match input {
        Input::Path(p) => p.to_string_lossy().into_owned(),
        Input::Stream { name, .. } => name.clone().unwrap_or_else(|| "(unnamed stream)".into()),
    }
}

/// Full path if the name looks like one (`C:\…` or `\\server\…`).
fn as_full_path(name: &str) -> Option<PathBuf> {
    let b = name.as_bytes();
    let drive = b.len() > 2 && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    (drive || name.starts_with(r"\\")).then(|| PathBuf::from(name))
}

fn extension_of(name: &str) -> Option<String> {
    Path::new(name).extension().map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase()))
}

fn on_desktop(path: &Path) -> bool {
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_PublicDesktop, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    let Some(parent) = path.parent() else { return false };
    [FOLDERID_Desktop, FOLDERID_PublicDesktop].iter().any(|id| unsafe {
        match SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) {
            Ok(p) => {
                let s = p.to_string().unwrap_or_default();
                windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const c_void));
                !s.is_empty() && pipeline::is_inside(parent, &s) && parent.to_string_lossy().len() <= s.trim_end_matches('\\').len()
            }
            Err(_) => false,
        }
    })
}

/// OneDrive (or other cloud provider) file that is not downloaded: reading it would force the download.
fn is_cloud_placeholder(path: &Path) -> bool {
    use windows::Win32::Storage::FileSystem::GetFileAttributesW;
    const RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
    const OFFLINE: u32 = 0x0000_1000;
    let attrs = unsafe { GetFileAttributesW(&windows::core::HSTRING::from(path.as_os_str())) };
    attrs != u32::MAX && attrs & (RECALL_ON_DATA_ACCESS | OFFLINE) != 0
}

/// All the work for one thumbnail: decision, delegation, rendering and history.
fn thumbnail(input: &Input, cx: u32) -> windows::core::Result<(HBITMAP, WTS_ALPHATYPE)> {
    let start = Instant::now();
    let shown = display_name(input);
    let path: Option<PathBuf> = match input {
        Input::Path(p) => Some(p.clone()),
        Input::Stream { name, .. } => name.as_deref().and_then(as_full_path),
    };

    let snapshot = settings::current();
    // Open the source and work out the format (by extension or, if there is no name, by content).
    let src: Box<dyn Source> = match input {
        Input::Path(p) => match FileSource::open(p) {
            Ok(s) => Box::new(s),
            Err(e) => {
                log::history(false, start.elapsed().as_millis(), &shown, Some(&format!("Couldn't open: {e}")));
                return Err(E_FAIL.into());
            }
        },
        Input::Stream { stream, .. } => Box::new(StreamSource::new(stream.clone())?),
    };
    // Content is only inspected when there is no extension: a foreign extension (e.g. .html, which shares a ProgID
    // with .svg) goes back to its own provider even if it contains "<svg".
    let ext = extension_of(&shown).or_else(|| {
        let head = src.read_vec(0, 4096).ok()?;
        formats::sniff(&head).map(|f| f.extensions()[0].clone())
    });
    let Some(eff) = ext.as_deref().and_then(|e| settings::resolve(&*snapshot, e)) else {
        // Another extension that shares a ProgID with one of Glancy's may arrive (e.g. .html with MSEdgeHTM, like
        // .svg): it goes back to the provider it had before, leaving no trace in the history.
        if let Some(other) = extension_of(&shown)
            && let Some(clsid) = delegate::resolve_without_glancy(&other)
        {
            let delegated = match input {
                Input::Path(p) => delegate::thumbnail(clsid, delegate::Input::Path(p), cx),
                Input::Stream { stream, .. } => delegate::thumbnail(clsid, delegate::Input::Stream(stream), cx),
            };
            if delegated.is_ok() {
                return delegated;
            }
        }
        log::debug(&format!("Format not handled by Glancy: {shown}"));
        return Err(E_FAIL.into());
    };
    log::set_debug(eff.global.debug_log);
    crate::registration::sync_if_revision_changed(&snapshot);
    log::debug(&format!("GetThumbnail {cx}px {shown} ({})", eff.extension));

    match pipeline::decide(&eff, path.as_deref(), cx) {
        Decision::Skip(why) => {
            log::debug(&format!("No thumbnail ({why}): {shown}"));
            return Err(E_FAIL.into());
        }
        Decision::Delegate => {
            if let Some(clsid) = delegate::previous_handler(&eff.extension) {
                let delegated = match input {
                    Input::Path(p) => delegate::thumbnail(clsid, delegate::Input::Path(p), cx),
                    Input::Stream { stream, .. } => delegate::thumbnail(clsid, delegate::Input::Stream(stream), cx),
                };
                match delegated {
                    Ok(r) => {
                        log::history(true, start.elapsed().as_millis(), &shown, None);
                        return Ok(r);
                    }
                    Err(e) => log::debug(&format!("Previous provider {clsid:?} failed ({e}); Glancy renders it")),
                }
            } else {
                log::debug("No previous provider; Glancy renders it");
            }
        }
        Decision::Render => {}
    }

    if let Some(p) = path.as_deref()
        && matches!(input, Input::Path(_))
        && is_cloud_placeholder(p)
    {
        log::history(false, start.elapsed().as_millis(), &shown, Some("Cloud-only file: not downloaded"));
        return Err(E_FAIL.into());
    }

    let req = Request {
        src: &*src,
        path: path.as_deref().filter(|_| matches!(input, Input::Path(_))),
        size: cx,
        on_desktop: path.as_deref().is_some_and(on_desktop),
    };
    let rendered = pipeline::render(&req, &eff).and_then(|img| crate::bitmap::to_hbitmap(&img));
    let ms = start.elapsed().as_millis();
    match rendered {
        Ok(hbmp) => {
            log::history(true, ms, &shown, None);
            Ok((hbmp, WTSAT_ARGB))
        }
        Err(Error(msg)) => {
            log::history(false, ms, &shown, Some(&msg));
            log::debug(&format!("Error: {msg}"));
            Err(E_FAIL.into())
        }
    }
}

#[implement(IClassFactory)]
pub struct Factory;

#[allow(clippy::not_unsafe_ptr_arg_deref)]
impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, punkouter: Option<&IUnknown>, riid: *const GUID, ppvobject: *mut *mut c_void) -> windows::core::Result<()> {
        if ppvobject.is_null() || riid.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *ppvobject = std::ptr::null_mut() };
        if punkouter.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let unknown: IUnknown = Provider::new().into();
        unsafe { unknown.query(riid, ppvobject).ok() }
    }

    fn LockServer(&self, flock: BOOL) -> windows::core::Result<()> {
        if flock.as_bool() {
            LOCKS.fetch_add(1, Ordering::SeqCst);
        } else {
            LOCKS.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

/// Implementation of `DllGetClassObject`.
///
/// # Safety
/// `ppv` must be null or point to writable memory.
pub unsafe fn get_class_object(rclsid: &GUID, riid: &GUID, ppv: *mut *mut c_void) -> HRESULT {
    use windows::Win32::Foundation::CLASS_E_CLASSNOTAVAILABLE;
    if ppv.is_null() {
        return E_POINTER;
    }
    unsafe { *ppv = std::ptr::null_mut() };
    if *rclsid != crate::registration::CLSID_GLANCY {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = Factory.into();
    unsafe { factory.query(riid, ppv) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_path_detection() {
        assert_eq!(as_full_path(r"C:\a\b.psd"), Some(PathBuf::from(r"C:\a\b.psd")));
        assert_eq!(as_full_path(r"\\srv\share\b.psd"), Some(PathBuf::from(r"\\srv\share\b.psd")));
        assert_eq!(as_full_path("b.psd"), None);
        assert_eq!(extension_of("Foto.PSB"), Some(".psb".into()));
        assert_eq!(extension_of("sin"), None);
    }

    #[test]
    fn write_modes_are_rejected() {
        assert!(check_mode(0).is_ok());
        assert!(check_mode(1).is_err());
        assert!(check_mode(2).is_err());
        assert!(check_mode(0x40).is_ok()); // STGM_SHARE_DENY_NONE
    }
}
