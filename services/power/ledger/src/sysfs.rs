//! Reading small kernel files. Every value is optional: a file that is
//! missing, unreadable or says something else reads as None.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// The file's contents, trimmed.
pub fn read(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

pub fn number<T: FromStr>(path: impl AsRef<Path>) -> Option<T> {
    read(path)?.parse().ok()
}

/// The entries of a directory, sorted, or none.
pub fn entries(dir: impl AsRef<Path>) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map(|it| it.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    paths.sort();
    paths
}

/// The file name of a path.
pub fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A CPU list such as `0-5`, `0,2,4-7` or `0 1 2 3`.
pub fn cpu_list(list: &str) -> Vec<u32> {
    let mut cpus = Vec::new();
    for part in list
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
    {
        match part.split_once('-') {
            Some((a, b)) => {
                if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
                    cpus.extend(a..=b);
                }
            }
            None => cpus.extend(part.parse::<u32>().ok()),
        }
    }
    cpus
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_lists() {
        assert_eq!(cpu_list("0-5"), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(cpu_list("0,2,6-7\n"), vec![0, 2, 6, 7]);
        assert_eq!(cpu_list("0 1 2"), vec![0, 1, 2]);
        assert!(cpu_list("").is_empty());
    }
}
