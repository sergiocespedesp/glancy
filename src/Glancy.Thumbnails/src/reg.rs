//! Minimal registry access (reads DWORD, SZ, EXPAND_SZ and MULTI_SZ; writes DWORD and SZ).

use crate::settings::RegValue;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_DWORD, REG_EXPAND_SZ,
    REG_MULTI_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
    RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::core::{HSTRING, PCWSTR};

#[derive(Clone, Copy, Debug)]
pub enum Root {
    CurrentUser,
    LocalMachine,
    /// Merged HKCU+HKLM view, the same one Explorer uses.
    ClassesRoot,
}

impl Root {
    fn hkey(self) -> HKEY {
        match self {
            Root::CurrentUser => HKEY_CURRENT_USER,
            Root::LocalMachine => HKEY_LOCAL_MACHINE,
            Root::ClassesRoot => HKEY_CLASSES_ROOT,
        }
    }
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn open(root: Root, path: &str, write: bool) -> Option<Key> {
    let mut h = HKEY::default();
    let access = if write { KEY_READ | KEY_WRITE } else { KEY_READ };
    let r = unsafe { RegOpenKeyExW(root.hkey(), &HSTRING::from(path), 0, access, &mut h) };
    (r == ERROR_SUCCESS).then_some(Key(h))
}

fn create(root: Root, path: &str) -> windows::core::Result<Key> {
    let mut h = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            root.hkey(),
            &HSTRING::from(path),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            None,
            &mut h,
            None,
        )
        .ok()?;
    }
    Ok(Key(h))
}

pub fn key_exists(root: Root, path: &str) -> bool {
    open(root, path, false).is_some()
}

/// Reads a value; an empty `name` means the key's default value.
pub fn read(root: Root, path: &str, name: &str) -> Option<RegValue> {
    let key = open(root, path, false)?;
    read_from(&key, name)
}

/// Every named value of a key, opening it only once (empty if the key does not exist).
pub fn read_values(root: Root, path: &str) -> Vec<(String, RegValue)> {
    use windows::Win32::System::Registry::RegEnumValueW;
    let Some(key) = open(root, path, false) else { return Vec::new() };
    let mut out = Vec::new();
    for i in 0..4096u32 {
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        let r = unsafe { RegEnumValueW(key.0, i, windows::core::PWSTR(buf.as_mut_ptr()), &mut len, None, None, None, None) };
        if r != ERROR_SUCCESS {
            break;
        }
        let name = String::from_utf16_lossy(&buf[..len as usize]);
        if let Some(v) = read_from(&key, &name) {
            out.push((name, v));
        }
    }
    out
}

fn read_from(key: &Key, name: &str) -> Option<RegValue> {
    let name = HSTRING::from(name);
    let mut ty = REG_VALUE_TYPE::default();
    let mut size = 0u32;
    unsafe {
        if RegQueryValueExW(key.0, &name, None, Some(&mut ty), None, Some(&mut size)) != ERROR_SUCCESS {
            return None;
        }
    }
    // Headroom for missing terminators.
    let mut buf = vec![0u8; size as usize + 4];
    let mut size2 = size;
    unsafe {
        if RegQueryValueExW(key.0, &name, None, Some(&mut ty), Some(buf.as_mut_ptr()), Some(&mut size2))
            != ERROR_SUCCESS
        {
            return None;
        }
    }
    buf.truncate(size2 as usize);
    match ty {
        REG_DWORD if buf.len() >= 4 => Some(RegValue::Dword(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))),
        REG_SZ | REG_EXPAND_SZ => Some(RegValue::Sz(utf16_to_string(&buf))),
        REG_MULTI_SZ => {
            let wide: Vec<u16> = buf.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            let list = wide
                .split(|c| *c == 0)
                .filter(|s| !s.is_empty())
                .map(String::from_utf16_lossy)
                .collect();
            Some(RegValue::MultiSz(list))
        }
        _ => None,
    }
}

fn utf16_to_string(bytes: &[u8]) -> String {
    let wide: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    let end = wide.iter().position(|c| *c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

pub fn read_hkcu(path: &str, name: &str) -> Option<RegValue> {
    read(Root::CurrentUser, path, name)
}

pub fn read_string(root: Root, path: &str, name: &str) -> Option<String> {
    match read(root, path, name)? {
        RegValue::Sz(s) => Some(s),
        _ => None,
    }
}

pub fn read_dword(root: Root, path: &str, name: &str) -> Option<u32> {
    match read(root, path, name)? {
        RegValue::Dword(d) => Some(d),
        _ => None,
    }
}

pub fn write_string(root: Root, path: &str, name: &str, value: &str) -> windows::core::Result<()> {
    let key = create(root, path)?;
    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
    unsafe { RegSetValueExW(key.0, &HSTRING::from(name), 0, REG_SZ, Some(bytes)).ok() }
}

pub fn write_dword(root: Root, path: &str, name: &str, value: u32) -> windows::core::Result<()> {
    let key = create(root, path)?;
    unsafe { RegSetValueExW(key.0, &HSTRING::from(name), 0, REG_DWORD, Some(&value.to_le_bytes())).ok() }
}

/// Deletes a value; not an error if it did not exist.
pub fn delete_value(root: Root, path: &str, name: &str) -> windows::core::Result<()> {
    let Some(key) = open(root, path, true) else { return Ok(()) };
    let r = unsafe { RegDeleteValueW(key.0, &HSTRING::from(name)) };
    if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND { Ok(()) } else { r.ok() }
}

/// Deletes a key and everything in it; not an error if it did not exist.
pub fn delete_tree(root: Root, path: &str) -> windows::core::Result<()> {
    let r = unsafe { RegDeleteTreeW(root.hkey(), &HSTRING::from(path)) };
    if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND { Ok(()) } else { r.ok() }
}

/// Names of the direct subkeys (empty if the key does not exist).
pub fn subkeys(root: Root, path: &str) -> Vec<String> {
    use windows::Win32::System::Registry::RegEnumKeyExW;
    let Some(key) = open(root, path, false) else { return Vec::new() };
    let mut names = Vec::new();
    for i in 0.. {
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        let r = unsafe {
            RegEnumKeyExW(key.0, i, windows::core::PWSTR(buf.as_mut_ptr()), &mut len, None, windows::core::PWSTR::null(), None, None)
        };
        if r != ERROR_SUCCESS {
            break;
        }
        names.push(String::from_utf16_lossy(&buf[..len as usize]));
    }
    names
}

/// Deletes the key if it is empty (no subkeys or values). Useful to leave no trace when unregistering.
pub fn delete_if_empty(root: Root, path: &str) {
    use windows::Win32::System::Registry::{RegDeleteKeyW, RegQueryInfoKeyW};
    let Some(key) = open(root, path, false) else { return };
    let (mut subkeys, mut values) = (0u32, 0u32);
    let ok = unsafe {
        RegQueryInfoKeyW(
            key.0,
            windows::core::PWSTR::null(),
            None,
            None,
            Some(&mut subkeys),
            None,
            None,
            Some(&mut values),
            None,
            None,
            None,
            None,
        )
    } == ERROR_SUCCESS;
    drop(key);
    if ok && subkeys == 0 && values == 0 {
        unsafe {
            let _ = RegDeleteKeyW(root.hkey(), &HSTRING::from(path));
        }
    }
}
