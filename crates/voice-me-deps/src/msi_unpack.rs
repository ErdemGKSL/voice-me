//! Unpacking a Windows Installer package without Windows Installer.
//!
//! eSpeak NG's official `espeak-ng.msi` is a per-machine package: `msiexec`,
//! even for an administrative install (`/a`), wants an administrator and
//! fails with exit code 1603 when run quietly without one. voice-me reads
//! the package itself instead: the `File`, `Component` and `Directory`
//! tables give every file's path, and the cabinets embedded in the package
//! hold the bytes. Plain Rust, so no prompt, no registry, and the same code
//! on every OS.
//!
//! A cabinet folder is one compressed stream, so its files are read in one
//! pass, in order ([`cab::Cabinet::all_files`]). Asking for one file at a
//! time decompresses the folder from its start for every file, which for
//! eSpeak NG's 443 files in one folder takes minutes.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use voice_me_core::VoiceMeError;

/// Unpack every file of the package at `msi` into `target`, laid out as a
/// real install would lay it out, with the package's root directory
/// (`TARGETDIR`, and anything that maps onto it such as
/// `ProgramFiles64Folder`) as `target` itself.
pub fn unpack(msi: &Path, target: &Path) -> Result<(), VoiceMeError> {
    unpack_inner(msi, target)
        .map_err(|error| VoiceMeError::Other(format!("Could not unpack eSpeak NG: {error}.")))
}

fn unpack_inner(msi: &Path, target: &Path) -> io::Result<()> {
    let mut package = msi::open(msi)?;

    let directories = read_directories(&mut package)?;
    let component_dirs: HashMap<String, String> = rows(&mut package, "Component")?
        .into_iter()
        .map(|row| Ok((text(&row, "Component")?, text(&row, "Directory_")?)))
        .collect::<io::Result<_>>()?;
    let mut files = HashMap::new();
    for row in rows(&mut package, "File")? {
        let key = text(&row, "File")?;
        let component = text(&row, "Component_")?;
        let directory = component_dirs
            .get(&component)
            .ok_or_else(|| invalid(format!("file {key} has no component {component}")))?;
        let relative = directory_path(&directories, directory)?
            .join(checked_name(long_name(&text(&row, "FileName")?))?);
        files.insert(key, relative);
    }

    fs::create_dir_all(target)?;
    for row in rows(&mut package, "Media")? {
        let Some(cabinet) = row["Cabinet"].as_str().filter(|name| !name.is_empty()) else {
            continue;
        };
        let Some(stream) = cabinet.strip_prefix('#') else {
            return Err(invalid(format!(
                "cabinet {cabinet} is not inside the package"
            )));
        };
        let mut bytes = Vec::new();
        io::copy(&mut package.read_stream(stream)?, &mut bytes)?;
        let cabinet = cab::Cabinet::new(io::Cursor::new(bytes))?;
        let mut entries = cabinet.all_files();
        while let Some((entry, mut reader)) = entries.next_file() {
            // A cabinet file the `File` table does not name is not installed.
            let Some(relative) = files.remove(entry.name()) else {
                continue;
            };
            let destination = target.join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            let written = io::copy(&mut reader, &mut fs::File::create(&destination)?)?;
            if written != u64::from(entry.uncompressed_size()) {
                return Err(invalid(format!("file {} is cut short", entry.name())));
            }
        }
    }
    if let Some(key) = files.keys().next() {
        return Err(invalid(format!(
            "file {key} is in none of the package's cabinets"
        )));
    }
    Ok(())
}

/// One `Directory` row: its parent, and its own name under that parent
/// (`None` for `.`, a directory that is its parent).
struct Directory {
    parent: Option<String>,
    name: Option<String>,
}

fn read_directories<F: io::Read + io::Seek>(
    package: &mut msi::Package<F>,
) -> io::Result<HashMap<String, Directory>> {
    rows(package, "Directory")?
        .into_iter()
        .map(|row| {
            let key = text(&row, "Directory")?;
            let parent = row["Directory_Parent"]
                .as_str()
                .filter(|parent| !parent.is_empty() && *parent != key)
                .map(str::to_string);
            // `DefaultDir` is `target[:source]`, each `short|long`; a real
            // install uses the target name.
            let default_dir = text(&row, "DefaultDir")?;
            let target_name = default_dir.split(':').next().unwrap_or_default();
            let name = match long_name(target_name) {
                "." => None,
                name => Some(checked_name(name)?.to_string()),
            };
            Ok((key, Directory { parent, name }))
        })
        .collect()
}

/// The path of directory `key` below the package's root directory, which
/// has no parent and is `target` itself whatever its name (`SourceDir`).
fn directory_path(directories: &HashMap<String, Directory>, key: &str) -> io::Result<PathBuf> {
    let mut names = Vec::new();
    let mut current = key;
    // A well-formed table is a tree; the bound catches a cycle.
    for _ in 0..=directories.len() {
        let directory = directories
            .get(current)
            .ok_or_else(|| invalid(format!("directory {current} is not in the package")))?;
        let Some(parent) = &directory.parent else {
            return Ok(names.iter().rev().collect());
        };
        if let Some(name) = &directory.name {
            names.push(name.as_str());
        }
        current = parent;
    }
    Err(invalid(format!("directory {key} is inside itself")))
}

/// The long half of a `short|long` name, or the name when it has only one.
fn long_name(name: &str) -> &str {
    name.rsplit('|').next().unwrap_or(name)
}

/// A single path component from the package, never one that could climb
/// out of `target` or name a drive.
fn checked_name(name: &str) -> io::Result<&str> {
    let bad =
        name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', ':', '\0']);
    if bad {
        Err(invalid(format!("the package names a file {name:?}")))
    } else {
        Ok(name)
    }
}

fn rows<F: io::Read + io::Seek>(
    package: &mut msi::Package<F>,
    table: &str,
) -> io::Result<Vec<msi::Row>> {
    Ok(package.select_rows(msi::Select::table(table))?.collect())
}

fn text(row: &msi::Row, column: &str) -> io::Result<String> {
    row[column]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| invalid(format!("{column} is empty")))
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directories(rows: &[(&str, Option<&str>, &str)]) -> HashMap<String, Directory> {
        rows.iter()
            .map(|(key, parent, name)| {
                let name = match *name {
                    "." => None,
                    name => Some(name.to_string()),
                };
                let parent = parent.map(str::to_string);
                (key.to_string(), Directory { parent, name })
            })
            .collect()
    }

    /// eSpeak NG's own tree: `ProgramFiles64Folder` is `.`, so the install
    /// directory sits straight in `target`.
    #[test]
    fn a_directory_path_skips_the_root_and_dot_directories() {
        let table = directories(&[
            ("TARGETDIR", None, "SourceDir"),
            ("ProgramFiles64Folder", Some("TARGETDIR"), "."),
            ("INSTALLDIR", Some("ProgramFiles64Folder"), "eSpeak NG"),
            ("data", Some("INSTALLDIR"), "espeak-ng-data"),
        ]);

        assert_eq!(directory_path(&table, "TARGETDIR").unwrap(), PathBuf::new());
        assert_eq!(
            directory_path(&table, "data").unwrap(),
            Path::new("eSpeak NG").join("espeak-ng-data")
        );
    }

    #[test]
    fn a_directory_cycle_or_a_missing_parent_is_an_error() {
        let cycle = directories(&[("a", Some("b"), "a"), ("b", Some("a"), "b")]);
        assert!(directory_path(&cycle, "a").is_err());

        let orphan = directories(&[("a", Some("gone"), "a")]);
        assert!(directory_path(&orphan, "a").is_err());
    }

    #[test]
    fn a_short_and_long_name_gives_the_long_one() {
        assert_eq!(long_name("zcdhwhmd.exe|espeak-ng.exe"), "espeak-ng.exe");
        assert_eq!(long_name("am_dict"), "am_dict");
    }

    #[test]
    fn a_name_that_leaves_the_target_is_refused() {
        for bad in ["", ".", "..", "a/b", "a\\b", "C:", "x\0"] {
            assert!(checked_name(bad).is_err(), "{bad:?}");
        }
        assert_eq!(checked_name("espeak-ng-data").unwrap(), "espeak-ng-data");
    }

    #[test]
    fn a_file_that_is_not_a_package_is_an_error_naming_espeak_ng() {
        let dir = tempfile::tempdir().unwrap();
        let msi = dir.path().join("espeak-ng.msi");
        fs::write(&msi, b"not a package").unwrap();

        let VoiceMeError::Other(message) = unpack(&msi, &dir.path().join("out")).unwrap_err()
        else {
            panic!("an Other error");
        };
        assert!(
            message.starts_with("Could not unpack eSpeak NG: "),
            "{message}"
        );
    }

    /// The real package, when `VOICE_ME_ESPEAK_MSI` points at the pinned
    /// `espeak-ng.msi`: every one of its 443 files lands where eSpeak NG
    /// looks for it.
    #[test]
    fn the_real_espeak_ng_package_unpacks_into_its_install_layout() {
        let Some(msi) = std::env::var_os("VOICE_ME_ESPEAK_MSI") else {
            eprintln!("skipped: VOICE_ME_ESPEAK_MSI is not set");
            return;
        };
        let dir = tempfile::tempdir().unwrap();

        unpack(Path::new(&msi), dir.path()).unwrap();

        let install = dir.path().join("eSpeak NG");
        assert!(install.join("espeak-ng.exe").is_file());
        assert!(install.join("libespeak-ng.dll").is_file());
        assert!(install.join("espeak-ng-data").join("phontab").is_file());
        let count = walk(dir.path());
        assert_eq!(count, 443);
    }

    fn walk(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                if path.is_dir() { walk(&path) } else { 1 }
            })
            .sum()
    }
}
