//! PDF (and PDF-compatible AI) with PDFium. `pdfium.dll` is loaded dynamically from the engine's folder by
//! absolute path (never from the PATH). PDFium is not reentrant: every call goes through a lock.

use crate::bail;
use crate::error::{Error, Result};
use crate::image::Image;
use crate::source::Source;
use std::ffi::c_void;
use std::os::raw::{c_char, c_int, c_uchar, c_ulong};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW};
use windows::core::{HSTRING, PCSTR};

type Doc = *mut c_void;
type Page = *mut c_void;
type Bitmap = *mut c_void;

#[repr(C)]
struct FileAccess {
    file_len: c_ulong,
    get_block: unsafe extern "C" fn(param: *mut c_void, position: c_ulong, buf: *mut c_uchar, size: c_ulong) -> c_int,
    param: *mut c_void,
}

#[repr(C)]
struct Pause {
    version: c_int,
    need_to_pause_now: unsafe extern "C" fn(this: *mut Pause) -> c_int,
    user: *mut c_void,
}

const FPDF_ERR_PASSWORD: c_ulong = 4;
const FPDF_ERR_SECURITY: c_ulong = 5;
const FPDF_ANNOT: c_int = 0x01;
const BITMAP_BGRA: c_int = 4;
const RENDER_TOBECONTINUED: c_int = 1;
const RENDER_DONE: c_int = 2;

/// Maximum time to draw a page before giving up (protects Explorer from pathological PDFs).
pub const RENDER_TIMEOUT: Duration = Duration::from_secs(20);

struct Api {
    _module: HMODULE,
    load_custom_document: unsafe extern "C" fn(*mut FileAccess, *const c_char) -> Doc,
    get_last_error: unsafe extern "C" fn() -> c_ulong,
    get_page_count: unsafe extern "C" fn(Doc) -> c_int,
    load_page: unsafe extern "C" fn(Doc, c_int) -> Page,
    page_width: unsafe extern "C" fn(Page) -> f32,
    page_height: unsafe extern "C" fn(Page) -> f32,
    close_page: unsafe extern "C" fn(Page),
    close_document: unsafe extern "C" fn(Doc),
    bitmap_create_ex: unsafe extern "C" fn(c_int, c_int, c_int, *mut c_void, c_int) -> Bitmap,
    bitmap_fill_rect: unsafe extern "C" fn(Bitmap, c_int, c_int, c_int, c_int, c_ulong) -> c_int,
    bitmap_buffer: unsafe extern "C" fn(Bitmap) -> *mut c_void,
    bitmap_stride: unsafe extern "C" fn(Bitmap) -> c_int,
    bitmap_destroy: unsafe extern "C" fn(Bitmap),
    render_start: unsafe extern "C" fn(Bitmap, Page, c_int, c_int, c_int, c_int, c_int, c_int, *mut Pause) -> c_int,
    render_continue: unsafe extern "C" fn(Page, *mut Pause) -> c_int,
    render_close: unsafe extern "C" fn(Page),
}

// The function pointers are read-only and their use is serialized by LOCK.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

static API: OnceLock<std::result::Result<Api, String>> = OnceLock::new();
static LOCK: Mutex<()> = Mutex::new(());

/// Folder of the module that contains this code (the engine DLL, or the executable in tests).
pub fn module_dir() -> Option<std::path::PathBuf> {
    use windows::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
        GetModuleHandleExW,
    };
    unsafe {
        let mut h = HMODULE::default();
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            windows::core::PCWSTR(module_dir as *const u16),
            &mut h,
        )
        .ok()?;
        let mut buf = vec![0u16; 32_768];
        let n = GetModuleFileNameW(h, &mut buf) as usize;
        if n == 0 {
            return None;
        }
        std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n])).parent().map(|p| p.to_path_buf())
    }
}

/// Path of `pdfium.dll`: next to the engine; during development `GLANCY_PDFIUM` (an absolute path) is accepted.
fn pdfium_path() -> Option<std::path::PathBuf> {
    let beside = module_dir()?.join("pdfium.dll");
    if beside.is_file() {
        return Some(beside);
    }
    let env = std::env::var_os("GLANCY_PDFIUM").map(std::path::PathBuf::from)?;
    (env.is_absolute() && env.is_file()).then_some(env)
}

// The target type of each `transmute` is set by the `Api` field it is assigned to.
#[allow(clippy::missing_transmute_annotations)]
fn load() -> std::result::Result<Api, String> {
    let path = pdfium_path().ok_or("pdfium.dll not found next to the engine")?;
    unsafe {
        let module = LoadLibraryExW(&HSTRING::from(path.as_os_str()), None, LOAD_WITH_ALTERED_SEARCH_PATH)
            .map_err(|e| format!("Couldn't load pdfium.dll: {}", e.message()))?;
        macro_rules! sym {
            ($name:literal) => {{
                let p = GetProcAddress(module, PCSTR(concat!($name, "\0").as_ptr()))
                    .ok_or_else(|| format!("pdfium.dll doesn't export {}", $name))?;
                std::mem::transmute(p)
            }};
        }
        let init: unsafe extern "C" fn() = sym!("FPDF_InitLibrary");
        let api = Api {
            _module: module,
            load_custom_document: sym!("FPDF_LoadCustomDocument"),
            get_last_error: sym!("FPDF_GetLastError"),
            get_page_count: sym!("FPDF_GetPageCount"),
            load_page: sym!("FPDF_LoadPage"),
            page_width: sym!("FPDF_GetPageWidthF"),
            page_height: sym!("FPDF_GetPageHeightF"),
            close_page: sym!("FPDF_ClosePage"),
            close_document: sym!("FPDF_CloseDocument"),
            bitmap_create_ex: sym!("FPDFBitmap_CreateEx"),
            bitmap_fill_rect: sym!("FPDFBitmap_FillRect"),
            bitmap_buffer: sym!("FPDFBitmap_GetBuffer"),
            bitmap_stride: sym!("FPDFBitmap_GetStride"),
            bitmap_destroy: sym!("FPDFBitmap_Destroy"),
            render_start: sym!("FPDF_RenderPageBitmap_Start"),
            render_continue: sym!("FPDF_RenderPage_Continue"),
            render_close: sym!("FPDF_RenderPage_Close"),
        };
        // A single initialization per process; the library is never unloaded (see DllCanUnloadNow).
        init();
        Ok(api)
    }
}

fn api() -> Result<&'static Api> {
    API.get_or_init(load).as_ref().map_err(|e| Error(e.clone()))
}

/// `true` if PDFium is already loaded (in which case the engine DLL must not be unloaded).
pub fn is_loaded() -> bool {
    matches!(API.get(), Some(Ok(_)))
}

unsafe extern "C" fn get_block(param: *mut c_void, position: c_ulong, buf: *mut c_uchar, size: c_ulong) -> c_int {
    let src = unsafe { &*(param as *const &dyn Source) };
    let out = unsafe { std::slice::from_raw_parts_mut(buf, size as usize) };
    // Never let a panic escape into PDFium's C code.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| src.read_exact_at(position as u64, out).is_ok()))
        .unwrap_or(false) as c_int
}

#[repr(C)]
struct Deadline {
    pause: Pause,
    until: Instant,
}

unsafe extern "C" fn need_to_pause(this: *mut Pause) -> c_int {
    // `pause` is the first field of Deadline (repr(C)).
    let d = unsafe { &*(this as *const Deadline) };
    (Instant::now() >= d.until) as c_int
}

/// First page, fitted to a `size`×`size` box, on a transparent background (straight alpha).
pub fn render_first_page(src: &dyn Source, size: u32) -> Result<Image> {
    if src.len() > c_ulong::MAX as u64 {
        bail!("PDF larger than 4 GB (unsupported)");
    }
    let api = api()?;
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut src_ref: &dyn Source = src;
    let mut access = FileAccess {
        file_len: src.len() as c_ulong,
        get_block,
        param: &mut src_ref as *mut &dyn Source as *mut c_void,
    };
    unsafe {
        let doc = (api.load_custom_document)(&mut access, std::ptr::null());
        if doc.is_null() {
            match (api.get_last_error)() {
                FPDF_ERR_PASSWORD => bail!("Password-protected PDF"),
                FPDF_ERR_SECURITY => bail!("PDF with unsupported encryption"),
                e => bail!("PDF damaged or invalid (code {e})"),
            }
        }
        let result = render_doc(api, doc, size);
        (api.close_document)(doc);
        result
    }
}

unsafe fn render_doc(api: &Api, doc: Doc, size: u32) -> Result<Image> {
    unsafe {
        if (api.get_page_count)(doc) < 1 {
            bail!("The PDF has no pages");
        }
        let page = (api.load_page)(doc, 0);
        if page.is_null() {
            bail!("Couldn't open the first page of the PDF");
        }
        let result = render_page(api, page, size);
        (api.close_page)(page);
        result
    }
}

unsafe fn render_page(api: &Api, page: Page, size: u32) -> Result<Image> {
    unsafe {
        let (pw, ph) = ((api.page_width)(page), (api.page_height)(page));
        if !(pw > 0.0 && ph > 0.0 && pw.is_finite() && ph.is_finite()) {
            bail!("PDF page has no valid size");
        }
        let scale = size as f32 / pw.max(ph);
        let w = ((pw * scale).round() as i32).clamp(1, size as i32);
        let h = ((ph * scale).round() as i32).clamp(1, size as i32);
        let bmp = (api.bitmap_create_ex)(w, h, BITMAP_BGRA, std::ptr::null_mut(), 0);
        if bmp.is_null() {
            bail!("Out of memory for the PDF thumbnail");
        }
        (api.bitmap_fill_rect)(bmp, 0, 0, w, h, 0x0000_0000);
        let mut deadline = Deadline {
            pause: Pause { version: 1, need_to_pause_now: need_to_pause, user: std::ptr::null_mut() },
            until: Instant::now() + RENDER_TIMEOUT,
        };
        let pause = &mut deadline as *mut Deadline as *mut Pause;
        let mut status = (api.render_start)(bmp, page, 0, 0, w, h, 0, FPDF_ANNOT, pause);
        while status == RENDER_TOBECONTINUED && Instant::now() < deadline.until {
            status = (api.render_continue)(page, pause);
        }
        (api.render_close)(page);
        let result = if status == RENDER_DONE {
            let stride = (api.bitmap_stride)(bmp) as usize;
            let buf = (api.bitmap_buffer)(bmp) as *const u8;
            let mut data = Vec::with_capacity(w as usize * h as usize * 4);
            for y in 0..h as usize {
                let row = std::slice::from_raw_parts(buf.add(y * stride), w as usize * 4);
                for p in row.as_chunks::<4>().0.iter() {
                    data.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
                }
            }
            Ok(Image::from_rgba(w as u32, h as u32, data))
        } else if status == RENDER_TOBECONTINUED {
            Err(Error(format!("The PDF page takes more than {} s to draw", RENDER_TIMEOUT.as_secs())))
        } else {
            Err(Error::new("PDFium couldn't draw the page"))
        };
        (api.bitmap_destroy)(bmp);
        result
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::source::MemorySource;

    /// Minimal PDF with one `w`×`h`-point page and a red 100×100 square at the bottom left.
    pub fn minimal_pdf(w: u32, h: u32, rotate: u32) -> Vec<u8> {
        let content = "1 0 0 rg 0 0 100 100 re f\n";
        let objects = [
            "<</Type/Catalog/Pages 2 0 R>>".to_string(),
            "<</Type/Pages/Kids[3 0 R]/Count 1>>".to_string(),
            format!("<</Type/Page/Parent 2 0 R/MediaBox[0 0 {w} {h}]/Rotate {rotate}/Contents 4 0 R>>"),
            format!("<</Length {}>>\nstream\n{content}endstream", content.len()),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        out
    }

    /// The PDFium tests need the DLL (tools/build-engine.ps1 sets GLANCY_PDFIUM).
    pub fn pdfium_available() -> bool {
        let ok = api().is_ok();
        if !ok {
            eprintln!("PDFium not available: skipping the test (set GLANCY_PDFIUM)");
        }
        ok
    }

    #[test]
    fn renders_first_page_with_transparent_background() {
        if !pdfium_available() {
            return;
        }
        let img = render_first_page(&MemorySource(minimal_pdf(200, 100, 0)), 100).unwrap();
        assert_eq!((img.width, img.height), (100, 50));
        assert_eq!(img.pixel(10, 45), [255, 0, 0, 255]);
        assert_eq!(img.pixel(90, 10)[3], 0);
    }

    #[test]
    fn page_rotation_is_applied() {
        if !pdfium_available() {
            return;
        }
        let img = render_first_page(&MemorySource(minimal_pdf(200, 100, 90)), 100).unwrap();
        assert_eq!((img.width, img.height), (50, 100));
    }

    #[test]
    fn corrupt_pdf_is_an_error() {
        if !pdfium_available() {
            return;
        }
        let err = render_first_page(&MemorySource(b"%PDF-1.4\nbasura".to_vec()), 64).unwrap_err();
        assert!(err.0.contains("PDF"), "{err}");
    }
}
