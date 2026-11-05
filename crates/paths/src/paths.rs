//! Paths to locations used by Oxed.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};

pub use util::paths::home_dir;
use util::rel_path::RelPath;

/// A default editorconfig file name to use when resolving project settings.
pub const EDITORCONFIG_NAME: &str = ".editorconfig";

/// The application name, used to derive platform-specific data, config, cache,
/// and state directory paths.
///
/// Forks should change this to avoid colliding with Zed's user data.
pub const APP_NAME: &str = "Oxed";

/// Lowercased form of [`APP_NAME`], for use in XDG-style paths.
pub const APP_NAME_LOWERCASE: &str = {
    assert!(!APP_NAME.is_empty(), "APP_NAME must not be empty");
    assert!(APP_NAME.as_bytes().is_ascii(), "APP_NAME must be ASCII");
    const BYTES: [u8; APP_NAME.len()] = {
        let mut bytes = [0u8; APP_NAME.len()];
        let mut i = 0;
        while i < APP_NAME.len() {
            assert!(
                APP_NAME.as_bytes()[i] != b'/' && APP_NAME.as_bytes()[i] != b'\\',
                "APP_NAME must not contain path separators",
            );
            assert!(
                APP_NAME.as_bytes()[i] >= 0x20,
                "APP_NAME must not contain control characters"
            );
            bytes[i] = APP_NAME.as_bytes()[i];
            i += 1;
        }
        bytes.make_ascii_lowercase();
        bytes
    };
    match std::str::from_utf8(&BYTES) {
        Ok(s) => s,
        Err(_) => unreachable!(),
    }
};

/// A custom data directory override, set only by `set_custom_data_dir`.
/// This is used to override the default data directory location.
/// The directory will be created if it doesn't exist when set.
static CUSTOM_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The resolved data directory, combining custom override or platform defaults.
/// This is set once and cached for subsequent calls.
static CURRENT_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The resolved config directory, combining custom override or platform defaults.
/// This is set once and cached for subsequent calls.
static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Sets a custom directory for all user data, overriding the default data directory.
/// This function must be called before any other path operations that depend on the data directory.
/// The directory's path will be canonicalized to an absolute path by a blocking FS operation.
/// The directory will be created if it doesn't exist.
///
/// # Arguments
///
/// * `dir` - The path to use as the custom data directory. This will be used as the base
///   directory for all user data, including databases, extensions, and logs.
///
/// # Returns
///
/// A reference to the static `PathBuf` containing the custom data directory path.
///
/// # Panics
///
/// Panics if:
/// * Called after the data directory has been initialized (e.g., via `data_dir` or `config_dir`)
/// * The directory's path cannot be canonicalized to an absolute path
/// * The directory cannot be created
pub fn set_custom_data_dir(dir: &str) -> &'static PathBuf {
    if CURRENT_DATA_DIR.get().is_some() || CONFIG_DIR.get().is_some() {
        panic!("set_custom_data_dir called after data_dir or config_dir was initialized");
    }
    CUSTOM_DATA_DIR.get_or_init(|| {
        let path = PathBuf::from(dir);
        std::fs::create_dir_all(&path).expect("failed to create custom data directory");
        let canonicalized = path
            .canonicalize()
            .expect("failed to canonicalize custom data directory's path to an absolute path");
        canonicalized
    })
}

/// Returns the path to the configuration directory used by Oxed.
pub fn config_dir() -> &'static PathBuf {
    CONFIG_DIR.get_or_init(|| {
        if let Some(custom_dir) = CUSTOM_DATA_DIR.get() {
            custom_dir.join("config")
        } else {
            dirs::config_dir()
                .expect("failed to determine XDG_CONFIG_HOME directory")
                .join(APP_NAME_LOWERCASE)
        }
    })
}

/// Returns the path to the data directory used by Oxed.
pub fn data_dir() -> &'static PathBuf {
    CURRENT_DATA_DIR.get_or_init(|| {
        if let Some(custom_dir) = CUSTOM_DATA_DIR.get() {
            custom_dir.clone()
        } else {
            dirs::data_local_dir()
                .expect("failed to determine XDG_DATA_HOME directory")
                .join(APP_NAME_LOWERCASE)
        }
    })
}

/// Returns the path to the temp directory used by Oxed.
pub fn temp_dir() -> &'static PathBuf {
    static TEMP_DIR: OnceLock<PathBuf> = OnceLock::new();
    TEMP_DIR.get_or_init(|| {
        dirs::cache_dir()
            .expect("failed to determine XDG_CACHE_HOME directory")
            .join(APP_NAME_LOWERCASE)
    })
}

/// Returns the path to the logs directory.
pub fn logs_dir() -> &'static PathBuf {
    static LOGS_DIR: OnceLock<PathBuf> = OnceLock::new();
    LOGS_DIR.get_or_init(|| data_dir().join("logs"))
}

/// Returns the path to the `Oxed.log` file.
pub fn log_file() -> &'static PathBuf {
    static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();
    LOG_FILE.get_or_init(|| logs_dir().join(format!("{}.log", APP_NAME)))
}

/// Returns the path to the `Oxed.log.old` file.
pub fn old_log_file() -> &'static PathBuf {
    static OLD_LOG_FILE: OnceLock<PathBuf> = OnceLock::new();
    OLD_LOG_FILE.get_or_init(|| logs_dir().join(format!("{}.log.old", APP_NAME)))
}

/// Returns the path to the database directory.
pub fn database_dir() -> &'static PathBuf {
    static DATABASE_DIR: OnceLock<PathBuf> = OnceLock::new();
    DATABASE_DIR.get_or_init(|| data_dir().join("db"))
}

/// Returns the path to the `settings.json` file.
pub fn settings_file() -> &'static PathBuf {
    static SETTINGS_FILE: OnceLock<PathBuf> = OnceLock::new();
    SETTINGS_FILE.get_or_init(|| config_dir().join("settings.json"))
}

/// Returns the path to the global settings file.
pub fn global_settings_file() -> &'static PathBuf {
    static GLOBAL_SETTINGS_FILE: OnceLock<PathBuf> = OnceLock::new();
    GLOBAL_SETTINGS_FILE.get_or_init(|| config_dir().join("global_settings.json"))
}

/// Returns the path to the `keymap.json` file.
pub fn keymap_file() -> &'static PathBuf {
    static KEYMAP_FILE: OnceLock<PathBuf> = OnceLock::new();
    KEYMAP_FILE.get_or_init(|| config_dir().join("keymap.json"))
}

/// Returns the path to the extensions directory.
///
/// This is where installed extensions are stored.
pub fn extensions_dir() -> &'static PathBuf {
    static EXTENSIONS_DIR: OnceLock<PathBuf> = OnceLock::new();
    EXTENSIONS_DIR.get_or_init(|| data_dir().join("extensions"))
}

/// Returns the path to the themes directory.
///
/// This is where themes that are not provided by extensions are stored.
pub fn themes_dir() -> &'static PathBuf {
    static THEMES_DIR: OnceLock<PathBuf> = OnceLock::new();
    THEMES_DIR.get_or_init(|| config_dir().join("themes"))
}

/// Returns the path to the snippets directory.
pub fn snippets_dir() -> &'static PathBuf {
    static SNIPPETS_DIR: OnceLock<PathBuf> = OnceLock::new();
    SNIPPETS_DIR.get_or_init(|| config_dir().join("snippets"))
}

/// Returns the path to the languages directory.
///
/// This is where language servers are downloaded to for languages built-in to Oxed.
pub fn languages_dir() -> &'static PathBuf {
    static LANGUAGES_DIR: OnceLock<PathBuf> = OnceLock::new();
    LANGUAGES_DIR.get_or_init(|| data_dir().join("languages"))
}

/// Returns the path to the default Prettier directory.
pub fn default_prettier_dir() -> &'static PathBuf {
    static DEFAULT_PRETTIER_DIR: OnceLock<PathBuf> = OnceLock::new();
    DEFAULT_PRETTIER_DIR.get_or_init(|| data_dir().join("prettier"))
}

/// Returns the relative path to a `.oxed` folder within a project.
pub fn local_settings_folder_name() -> &'static str {
    ".oxed"
}

/// Returns the relative path to a `.vscode` folder within a project.
pub fn local_vscode_folder_name() -> &'static str {
    ".vscode"
}

/// Returns the relative path to a `settings.json` file within a project.
pub fn local_settings_file_relative_path() -> &'static RelPath {
    static CACHED: LazyLock<&'static RelPath> =
        LazyLock::new(|| RelPath::from_unix_str(".oxed/settings.json").unwrap());
    *CACHED
}

/// Returns candidate paths for the vscode user settings file
pub fn vscode_settings_file_paths() -> Vec<PathBuf> {
    let mut paths = vscode_user_data_paths();
    for path in paths.iter_mut() {
        path.push("User/settings.json");
    }
    paths
}

/// Returns candidate paths for the cursor user settings file
pub fn cursor_settings_file_paths() -> Vec<PathBuf> {
    let mut paths = cursor_user_data_paths();
    for path in paths.iter_mut() {
        path.push("User/settings.json");
    }
    paths
}

fn vscode_user_data_paths() -> Vec<PathBuf> {
    // https://github.com/microsoft/vscode/blob/23e7148cdb6d8a27f0109ff77e5b1e019f8da051/src/vs/platform/environment/node/userDataPath.ts#L45
    const VSCODE_PRODUCT_NAMES: &[&str] = &[
        "Code",
        "Code - Insiders",
        "Code - OSS",
        "VSCodium",
        "VSCodium - Insiders",
        "Code Dev",
        "Code - OSS Dev",
        "code-oss-dev",
    ];
    let mut paths = Vec::new();
    if let Ok(portable_path) = env::var("VSCODE_PORTABLE") {
        paths.push(Path::new(&portable_path).join("user-data"));
    }
    if let Ok(vscode_appdata) = env::var("VSCODE_APPDATA") {
        for product_name in VSCODE_PRODUCT_NAMES {
            paths.push(Path::new(&vscode_appdata).join(product_name));
        }
    }
    for product_name in VSCODE_PRODUCT_NAMES {
        add_vscode_user_data_paths(&mut paths, product_name);
    }
    paths
}

fn cursor_user_data_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    add_vscode_user_data_paths(&mut paths, "Cursor");
    paths
}

fn add_vscode_user_data_paths(paths: &mut Vec<PathBuf>, product_name: &str) {
    paths.push(
        dirs::config_dir()
            .unwrap_or(home_dir().join(".config"))
            .join(product_name),
    );
}

#[cfg(any(test, feature = "test-support"))]
pub fn global_gitignore_path() -> Option<PathBuf> {
    Some(home_dir().join(".config").join("git").join("ignore"))
}

#[cfg(not(any(test, feature = "test-support")))]
pub fn global_gitignore_path() -> Option<PathBuf> {
    static GLOBAL_GITIGNORE_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    GLOBAL_GITIGNORE_PATH
        .get_or_init(::ignore::gitignore::gitconfig_excludes_path)
        .clone()
}
