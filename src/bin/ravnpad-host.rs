//! Persistent document host. `start` detaches; `serve` is the child entry point.

#[path = "../agent.rs"]
mod agent;
#[path = "../document.rs"]
mod document;
#[path = "../host.rs"]
mod host;
#[path = "../recovery.rs"]
mod recovery;
#[path = "../storage.rs"]
mod storage;

mod prefs {
    use std::path::PathBuf;
    pub fn config_dir() -> Option<PathBuf> {
        #[cfg(windows)]
        {
            Some(PathBuf::from(std::env::var_os("APPDATA")?).join("RavnPad"))
        }
        #[cfg(target_os = "macos")]
        {
            Some(
                PathBuf::from(std::env::var_os("HOME")?)
                    .join("Library/Application Support/RavnPad"),
            )
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let base = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?;
            Some(base.join("ravnpad"))
        }
    }
}

fn main() {
    if let Err(error) = host::run_launcher(std::env::args_os().skip(1), false) {
        eprintln!("ravnpad-host: {error}");
        std::process::exit(1);
    }
}
