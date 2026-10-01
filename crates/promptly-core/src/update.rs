//! Self-update from GitHub Releases.
//!
//! Flow: query the latest release, pick the asset for this platform,
//! download it and the release's `SHA256SUMS`, verify the checksum, unpack,
//! then swap the installed app/binaries atomically-ish (old copy kept until
//! the new one is in place). Nothing is installed without a matching checksum.
//! Network access is the system `curl` over HTTPS only.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const REPO: &str = "martinykiriloff/promptly";
const CHECKSUMS: &str = "SHA256SUMS";

#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub tag: String,
    pub name: String,
    pub notes: String,
    pub html_url: String,
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn version(&self) -> &str {
        self.tag.trim_start_matches('v')
    }

    /// The asset that installs on this machine, if the release has one.
    pub fn platform_asset(&self) -> Option<&Asset> {
        let want = platform_suffix()?;
        self.assets.iter().find(|a| a.name.ends_with(want))
    }

    pub fn checksums_asset(&self) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == CHECKSUMS)
    }
}

fn platform_suffix() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("-macos-arm64.zip"),
        ("linux", "x86_64") => Some("-linux-x86_64.tar.gz"),
        ("linux", "aarch64") => Some("-linux-arm64.tar.gz"),
        _ => None,
    }
}

/// Compare dotted numeric versions ("2026.1" < "2026.1.1" < "2026.2").
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split(['.', '-', '+'])
            .map_while(|p| p.parse().ok())
            .collect()
    };
    let (a, b) = (parse(candidate), parse(current));
    let n = a.len().max(b.len());
    for i in 0..n {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

fn curl() -> Command {
    let mut c = Command::new("curl");
    c.args([
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--proto",
        "=https",
        "--tlsv1.2",
    ])
    .args(["--connect-timeout", "15", "--retry", "2"])
    .args([
        "-H",
        concat!("User-Agent: Promptly/", env!("CARGO_PKG_VERSION")),
    ]);
    c
}

fn run(mut c: Command) -> Result<Vec<u8>, String> {
    let out = c.output().map_err(|e| format!("could not run curl: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub fn parse_release(v: &Value) -> Option<Release> {
    Some(Release {
        tag: v["tag_name"].as_str()?.to_string(),
        name: v["name"].as_str().unwrap_or_default().to_string(),
        notes: v["body"].as_str().unwrap_or_default().to_string(),
        html_url: v["html_url"].as_str().unwrap_or_default().to_string(),
        assets: v["assets"]
            .as_array()?
            .iter()
            .filter_map(|a| {
                Some(Asset {
                    name: a["name"].as_str()?.to_string(),
                    url: a["browser_download_url"].as_str()?.to_string(),
                    size: a["size"].as_u64().unwrap_or(0),
                })
            })
            .collect(),
    })
}

/// Latest published release, or `None` when it is not newer than `current`.
pub fn check(current: &str) -> Result<Option<Release>, String> {
    let mut c = curl();
    c.args(["-H", "Accept: application/vnd.github+json"])
        .arg(format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ));
    let body = run(c)?;
    let v: Value = serde_json::from_slice(&body).map_err(|e| format!("bad release JSON: {e}"))?;
    let r = parse_release(&v).ok_or("release has no tag")?;
    Ok(is_newer(r.version(), current).then_some(r))
}

pub fn sha256_file(p: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Expected hash for `name` from a `sha256sum`-style file.
pub fn expected_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut parts = l.split_whitespace();
        let hash = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        (file == name && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

/// Where the running app lives, and how to replace it.
#[derive(Debug, Clone, PartialEq)]
pub enum InstallTarget {
    /// `/Applications/Promptly.app` (macOS bundle).
    MacBundle(PathBuf),
    /// Directory holding `promptly` and `promptly-ctl` (Linux tarball / dev).
    BinDir(PathBuf),
}

pub fn install_target() -> Result<InstallTarget, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let dir = exe.parent().ok_or("no exe dir")?.to_path_buf();
    if cfg!(target_os = "macos") {
        // .../Promptly.app/Contents/MacOS/promptly
        let bundle = dir
            .parent()
            .and_then(Path::parent)
            .filter(|p| p.extension().is_some_and(|e| e == "app"));
        return match bundle {
            Some(b) => Ok(InstallTarget::MacBundle(b.to_path_buf())),
            None => Err("Updates install into Promptly.app; this is a development build.".into()),
        };
    }
    if dir.starts_with("/usr") {
        return Err(
            "Installed by the package manager; update with the .deb from the release page.".into(),
        );
    }
    Ok(InstallTarget::BinDir(dir))
}

/// Download, verify and stage `release` for this platform. Returns the
/// staged path (an unpacked `.app` or a directory with the binaries).
pub fn download_and_verify(
    release: &Release,
    work: &Path,
    progress: &dyn Fn(&str),
) -> Result<PathBuf, String> {
    let asset = release
        .platform_asset()
        .ok_or("this release has no build for your platform")?;
    let sums = release
        .checksums_asset()
        .ok_or("this release has no SHA256SUMS; refusing to install an unverified build")?;
    let _ = std::fs::remove_dir_all(work);
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;

    progress("Downloading checksums…");
    let mut c = curl();
    c.arg(&sums.url);
    let sums_text = String::from_utf8(run(c)?).map_err(|_| "SHA256SUMS is not text")?;
    let want =
        expected_hash(&sums_text, &asset.name).ok_or("SHA256SUMS has no entry for this build")?;

    progress(&format!(
        "Downloading {} ({:.1} MB)…",
        asset.name,
        asset.size as f64 / 1e6
    ));
    let archive = work.join(&asset.name);
    let mut c = curl();
    c.arg("-o").arg(&archive).arg(&asset.url);
    run(c)?;

    progress("Verifying checksum…");
    let got = sha256_file(&archive).map_err(|e| e.to_string())?;
    if got != want {
        return Err(format!(
            "checksum mismatch for {} (expected {want}, got {got})",
            asset.name
        ));
    }

    progress("Unpacking…");
    let out = work.join("unpacked");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let status = if asset.name.ends_with(".zip") {
        Command::new("ditto")
            .arg("-x")
            .arg("-k")
            .arg(&archive)
            .arg(&out)
            .status()
    } else {
        Command::new("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&out)
            .status()
    }
    .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("could not unpack the update".into());
    }
    if asset.name.ends_with(".zip") {
        let app = out.join("Promptly.app");
        if !app.join("Contents/MacOS/promptly").exists() {
            return Err("update archive does not contain Promptly.app".into());
        }
        Ok(app)
    } else {
        if !out.join("promptly").exists() || !out.join("promptly-ctl").exists() {
            return Err("update archive does not contain the binaries".into());
        }
        Ok(out)
    }
}

/// Swap the staged build into place. The previous version is moved aside
/// first and restored if the swap fails.
pub fn install(staged: &Path, target: &InstallTarget) -> Result<PathBuf, String> {
    match target {
        InstallTarget::MacBundle(app) => {
            let backup = app.with_extension("app.old");
            let _ = std::fs::remove_dir_all(&backup);
            std::fs::rename(app, &backup)
                .map_err(|e| format!("cannot replace {}: {e}", app.display()))?;
            let moved = std::fs::rename(staged, app).is_ok()
                || Command::new("ditto")
                    .arg(staged)
                    .arg(app)
                    .status()
                    .is_ok_and(|s| s.success());
            if !moved {
                let _ = std::fs::rename(&backup, app);
                return Err("could not install the new version; the old one was restored".into());
            }
            let _ = std::fs::remove_dir_all(&backup);
            // Downloads through curl carry no quarantine flag, but be explicit.
            let _ = Command::new("xattr")
                .args(["-dr", "com.apple.quarantine"])
                .arg(app)
                .status();
            Ok(app.join("Contents/MacOS/promptly"))
        }
        InstallTarget::BinDir(dir) => {
            for name in ["promptly-ctl", "promptly"] {
                let src = staged.join(name);
                let dst = dir.join(name);
                let tmp = dir.join(format!(".{name}.new"));
                std::fs::copy(&src, &tmp)
                    .map_err(|e| format!("cannot write {}: {e}", dir.display()))?;
                // rename over a running executable is safe on Unix.
                std::fs::rename(&tmp, &dst).map_err(|e| e.to_string())?;
            }
            Ok(dir.join("promptly"))
        }
    }
}

/// Start the freshly installed app after this process exits.
pub fn relaunch(target: &InstallTarget, exe: &Path) {
    let cmd = match target {
        InstallTarget::MacBundle(app) => format!(
            "sleep 1; open -n {}",
            crate::hooks::shell_quote(&app.to_string_lossy())
        ),
        InstallTarget::BinDir(_) => format!(
            "sleep 1; {} >/dev/null 2>&1 &",
            crate::hooks::shell_quote(&exe.to_string_lossy())
        ),
    };
    let _ = Command::new("/bin/sh").arg("-c").arg(cmd).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        assert!(is_newer("2026.2", "2026.1.0"));
        assert!(is_newer("v2026.1.1", "2026.1.0"));
        assert!(!is_newer("2026.1", "2026.1.0"));
        assert!(!is_newer("2026.1.0", "2026.2"));
        assert!(is_newer("2027.0", "2026.12.3"));
        assert!(!is_newer("garbage", "2026.1.0"));
    }

    #[test]
    fn checksum_lookup() {
        let sums = "aa  other.zip\n\
            0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  Promptly-v2026.2-macos-arm64.zip\n";
        assert_eq!(
            expected_hash(sums, "Promptly-v2026.2-macos-arm64.zip")
                .unwrap()
                .len(),
            64
        );
        assert!(
            expected_hash(sums, "other.zip").is_none(),
            "short hash rejected"
        );
        assert!(expected_hash(sums, "missing").is_none());
    }

    #[test]
    fn sha256_known_value() {
        let t = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(t.path(), b"abc").unwrap();
        assert_eq!(
            sha256_file(t.path()).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn release_parsing_and_platform_asset() {
        let v = serde_json::json!({
            "tag_name": "v2026.2", "name": "Promptly v2026.2", "body": "notes",
            "html_url": "https://github.com/x",
            "assets": [
                {"name": "Promptly-v2026.2-macos-arm64.zip", "browser_download_url": "https://a/mac.zip", "size": 10},
                {"name": "promptly-v2026.2-linux-x86_64.tar.gz", "browser_download_url": "https://a/l.tgz", "size": 10},
                {"name": "promptly-v2026.2-linux-arm64.tar.gz", "browser_download_url": "https://a/la.tgz", "size": 10},
                {"name": "SHA256SUMS", "browser_download_url": "https://a/sums", "size": 1}
            ]
        });
        let r = parse_release(&v).unwrap();
        assert_eq!(r.version(), "2026.2");
        assert!(r.checksums_asset().is_some());
        if platform_suffix().is_some() {
            assert!(r.platform_asset().is_some());
        }
    }
}
