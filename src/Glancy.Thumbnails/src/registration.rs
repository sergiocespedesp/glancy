//! Engine registration for the current user (HKCU, no administrator rights).
//!
//! * `HKCU\Software\Classes\CLSID\{6C3A8B2E-…}\InprocServer32` → the DLL (`ThreadingModel = Apartment`), with
//!   `DisableProcessIsolation = 1`: the isolated thumbnail process does not see HKCU CLSIDs.
//! * `HKCU\Software\Classes\<class>\ShellEx\{e357fccd-…}` = Glancy's CLSID, for every extension and in the three
//!   classes Explorer consults: the ProgID the user chose (`UserChoice`), the default ProgID and the extension
//!   itself. Explorer gives the ProgID priority over the extension: with Glancy registered only on the extension,
//!   another thumbnail handler registered on the ProgID kept answering.
//! * Before touching anything, the previous state is saved under `HKCU\Software\Glancy\Engine\Extensions\<ext>`:
//!   the user value replaced in each class (`Locations\<n>`, to restore it on unregister) and the provider Explorer
//!   was using (`PreviousHandler`, for the "System provider" mode).
//! * The shadow (`adornment`) and the overlay icon (`showAssocIcon`, `customIcon`) are drawn by Explorer according
//!   to `Treatment` and `TypeOverlay` in `HKCU\Software\Classes\SystemFileAssociations\<ext>`. The engine syncs
//!   them when it registers and every time the app increments `Revision`.

use crate::error::{Error, Result};
use crate::reg::{self, Root};
use crate::settings::{self, RegValue, SettingsSource};
use std::sync::Mutex;
use windows::core::GUID;

pub const CLSID_GLANCY: GUID = GUID::from_u128(0x6C3A8B2E_5F1D_4E7A_9B0C_2D8E4F61A7C5);
pub const CLSID_STRING: &str = "{6C3A8B2E-5F1D-4E7A-9B0C-2D8E4F61A7C5}";
pub const THUMBNAIL_HANDLER: &str = "{e357fccd-a995-4576-b01f-234630154e96}";
pub const ENGINE_KEY: &str = r"Software\Glancy\Engine";
const DESCRIPTION: &str = "Glancy Thumbnail Provider";

fn clsid_key() -> String {
    format!(r"Software\Classes\CLSID\{CLSID_STRING}")
}

fn handler_key(class: &str) -> String {
    format!(r"Software\Classes\{class}\ShellEx\{THUMBNAIL_HANDLER}")
}

/// ProgID the user chose to open the extension (may not exist).
pub fn user_choice(ext: &str) -> Option<String> {
    reg::read_string(
        Root::CurrentUser,
        &format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\{ext}\UserChoice"),
        "ProgId",
    )
    .filter(|p| !p.trim().is_empty())
}

fn assoc_query(ext: &str, what: windows::Win32::UI::Shell::ASSOCSTR, extra: Option<&str>) -> Option<String> {
    use windows::Win32::UI::Shell::{ASSOCF_NONE, AssocQueryStringW};
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    let extra = extra.map(HSTRING::from);
    let hr = unsafe {
        AssocQueryStringW(
            ASSOCF_NONE,
            what,
            &HSTRING::from(ext),
            extra.as_ref().map(|e| PCWSTR(e.as_ptr())).unwrap_or(PCWSTR::null()),
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    let n = (len as usize).saturating_sub(1).min(buf.len());
    (hr.is_ok() && n > 0).then(|| String::from_utf16_lossy(&buf[..n]))
}

/// ProgID Windows actually uses for the extension (resolves Windows 11's `UserChoiceLatest`, application
/// associations and defaults). It may differ from `UserChoice`.
pub fn effective_progid(ext: &str) -> Option<String> {
    assoc_query(ext, windows::Win32::UI::Shell::ASSOCSTR_PROGID, None).filter(|p| !p.eq_ignore_ascii_case(ext))
}

/// Thumbnail provider that Explorer currently resolves for the extension.
pub fn resolved_handler(ext: &str) -> Option<GUID> {
    assoc_query(ext, windows::Win32::UI::Shell::ASSOCSTR_SHELLEXTENSION, Some(THUMBNAIL_HANDLER))
        .as_deref()
        .and_then(crate::delegate::parse_guid)
}

/// Classes shared by every file (or every unknown file): registering there would make Glancy the provider for any
/// type. Windows returns `Unknown` as the effective ProgID of unassociated extensions.
pub fn is_generic_class(class: &str) -> bool {
    let c = class.trim().to_ascii_lowercase();
    matches!(c.as_str(), "unknown" | "*" | "allfilesystemobjects" | "folder" | "directory" | "drive")
        || c.starts_with("systemfileassociations")
}

/// Classes under `Classes` where Explorer looks for the thumbnail provider, from highest to lowest priority:
/// the effective ProgID (the one Windows returns; on Windows 11 it may come from `UserChoiceLatest` or from an
/// application), the `UserChoice` one, the default one (the extension's default value) and the extension itself.
pub fn handler_classes(ext: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let default_progid = reg::read_string(Root::ClassesRoot, ext, "").filter(|p| !p.trim().is_empty());
    for c in [effective_progid(ext), user_choice(ext), default_progid, Some(ext.to_string())].into_iter().flatten() {
        if is_generic_class(&c) {
            continue;
        }
        if !v.iter().any(|x| x.eq_ignore_ascii_case(&c)) {
            v.push(c);
        }
    }
    v
}

/// Per-class backups: (`Locations\<n>`, class).
fn location_records(ext: &str) -> Vec<(String, String)> {
    let base = format!(r"{}\Locations", backup_key(ext));
    reg::subkeys(Root::CurrentUser, &base)
        .into_iter()
        .filter_map(|n| {
            let k = format!(r"{base}\{n}");
            reg::read_string(Root::CurrentUser, &k, "Key").map(|class| (k, class))
        })
        .collect()
}

fn location_record(ext: &str, class: &str) -> Option<String> {
    location_records(ext).into_iter().find(|(_, c)| c.eq_ignore_ascii_case(class)).map(|(k, _)| k)
}

/// Value that `HKCU\Software\Classes\<class>` held before Glancy (according to the backup).
pub fn saved_user_value(ext: &str, class: &str) -> Option<String> {
    location_record(ext, class).and_then(|k| reg::read_string(Root::CurrentUser, &k, "PreviousUserValue"))
}

fn sfa_key(ext: &str) -> String {
    format!(r"Software\Classes\SystemFileAssociations\{ext}")
}

fn backup_key(ext: &str) -> String {
    format!(r"{ENGINE_KEY}\Extensions\{ext}")
}

fn is_glancy(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim().eq_ignore_ascii_case(CLSID_STRING))
}

/// Full path of the DLL that contains this code.
pub fn module_path() -> Option<std::path::PathBuf> {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
        GetModuleHandleExW,
    };
    unsafe {
        let mut h = HMODULE::default();
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            windows::core::PCWSTR(module_path as *const u16),
            &mut h,
        )
        .ok()?;
        let mut buf = vec![0u16; 32_768];
        let n = GetModuleFileNameW(h, &mut buf) as usize;
        (n > 0).then(|| std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n])))
    }
}

pub fn notify_association_change() {
    use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

pub fn register(dll: &std::path::Path) -> Result<()> {
    let dll = dll.to_string_lossy();
    reg::write_string(Root::CurrentUser, &clsid_key(), "", DESCRIPTION)?;
    reg::write_string(Root::CurrentUser, &format!(r"{}\InprocServer32", clsid_key()), "", &dll)?;
    reg::write_string(Root::CurrentUser, &format!(r"{}\InprocServer32", clsid_key()), "ThreadingModel", "Apartment")?;
    // The isolated thumbnail process (dllhost /Processid:{AB8902B4-…}) does not see CLSIDs registered only in
    // HKCU: it returns REGDB_E_CLASSNOTREG (verified). Registered per user, the engine loads inside Explorer.
    reg::write_dword(Root::CurrentUser, &clsid_key(), "DisableProcessIsolation", 1)?;

    for ext in settings::all_extensions() {
        let backup = backup_key(ext);
        // The backup is only taken the first time: on reinstall, the current value is already Glancy.
        if reg::read_dword(Root::CurrentUser, &backup, "Saved").is_none() {
            let previous = crate::delegate::resolve_without_glancy(ext).map(|g| format!("{{{g:?}}}")).unwrap_or_default();
            reg::write_string(Root::CurrentUser, &backup, "PreviousHandler", &previous)?;
            match reg::read_dword(Root::CurrentUser, &sfa_key(ext), "Treatment") {
                Some(t) => reg::write_dword(Root::CurrentUser, &backup, "PreviousTreatment", t)?,
                None => reg::delete_value(Root::CurrentUser, &backup, "PreviousTreatment")?,
            }
            match reg::read_string(Root::CurrentUser, &sfa_key(ext), "TypeOverlay") {
                Some(t) => reg::write_string(Root::CurrentUser, &backup, "PreviousTypeOverlay", &t)?,
                None => reg::delete_value(Root::CurrentUser, &backup, "PreviousTypeOverlay")?,
            }
            reg::write_dword(Root::CurrentUser, &backup, "Saved", 1)?;
        }
        // Earlier versions may have registered on a generic class ("Unknown"): restore it and forget it.
        for (rec, class) in location_records(ext).into_iter().filter(|(_, c)| is_generic_class(c)) {
            if is_glancy(reg::read_string(Root::CurrentUser, &handler_key(&class), "").as_deref()) {
                match reg::read_string(Root::CurrentUser, &rec, "PreviousUserValue") {
                    Some(v) => reg::write_string(Root::CurrentUser, &handler_key(&class), "", &v)?,
                    None => reg::delete_tree(Root::CurrentUser, &handler_key(&class))?,
                }
            }
            reg::delete_tree(Root::CurrentUser, &rec)?;
        }
        for class in handler_classes(ext) {
            // Each class is backed up the first time Glancy writes to it.
            if location_record(ext, &class).is_none() {
                let rec = format!(r"{backup}\Locations\{}", location_records(ext).len());
                reg::write_string(Root::CurrentUser, &rec, "Key", &class)?;
                let current = reg::read_string(Root::CurrentUser, &handler_key(&class), "");
                // Earlier versions of this engine stored the extension's value at the root of the backup.
                let legacy = (class.eq_ignore_ascii_case(ext) && is_glancy(current.as_deref()))
                    .then(|| reg::read_string(Root::CurrentUser, &backup, "PreviousUserValue"))
                    .flatten();
                if let Some(v) = current.filter(|v| !is_glancy(Some(v))).or(legacy) {
                    reg::write_string(Root::CurrentUser, &rec, "PreviousUserValue", &v)?;
                }
            }
            reg::write_string(Root::CurrentUser, &handler_key(&class), "", CLSID_STRING)?;
        }
        reg::delete_value(Root::CurrentUser, &backup, "PreviousUserValue")?;
    }
    reg::write_string(Root::CurrentUser, ENGINE_KEY, "DllPath", &dll)?;
    // Check with the same function Explorer uses: extensions where another provider still wins.
    let others: Vec<&str> = settings::all_extensions().filter(|e| resolved_handler(e) != Some(CLSID_GLANCY)).collect();
    reg::write_string(Root::CurrentUser, ENGINE_KEY, "NotResolved", &others.join(" "))?;
    sync_shell_values(&settings::Snapshot::capture(&settings::RegistrySource::new(settings::SETTINGS_KEY)));
    notify_association_change();
    Ok(())
}

pub fn unregister() -> Result<()> {
    let mut errors = Vec::new();
    for ext in settings::all_extensions() {
        let backup = backup_key(ext);
        let mut classes = location_records(ext);
        if classes.is_empty() {
            classes.push((String::new(), ext.to_string()));
        }
        for (rec, class) in classes {
            let key = handler_key(&class);
            if !is_glancy(reg::read_string(Root::CurrentUser, &key, "").as_deref()) {
                continue;
            }
            let previous = if rec.is_empty() { None } else { reg::read_string(Root::CurrentUser, &rec, "PreviousUserValue") };
            let r = match previous {
                Some(v) => reg::write_string(Root::CurrentUser, &key, "", &v),
                None => reg::delete_tree(Root::CurrentUser, &key),
            };
            if let Err(e) = r {
                errors.push(format!("{class}: {e}"));
            }
            reg::delete_if_empty(Root::CurrentUser, &format!(r"Software\Classes\{class}\ShellEx"));
            reg::delete_if_empty(Root::CurrentUser, &format!(r"Software\Classes\{class}"));
        }
        if reg::read_dword(Root::CurrentUser, &backup, "Saved").is_some() {
            let sfa = sfa_key(ext);
            let _ = match reg::read_dword(Root::CurrentUser, &backup, "PreviousTreatment") {
                Some(t) => reg::write_dword(Root::CurrentUser, &sfa, "Treatment", t),
                None => reg::delete_value(Root::CurrentUser, &sfa, "Treatment"),
            };
            let _ = match reg::read_string(Root::CurrentUser, &backup, "PreviousTypeOverlay") {
                Some(t) => reg::write_string(Root::CurrentUser, &sfa, "TypeOverlay", &t),
                None => reg::delete_value(Root::CurrentUser, &sfa, "TypeOverlay"),
            };
            reg::delete_if_empty(Root::CurrentUser, &sfa);
        }
    }
    if !errors.is_empty() {
        // Keep the CLSID and the backups so the restore can be retried.
        notify_association_change();
        return Err(Error(errors.join("; ")));
    }
    reg::delete_tree(Root::CurrentUser, &clsid_key())?;
    reg::delete_tree(Root::CurrentUser, ENGINE_KEY)?;
    reg::delete_if_empty(Root::CurrentUser, r"Software\Glancy");
    notify_association_change();
    Ok(())
}

/// Desired shell values for an extension: (`Treatment`, `TypeOverlay`; `None` = restore the previous one).
pub fn desired_shell_values(src: &dyn SettingsSource, ext: &str) -> Option<(u32, Option<String>)> {
    let f = settings::resolve(src, ext)?.format_settings;
    let overlay = if !f.show_assoc_icon {
        Some(String::new()) // empty = no overlay icon
    } else if f.custom_icon && !f.custom_icon_path.trim().is_empty() {
        Some(f.custom_icon_path.trim().to_string())
    } else {
        None
    };
    Some((f.drop_shadow as u32, overlay))
}

/// Writes `Treatment`/`TypeOverlay` only if they change. Returns `true` if it wrote anything.
pub fn sync_shell_values(src: &dyn SettingsSource) -> bool {
    // Only if the engine is registered (there is a backup to restore later).
    if reg::read_string(Root::CurrentUser, ENGINE_KEY, "DllPath").is_none() {
        return false;
    }
    let mut changed = false;
    for ext in settings::all_extensions() {
        let Some((treatment, overlay)) = desired_shell_values(src, ext) else { continue };
        let sfa = sfa_key(ext);
        if reg::read_dword(Root::CurrentUser, &sfa, "Treatment") != Some(treatment) {
            changed |= reg::write_dword(Root::CurrentUser, &sfa, "Treatment", treatment).is_ok();
        }
        let current = reg::read_string(Root::CurrentUser, &sfa, "TypeOverlay");
        let wanted = overlay.or_else(|| reg::read_string(Root::CurrentUser, &backup_key(ext), "PreviousTypeOverlay"));
        if current != wanted {
            changed |= match &wanted {
                Some(v) => reg::write_string(Root::CurrentUser, &sfa, "TypeOverlay", v).is_ok(),
                None => reg::delete_value(Root::CurrentUser, &sfa, "TypeOverlay").is_ok(),
            };
        }
    }
    changed
}

static LAST_REVISION: Mutex<Option<Option<u32>>> = Mutex::new(None);

/// Called on every thumbnail: if the app applied changes (new `Revision`), syncs the shell values.
pub fn sync_if_revision_changed(snapshot: &settings::Snapshot) {
    let revision = match reg::read_hkcu(settings::SETTINGS_KEY, "Revision") {
        Some(RegValue::Dword(r)) => Some(r),
        _ => None,
    };
    {
        let mut last = LAST_REVISION.lock().unwrap_or_else(|e| e.into_inner());
        if *last == Some(revision) {
            return;
        }
        *last = Some(revision);
    }
    if sync_shell_values(snapshot) {
        crate::log::debug("Shell values (Treatment/TypeOverlay) updated");
        notify_association_change();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_values_from_settings() {
        let mut s = settings::Snapshot::default();
        assert_eq!(desired_shell_values(&s, ".psd"), Some((1, None)));
        s.insert(r"Formats\psd", "adornment", RegValue::Dword(0));
        s.insert(r"Formats\psd\Extensions\.psb", "showAssocIcon", RegValue::Dword(0));
        s.insert(r"Formats\pdf", "customIcon", RegValue::Dword(1));
        s.insert(r"Formats\pdf", "customIconPath", RegValue::Sz(r"C:\iconos\pdf.ico,0".into()));
        assert_eq!(desired_shell_values(&s, ".psd"), Some((0, None)));
        assert_eq!(desired_shell_values(&s, ".psb"), Some((0, Some(String::new()))));
        assert_eq!(desired_shell_values(&s, ".pdf"), Some((1, Some(r"C:\iconos\pdf.ico,0".into()))));
        assert_eq!(desired_shell_values(&s, ".exe"), None);
    }

    #[test]
    fn generic_classes_are_never_used() {
        for c in ["Unknown", "*", "AllFilesystemObjects", r"SystemFileAssociations\image", "Folder"] {
            assert!(is_generic_class(c), "{c}");
        }
        assert!(!is_generic_class("jpegfile"));
        assert!(!is_generic_class("Google.PhotoViewer.3.0"));
    }

    #[test]
    fn clsid_constants_agree() {
        assert_eq!(format!("{{{CLSID_GLANCY:?}}}"), CLSID_STRING);
        assert!(is_glancy(Some("{6c3a8b2e-5f1d-4e7a-9b0c-2d8e4f61a7c5}")));
        assert!(!is_glancy(None));
    }
}
