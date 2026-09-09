use std::io;
use std::path::Path;

use winreg::enums::*;
use winreg::RegKey;

const PROGID: &str = "RavnPad.txt";
const APP_NAME: &str = "RavnPad";

const DEFAULT_EXTS: &[&str] = &[".txt", ".text", ".log", ".md"];
const OPEN_WITH_EXTS: &[&str] = &[
    ".txt", ".text", ".log", ".md", ".csv", ".ini", ".cfg", ".conf", ".nfo", ".asc",
];

pub fn register() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let exe = dunce_path(&exe);
    let _ = register_with(&exe);
}

fn dunce_path(path: &Path) -> std::path::PathBuf {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .strip_prefix(r"\\?\")
        .map(str::to_string)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| path.to_path_buf())
}

fn register_with(exe: &Path) -> io::Result<()> {
    let exe_str = exe.to_string_lossy();
    let command = format!("\"{exe_str}\" \"%1\"");
    let icon = format!("{exe_str},0");
    let exe_name = exe
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("ravnpad.exe");

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (classes, _) = hkcu.create_subkey("Software\\Classes")?;

    let (prog, _) = classes.create_subkey(PROGID)?;
    prog.set_value("", &format!("{APP_NAME}-dokument"))?;
    prog.set_value("FriendlyTypeName", &format!("{APP_NAME}-dokument"))?;
    let (default_icon, _) = prog.create_subkey("DefaultIcon")?;
    default_icon.set_value("", &icon)?;
    let (open_verb, _) = prog.create_subkey(r"shell\open")?;
    open_verb.set_value("", &"Åpne")?;
    open_verb.set_value("FriendlyAppName", &APP_NAME)?;
    let (open_cmd, _) = prog.create_subkey(r"shell\open\command")?;
    open_cmd.set_value("", &command)?;

    for ext in DEFAULT_EXTS {
        let (ext_key, _) = classes.create_subkey(ext)?;
        ext_key.set_value("", &PROGID)?;
        let (progids, _) = ext_key.create_subkey("OpenWithProgids")?;
        progids.set_value(PROGID, &"")?;
        let _ = clear_user_choice(ext);
    }

    for ext in OPEN_WITH_EXTS {
        let (ext_key, _) = classes.create_subkey(ext)?;
        let (progids, _) = ext_key.create_subkey("OpenWithProgids")?;
        progids.set_value(PROGID, &"")?;
        if let Ok((file_exts, _)) = hkcu.create_subkey(format!(
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\{ext}\OpenWithProgids"
        )) {
            let _ = file_exts.set_value(PROGID, &"");
        }
    }

    let (app, _) = classes.create_subkey(format!(r"Applications\{exe_name}"))?;
    app.set_value("FriendlyAppName", &APP_NAME)?;
    let (app_open, _) = app.create_subkey(r"shell\open\command")?;
    app_open.set_value("", &command)?;
    let (types, _) = app.create_subkey("SupportedTypes")?;
    for ext in OPEN_WITH_EXTS {
        types.set_value(*ext, &"")?;
    }

    let (app_path, _) = hkcu.create_subkey(format!(
        r"Software\Microsoft\Windows\CurrentVersion\App Paths\{exe_name}"
    ))?;
    app_path.set_value("", &exe_str.as_ref())?;
    if let Some(parent) = exe.parent() {
        app_path.set_value("Path", &parent.to_string_lossy().as_ref())?;
    }

    let (caps, _) = hkcu.create_subkey(r"Software\RavnPad\Capabilities")?;
    caps.set_value("ApplicationName", &APP_NAME)?;
    caps.set_value("ApplicationDescription", &"Enkel les/skriv-notisblokk")?;
    caps.set_value("ApplicationIcon", &icon)?;
    let (file_assoc, _) = caps.create_subkey("FileAssociations")?;
    for ext in DEFAULT_EXTS {
        file_assoc.set_value(*ext, &PROGID)?;
    }

    let (registered, _) = hkcu.create_subkey("Software\\RegisteredApplications")?;
    registered.set_value(APP_NAME, &r"Software\RavnPad\Capabilities")?;

    notify_assoc_changed();
    Ok(())
}

fn clear_user_choice(ext: &str) -> io::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey_with_flags(
        format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\{ext}"),
        KEY_READ | KEY_WRITE,
    )?;
    let _ = key.delete_subkey_all("UserChoice");
    let _ = key.delete_subkey_all("UserChoiceNew");
    Ok(())
}

fn notify_assoc_changed() {
    const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;
    const SHCNF_IDLIST: u32 = 0x0000;
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn SHChangeNotify(
        event: i32,
        flags: u32,
        item1: *const std::ffi::c_void,
        item2: *const std::ffi::c_void,
    );
}
