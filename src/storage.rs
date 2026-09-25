use sha2::{Digest as _, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
};

#[derive(Debug)]
pub struct SaveOutcome {
    // The replacement has committed even when directory durability cannot be confirmed.
    pub durability_warning: Option<io::Error>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentFingerprint {
    len: u64,
    sha256: [u8; 32],
}

pub fn fingerprint(path: &Path) -> io::Result<Option<ContentFingerprint>> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut hash = Sha256::new();
    let mut len = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        len += count as u64;
        hash.update(&buffer[..count]);
    }
    Ok(Some(ContentFingerprint {
        len,
        sha256: hash.finalize().into(),
    }))
}

pub fn save_checked_fingerprint(
    path: &Path,
    bytes: &[u8],
    expected: Option<ContentFingerprint>,
) -> io::Result<SaveOutcome> {
    save_with_check(path, bytes, |target| {
        if fingerprint(target)? != expected {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "File changed outside RavnPad",
            ));
        }
        Ok(())
    })
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
    sync_directory_with(
        parent,
        |path| fs::File::open(path),
        fs::File::sync_all,
        |directory| {
            #[cfg(target_os = "macos")]
            return is_macos_smb(directory);
            #[cfg(not(target_os = "macos"))]
            false
        },
    )?;
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
}

#[cfg(unix)]
fn sync_directory_with<T>(
    parent: &Path,
    open: impl FnOnce(&Path) -> io::Result<T>,
    sync: impl FnOnce(&T) -> io::Result<()>,
    is_supported_network_provider: impl FnOnce(&T) -> bool,
) -> io::Result<()> {
    // Opening and syncing are deliberately separate: an opening failure says
    // nothing about whether this is a provider with a known fsync limitation.
    let directory = open(parent)?;
    match sync(&directory) {
        #[cfg(target_os = "macos")]
        Err(error)
            if error.kind() == io::ErrorKind::PermissionDenied
                && is_supported_network_provider(&directory) =>
        {
            Ok(())
        }
        result => result,
    }
}

#[cfg(target_os = "macos")]
fn is_macos_smb(directory: &fs::File) -> bool {
    use std::{ffi::CStr, mem::MaybeUninit, os::fd::AsRawFd};

    let mut info = MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::fstatfs(directory.as_raw_fd(), info.as_mut_ptr()) } != 0 {
        return false;
    }
    let info = unsafe { info.assume_init() };
    let file_system = unsafe { CStr::from_ptr(info.f_fstypename.as_ptr()) };
    file_system.to_bytes() == b"smbfs"
}

fn save_with_sync(
    path: &Path,
    bytes: &[u8],
    check: impl FnOnce(&Path) -> io::Result<()>,
    sync: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<SaveOutcome> {
    save_with_operations(path, bytes, check, replace, sync)
}

fn save_with_operations(
    path: &Path,
    bytes: &[u8],
    check: impl FnOnce(&Path) -> io::Result<()>,
    replace_operation: impl FnOnce(tempfile::NamedTempFile, &Path) -> io::Result<()>,
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
    if let Err(error) = replace_operation(tmp, &target) {
        // Some macOS network file systems can report EACCES after the server
        // has already committed the rename. Only accept that ambiguous result
        // when the complete destination can be read back byte-for-byte.
        #[cfg(target_os = "macos")]
        if error.kind() == io::ErrorKind::PermissionDenied
            && fs::read(&target).is_ok_and(|current| current == bytes)
        {
            // The replacement committed despite the reported error. Continue
            // through the normal directory durability check below.
        } else {
            return Err(error);
        }
        #[cfg(not(target_os = "macos"))]
        return Err(error);
    }
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
    fn ambiguous_committed_replace_still_syncs_directory() {
        use std::cell::Cell;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note");
        fs::write(&path, "old").unwrap();
        let sync_calls = Cell::new(0);
        let result = save_with_operations(
            &path,
            b"saved",
            |_| Ok(()),
            |tmp, target| {
                tmp.persist(target).map_err(|error| error.error)?;
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            },
            |_| {
                sync_calls.set(sync_calls.get() + 1);
                Err(io::Error::other("injected directory sync failure"))
            },
        )
        .unwrap();
        assert_eq!(sync_calls.get(), 1);
        assert!(result.durability_warning.is_some());
        assert_eq!(fs::read(path).unwrap(), b"saved");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ambiguous_replace_with_different_bytes_preserves_original_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note");
        fs::write(&path, "old").unwrap();
        let error = save_with_operations(
            &path,
            b"saved",
            |_| Ok(()),
            |_tmp, _target| Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            |_| panic!("directory sync must not run"),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(path).unwrap(), b"old");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn directory_open_permission_error_is_preserved() {
        let result = sync_directory_with::<()>(
            Path::new("ignored"),
            |_| Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            |_| panic!("sync must not run"),
            |_| true,
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn directory_sync_permission_error_is_ignored_only_for_supported_provider() {
        let denied = || Err(io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(
            sync_directory_with(Path::new("ignored"), |_| Ok(()), |_| denied(), |_| true).is_ok()
        );
        let error = sync_directory_with(Path::new("ignored"), |_| Ok(()), |_| denied(), |_| false)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn smb_metadata_fallback_rejects_non_provenance_xattrs() {
        assert!(!has_non_provenance_extended_attribute_names(
            b"com.apple.provenance\0"
        ));
        assert!(has_non_provenance_extended_attribute_names(
            b"com.apple.provenance\0user.note\0"
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires RAVNPAD_MACOS_SMB_TEST_DIR pointing to a mounted SMB share"]
    fn saves_repeatedly_on_macos_smb_share() {
        let root = std::env::var_os("RAVNPAD_MACOS_SMB_TEST_DIR")
            .map(std::path::PathBuf::from)
            .expect("RAVNPAD_MACOS_SMB_TEST_DIR is not set");
        let dir = tempfile::Builder::new()
            .prefix("ravnpad-smb-test-")
            .tempdir_in(root)
            .unwrap();
        let path = dir.path().join("note.txt");

        save(&path, b"first").unwrap();
        save_checked(&path, b"second", Some(b"first")).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
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
    fn streaming_fingerprint_detects_same_size_external_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large-target");
        fs::write(&path, vec![b'a'; 2 * 1024 * 1024]).unwrap();
        let expected = fingerprint(&path).unwrap();
        fs::write(&path, vec![b'b'; 2 * 1024 * 1024]).unwrap();
        let error = save_checked_fingerprint(&path, b"replacement", expected).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::metadata(&path).unwrap().len(), 2 * 1024 * 1024);
    }

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
    const COPYFILE_SECURITY: u32 = 3;
    const COPYFILE_METADATA: u32 = 7;
    unsafe extern "C" {
        fn fcopyfile(from: i32, to: i32, state: *mut std::ffi::c_void, flags: u32) -> i32;
    }
    let original = fs::File::open(source)?;
    // copyfile.h: COPYFILE_METADATA = STAT | ACL | XATTR. No file data copied.
    let mut result = unsafe {
        fcopyfile(
            original.as_raw_fd(),
            tmp.as_file().as_raw_fd(),
            std::ptr::null_mut(),
            COPYFILE_METADATA,
        )
    };
    if result != 0 {
        let error = io::Error::last_os_error();
        // Some SMB providers reject COPYFILE_XATTR when the source has no user
        // xattrs and only macOS's non-copyable provenance marker. In that exact
        // case, retain stat data and ACLs using the supported subset. Never use
        // the fallback when other xattrs exist or their absence is uncertain.
        if error.kind() != io::ErrorKind::PermissionDenied
            || !is_macos_smb(&original)
            || source_has_non_provenance_extended_attributes(&original)?
        {
            return Err(error);
        }
        result = unsafe {
            fcopyfile(
                original.as_raw_fd(),
                tmp.as_file().as_raw_fd(),
                std::ptr::null_mut(),
                COPYFILE_SECURITY,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
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

#[cfg(target_os = "macos")]
fn source_has_non_provenance_extended_attributes(source: &fs::File) -> io::Result<bool> {
    use std::os::fd::AsRawFd;

    let size = unsafe { libc::flistxattr(source.as_raw_fd(), std::ptr::null_mut(), 0, 0) };
    if size < 0 {
        return Err(io::Error::last_os_error());
    }
    if size == 0 {
        return Ok(false);
    }
    let mut names = vec![0_u8; size as usize];
    let read = unsafe {
        libc::flistxattr(
            source.as_raw_fd(),
            names.as_mut_ptr().cast(),
            names.len(),
            0,
        )
    };
    if read < 0 {
        return Err(io::Error::last_os_error());
    }
    let names = &names[..read as usize];
    Ok(has_non_provenance_extended_attribute_names(names))
}

#[cfg(target_os = "macos")]
fn has_non_provenance_extended_attribute_names(names: &[u8]) -> bool {
    names
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .any(|name| name != b"com.apple.provenance")
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
