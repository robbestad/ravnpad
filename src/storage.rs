use std::{
    fs,
    io::{self, Write},
    path::Path,
};

// Never truncate the destination. Persist uses the platform's atomic replacement.
pub fn save(path: &Path, bytes: &[u8]) -> io::Result<()> {
    save_with_check(path, bytes, |_| Ok(()))
}

fn save_with_check(
    path: &Path,
    bytes: &[u8],
    check: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
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
    if let Ok(meta) = fs::metadata(&target) {
        tmp.as_file().set_permissions(meta.permissions())?;
    }
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    // Check immediately before replacement, after the potentially slow write.
    check(&target)?;
    tmp.persist(&target).map_err(|e| e.error)?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
}

// Compare actual contents, not just timestamps (which can be coarse on network drives).
pub fn save_checked(path: &Path, bytes: &[u8], expected: Option<&[u8]>) -> io::Result<()> {
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
