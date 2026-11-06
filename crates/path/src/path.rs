//! Relative path types for deltadb.
//!
//! Provides [`RelPath`] and [`RelPathBuf`] — path types that are guaranteed to be
//! relative, normalized, and valid unicode. Internally stored in POSIX (`/`-delimited)
//! format regardless of host platform.
//!
//! Adapted from Zed's `util::rel_path` module.

use std::os::unix::prelude::OsStrExt;
use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

use crate::rel_path::RelPath;

pub mod abs_path;
pub mod rel_path;

pub fn try_from_bytes<'a, T>(bytes: &'a [u8]) -> anyhow::Result<T>
where
    T: From<&'a Path>,
{
    Ok(T::from(Path::new(OsStr::from_bytes(bytes))))
}

pub trait PathExt {
    fn to_rel_path_buf(&self) -> anyhow::Result<rel_path::RelPathBuf>;
}

impl<T: AsRef<Path> + ?Sized> PathExt for T {
    fn to_rel_path_buf(&self) -> anyhow::Result<rel_path::RelPathBuf> {
        Ok(RelPath::new(self.as_ref())?.into_owned())
    }
}

/// Joins `right` onto `left` with a `/`, or returns `None` when `right` is absolute or not UTF-8.
pub fn join_relative(left: impl AsRef<Path>, right: impl AsRef<Path>) -> Option<String> {
    let right = right.as_ref();
    if right.has_root() {
        return None;
    }
    let right = right.to_str()?;
    let left = left.as_ref().to_str()?;
    if left.is_empty() {
        Some(right.into())
    } else {
        Some(format!(
            "{left}{}{right}",
            if left.ends_with('/') { "" } else { "/" }
        ))
    }
}

/// Joins a relative `right` onto `left`, then resolves `.` and `..` lexically.
pub fn join_path(left: impl AsRef<Path>, right: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
    let left = left
        .as_ref()
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Path contains invalid UTF-8"))?;
    let right = normalize_separators(right);
    let right_string = right
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Path contains invalid UTF-8"))?;
    let joined = join_relative(left, right_string)
        .ok_or_else(|| anyhow::anyhow!("Path must be relative: {right:?}"))?;
    Ok(collapse_dot_segments(&joined))
}

/// Joins a relative `right` onto `left`, keeping its `.` and `..` components.
pub fn join_path_preserving_components(
    left: impl AsRef<Path>,
    right: impl AsRef<Path>,
) -> anyhow::Result<PathBuf> {
    let left = left
        .as_ref()
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Path contains invalid UTF-8"))?;
    let right = right.as_ref();
    let right_string = right
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Path contains invalid UTF-8"))?;
    let joined = join_relative(left, right_string)
        .ok_or_else(|| anyhow::anyhow!("Path must be relative: {right:?}"))?;
    Ok(PathBuf::from(&joined))
}

/// Collapses repeated separators and inner `.` components.
pub fn normalize_separators(path: impl AsRef<Path>) -> PathBuf {
    path.as_ref().components().collect()
}

/// Resolves `.` and `..` without touching the file system; `..` above a root is dropped.
pub fn collapse_dot_segments(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let mut normalized: Vec<Component<'_>> = Vec::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.last() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::RootDir) => {}
                _ => normalized.push(component),
            },
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component);
            }
        }
    }

    if normalized.is_empty() && !path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_path() {
        let posix_path = crate::join_path(Path::new("/home/user/dev"), "worktrees").unwrap();

        assert_eq!(posix_path, PathBuf::from("/home/user/dev/worktrees"));
    }

    #[test]
    fn test_join_path_preserving_components() {
        let posix_path =
            crate::join_path_preserving_components(Path::new("/home/user/symlink"), "../worktrees")
                .unwrap();

        assert_eq!(posix_path, PathBuf::from("/home/user/symlink/../worktrees"));
    }

    #[test]
    fn test_normalize_separators() {
        let paths_and_normalized_unix = [
            ("", ""),
            ("/", "/"),
            ("a//b/", "a/b"),
            ("./a/./b", "./a/b"),
            ("a/../b", "a/../b"),
            (r"a\b", r"a\b"),
        ];
        for (path, normalized) in paths_and_normalized_unix {
            assert_eq!(
                crate::normalize_separators(path).as_os_str(),
                OsStr::new(normalized),
                "normalize_separators({path:?})"
            );
        }
    }

    #[test]
    fn test_collapse_dot_segments() {
        let paths_and_normalized_unix = [
            ("", ""),
            ("/", "/"),
            ("/a/b/c", "/a/b/c"),
            ("/a/b/c/.././d", "/a/b/d"),
            ("/..", "/"),
            ("/../a/b", "/a/b"),
            ("./a", "a"),
            ("a//b/", "a/b"),
            ("a/..", "."),
            ("../..", "../.."),
            ("a/../../b", "../b"),
            ("a/..b/...", "a/..b/..."),
            (r"a\..\b", r"a\..\b"),
        ];
        for (path, normalized) in paths_and_normalized_unix {
            assert_eq!(
                crate::collapse_dot_segments(path).as_os_str(),
                OsStr::new(normalized),
                "collapse_dot_segments({path:?})"
            );
        }
    }
}
