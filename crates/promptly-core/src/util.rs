use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::Command;

/// Write a file readable only by the current user, atomically.
pub fn write_private(path: &Path, body: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(body)?;
    }
    std::fs::rename(tmp, path)
}

/// Run a command and return trimmed stdout if it exits successfully.
pub fn run_stdout(cmd: &mut Command) -> Option<String> {
    let out = cmd.output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Current working directory of a process (the session's live folder).
pub fn process_cwd(pid: u32) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
        // SAFETY: `info` is a properly sized, writable buffer for this flavor.
        let n = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            )
        };
        if n != size {
            return None;
        }
        // vip_path is a MAXPATHLEN byte buffer, split into chunks by libc.
        let raw: Vec<u8> = info
            .pvi_cdir
            .vip_path
            .iter()
            .flatten()
            .map(|c| *c as u8)
            .collect();
        let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
        let s = String::from_utf8_lossy(&raw[..end]).to_string();
        (!s.is_empty()).then(|| std::path::PathBuf::from(s))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

pub fn new_uuid() -> String {
    let b: [u8; 16] = rand::random();
    let mut b = b;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

pub fn random_token() -> String {
    let b: [u8; 32] = rand::random();
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Constant-time comparison for socket tokens.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_process_cwd_matches() {
        let want = std::env::current_dir().unwrap().canonicalize().unwrap();
        let got = process_cwd(std::process::id())
            .unwrap()
            .canonicalize()
            .unwrap();
        assert_eq!(got, want);
    }

    #[test]
    fn uuid_is_v4_shaped() {
        let u = new_uuid();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
    }
}
