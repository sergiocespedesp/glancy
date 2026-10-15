//! Engine test tool. Usage:
//!
//! ```text
//! glancy-thumb render <file> [size] [output.png]
//!     Runs the engine in this process, with the real settings from HKCU\Software\Glancy\Thumbnails.
//! glancy-thumb com <dll> <file> [size] [output.png] [item|file]
//!     Loads the DLL without registering it and uses it through COM, like Explorer (IInitializeWithItem by
//!     default).
//! glancy-thumb clsid <{CLSID}> <file> [size] [output.png]
//!     Asks a registered provider (e.g. another thumbnail handler) for the thumbnail, for comparison.
//! glancy-thumb shell <file> [size] [output.png]
//!     Forced extraction through the Windows thumbnail cache (IThumbnailCache), with whichever provider
//!     Explorer has registered for that extension.
//! ```

use glancy_thumbnails::{bitmap, delegate, image::Image, pipeline, settings, source::FileSource, wic};
use std::path::{Path, PathBuf};
use std::time::Instant;
use windows::Win32::Graphics::Gdi::{DeleteObject, HBITMAP};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IClassFactory};
use windows::Win32::UI::Shell::{IShellItem, IThumbnailProvider, SHCreateItemFromParsingName, WTS_ALPHATYPE};
use windows::core::{GUID, HSTRING, Interface};

fn usage() -> ! {
    eprintln!(
        "usage:\n  glancy-thumb render <file> [size] [output.png]\n  glancy-thumb com <dll> <file> [size] [output.png] [item|file]\n  glancy-thumb clsid <{{CLSID}}> <file> [size] [output.png]\n  glancy-thumb shell <file> [size] [output.png]"
    );
    std::process::exit(2)
}

fn default_out(file: &Path, tag: &str) -> PathBuf {
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "thumbnail".into());
    std::env::temp_dir().join(format!("{stem}.{tag}.png"))
}

fn report(tag: &str, img: &Image, out: &Path, start: Instant, alpha: Option<WTS_ALPHATYPE>) {
    wic::save_png(img, out).unwrap_or_else(|e| panic!("Couldn't save {}: {e}", out.display()));
    let transparent = img.data.as_chunks::<4>().0.iter().filter(|p| p[3] < 255).count();
    println!(
        "{tag}: {}×{} in {} ms{} · non-opaque pixels: {transparent} → {}",
        img.width,
        img.height,
        start.elapsed().as_millis(),
        alpha.map(|a| format!(" · WTS_ALPHATYPE {}", a.0)).unwrap_or_default(),
        out.display()
    );
}

fn hbitmap_to_image(h: HBITMAP) -> Image {
    let img = bitmap::from_hbitmap(h).expect("Unreadable HBITMAP");
    unsafe {
        let _ = DeleteObject(h);
    }
    img
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The thumbnail cache, like Explorer, is used from an STA apartment.
    if args.first().map(String::as_str) == Some("shell") {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        }
    }
    wic::ensure_com();
    let Some(cmd) = args.first() else { usage() };
    let start = Instant::now();
    match cmd.as_str() {
        "render" => {
            let file = PathBuf::from(args.get(1).unwrap_or_else(|| usage()));
            let size: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(256);
            let out = args.get(3).map(PathBuf::from).unwrap_or_else(|| default_out(&file, "glancy"));
            let ext = file.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            let snap = settings::current();
            let eff = settings::resolve(&*snap, &ext).unwrap_or_else(|| {
                eprintln!("Extension not handled by Glancy: {ext}");
                std::process::exit(1)
            });
            let src = FileSource::open(&file).expect("Couldn't open the file");
            let req = pipeline::Request { src: &src, path: Some(&file), size, on_desktop: false };
            match pipeline::render(&req, &eff) {
                Ok(img) => report("render", &img, &out, start, None),
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1)
                }
            }
        }
        "com" => {
            let dll = args.get(1).unwrap_or_else(|| usage());
            let file = PathBuf::from(args.get(2).unwrap_or_else(|| usage()));
            let size: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(256);
            let out = args.get(4).map(PathBuf::from).unwrap_or_else(|| default_out(&file, "com"));
            let mode = args.get(5).map(String::as_str).unwrap_or("item");
            match via_dll(dll, &file, size, mode) {
                Ok((h, a)) => report(&format!("com/{mode}"), &hbitmap_to_image(h), &out, start, Some(a)),
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1)
                }
            }
        }
        "clsid" => {
            let clsid = delegate::parse_guid(args.get(1).unwrap_or_else(|| usage())).unwrap_or_else(|| usage());
            let file = PathBuf::from(args.get(2).unwrap_or_else(|| usage()));
            let size: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(256);
            let out = args.get(4).map(PathBuf::from).unwrap_or_else(|| default_out(&file, "clsid"));
            match delegate::thumbnail(clsid, delegate::Input::Path(&file), size) {
                Ok((h, a)) => report("clsid", &hbitmap_to_image(h), &out, start, Some(a)),
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1)
                }
            }
        }
        "shell" => {
            let file = PathBuf::from(args.get(1).unwrap_or_else(|| usage()));
            let size: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(256);
            let out = args.get(3).map(PathBuf::from).unwrap_or_else(|| default_out(&file, "shell"));
            match via_cache(&file, size) {
                Ok((h, a)) => report("shell", &hbitmap_to_image(h), &out, start, Some(a)),
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1)
                }
            }
        }
        _ => usage(),
    }
}

/// Loads the DLL with LoadLibrary and creates the provider through its `DllGetClassObject`, bypassing the registry.
fn via_dll(dll: &str, file: &Path, size: u32, mode: &str) -> windows::core::Result<(HBITMAP, WTS_ALPHATYPE)> {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::Win32::UI::Shell::IInitializeWithItem;
    use windows::Win32::UI::Shell::PropertiesSystem::IInitializeWithFile;
    type GetClassObject = unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut std::ffi::c_void) -> windows::core::HRESULT;
    unsafe {
        let module = LoadLibraryW(&HSTRING::from(dll))?;
        let f = GetProcAddress(module, windows::core::s!("DllGetClassObject"))
            .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_NOINTERFACE))?;
        let get: GetClassObject = std::mem::transmute(f);
        let clsid = glancy_thumbnails::registration::CLSID_GLANCY;
        let mut factory = std::ptr::null_mut();
        get(&clsid, &IClassFactory::IID, &mut factory).ok()?;
        let factory = IClassFactory::from_raw(factory);
        let provider: IThumbnailProvider = factory.CreateInstance(None)?;
        let wide = HSTRING::from(file.as_os_str());
        match mode {
            "file" => provider.cast::<IInitializeWithFile>()?.Initialize(&wide, 0)?,
            _ => {
                let item: IShellItem = SHCreateItemFromParsingName(&wide, None)?;
                provider.cast::<IInitializeWithItem>()?.Initialize(&item, 0)?
            }
        }
        let mut h = HBITMAP::default();
        let mut a = WTS_ALPHATYPE::default();
        provider.GetThumbnail(size, &mut h, &mut a)?;
        Ok((h, a))
    }
}

/// Forced extraction through the system thumbnail cache (the same path Explorer takes).
fn via_cache(file: &Path, size: u32) -> windows::core::Result<(HBITMAP, WTS_ALPHATYPE)> {
    use windows::Win32::UI::Shell::{
        LocalThumbnailCache, ISharedBitmap, IThumbnailCache, WTS_EXTRACT, WTS_FORCEEXTRACTION,
    };
    unsafe {
        let cache: IThumbnailCache = CoCreateInstance(&LocalThumbnailCache, None, CLSCTX_INPROC_SERVER)?;
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(file.as_os_str()), None)?;
        // The first call forces the extraction; if the cache performs it in the background
        // (WTS_E_EXTRACTIONPENDING), wait for it to finish.
        const PENDING: windows::core::HRESULT = windows::core::HRESULT(0x8004_B205_u32 as i32);
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        let mut flags = WTS_EXTRACT | WTS_FORCEEXTRACTION;
        let shared = loop {
            let mut shared: Option<ISharedBitmap> = None;
            match cache.GetThumbnail(&item, size, flags, Some(&mut shared), None, None) {
                Ok(()) => break shared.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?,
                Err(e) if e.code() == PENDING && Instant::now() < deadline => {
                    flags = WTS_EXTRACT;
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                Err(e) => return Err(e),
            }
        };
        let alpha = shared.GetFormat()?;
        // Detach: the HBITMAP is now ours.
        let h = shared.Detach()?;
        Ok((h, alpha))
    }
}
