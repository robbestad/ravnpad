use std::fs::{self, File};
use std::io::{self, Write};
#[cfg(windows)]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

const OWNER: &str = "robbestad";
const REPO: &str = "ravnpad";
const USER_AGENT: &str = concat!("RavnPad/", env!("CARGO_PKG_VERSION"), " (+https://github.com/robbestad/ravnpad)");

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug)]
pub enum Error {
    Network(String),
    NoAsset,
    Zip(String),
    Install(String),
    Io(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(msg) | Self::Zip(msg) | Self::Install(msg) => write!(f, "{msg}"),
            Self::NoAsset => write!(f, "no download for this system"),
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

pub enum Check {
    UpToDate,
    Available { version: String, url: String },
}

pub enum Restart {
    Spawn(PathBuf),
    #[cfg(target_os = "macos")]
    Helper,
}

pub fn asset_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("ravnpad-windows-x86_64.zip"),
        ("macos", "aarch64") => Some("ravnpad-macos-aarch64.zip"),
        ("macos", "x86_64") => Some("ravnpad-macos-x86_64.zip"),
        _ => None,
    }
}

pub fn check_latest() -> Result<Check, Error> {
    let asset = asset_name().ok_or(Error::NoAsset)?;
    let url = format!("https://api.github.com/repos/{OWNER}/{REPO}/releases/latest");
    let json = get_json(&url)?;
    let release = parse_release(&json, asset)?;
    if is_newer(&release.version, CURRENT) {
        Ok(Check::Available {
            version: release.version,
            url: release.url,
        })
    } else {
        Ok(Check::UpToDate)
    }
}

pub fn install(url: &str) -> Result<Restart, Error> {
    let asset = asset_name().ok_or(Error::NoAsset)?;
    let work = std::env::temp_dir().join(format!("ravnpad-update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    let zip_path = work.join(asset);
    download(url, &zip_path)?;
    let unpacked = work.join("unpack");
    fs::create_dir_all(&unpacked)?;
    extract_zip(&zip_path, &unpacked)?;
    apply_payload(&unpacked)
}

pub fn cleanup_old() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(with_suffix(&exe, ".old"));
    }
    if let Ok(entries) = fs::read_dir(std::env::temp_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with("ravnpad-update-") {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
}

struct Release {
    version: String,
    url: String,
}

fn parse_release(json: &serde_json::Value, asset: &str) -> Result<Release, Error> {
    let tag = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Network("missing release tag".into()))?;
    let version = normalize_tag(tag);
    let url = json
        .get("assets")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .find_map(|item| {
            let name = item.get("name")?.as_str()?;
            (name == asset).then(|| item.get("browser_download_url")?.as_str().map(str::to_owned))?
        })
        .ok_or(Error::NoAsset)?;
    Ok(Release { version, url })
}

fn normalize_tag(tag: &str) -> String {
    tag.trim()
        .trim_start_matches('v')
        .trim_start_matches('V')
        .trim_start_matches('.')
        .to_string()
}

fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let tag = normalize_tag(tag);
    let mut parts = tag.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => normalize_tag(latest) != normalize_tag(current),
    }
}

fn get_json(url: &str) -> Result<serde_json::Value, Error> {
    let response = agent()
        .get(url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|err| Error::Network(err.to_string()))?;
    response
        .into_json()
        .map_err(|err| Error::Network(err.to_string()))
}

fn download(url: &str, dest: &Path) -> Result<(), Error> {
    let response = agent()
        .get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|err| Error::Network(err.to_string()))?;
    let mut file = File::create(dest)?;
    io::copy(&mut response.into_reader(), &mut file)?;
    file.flush()?;
    Ok(())
}

fn agent() -> ureq::Agent {
    ureq::builder()
        .timeout_connect(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .build()
}

fn extract_zip(zip_path: &Path, dest: &Path) -> Result<(), Error> {
    let file = File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|err| Error::Zip(err.to_string()))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|err| Error::Zip(err.to_string()))?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut outfile = File::create(&out)?;
        io::copy(&mut entry, &mut outfile)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&out, fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}

fn apply_payload(unpacked: &Path) -> Result<Restart, Error> {
    let exe = std::env::current_exe()?;
    #[cfg(target_os = "macos")]
    {
        if let Some(app) = app_bundle(&exe) {
            let new_app = unpacked.join("RavnPad.app");
            if !new_app.is_dir() {
                return Err(Error::Zip("release zip is missing RavnPad.app".into()));
            }
            ensure_replaceable(&app)?;
            write_macos_helper(&app, &new_app)?;
            return Ok(Restart::Helper);
        }
        let bin = mac_binary(unpacked).ok_or_else(|| Error::Zip("release zip is missing ravnpad".into()))?;
        replace_running(&exe, &bin)?;
        return Ok(Restart::Spawn(exe));
    }
    #[cfg(windows)]
    {
        let bin = unpacked.join("ravnpad.exe");
        if !bin.is_file() {
            return Err(Error::Zip("release zip is missing ravnpad.exe".into()));
        }
        if !is_windows_pe(&bin) {
            return Err(Error::Zip(
                "downloaded file is not a Windows program".into(),
            ));
        }
        replace_running(&exe, &bin)?;
        return Ok(Restart::Spawn(exe));
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (unpacked, exe);
        Err(Error::NoAsset)
    }
}

fn replace_running(current: &Path, new_file: &Path) -> Result<(), Error> {
    let old = with_suffix(current, ".old");
    let staged = with_suffix(current, ".new");
    fs::copy(new_file, &staged)?;
    let _ = fs::remove_file(&old);
    if let Err(err) = fs::rename(current, &old) {
        let _ = fs::remove_file(&staged);
        return Err(err.into());
    }
    if let Err(err) = fs::rename(&staged, current) {
        let _ = fs::rename(&old, current);
        let _ = fs::remove_file(&staged);
        return Err(err.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(current, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

#[cfg(windows)]
fn is_windows_pe(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 2];
    file.read_exact(&mut magic).is_ok() && magic == *b"MZ"
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut raw = path.as_os_str().to_os_string();
    raw.push(suffix);
    PathBuf::from(raw)
}

#[cfg(target_os = "macos")]
fn app_bundle(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let app = contents.parent()?;
    (app.extension()? == "app").then(|| app.to_path_buf())
}

#[cfg(target_os = "macos")]
fn mac_binary(unpacked: &Path) -> Option<PathBuf> {
    let nested = unpacked.join("RavnPad.app/Contents/MacOS/ravnpad");
    if nested.is_file() {
        return Some(nested);
    }
    let flat = unpacked.join("ravnpad");
    flat.is_file().then_some(flat)
}

// The app must be swappable by the helper script after this process exits.
// Fail here, while we can still show an error, instead of letting the script
// fail silently on a read-only or permission-locked location.
#[cfg(target_os = "macos")]
fn ensure_replaceable(app: &Path) -> Result<(), Error> {
    if app.to_string_lossy().contains("AppTranslocation") {
        return Err(Error::Install(format!(
            "macOS is running RavnPad from a temporary read-only location \
             ({}). Move RavnPad.app to /Applications and try again.",
            app.display()
        )));
    }
    let Some(dir) = app.parent() else {
        return Err(Error::Install(format!(
            "cannot locate the app directory for {}",
            app.display()
        )));
    };
    let probe = dir.join(".ravnpad-write-test");
    match fs::File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            Ok(())
        }
        Err(err) => Err(Error::Install(format!(
            "no write access to {}: {err}",
            dir.display()
        ))),
    }
}

#[cfg(target_os = "macos")]
fn write_macos_helper(app: &Path, new_app: &Path) -> Result<(), Error> {
    let script = std::env::temp_dir().join(format!("ravnpad-relaunch-{}.sh", std::process::id()));
    let body = format!(
        "#!/bin/sh\n\
         pid=\"$1\"\n\
         app=\"$2\"\n\
         new=\"$3\"\n\
         shift 3\n\
         log=\"${{TMPDIR:-/tmp}}/ravnpad-update.log\"\n\
         i=0\n\
         while kill -0 \"$pid\" 2>/dev/null && [ \"$i\" -lt 600 ]; do sleep 0.2; i=$((i + 1)); done\n\
         backup=\"$app.ravnpad-bak\"\n\
         rm -rf \"$backup\"\n\
         if mv \"$app\" \"$backup\" 2>>\"$log\" && mv \"$new\" \"$app\" 2>>\"$log\"; then\n\
         \x20   rm -rf \"$backup\"\n\
         \x20   xattr -dr com.apple.quarantine \"$app\" 2>/dev/null || true\n\
         else\n\
         \x20   echo \"$(date): failed to replace $app\" >>\"$log\"\n\
         \x20   mv \"$backup\" \"$app\" 2>>\"$log\"\n\
         fi\n\
         if [ \"$#\" -gt 0 ]; then open -a \"$app\" \"$@\" 2>>\"$log\"; else open \"$app\" 2>>\"$log\"; fi\n\
         rm -f \"$0\"\n"
    );
    fs::write(&script, body)?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg(&script)
        .arg(std::process::id().to_string())
        .arg(app)
        .arg(new_app);
    for arg in std::env::args_os().skip(1) {
        cmd.arg(arg);
    }
    cmd.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn version_parse_accepts_v_prefix_and_old_dot_tag() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v.1.1.2"), Some((1, 1, 2)));
    }

    #[test]
    fn newer_compares_semver() {
        assert!(is_newer("1.2.3", "1.2.2"));
        assert!(!is_newer("1.2.2", "1.2.2"));
        assert!(!is_newer("1.2.1", "1.2.2"));
        assert!(is_newer("v2.0.0", "1.9.9"));
    }

    #[test]
    fn parse_release_picks_exact_asset() {
        let json = serde_json::json!({
            "tag_name": "v1.2.3",
            "assets": [
                {
                    "name": "ravnpad-macos-aarch64.zip",
                    "browser_download_url": "https://example.com/mac.zip"
                },
                {
                    "name": "ravnpad-windows-x86_64.zip",
                    "browser_download_url": "https://example.com/win.zip"
                }
            ]
        });
        let release = parse_release(&json, "ravnpad-windows-x86_64.zip").unwrap();
        assert_eq!(release.version, "1.2.3");
        assert_eq!(release.url, "https://example.com/win.zip");
        assert!(parse_release(&json, "ravnpad-linux-x86_64.zip").is_err());
    }

    #[test]
    fn extract_rejects_zip_slip_and_keeps_payload() {
        let dir = std::env::temp_dir().join("ravnpad-zip-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("in.zip");
        let unpacked = dir.join("out");
        {
            let file = File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("ravnpad.exe", opts).unwrap();
            zip.write_all(b"new-bin").unwrap();
            zip.start_file("../escape.txt", opts).unwrap();
            zip.write_all(b"nope").unwrap();
            zip.finish().unwrap();
        }
        extract_zip(&zip_path, &unpacked).unwrap();
        assert_eq!(fs::read(unpacked.join("ravnpad.exe")).unwrap(), b"new-bin");
        assert!(!dir.join("escape.txt").exists());
        assert!(!unpacked.join("escape.txt").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn windows_payload_must_start_with_mz() {
        let dir = std::env::temp_dir().join("ravnpad-pe-test");
        let _ = fs::create_dir_all(&dir);
        let pe = dir.join("ok.exe");
        let txt = dir.join("no.exe");
        fs::write(&pe, b"MZ\x90\x00").unwrap();
        fs::write(&txt, b"not-an-exe").unwrap();
        assert!(is_windows_pe(&pe));
        assert!(!is_windows_pe(&txt));
        let _ = fs::remove_dir_all(dir);
    }
}
