//! History of processed files (`history.log`, always on) and diagnostic log (`engine.log`, only with
//! `debugLog = 1`), both in `%LOCALAPPDATA%\Glancy\Logs`. History format (contract with the app):
//! `2026-10-14T01:00:05Z<TAB>ok|error<TAB>milliseconds<TAB>full path[<TAB>error detail]`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Past this size the file is rotated to `<file>.1`. The app only reads the last 64 KB.
pub const ROTATE_BYTES: u64 = 1024 * 1024;

static DEBUG: AtomicBool = AtomicBool::new(false);

pub fn set_debug(enabled: bool) {
    DEBUG.store(enabled, Ordering::Relaxed);
}

pub fn logs_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).or_else(known_local_app_data)?;
    Some(base.join("Glancy").join("Logs"))
}

fn known_local_app_data() -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// UTC date in ISO 8601 with seconds (`2026-10-14T01:00:05Z`).
pub fn utc_timestamp(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Fields cannot contain tabs or line breaks (they would break the format).
fn clean(s: &str) -> String {
    s.chars().map(|c| if c == '\t' || c == '\r' || c == '\n' { ' ' } else { c }).collect()
}

pub fn history_line(time: SystemTime, ok: bool, ms: u128, path: &str, detail: Option<&str>) -> String {
    let mut line = format!("{}\t{}\t{}\t{}", utc_timestamp(time), if ok { "ok" } else { "error" }, ms, clean(path));
    if let Some(d) = detail.filter(|d| !d.is_empty()) {
        line.push('\t');
        line.push_str(&clean(d));
    }
    line.push('\n');
    line
}

/// Appends a line in a single write (several Explorer instances may write at once) and rotates.
pub fn append_rotating(file: &Path, line: &str) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    if fs::metadata(file).map(|m| m.len() > ROTATE_BYTES).unwrap_or(false) {
        let mut rotated = file.as_os_str().to_owned();
        rotated.push(".1");
        // If another process rotates at the same time, one of the two fails: harmless.
        let _ = fs::rename(file, PathBuf::from(rotated));
    }
    // std opens with FILE_SHARE_READ | WRITE | DELETE, so the app can read and another process can rotate.
    let mut f = OpenOptions::new().create(true).append(true).open(file)?;
    f.write_all(line.as_bytes())
}

/// Logs a thumbnail in the history. Never fails outwards: the history must not prevent the thumbnail.
pub fn history(ok: bool, ms: u128, path: &str, detail: Option<&str>) {
    let Some(dir) = logs_dir() else { return };
    let line = history_line(SystemTime::now(), ok, ms, path, detail);
    if let Err(e) = append_rotating(&dir.join("history.log"), &line) {
        debug(&format!("Couldn't write history.log: {e}"));
    }
}

/// Diagnostic log; does nothing if `debugLog` is off.
pub fn debug(message: &str) {
    if !DEBUG.load(Ordering::Relaxed) {
        return;
    }
    let Some(dir) = logs_dir() else { return };
    let line = format!(
        "{}\t{}\t{:?}\t{}\n",
        utc_timestamp(SystemTime::now()),
        std::process::id(),
        std::thread::current().id(),
        clean(message)
    );
    let _ = append_rotating(&dir.join("engine.log"), &line);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamp_format() {
        let t = UNIX_EPOCH + Duration::from_secs(1_791_939_605); // 2026-10-14T01:00:05Z
        assert_eq!(utc_timestamp(t), "2026-10-14T01:00:05Z");
        assert_eq!(utc_timestamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400); // 2000-02-29
        assert_eq!(utc_timestamp(leap), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn line_format_matches_contract() {
        let t = UNIX_EPOCH + Duration::from_secs(1_791_939_605);
        assert_eq!(history_line(t, true, 42, r"C:\a\b.psd", None), "2026-10-14T01:00:05Z\tok\t42\tC:\\a\\b.psd\n");
        assert_eq!(
            history_line(t, false, 7, "C:\\x\ty.pdf", Some("Password-protected\tPDF\nfile")),
            "2026-10-14T01:00:05Z\terror\t7\tC:\\x y.pdf\tPassword-protected PDF file\n"
        );
    }

    #[test]
    fn rotation_happens_over_limit() {
        let dir = std::env::temp_dir().join(format!("glancy-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let file = dir.join("history.log");
        let big = "x".repeat(ROTATE_BYTES as usize + 10);
        append_rotating(&file, &big).unwrap();
        append_rotating(&file, "nueva\n").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "nueva\n");
        assert!(dir.join("history.log.1").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
