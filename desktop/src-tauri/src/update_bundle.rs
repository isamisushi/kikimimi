//! Check identity and version inside the authenticated archive, not just the
//! unsigned update manifest. Shared with portable regression tests.
use std::path::Path;

pub(crate) const EXECUTABLES: [&str; 3] = ["kikimimi-desktop", "kikimimi", "duckdb"];

pub(crate) fn validate(bundle: &Path, expected_version: &str) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(bundle).map_err(|e| e.to_string())?;
    if !metadata.file_type().is_dir() {
        return Err("Update bundle must be a directory, not a symlink".into());
    }
    let info =
        plist::Value::from_file(bundle.join("Contents/Info.plist")).map_err(|e| e.to_string())?;
    let info = info
        .as_dictionary()
        .ok_or("Update Info.plist is not a dictionary")?;
    for (key, expected) in [
        ("CFBundleIdentifier", "dev.kikimimi.desktop"),
        ("CFBundleExecutable", "kikimimi-desktop"),
        ("CFBundleShortVersionString", expected_version),
        ("CFBundleVersion", expected_version),
    ] {
        if info.get(key).and_then(plist::Value::as_string) != Some(expected) {
            return Err(format!(
                "Update {key} does not match the expected app/version"
            ));
        }
    }
    for name in EXECUTABLES {
        let path = bundle.join("Contents/MacOS").join(name);
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !metadata.file_type().is_file() {
            return Err(format!("Update executable {name} must be a regular file"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                return Err(format!("Update executable {name} lacks execute permission"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Contents/MacOS")).unwrap();
        for name in EXECUTABLES {
            let path = dir.path().join("Contents/MacOS").join(name);
            std::fs::write(&path, b"fixture").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let mut info = plist::Dictionary::new();
        for (key, value) in [
            ("CFBundleIdentifier", "dev.kikimimi.desktop"),
            ("CFBundleExecutable", "kikimimi-desktop"),
            ("CFBundleShortVersionString", "0.7.2"),
            ("CFBundleVersion", "0.7.2"),
        ] {
            info.insert(key.into(), value.into());
        }
        plist::Value::Dictionary(info)
            .to_file_xml(dir.path().join("Contents/Info.plist"))
            .unwrap();
        dir
    }
    #[test]
    fn manifest_cannot_relabel_a_signed_old_archive_as_a_new_version() {
        let dir = fixture();
        assert!(validate(dir.path(), "0.7.2").is_ok());
        assert!(validate(dir.path(), "0.7.3")
            .unwrap_err()
            .contains("Version"));
    }
    #[test]
    fn rejects_wrong_application_identity_and_missing_sidecar() {
        let dir = fixture();
        let path = dir.path().join("Contents/Info.plist");
        let mut info = plist::Value::from_file(&path).unwrap();
        info.as_dictionary_mut()
            .unwrap()
            .insert("CFBundleIdentifier".into(), "dev.other.app".into());
        info.to_file_binary(&path).unwrap();
        assert!(validate(dir.path(), "0.7.2")
            .unwrap_err()
            .contains("Identifier"));
        let dir = fixture();
        std::fs::remove_file(dir.path().join("Contents/MacOS/duckdb")).unwrap();
        assert!(validate(dir.path(), "0.7.2").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_non_executable_sidecar() {
        use std::os::unix::fs::PermissionsExt;
        let dir = fixture();
        std::fs::set_permissions(
            dir.path().join("Contents/MacOS/duckdb"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(validate(dir.path(), "0.7.2")
            .unwrap_err()
            .contains("execute permission"));
    }
    #[cfg(unix)]
    #[test]
    fn rejects_external_executable_symlinks() {
        let dir = fixture();
        let path = dir.path().join("Contents/MacOS/duckdb");
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("/bin/sh", path).unwrap();
        assert!(validate(dir.path(), "0.7.2")
            .unwrap_err()
            .contains("regular file"));
    }
}
