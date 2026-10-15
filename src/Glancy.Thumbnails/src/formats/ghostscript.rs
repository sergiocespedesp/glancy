//! Ghostscript, if installed, for EPS and old AI files (pure PostScript). Glancy does not bundle it (AGPL
//! license): it finds it through the registry. Instead of loading `gsdll64.dll` inside Explorer, it runs
//! `gswin64c.exe` as a separate process, so a hung Ghostscript cannot block Explorer:
//!
//! * with a timeout, inside a *Job* that kills it if the engine goes away and caps its memory;
//! * without inheriting any handle from Explorer;
//! * only if the version is 10.03.1 or later: merely opening a folder gets a downloaded EPS interpreted, and
//!   earlier versions have known `-dSAFER` escapes (CVE-2023-36664, CVE-2024-29510).

use crate::bail;
use crate::error::{Error, Result};
use crate::image::Image;
use crate::reg::{self, Root};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const TIMEOUT: Duration = Duration::from_secs(20);
/// Minimum accepted version.
pub const MIN_VERSION: [u32; 3] = [10, 3, 1];
/// Maximum memory of the Ghostscript process.
const MEMORY_LIMIT: usize = 1024 * 1024 * 1024;
/// Size cap for the file copied to a temporary folder when Explorer gives no path.
pub const MAX_COPY_BYTES: u64 = 512 * 1024 * 1024;

/// Version as a numeric tuple, to pick the most recent one ("10.05.1" → [10, 5, 1]).
fn version_key(v: &str) -> Vec<u32> {
    v.split(['.', '-']).map(|p| p.parse().unwrap_or(0)).collect()
}

/// Most recent `gswin64c.exe` with its version: from the Ghostscript registry keys (GPL or Artifex) and,
/// failing that, `C:\Program Files\gs\*`.
pub fn find() -> Option<(Vec<u32>, PathBuf)> {
    let mut found: Vec<(Vec<u32>, PathBuf)> = Vec::new();
    for vendor in ["GPL Ghostscript", "Artifex Ghostscript"] {
        let base = format!(r"SOFTWARE\{vendor}");
        for ver in reg::subkeys(Root::LocalMachine, &base) {
            if let Some(dll) = reg::read_string(Root::LocalMachine, &format!(r"{base}\{ver}"), "GS_DLL") {
                let exe = Path::new(&dll).with_file_name("gswin64c.exe");
                if exe.is_file() {
                    found.push((version_key(&ver), exe));
                }
            }
        }
    }
    if found.is_empty() {
        let root = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| r"C:\Program Files".into());
        if let Ok(dirs) = std::fs::read_dir(root.join("gs")) {
            for d in dirs.flatten() {
                let exe = d.path().join("bin").join("gswin64c.exe");
                let name = d.file_name().to_string_lossy().trim_start_matches("gs").to_string();
                if exe.is_file() {
                    found.push((version_key(&name), exe));
                }
            }
        }
    }
    found.sort();
    found.pop()
}

pub fn version_ok(version: &[u32]) -> bool {
    let mut v = version.to_vec();
    v.resize(3, 0);
    v[..3] >= MIN_VERSION[..]
}

/// Glancy's own temporary directory for exchanging files with Ghostscript.
fn temp_dir() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("Glancy");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn unique_name(ext: &str) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("gs-{}-{}-{}.{ext}", std::process::id(), N.fetch_add(1, Ordering::Relaxed), nanos)
}

struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Draws the first page of a PostScript/EPS file. `bbox` in points (width, height) to work out the resolution.
pub fn render(input: Option<&Path>, data: impl FnOnce() -> Result<Vec<u8>>, bbox: Option<(f64, f64)>, size: u32) -> Result<Image> {
    let Some((version, exe)) = find() else {
        bail!("Ghostscript is not installed (needed for EPS without a preview)");
    };
    if !version_ok(&version) {
        let v: Vec<String> = version.iter().map(u32::to_string).collect();
        bail!("Ghostscript {} is too old (insecure); 10.03.1 or later is required", v.join("."));
    }
    let dir = temp_dir()?;
    // If Explorer only gave a stream, it is dumped to a temporary file.
    let _tmp_in;
    let input = match input {
        Some(p) => p.to_path_buf(),
        None => {
            let p = dir.join(unique_name("eps"));
            std::fs::write(&p, data()?)?;
            _tmp_in = TempFile(p.clone());
            p
        }
    };
    let out = TempFile(dir.join(unique_name("png")));
    let (bw, bh) = bbox.filter(|(w, h)| *w > 0.0 && *h > 0.0).unwrap_or((612.0, 792.0));
    let dpi = (72.0 * size as f64 / bw.max(bh)).clamp(1.0, 4800.0);

    let args = [
        "-q",
        "-dSAFER",
        "-dBATCH",
        "-dNOPAUSE",
        "-dNOPROMPT",
        "-sDEVICE=pngalpha",
        "-dTextAlphaBits=4",
        "-dGraphicsAlphaBits=4",
        "-dEPSCrop",
        "-dFirstPage=1",
        "-dLastPage=1",
    ];
    // Windows file names can't contain quotes: wrapping them in quotes is enough.
    let cmdline = format!(
        "\"{}\" {} -r{dpi:.3} \"-sOutputFile={}\" -f \"{}\"",
        exe.display(),
        args.join(" "),
        out.0.display(),
        input.display()
    );
    let code = run_contained(&exe, &cmdline)?;
    if code != 0 {
        bail!("Ghostscript couldn't interpret the file (code {code})");
    }
    let png = std::fs::read(&out.0).map_err(|_| Error::new("Ghostscript produced no image"))?;
    Ok(crate::wic::decode(&png)?)
}

/// Runs a program without a window, without inheriting handles, inside a memory-limited Job that kills it when
/// closed (also if the engine process ends). Returns the exit code.
fn run_contained(exe: &Path, cmdline: &str) -> Result<u32> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };
    use windows::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW, GetExitCodeProcess, PROCESS_INFORMATION, ResumeThread,
        STARTUPINFOW, WaitForSingleObject,
    };
    use windows::core::{HSTRING, PCWSTR, PWSTR};

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }

    unsafe {
        let job = Handle(CreateJobObjectW(None, PCWSTR::null())?);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        limits.ProcessMemoryLimit = MEMORY_LIMIT;
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )?;

        let mut line: Vec<u16> = cmdline.encode_utf16().chain(std::iter::once(0)).collect();
        let startup = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut pi = PROCESS_INFORMATION::default();
        // Suspended until it is inside the Job, so it never runs unrestricted, not even for an instant.
        CreateProcessW(
            &HSTRING::from(exe.as_os_str()),
            PWSTR(line.as_mut_ptr()),
            None,
            None,
            false,
            CREATE_NO_WINDOW | CREATE_SUSPENDED,
            None,
            PCWSTR::null(),
            &startup,
            &mut pi,
        )
        .map_err(|e| Error(format!("Couldn't run Ghostscript: {}", e.message())))?;
        let process = Handle(pi.hProcess);
        let thread = Handle(pi.hThread);
        if let Err(e) = AssignProcessToJobObject(job.0, process.0) {
            let _ = TerminateJobObject(job.0, 1);
            let _ = windows::Win32::System::Threading::TerminateProcess(process.0, 1);
            return Err(e.into());
        }
        ResumeThread(thread.0);
        if WaitForSingleObject(process.0, TIMEOUT.as_millis() as u32) != WAIT_OBJECT_0 {
            let _ = TerminateJobObject(job.0, 1);
            bail!("Ghostscript took more than {} s", TIMEOUT.as_secs());
        }
        let mut code = 0u32;
        GetExitCodeProcess(process.0, &mut code)?;
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimum_version() {
        assert!(version_ok(&[10, 3, 1]));
        assert!(version_ok(&[10, 5]));
        assert!(!version_ok(&[10, 3, 0]));
        assert!(!version_ok(&[9, 56, 1]));
        assert_eq!(version_key("10.05.1"), vec![10, 5, 1]);
    }

    #[test]
    fn contained_process_reports_exit_code() {
        let cmd = std::path::PathBuf::from(std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into()));
        assert_eq!(run_contained(&cmd, r#""cmd.exe" /c exit 3"#).unwrap(), 3);
    }
}
