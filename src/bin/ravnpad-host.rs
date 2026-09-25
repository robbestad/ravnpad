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
    if let Err(error) = run() {
        eprintln!("ravnpad-host: {error}");
        std::process::exit(1);
    }
}

fn run() -> std::io::Result<()> {
    use std::io::{BufRead as _, Write as _};
    let mut args = std::env::args_os().skip(1);
    let command = args.next().unwrap_or_default();
    if command != "start" && command != "serve" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "usage: ravnpad-host start|serve [--path FILE] [--agent off|explore|edit]",
        ));
    }
    let mut path = None;
    let mut mode = if command == "start" {
        host::AgentMode::Explore
    } else {
        host::AgentMode::Off
    };
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--path" => {
                path =
                    Some(std::path::PathBuf::from(args.next().ok_or_else(|| {
                        std::io::Error::other("--path requires a file")
                    })?))
            }
            "--agent" => {
                let value = args
                    .next()
                    .ok_or_else(|| std::io::Error::other("--agent requires a mode"))?;
                mode = host::AgentMode::parse(&value.to_string_lossy())
                    .ok_or_else(|| std::io::Error::other("invalid agent mode"))?;
            }
            "--edit" => mode = host::AgentMode::Edit,
            "--off" => mode = host::AgentMode::Off,
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "unknown option",
                ));
            }
        }
    }
    if command == "serve" {
        return host::run(path, mode);
    }
    let exe = std::env::current_exe()?;
    let mut child = std::process::Command::new(exe);
    child.arg("serve").arg("--agent").arg(mode.name());
    if let Some(path) = path {
        child.arg("--path").arg(path);
    }
    child.stdin(std::process::Stdio::null());
    child
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        child.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // The serving process must outlive the terminal that invoked `start`.
        unsafe {
            child.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    let mut child = child.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("host stdout unavailable"))?;
    let mut line = String::new();
    std::io::BufReader::new(stdout).read_line(&mut line)?;
    if line.is_empty() {
        return Err(std::io::Error::other("host failed to start"));
    }
    std::io::stdout().write_all(line.as_bytes())?;
    Ok(())
}
