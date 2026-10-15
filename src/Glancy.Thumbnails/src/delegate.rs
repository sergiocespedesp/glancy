//! "System provider" mode (`mode = 2`): the thumbnail is requested from the provider Explorer used before Glancy.
//! If there is none, or it fails, Glancy generates the thumbnail itself.

use crate::registration::{CLSID_GLANCY, ENGINE_KEY, THUMBNAIL_HANDLER};
use crate::reg::{self, Root};
use std::path::Path;
use windows::Win32::Graphics::Gdi::HBITMAP;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IStream, STREAM_SEEK_SET};
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithStream};
use windows::Win32::UI::Shell::{IInitializeWithItem, IShellItem, IThumbnailProvider, SHCreateItemFromParsingName, WTS_ALPHATYPE};
use windows::core::{GUID, HSTRING, Interface};

/// `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` → GUID. (In windows-core 0.58, `GUID::try_from(&str)` calls `panic!`
/// on invalid text, and the registry may contain anything.)
pub fn parse_guid(s: &str) -> Option<GUID> {
    let t = s.trim().trim_start_matches('{').trim_end_matches('}');
    let b = t.as_bytes();
    if b.len() != 36 || [8, 13, 18, 23].iter().any(|i| b[*i] != b'-') {
        return None;
    }
    let hex: String = t.chars().filter(|c| *c != '-').collect();
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(GUID::from_u128(u128::from_str_radix(&hex, 16).ok()?))
}

fn parse_clsid(s: &str) -> Option<GUID> {
    let g = parse_guid(s)?;
    (g != CLSID_GLANCY && g != GUID::zeroed()).then_some(g)
}

/// An extension's previous provider: the one saved when Glancy was registered or, failing that, the one Explorer
/// would resolve without Glancy.
pub fn previous_handler(ext: &str) -> Option<GUID> {
    let saved = reg::read_string(Root::CurrentUser, &format!(r"{ENGINE_KEY}\Extensions\{ext}"), "PreviousHandler");
    if let Some(g) = saved.as_deref().and_then(parse_clsid) {
        return Some(g);
    }
    resolve_without_glancy(ext)
}

/// The provider of a `Classes` class, not counting Glancy: the per-user value (or, if that is Glancy, the one there
/// before according to the backup) and, if there is none, the per-machine one.
fn handler_of_class(ext: &str, class: &str) -> Option<GUID> {
    let key = format!(r"Software\Classes\{class}\ShellEx\{THUMBNAIL_HANDLER}");
    let user = match reg::read_string(Root::CurrentUser, &key, "") {
        Some(v) if parse_guid(&v) == Some(CLSID_GLANCY) => crate::registration::saved_user_value(ext, class),
        other => other,
    };
    user.as_deref()
        .and_then(parse_clsid)
        .or_else(|| reg::read_string(Root::LocalMachine, &key, "").as_deref().and_then(parse_clsid))
}

/// The provider Explorer would use without Glancy, in its lookup order: the `UserChoice` ProgID, the default
/// ProgID, the extension, and `SystemFileAssociations` for the extension and for the perceived type.
pub fn resolve_without_glancy(ext: &str) -> Option<GUID> {
    for class in crate::registration::handler_classes(ext) {
        if let Some(g) = handler_of_class(ext, &class) {
            return Some(g);
        }
    }
    if let Some(g) = handler_of_class(ext, &format!(r"SystemFileAssociations\{ext}")) {
        return Some(g);
    }
    let perceived = reg::read_string(Root::ClassesRoot, ext, "PerceivedType")?;
    handler_of_class(ext, &format!(r"SystemFileAssociations\{}", perceived.to_lowercase()))
}

/// The data Glancy was initialized with, to hand over to the other provider.
pub enum Input<'a> {
    Path(&'a Path),
    Stream(&'a IStream),
}

thread_local! {
    /// Prevents two providers that delegate to each other (e.g. another one with its own "fallback" pointing at
    /// Glancy) from calling each other endlessly until Explorer's stack overflows.
    static DELEGATING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct DelegationGuard;

impl Drop for DelegationGuard {
    fn drop(&mut self) {
        DELEGATING.with(|d| d.set(false));
    }
}

/// `true` if this thread is already inside a delegation (the thumbnail is being requested from Glancy by another
/// provider).
pub fn is_delegating() -> bool {
    DELEGATING.with(|d| d.get())
}

/// Requests the thumbnail from `clsid`. Returns the other provider's HBITMAP as is.
pub fn thumbnail(clsid: GUID, input: Input, cx: u32) -> windows::core::Result<(HBITMAP, WTS_ALPHATYPE)> {
    if is_delegating() {
        return Err(windows::Win32::Foundation::E_FAIL.into());
    }
    DELEGATING.with(|d| d.set(true));
    let _guard = DelegationGuard;
    unsafe {
        let provider: IThumbnailProvider = CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER)?;
        let mut initialized = false;
        match input {
            Input::Path(path) => {
                let wide = HSTRING::from(path.as_os_str());
                if let Ok(init) = provider.cast::<IInitializeWithItem>()
                    && let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&wide, None)
                {
                    initialized = init.Initialize(&item, 0).is_ok();
                }
                if !initialized && let Ok(init) = provider.cast::<IInitializeWithFile>() {
                    initialized = init.Initialize(&wide, 0).is_ok();
                }
                if !initialized && let Ok(init) = provider.cast::<IInitializeWithStream>() {
                    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
                    use windows::Win32::System::Com::{STGM_READ, STGM_SHARE_DENY_NONE};
                    use windows::Win32::UI::Shell::SHCreateStreamOnFileEx;
                    let stream =
                        SHCreateStreamOnFileEx(&wide, (STGM_READ | STGM_SHARE_DENY_NONE).0, FILE_ATTRIBUTE_NORMAL.0, false, None)?;
                    initialized = init.Initialize(&stream, 0).is_ok();
                }
            }
            Input::Stream(stream) => {
                if let Ok(init) = provider.cast::<IInitializeWithStream>() {
                    stream.Seek(0, STREAM_SEEK_SET, None)?;
                    initialized = init.Initialize(stream, 0).is_ok();
                }
            }
        }
        if !initialized {
            return Err(windows::Win32::Foundation::E_NOINTERFACE.into());
        }
        let mut hbmp = HBITMAP::default();
        let mut alpha = WTS_ALPHATYPE::default();
        provider.GetThumbnail(cx, &mut hbmp, &mut alpha)?;
        if hbmp.is_invalid() {
            return Err(windows::Win32::Foundation::E_FAIL.into());
        }
        Ok((hbmp, alpha))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clsid_parsing_skips_glancy() {
        assert!(parse_clsid("{6C3A8B2E-5F1D-4E7A-9B0C-2D8E4F61A7C5}").is_none());
        assert!(parse_clsid("{00000000-0000-0000-0000-000000000000}").is_none());
        assert_eq!(
            parse_clsid("{68C300AC-2314-4992-99E3-957A9F6C6D79}"),
            Some(GUID::from_u128(0x68C300AC_2314_4992_99E3_957A9F6C6D79))
        );
        assert!(parse_clsid("basura").is_none());
    }
}
