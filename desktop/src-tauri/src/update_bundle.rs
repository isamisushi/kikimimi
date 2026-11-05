//! Check identity and version inside the authenticated archive, not just the
//! unsigned update manifest. Shared with portable regression tests.
use std::path::Path;

pub(crate) const EXECUTABLES: [&str; 3] = ["kikimimi-desktop", "kikimimi", "duckdb"];

pub(crate) fn architecture_check(binary: &Path, arch: &str) -> std::process::Command {
    let mut command = std::process::Command::new("/usr/bin/lipo");
    // -verify_arch treats every following argument as an architecture.
    command.arg(binary).args(["-verify_arch", arch]);
    command
}

/// Unpack the gzip tar payload, skipping AppleDouble ("._*") entries. macOS tar
/// stores extended attributes that way. The tar crate would write them as plain
/// files, which codesign then rejects as resources added to the sealed bundle.
pub(crate) fn unpack(bytes: &[u8], destination: &Path) -> Result<(), String> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let apple_double = entry
            .path()
            .map_err(|e| e.to_string())?
            .file_name()
            .is_some_and(|name| name.as_encoded_bytes().starts_with(b"._"));
        if apple_double {
            continue;
        }
        // unpack_in refuses paths that would escape the destination.
        entry.unpack_in(destination).map_err(|e| e.to_string())?;
    }
    Ok(())
}

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
    #[test]
    fn lipo_binary_precedes_architecture_list() {
        let path = Path::new("/Applications/Test App.app/Contents/MacOS/duckdb");
        for arch in ["arm64", "x86_64"] {
            let command = architecture_check(path, arch);
            assert_eq!(command.get_program(), "/usr/bin/lipo");
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                vec![
                    path.as_os_str(),
                    std::ffi::OsStr::new("-verify_arch"),
                    std::ffi::OsStr::new(arch)
                ]
            );
        }
    }
    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        for (path, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, path, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }
    #[test]
    fn unpack_skips_apple_double_metadata() {
        let bytes = archive(&[
            ("kikimimi.app/Contents/Info.plist", b"plist"),
            ("kikimimi.app/Contents/._Info.plist", b"xattrs"),
            ("kikimimi.app/Contents/.__CodeSignature", b"xattrs"),
            ("kikimimi.app/Contents/MacOS/duckdb", b"binary"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        unpack(&bytes, dir.path()).unwrap();
        let contents = dir.path().join("kikimimi.app/Contents");
        let mut names: Vec<_> = std::fs::read_dir(&contents)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["Info.plist", "MacOS"]);
        assert_eq!(std::fs::read(contents.join("MacOS/duckdb")).unwrap(), b"binary");
    }
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
