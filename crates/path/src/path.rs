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

use crate::rel_path::{RelPath, StripPrefixError};

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
        Ok(RelPath::new(self.as_ref(), PathStyle::local())?.into_owned())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathStyle {
    Unix,
}

impl PathStyle {
    pub const fn local() -> Self {
        PathStyle::Unix
    }

    #[inline]
    pub fn primary_separator(&self) -> &'static str {
        "/"
    }

    pub fn separators(&self) -> &'static [&'static str] {
        &["/"]
    }

    pub fn separators_ch(&self) -> &'static [char] {
        &['/']
    }

    pub fn is_absolute(&self, path: &Path) -> bool {
        path.is_absolute()
    }

    pub fn has_root(&self, path: &Path) -> bool {
        path.has_root()
    }

    pub fn join(self, left: impl AsRef<Path>, right: impl AsRef<Path>) -> Option<String> {
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

    pub fn join_path(
        self,
        left: impl AsRef<Path>,
        right: impl AsRef<Path>,
    ) -> anyhow::Result<PathBuf> {
        let left = left
            .as_ref()
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Path contains invalid UTF-8"))?;
        let right = self.normalize_separators(right);
        let right_string = right
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Path contains invalid UTF-8"))?;
        let joined = self
            .join(left, right_string)
            .ok_or_else(|| anyhow::anyhow!("Path must be relative: {right:?}"))?;
        Ok(self.normalize_lexically(&joined))
    }

    pub fn join_path_preserving_components(
        self,
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
        let joined = self
            .join(left, right_string)
            .ok_or_else(|| anyhow::anyhow!("Path must be relative: {right:?}"))?;
        Ok(PathBuf::from(&joined))
    }

    pub fn normalize_separators(&self, path: impl AsRef<Path>) -> PathBuf {
        path.as_ref().components().collect()
    }

    pub fn normalize_lexically(self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        let mut normalized: Vec<Component<'_>> = Vec::new();

        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => match normalized.last() {
                    Some(Component::Normal(_)) => {
                        normalized.pop();
                    }
                    // There is no parent above a root, so discard the attempt.
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

    pub fn file_name<'a>(&self, path: &'a Path) -> Option<&'a OsStr> {
        path.file_name()
    }

    pub fn parent<'a>(&self, path: &'a Path) -> Option<&'a Path> {
        path.parent()
    }

    pub fn starts_with(&self, path: impl AsRef<Path>, prefix: impl AsRef<Path>) -> bool {
        path.as_ref().starts_with(prefix)
    }

    pub fn strip_prefix<'a>(
        &self,
        path: &'a (impl AsRef<Path> + ?Sized),
        prefix: impl AsRef<Path>,
    ) -> Result<&'a Path, StripPrefixError> {
        path.as_ref()
            .strip_prefix(prefix)
            .map_err(|_| StripPrefixError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_path_uses_path_style_separator() {
        let posix_path = PathStyle::Unix
            .join_path(Path::new("/home/user/dev"), "worktrees")
            .unwrap();

        assert_eq!(posix_path, PathBuf::from("/home/user/dev/worktrees"));
    }

    #[test]
    fn test_join_path_preserving_components() {
        let posix_path = PathStyle::Unix
            .join_path_preserving_components(Path::new("/home/user/symlink"), "../worktrees")
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
                PathStyle::Unix.normalize_separators(path).as_os_str(),
                OsStr::new(normalized),
                "normalize_separators({path:?})"
            );
        }
    }

    #[test]
    fn test_normalize_lexically() {
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
                PathStyle::Unix.normalize_lexically(path).as_os_str(),
                OsStr::new(normalized),
                "normalize_lexically({path:?})"
            );
        }
    }

    #[test]
    fn test_unix_path_style_starts_with_and_strip_prefix() {
        let unix_paths_and_prefixes = [
            ("", "", true, Some("")),
            ("foo", "", true, Some("foo")),
            ("", "foo", false, None),
            ("foo", "foo", true, Some("")),
            ("foo/", "foo", true, Some("")),
            ("foo//", "foo", true, Some("")),
            ("foo///", "foo", true, Some("")),
            ("foo/.", "foo", true, Some("")),
            ("foo/./bar", "foo/bar", true, Some("")),
            ("foo/.//bar", "foo/bar", true, Some("")),
            ("foo//./bar", "foo/bar", true, Some("")),
            ("foo/bar", "foo", true, Some("bar")),
            ("foo/bar", "foobar", false, None),
            ("foo/bar/baz", "foo/bar", true, Some("baz")),
            ("foo/bar", "foo/bar/baz", false, None),
            ("./foo/bar/", ".", true, Some("foo/bar")),
            ("/etc/passwd", "/etc", true, Some("passwd")),
            ("/etc/passwd", "/etc/", true, Some("passwd")),
            ("/etc/passwd", "/etc/passwd", true, Some("")),
            ("/etc/passwd", "/etc/passwd/", true, Some("")),
            ("/etc/passwd", "/etc/passwd///", true, Some("")),
            ("/etc/passwd", "/e", false, None),
            ("/etc/passwd", "/etc/passwd.txt", false, None),
            ("/etc/foo.rs", "/etc/foo", false, None),
            ("/foo/bar/baz", "/", true, Some("foo/bar/baz")),
            ("/foo/bar/baz", "", true, Some("/foo/bar/baz")),
            ("/foo/bar//baz", "/foo/bar/", true, Some("baz")),
            ("/foo/barbaz", "/foo/bar", false, None),
        ];

        for (path, prefix, expected_starts_with, expected_stripped) in unix_paths_and_prefixes {
            assert_eq!(
                PathStyle::Unix.starts_with(Path::new(path), Path::new(prefix)),
                expected_starts_with,
                "{path:?}.starts_with({prefix:?})"
            );
            assert_eq!(
                PathStyle::Unix
                    .strip_prefix(Path::new(path), Path::new(prefix))
                    .ok(),
                expected_stripped.map(Path::new),
                "{path:?}.strip_prefix({prefix:?})"
            );
        }
    }

    #[test]
    fn test_unix_path_style_file_name() {
        let unix_paths_and_filenames = [
            (Path::new(""), None),
            (Path::new("/usr/bin/"), Some("bin")),
            (Path::new("tmp/foo.txt"), Some("foo.txt")),
            (Path::new("."), None),
            (Path::new("./"), None),
            (Path::new("foo.txt/."), Some("foo.txt")),
            (Path::new("foo.txt/.//"), Some("foo.txt")),
            (Path::new("foo/./bar"), Some("bar")),
            (Path::new("foo.txt/.."), None),
            (Path::new("/.."), None),
            (Path::new("/"), None),
        ];

        for (path, filename) in unix_paths_and_filenames {
            assert_eq!(PathStyle::Unix.file_name(path), filename.map(OsStr::new));
        }
    }

    #[test]
    fn test_unix_path_style_parent() {
        let unix_paths_and_parents = [
            ("", None),
            ("/foo/bar", Some("/foo")),
            ("/foo", Some("/")),
            ("/", None),
            ("///foo///", Some("/")),
            ("///foo///bar", Some("///foo")),
            ("foo/bar", Some("foo")),
            ("foo", Some("")),
            ("foo/.", Some("")),
            ("foo/./bar", Some("foo")),
            ("foo/../bar", Some("foo/..")),
            ("./a", Some(".")),
            ("./.", Some("")),
            ("/..", Some("/")),
            (".", Some("")),
            ("..", Some("")),
        ];

        for (path, parent) in unix_paths_and_parents {
            assert_eq!(
                PathStyle::Unix.parent(Path::new(path)),
                parent.map(Path::new)
            );
        }
    }

    #[test]
    fn test_local_path_style_non_utf8() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let path = Path::new(OsStr::from_bytes(b"/home/user/\xff\xfe/repo/file.txt"));
        assert_eq!(
            PathStyle::Unix.file_name(path),
            Some(OsStr::new("file.txt"))
        );
        assert_eq!(
            PathStyle::Unix.parent(path),
            Some(Path::new(OsStr::from_bytes(b"/home/user/\xff\xfe/repo")))
        );

        let another_path = Path::new(OsStr::from_bytes(b"/home/user/repo/\xff\xfe.txt"));
        assert_eq!(
            PathStyle::Unix.file_name(another_path),
            Some(OsStr::from_bytes(b"\xff\xfe.txt"))
        );
        assert_eq!(
            PathStyle::Unix.parent(another_path),
            Some(Path::new("/home/user/repo"))
        );
    }
}
