use std::{
    fs,
    io::{self, Write},
    path::Path,
};

#[derive(Debug)]
pub struct SaveOutcome {
    // The replacement has committed even when directory durability cannot be confirmed.
    pub durability_warning: Option<io::Error>,
}

// Never truncate the destination. Persist uses the platform's atomic replacement.
pub fn save(path: &Path, bytes: &[u8]) -> io::Result<SaveOutcome> {
    save_with_check(path, bytes, |_| Ok(()))
}

fn save_with_check(
    path: &Path,
    bytes: &[u8],
    check: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<SaveOutcome> {
    save_with_sync(path, bytes, check, sync_directory)
}

fn sync_directory(parent: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
}

fn save_with_sync(
    path: &Path,
    bytes: &[u8],
    check: impl FnOnce(&Path) -> io::Result<()>,
    sync: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<SaveOutcome> {
    let target = if path.is_symlink() {
        fs::canonicalize(path)?
    } else {
        path.to_owned()
    };
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    match fs::metadata(&target) {
        Ok(_) => preserve_metadata(&target, &tmp)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    // Check immediately before replacement, after the potentially slow write.
    check(&target)?;
    replace(tmp, &target)?;
    Ok(SaveOutcome {
        durability_warning: sync(parent).err(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_sync_failure_returns_committed_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note");
        fs::write(&path, "old").unwrap();
        let result = save_with_sync(
            &path,
            b"new",
            |_| Ok(()),
            |_| Err(io::Error::other("injected fsync failure")),
        )
        .unwrap();
        assert!(result.durability_warning.is_some());
        assert_eq!(fs::read(&path).unwrap(), b"new");
        // A caller that accepts the committed baseline can save again normally.
        save_checked(&path, b"next", Some(b"new")).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn replacement_preserves_acl_xattr_and_owner() {
        use std::os::unix::fs::MetadataExt;
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note");
        fs::write(&path, "old").unwrap();
        assert!(
            Command::new("/bin/chmod")
                .args(["+a", "everyone allow read"])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("/usr/bin/xattr")
                .args(["-w", "com.ravnpad.test", "retained"])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let acl = |path: &Path| {
            let output = Command::new("/bin/ls")
                .arg("-le")
                .arg(path)
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .skip(1)
                .collect::<Vec<_>>()
                .join("\n")
        };
        let before_acl = acl(&path);
        assert!(!before_acl.is_empty());
        let before = fs::metadata(&path).unwrap();
        save(&path, b"new").unwrap();
        let after = fs::metadata(&path).unwrap();
        assert_eq!((before.uid(), before.gid()), (after.uid(), after.gid()));
        assert_eq!(acl(&path), before_acl);
        let xattr = Command::new("/usr/bin/xattr")
            .args(["-p", "com.ravnpad.test"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(xattr.status.success());
        assert_eq!(String::from_utf8(xattr.stdout).unwrap().trim(), "retained");
        assert_eq!(fs::read(path).unwrap(), b"new");
    }

    #[test]
    fn replaces_complete_document() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note");
        fs::write(&path, "old long content").unwrap();
        save(&path, "æøå".as_bytes()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "æøå");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn failed_replace_keeps_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing-directory");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("original"), "safe").unwrap();
        assert!(save(&path, b"replacement").is_err());
        assert_eq!(fs::read_to_string(path.join("original")).unwrap(), "safe");
    }

    #[cfg(windows)]
    #[test]
    fn windows_move_fallback_replaces_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("note");
        fs::write(&target, "old").unwrap();
        let mut tmp = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        tmp.write_all(b"new").unwrap();
        tmp.as_file().sync_all().unwrap();
        let tmp = tmp.into_temp_path();

        move_replace(&tmp, &target).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(!tmp.exists());
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires RAVNPAD_WSL_TEST_DIR pointing to a running WSL distribution"]
    fn saves_through_wsl_file_system_provider() {
        let root = std::env::var_os("RAVNPAD_WSL_TEST_DIR")
            .map(std::path::PathBuf::from)
            .expect("RAVNPAD_WSL_TEST_DIR is not set");
        let dir = tempfile::Builder::new()
            .prefix("ravnpad-wsl-test-")
            .tempdir_in(root)
            .unwrap();
        let path = dir.path().join("note.txt");

        save(&path, b"first").unwrap();
        save_checked(&path, b"second", Some(b"first")).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

// Compare actual contents, not just timestamps (which can be coarse on network drives).
pub fn save_checked(path: &Path, bytes: &[u8], expected: Option<&[u8]>) -> io::Result<SaveOutcome> {
    save_with_check(path, bytes, |path| {
        let current = match fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        if current.as_deref() != expected {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "File changed outside RavnPad",
            ));
        }
        Ok(())
    })
}

#[cfg(test)]
mod conflict_tests {
    use super::*;
    #[test]
    fn external_changes_and_deletion_are_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note");
        fs::write(&path, "external").unwrap();
        assert!(save_checked(&path, b"edits", Some(b"original")).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"external");
        fs::remove_file(&path).unwrap();
        assert!(save_checked(&path, b"edits", Some(b"original")).is_err());
        assert!(!path.exists());
        save_checked(&path, b"new", None).unwrap();
        save_checked(&path, b"edits", Some(b"new")).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"edits");
    }
    #[cfg(unix)]
    #[test]
    fn saving_through_symlink_preserves_link_and_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("link");
        fs::write(&target, "before").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        symlink(&target, &link).unwrap();
        save(&link, b"after").unwrap();
        assert!(link.is_symlink());
        assert_eq!(fs::read(target).unwrap(), b"after");
        assert_eq!(
            fs::metadata(link).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
}

#[cfg(target_os = "macos")]
fn preserve_metadata(source: &Path, tmp: &tempfile::NamedTempFile) -> io::Result<()> {
    use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
    unsafe extern "C" {
        fn fcopyfile(from: i32, to: i32, state: *mut std::ffi::c_void, flags: u32) -> i32;
    }
    let original = fs::File::open(source)?;
    // copyfile.h: COPYFILE_METADATA = STAT | ACL | XATTR. No file data copied.
    let result = unsafe {
        fcopyfile(
            original.as_raw_fd(),
            tmp.as_file().as_raw_fd(),
            std::ptr::null_mut(),
            7,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    let before = original.metadata()?;
    let after = tmp.as_file().metadata()?;
    // fcopyfile may be unable to retain ownership without reporting it as fatal.
    if before.uid() != after.uid() || before.gid() != after.gid() || before.mode() != after.mode() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Cannot preserve file ownership and permissions",
        ));
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn preserve_metadata(source: &Path, tmp: &tempfile::NamedTempFile) -> io::Result<()> {
    // Explicit attributes make failure fatal (unlike --preserve=all, which can
    // silently ignore failures for some attributes). GNU cp also copies POSIX ACLs.
    let output = std::process::Command::new("cp")
        .args(["--attributes-only", "--preserve=mode,ownership,xattr", "--"])
        .arg(source)
        .arg(tmp.path())
        .output()?;
    if !output.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn preserve_metadata(_source: &Path, _tmp: &tempfile::NamedTempFile) -> io::Result<()> {
    // ReplaceFileW merges the original file's attributes and ACLs into the
    // replacement. Doing this first through PowerShell is redundant and can
    // fail when the Security module is unavailable or Get-Acl cannot handle a
    // particular file-system provider. Zero flags in replace() keep metadata
    // merge errors fatal instead of silently dropping metadata.
    Ok(())
}

#[cfg(not(windows))]
fn replace(tmp: tempfile::NamedTempFile, target: &Path) -> io::Result<()> {
    tmp.persist(target).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(windows)]
fn replace(tmp: tempfile::NamedTempFile, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    const ERROR_NOT_SUPPORTED: i32 = 50;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
        ) -> i32;
    }
    // ReplaceFileW opens the replacement without a sharing mode, so the
    // NamedTempFile handle must be closed before calling it.
    let tmp = tmp.into_temp_path();
    if !target.try_exists()? {
        tmp.persist_noclobber(target).map_err(|e| e.error)?;
        return Ok(());
    }
    let old: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let new: Vec<_> = tmp.as_os_str().encode_wide().chain(Some(0)).collect();
    // Zero flags: never ignore ACL/metadata merge errors.
    let result = unsafe {
        ReplaceFileW(
            old.as_ptr(),
            new.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if result == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_NOT_SUPPORTED) {
            // Some file-system providers, including WSL's \\wsl.localhost
            // shares, do not implement ReplaceFileW. The temp file is in the
            // same directory, so MoveFileExW can still replace it without a
            // cross-volume copy.
            return move_replace(&tmp, target);
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(windows)]
fn move_replace(source: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let source: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
