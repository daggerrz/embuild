use std::collections::HashSet;
use std::path::{Path, PathBuf};
use anyhow::{anyhow, Result};
use globwalk::GlobWalkerBuilder;

static DEFAULT_EXCLUDE: &'static [&'static str] = &[
    // Python files
    "**/__pycache__",
    "**/*.pyc",
    "**/*.pyd",
    "**/*.pyo",
    // macOS files
    "**/.DS_Store",
    // Git
    "**/.git/**/*",
    // SVN
    "**/.svn/**/*",
    // dist and build artefacts
    "**/dist/**/*",
    "**/build/**/*",
    // artifacts from example projects
    "**/managed_components/**/*",
    "**/dependencies.lock",
    // CI files
    "**/.github/**/*",
    "**/.gitlab-ci.yml",
    // IDE files
    "**/.idea/**/*",
    "**/.vscode/**/*",
    // Configs
    "**/.settings/**/*",
    "**/sdkconfig",
    "**/sdkconfig.old",
    // Hash file
    "**/.component_hash"
];


/// Return a set of paths that match the given include and exclude patterns.
/// Based on the `filtered_paths` function in `file_tools.py` from the ESP-IDF.
pub(crate) fn filtered_paths(
    root: &Path,
    exclude: Vec<&str>,
    exclude_default: bool,
) -> Result<Vec<PathBuf>> {
    let mut paths: HashSet<PathBuf> = HashSet::new();

    let evaluate_glob = |pattern: &str| -> Result<_> {
        GlobWalkerBuilder::from_patterns(root, &[pattern])
            .build()
            .map_err(|e| anyhow!("Failed to build glob walker for {} and pattern {}: {}", root.display(), pattern, e))
            .map(|e| e.flatten())
    };

    let include_paths = |paths: &mut HashSet<PathBuf>, pattern: &str| -> Result<()> {
        evaluate_glob(pattern)?
            .for_each(|file| {
                paths.insert(file.into_path());
            });
        Ok(())
    };

    let exclude_paths = |paths: &mut HashSet<PathBuf>, pattern: &str| -> Result<()> {
        evaluate_glob(pattern)?
            .for_each(|file| {
                paths.remove(&file.into_path());
            });
        Ok(())
    };

    // First, include all
    include_paths(&mut paths, "**/*")?;

    if exclude_default {
        for pattern in DEFAULT_EXCLUDE.iter() {
            exclude_paths(&mut paths, pattern)?;
            // Exclude the directory explicitly if it is a globstar pattern
            if pattern.ends_with("/**/*") {
                let index = pattern.rfind("/**/*").unwrap();
                exclude_paths(&mut paths, &pattern[..index])?;
            }
        }
    }

    // Exclude user patterns
    for pattern in exclude {
        exclude_paths(&mut paths, &pattern)?;
    }

    Ok(paths.into_iter().collect())
}


