use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const BLOCK_SIZE: usize = 65536;
const HASH_FILENAME: &str = ".component_hash";
const SHA256_RE: &str = r"^[A-Fa-f0-9]{64}$";

fn hash_object(obj: Value) -> String {
    let mut sha = Sha256::new();
    let json_string = json!(obj).to_string();
    sha.update(json_string.as_bytes());
    sha.finalize().to_vec().into_iter().map(|b| format!("{:02x}", b)).collect::<Vec<String>>().concat()
}

fn hash_file(file_path: &Path) -> Result<String> {
    let mut sha = Sha256::new();
    let file = File::open(file_path)?;
    let mut reader = BufReader::new(file);

    let mut buffer = vec![0; BLOCK_SIZE];
    loop {
        let byte_count = reader.read(&mut buffer[..])?;
        if byte_count == 0 {
            break;
        }
        sha.update(&buffer[..byte_count]);
    }
    Ok(sha.finalize().to_vec().into_iter().map(|b| format!("{:02x}", b)).collect::<Vec<String>>().concat())
}


/// Hashes a directory recursively, excluding files and directories matching the given glob patterns.
/// Based on the `hash_dir` function in `hash_tools.py` from the ESP-IDF.
fn hash_dir(root: &Path, excludes: Vec<&str>, exclude_default: bool) -> Result<String> {
    let mut sha = Sha256::new();

    let entries = crate::espidf::components::file_util::filtered_paths(root, excludes, exclude_default)?;
    let mut entries: Vec<(PathBuf, String)> = entries
        .into_iter()
        .map(|p| {
            let rel_path = to_relative_posix_path(root, &p);
            (p, rel_path)
        })
        .collect();
    // As per `hash_dir` in `hash_tools.py` from the ESP-IDF,
    // sort by relative path in posix format
    entries.sort_by(|(_, a), (_, b)| a.cmp(&b));

    for (path, rel_path) in entries {
        if path.is_dir() {
            continue;
        }

        // Add relative file path to hash
        sha.update(rel_path.as_bytes());

        // Calculate hash of file content and add to hash
        sha.update(hash_file(&path)?.as_bytes());
    }
    Ok(sha.finalize().to_vec().into_iter().map(|b| format!("{:02x}", b)).collect::<Vec<String>>().concat())
}

pub fn validate_dir(component_root: &Path, dir_hash: &str) -> Result<bool> {
    let current_hash = if component_root.is_dir() {
        hash_dir(component_root, vec![".component_hash"], true)?
    } else {
        return Err(anyhow!("Root path is not a directory: {}", component_root.display()));
    };
    Ok(current_hash == dir_hash)
}

pub fn validate_dir_with_hash_file(component_root: &Path) -> Result<()> {
    let hash_file_path = component_root.join(HASH_FILENAME);

    if !component_root.is_dir() || !hash_file_path.exists() {
        return Err(anyhow!("Hash file does not exist: {}", hash_file_path.display()));
    }

    let hash_from_file = std::fs::read_to_string(&hash_file_path)?.trim().to_owned();

    let re = Regex::new(SHA256_RE).unwrap();
    if !re.is_match(&hash_from_file) {
        return Err(anyhow!("Hash is not a SHA256"));
    }

    if !validate_dir(component_root, &hash_from_file)? {
        return Err(anyhow!("Hash in file '{}' has changed since it was downloaded. Please download the component again.", hash_file_path.display()));
    }
    Ok(())
}

pub fn create_hash_file(root: &Path, hash: &str) -> Result<()> {
    let hash_file_path = root.join(HASH_FILENAME);
    let mut file = File::create(&hash_file_path)?;
    file.write(hash.as_bytes())?;
    Ok(())
}

fn to_relative_posix_path(root: &Path, path: &Path) -> String {
    let stripped_path = path
        .strip_prefix(root)
        .expect(&format!("Unable to strip {} from {}", root.display(), path.display()))
        .to_str()
        .unwrap();

    #[cfg(windows)]
    // Same implementation as `as_posix` in `pathlib` used by the Python component manager
    stripped_path.replace(r"\", "/");

    #[cfg(not(windows))]
    stripped_path.to_string()
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Write;
    use crate::espidf::components::IdfComponentManager;

    use super::*;

    #[test]
    fn test_globing() {
        let tmp_dir = tempdir::TempDir::new("hashing").unwrap();

        let cf = |name: &str| {
            File::create(tmp_dir.path().join(name)).unwrap().write(name.as_bytes()).unwrap();
        };

        let get_hash = || {
            hash_dir(tmp_dir.path(), vec![], true).unwrap()
        };

        // Write a new file, which is not on the ignore list
        cf("foo.txt");
        let hash_with_just_foo = get_hash();

        // Write a second file, which is also not on the ignore list
        cf("bar.txt");
        let hash_with_foo_and_bar = get_hash();

        // Check that the hashes are different
        assert_ne!(hash_with_just_foo, hash_with_foo_and_bar);

        // Write a new file, which is on the ignore list
        cf(".component_hash");
        let hash_with_foo_bar_and_hash = get_hash();

        // Check that they're the same
        assert_eq!(hash_with_foo_bar_and_hash, hash_with_foo_bar_and_hash);
    }

    #[test]
    #[ignore]
    fn test_dir_hashing() {
        let tmp_dir = tempdir::TempDir::new("components").unwrap();

        // Download and install a component with a known hash
        let paths = IdfComponentManager::new(tmp_dir.path().clone().to_path_buf())
            .with_component("espressif/mdns".into(), "1.1.0".into())
            .unwrap()
            .install()
            .unwrap();

        // Check with the known hash
        validate_dir(paths.first().unwrap(), "46ee81d32fbf850462d8af1e83303389602f6a6a9eddd2a55104cb4c063858ed").unwrap();
    }

    #[test]
    fn test_posix_formatting() {
        let absolute_path = Path::new("/path/to/file.txt");
        let prefix = Path::new("/path");

        if let Ok(remaining) = absolute_path.strip_prefix(prefix) {
            // The remaining path is relative, not absolute
            println!("Remaining relative path: {:?}", remaining);
        } else {
            println!("Prefix does not match the absolute path.");
        }

        let root = Path::new(r"C:\path\");
        let path = Path::new(r"C:\path\to\file.txt");

        path.strip_prefix(root).unwrap();

        assert_eq!("to/file.txt", to_relative_posix_path(root, path.as_ref()));
    }
}