#![allow(
    clippy::disallowed_methods,
    reason = "We are not in an async environment, so std::process::Command is fine"
)]

mod completions;

use crate::completions::Shell;

use anyhow::{Context as _, Result};
use clap::{CommandFactory, Parser};
use cli::{CliRequest, CliResponse, IpcHandshake, ipc::IpcOneShotServer};
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::Arc,
    thread::{self, JoinHandle},
};
use tempfile::{NamedTempFile, TempDir};
use util::paths::PathWithPosition;
use walkdir::WalkDir;

use std::io::IsTerminal;

const URL_PREFIX: [&'static str; 4] = ["oxed://", "http://", "https://", "file://"];

struct Detect;

trait InstalledApp {
    fn zed_version_string(&self) -> String;
    fn launch(&self, ipc_url: String, user_data_dir: Option<&str>) -> anyhow::Result<()>;
    fn run_foreground(
        &self,
        ipc_url: String,
        user_data_dir: Option<&str>,
    ) -> io::Result<ExitStatus>;
    fn path(&self) -> PathBuf;
}

#[derive(Parser, Debug)]
#[command(
    name = "oxed",
    disable_version_flag = true,
    before_help = "The Oxed CLI binary.
This CLI is a separate binary that invokes Oxed.

Examples:
    `oxed`
          Simply opens Oxed
    `oxed --foreground`
          Runs in foreground (shows all logs)
    `oxed path-to-your-project`
          Open your project in Oxed
    `oxed -n path-to-file `
          Open file/folder in a new window",
    after_help = "To read from stdin, append '-', e.g. 'ps axf | oxed -'"
)]
struct Args {
    /// Wait for all of the given paths to be opened/closed before exiting.
    ///
    /// When opening a directory, waits until the created window is closed.
    #[arg(short, long)]
    wait: bool,
    /// Add files to the currently open workspace
    #[arg(short, long, overrides_with_all = ["new", "reuse", "existing", "classic"])]
    add: bool,
    /// Create a new workspace
    #[arg(short, long, overrides_with_all = ["add", "reuse", "existing", "classic"])]
    new: bool,
    /// Reuse an existing window, replacing its workspace
    #[arg(short, long, overrides_with_all = ["add", "new", "existing", "classic"], hide = true)]
    reuse: bool,
    /// Open in existing Zed window
    #[arg(short = 'e', long = "existing", overrides_with_all = ["add", "new", "reuse", "classic"])]
    existing: bool,
    /// Use the classic open behavior: new window for directories, reuse for files
    #[arg(long, hide = true, overrides_with_all = ["add", "new", "reuse", "existing"])]
    classic: bool,
    /// Sets a custom directory for all user data (e.g., database, extensions, logs).
    /// This overrides the default platform-specific data directory location:
    #[doc = "`$XDG_DATA_HOME/zed`."]
    #[arg(long, value_name = "DIR", value_hint = clap::ValueHint::DirPath)]
    user_data_dir: Option<String>,
    /// The paths to open in Zed (space-separated).
    ///
    /// Use `path:line:column` syntax to open a file at the given line and column.
    #[arg(trailing_var_arg = true, value_hint = clap::ValueHint::AnyPath)]
    paths_with_position: Vec<String>,
    /// Print Zed's version and the app path.
    #[arg(short, long)]
    version: bool,
    /// Run zed in the foreground (useful for debugging)
    #[arg(long)]
    foreground: bool,
    /// Custom path to Zed.app or the zed binary
    #[arg(long)]
    zed: Option<PathBuf>,
    /// Not supported in Zed CLI, only supported on Zed binary
    /// Will attempt to give the correct command to run
    #[arg(long)]
    system_specs: bool,
    /// Pairs of file paths to diff. Can be specified multiple times.
    /// When directories are provided, recurses into them and shows all changed files in a single multi-diff view.
    #[arg(long, action = clap::ArgAction::Append, num_args = 2, value_names = ["OLD_PATH", "NEW_PATH"], value_hint = clap::ValueHint::AnyPath)]
    diff: Vec<String>,
    /// Generate shell completions for Zed
    #[arg(long, value_names = ["SHELL"])]
    completions: Option<Shell>,
    /// Used for SSH/Git password authentication, to remove the need for netcat as a dependency,
    /// by having Zed act like netcat communicating over a Unix socket.
    #[arg(long, hide = true)]
    askpass: Option<String>,
}

/// Parses a path containing a position (e.g. `path:line:column`)
/// and returns its canonicalized string representation.
///
/// If a part of path doesn't exist, it will canonicalize the
/// existing part and append the non-existing part.
///
/// This method must return an absolute path, as many zed
/// crates assume absolute paths.
fn parse_path_with_position(argument_str: &str) -> anyhow::Result<String> {
    match Path::new(argument_str).canonicalize() {
        Ok(existing_path) => Ok(PathWithPosition::from_path(existing_path)),
        Err(_) => PathWithPosition::parse_str(argument_str).map_path(|mut path| {
            let curdir = env::current_dir().context("retrieving current directory")?;
            let mut children = Vec::new();
            let root;
            loop {
                // canonicalize handles './', and '/'.
                if let Ok(canonicalized) = fs::canonicalize(&path) {
                    root = canonicalized;
                    break;
                }
                // The comparison to `curdir` is just a shortcut
                // since we know it is canonical. The other one
                // is if `argument_str` is a string that starts
                // with a name (e.g. "foo/bar").
                if path == curdir || path == Path::new("") {
                    root = curdir;
                    break;
                }
                children.push(
                    path.file_name()
                        .with_context(|| format!("parsing as path with position {argument_str}"))?
                        .to_owned(),
                );
                if !path.pop() {
                    unreachable!("parsing as path with position {argument_str}");
                }
            }
            Ok(children.iter().rev().fold(root, |mut path, child| {
                path.push(child);
                path
            }))
        }),
    }
    .map(|path_with_pos| path_with_pos.to_string(&|path| path.to_string_lossy().into_owned()))
}

/// Returns whether a `--diff` argument refers to an existing path, allowing a
/// trailing `:line:column` suffix (parsed later by the Zed side, matching how
/// regular `zed path:line:column` arguments are handled).
fn diff_path_exists(diff_path: &str) -> bool {
    Path::new(diff_path).exists() || PathWithPosition::parse_str(diff_path).path.exists()
}

fn expand_directory_diff_pairs(
    diff_pairs: Vec<[String; 2]>,
) -> anyhow::Result<(Vec<[String; 2]>, Vec<TempDir>)> {
    let mut expanded = Vec::new();
    let mut temp_dirs = Vec::new();

    for pair in diff_pairs {
        let left = PathBuf::from(&pair[0]);
        let right = PathBuf::from(&pair[1]);

        if left.is_dir() && right.is_dir() {
            let (mut pairs, temp_dir) = expand_directory_pair(&left, &right)?;
            expanded.append(&mut pairs);
            if let Some(temp_dir) = temp_dir {
                temp_dirs.push(temp_dir);
            }
        } else {
            expanded.push(pair);
        }
    }

    Ok((expanded, temp_dirs))
}

fn expand_directory_pair(
    left: &Path,
    right: &Path,
) -> anyhow::Result<(Vec<[String; 2]>, Option<TempDir>)> {
    let left_files = collect_files(left)?;
    let right_files = collect_files(right)?;

    let mut rel_paths = BTreeSet::new();
    rel_paths.extend(left_files.keys().cloned());
    rel_paths.extend(right_files.keys().cloned());

    let mut temp_dir = TempDir::new()?;
    let mut temp_dir_used = false;
    let mut pairs = Vec::new();

    for rel in rel_paths {
        match (left_files.get(&rel), right_files.get(&rel)) {
            (Some(left_path), Some(right_path)) => {
                pairs.push([
                    left_path.to_string_lossy().into_owned(),
                    right_path.to_string_lossy().into_owned(),
                ]);
            }
            (Some(left_path), None) => {
                let stub = create_empty_stub(&mut temp_dir, &rel)?;
                temp_dir_used = true;
                pairs.push([
                    left_path.to_string_lossy().into_owned(),
                    stub.to_string_lossy().into_owned(),
                ]);
            }
            (None, Some(right_path)) => {
                let stub = create_empty_stub(&mut temp_dir, &rel)?;
                temp_dir_used = true;
                pairs.push([
                    stub.to_string_lossy().into_owned(),
                    right_path.to_string_lossy().into_owned(),
                ]);
            }
            (None, None) => {}
        }
    }

    let temp_dir = if temp_dir_used { Some(temp_dir) } else { None };
    Ok((pairs, temp_dir))
}

fn collect_files(root: &Path) -> anyhow::Result<BTreeMap<PathBuf, PathBuf>> {
    let mut files = BTreeMap::new();

    for entry in WalkDir::new(root) {
        let entry = entry?;
        if entry.file_type().is_file() {
            let rel = entry
                .path()
                .strip_prefix(root)
                .context("stripping directory prefix")?
                .to_path_buf();
            files.insert(rel, entry.into_path());
        }
    }

    Ok(files)
}

fn create_empty_stub(temp_dir: &mut TempDir, rel: &Path) -> anyhow::Result<PathBuf> {
    let stub_path = temp_dir.path().join(rel);
    if let Some(parent) = stub_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::File::create(&stub_path)?;
    Ok(stub_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use util::path;
    use util::paths::SanitizedPath;
    use util::test::TempTree;

    macro_rules! assert_path_eq {
        ($left:expr, $right:expr) => {
            assert_eq!(
                SanitizedPath::new(Path::new(&$left)),
                SanitizedPath::new(Path::new(&$right))
            )
        };
    }

    fn cwd() -> PathBuf {
        env::current_dir().unwrap()
    }

    static CWD_LOCK: Mutex<()> = Mutex::new(());

    fn with_cwd<T>(path: &Path, f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
        let _lock = CWD_LOCK.lock();
        let old_cwd = cwd();
        env::set_current_dir(path)?;
        let result = f();
        env::set_current_dir(old_cwd)?;
        result
    }

    #[test]
    fn test_parse_non_existing_path() {
        // Absolute path
        let result = parse_path_with_position(path!("/non/existing/path.txt")).unwrap();
        assert_path_eq!(result, path!("/non/existing/path.txt"));

        // Absolute path in cwd
        let path = cwd().join(path!("non/existing/path.txt"));
        let expected = path.to_string_lossy().to_string();
        let result = parse_path_with_position(&expected).unwrap();
        assert_path_eq!(result, expected);

        // Relative path
        let result = parse_path_with_position(path!("non/existing/path.txt")).unwrap();
        assert_path_eq!(result, expected)
    }

    #[test]
    fn test_parse_existing_path() {
        let temp_tree = TempTree::new(json!({
            "file.txt": "",
        }));
        let file_path = temp_tree.path().join("file.txt");
        let expected = file_path.to_string_lossy().to_string();

        // Absolute path
        let result = parse_path_with_position(file_path.to_str().unwrap()).unwrap();
        assert_path_eq!(result, expected);

        // Relative path
        let result = with_cwd(temp_tree.path(), || parse_path_with_position("file.txt")).unwrap();
        assert_path_eq!(result, expected);
    }

    #[test]
    fn test_parse_symlink_file() {
        let temp_tree = TempTree::new(json!({
            "target.txt": "",
        }));
        let target_path = temp_tree.path().join("target.txt");
        let symlink_path = temp_tree.path().join("symlink.txt");
        std::os::unix::fs::symlink(&target_path, &symlink_path).unwrap();

        // Absolute path
        let result = parse_path_with_position(symlink_path.to_str().unwrap()).unwrap();
        assert_eq!(result, target_path.to_string_lossy());

        // Relative path
        let result =
            with_cwd(temp_tree.path(), || parse_path_with_position("symlink.txt")).unwrap();
        assert_eq!(result, target_path.to_string_lossy());
    }

    #[test]
    fn test_parse_symlink_dir() {
        let temp_tree = TempTree::new(json!({
            "some": {
                "dir": { // symlink target
                    "ec": {
                        "tory": {
                            "file.txt": "",
        }}}}}));

        let target_file_path = temp_tree.path().join("some/dir/ec/tory/file.txt");
        let expected = target_file_path.to_string_lossy();

        let dir_path = temp_tree.path().join("some/dir");
        let symlink_path = temp_tree.path().join("symlink");
        std::os::unix::fs::symlink(&dir_path, &symlink_path).unwrap();

        // Absolute path
        let result =
            parse_path_with_position(symlink_path.join("ec/tory/file.txt").to_str().unwrap())
                .unwrap();
        assert_eq!(result, expected);

        // Relative path
        let result = with_cwd(temp_tree.path(), || {
            parse_path_with_position("symlink/ec/tory/file.txt")
        })
        .unwrap();
        assert_eq!(result, expected);
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    util::prevent_root_execution();

    let args = Args::parse();

    // `zed --askpass` Makes zed operate in nc/netcat mode for use with askpass
    if let Some(socket) = &args.askpass {
        askpass::main(socket);
        return Ok(());
    }

    // Set custom data directory before any path operations
    let user_data_dir = args.user_data_dir.clone();
    if let Some(dir) = &user_data_dir {
        paths::set_custom_data_dir(dir);
    }

    let app = Detect::detect(args.zed.as_deref()).context("Bundle detection")?;

    if let Some(shell) = &args.completions {
        let file_path = std::env::current_exe()?;
        let file_name = file_path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or("--completions expects a UTF-8 name for cli bin")
            .map_err(anyhow::Error::msg)?;
        let mut cmd = Args::command();
        cmd.set_bin_name(file_name);
        cmd.build();
        crate::completions::main(&cmd, shell);
        return Ok(());
    }

    if args.version {
        println!("{}", app.zed_version_string());
        return Ok(());
    }

    if args.system_specs {
        let path = app.path();
        let msg = [
            "The `--system-specs` argument is not supported in the Oxed CLI, only on the Oxed binary.",
            "To retrieve the system specs on the command line, run the following command:",
            &format!("{} --system-specs", path.display()),
        ];
        anyhow::bail!(msg.join("\n"));
    }

    let (server, server_name) =
        IpcOneShotServer::<IpcHandshake>::new().context("Handshake before Oxed spawn")?;
    let url = format!("oxed-cli://{server_name}");

    let open_behavior = if args.new {
        cli::OpenBehavior::AlwaysNew
    } else if args.add {
        cli::OpenBehavior::Add
    } else if args.existing {
        cli::OpenBehavior::ExistingWindow
    } else if args.classic {
        cli::OpenBehavior::Classic
    } else if args.reuse {
        cli::OpenBehavior::Reuse
    } else {
        cli::OpenBehavior::Default
    };

    let env = {
        use collections::HashMap;

        // On Linux, the desktop entry uses `cli` to spawn `zed`.
        // We need to handle env vars correctly since std::env::vars() may not contain
        // project-specific vars (e.g. those set by direnv).
        // By setting env to None here, the LSP will use worktree env vars instead,
        // which is what we want.
        if !std::io::stdout().is_terminal() {
            None
        } else {
            Some(std::env::vars().collect::<HashMap<_, _>>())
        }
    };

    let exit_status = Arc::new(Mutex::new(None));
    let mut paths = vec![];
    let mut urls = vec![];
    let mut diff_paths = vec![];
    let mut stdin_tmp_file: Option<fs::File> = None;
    let mut anonymous_fd_tmp_files = vec![];

    // Check if any diff paths are directories to determine diff_all mode
    let diff_all_mode = args
        .diff
        .chunks(2)
        .any(|pair| Path::new(&pair[0]).is_dir() || Path::new(&pair[1]).is_dir());

    for path in args.diff.chunks(2) {
        let left = parse_path_with_position(&path[0])?;
        let right = parse_path_with_position(&path[1])?;
        for diff_path in [&left, &right] {
            anyhow::ensure!(
                diff_path_exists(diff_path),
                "--diff path does not exist: {diff_path}"
            );
        }
        diff_paths.push([left, right]);
    }

    let (expanded_diff_paths, temp_dirs) = expand_directory_diff_pairs(diff_paths)?;
    diff_paths = expanded_diff_paths;
    // Prevent automatic cleanup of temp directories containing empty stub files
    // for directory diffs. The CLI process may exit before Zed has read these
    // files (e.g., when RPC-ing into an already-running instance). The files
    // live in the OS temp directory and will be cleaned up on reboot.
    for temp_dir in temp_dirs {
        let _ = temp_dir.keep();
    }

    for path in args.paths_with_position.iter() {
        if URL_PREFIX.iter().any(|&prefix| path.starts_with(prefix)) {
            urls.push(path.to_string());
        } else if path == "-" && args.paths_with_position.len() == 1 {
            let file = NamedTempFile::new()?;
            paths.push(file.path().to_string_lossy().into_owned());
            let (file, _) = file.keep()?;
            stdin_tmp_file = Some(file);
        } else if let Some(file) = anonymous_fd(path) {
            let tmp_file = NamedTempFile::new()?;
            paths.push(tmp_file.path().to_string_lossy().into_owned());
            let (tmp_file, _) = tmp_file.keep()?;
            anonymous_fd_tmp_files.push((file, tmp_file));
        } else {
            paths.push(parse_path_with_position(path)?);
        }
    }

    rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .stack_size(10 * 1024 * 1024)
        .thread_name(|ix| format!("RayonWorker{}", ix))
        .build_global()
        .unwrap();

    let sender: JoinHandle<anyhow::Result<()>> = thread::Builder::new()
        .name("CliReceiver".to_string())
        .spawn({
            let exit_status = exit_status.clone();
            let user_data_dir_for_thread = user_data_dir.clone();
            move || {
                let (_, handshake) = server.accept().context("Handshake after Oxed spawn")?;
                let (tx, rx) = (handshake.requests, handshake.responses);

                let open_request = CliRequest::Open {
                    paths,
                    urls,
                    diff_paths,
                    diff_all: diff_all_mode,
                    wait: args.wait,
                    open_behavior,
                    env,
                    user_data_dir: user_data_dir_for_thread,
                    cwd: env::current_dir().ok(),
                };

                tx.send(open_request)?;

                while let Ok(response) = rx.recv() {
                    match response {
                        CliResponse::Ping => {}
                        CliResponse::Stdout { message } => println!("{message}"),
                        CliResponse::Stderr { message } => eprintln!("{message}"),
                        CliResponse::Exit { status } => {
                            exit_status.lock().replace(status);
                            return Ok(());
                        }
                        CliResponse::PromptOpenBehavior => {
                            let behavior = prompt_open_behavior()
                                .unwrap_or(cli::CliBehaviorSetting::ExistingWindow);
                            tx.send(CliRequest::SetOpenBehavior { behavior })?;
                        }
                    }
                }

                Ok(())
            }
        })
        .unwrap();

    let stdin_pipe_handle: Option<JoinHandle<anyhow::Result<()>>> =
        stdin_tmp_file.map(|mut tmp_file| {
            thread::Builder::new()
                .name("CliStdin".to_string())
                .spawn(move || {
                    let mut stdin = std::io::stdin().lock();
                    if !io::IsTerminal::is_terminal(&stdin) {
                        io::copy(&mut stdin, &mut tmp_file)?;
                    }
                    Ok(())
                })
                .unwrap()
        });

    let anonymous_fd_pipe_handles: Vec<_> = anonymous_fd_tmp_files
        .into_iter()
        .map(|(mut file, mut tmp_file)| {
            thread::Builder::new()
                .name("CliAnonymousFd".to_string())
                .spawn(move || io::copy(&mut file, &mut tmp_file))
                .unwrap()
        })
        .collect();

    if args.foreground {
        app.run_foreground(url, user_data_dir.as_deref())?;
    } else {
        app.launch(url, user_data_dir.as_deref())?;
        sender.join().unwrap()?;
        if let Some(handle) = stdin_pipe_handle {
            handle.join().unwrap()?;
        }
        for handle in anonymous_fd_pipe_handles {
            handle.join().unwrap()?;
        }
    }

    if let Some(exit_status) = exit_status.lock().take() {
        std::process::exit(exit_status);
    }
    Ok(())
}

fn anonymous_fd(path: &str) -> Option<fs::File> {
    use std::os::fd::{self, FromRawFd};

    let fd_str = path.strip_prefix("/proc/self/fd/")?;

    let link = fs::read_link(path).ok()?;
    if !link.starts_with("memfd:") {
        return None;
    }

    let fd: fd::RawFd = fd_str.parse().ok()?;
    let file = unsafe { fs::File::from_raw_fd(fd) };
    Some(file)
}

/// Shows an interactive prompt asking the user to choose the default open
/// behavior for `zed <path>`. Returns `None` if the prompt cannot be shown
/// (e.g. stdin is not a terminal) or the user cancels.
fn prompt_open_behavior() -> Option<cli::CliBehaviorSetting> {
    if !std::io::stdin().is_terminal() {
        return None;
    }

    let blue = console::Style::new().blue();
    let items = [
        format!(
            "Add to existing Oxed window ({})",
            blue.apply_to("zed --existing")
        ),
        format!("Open a new window ({})", blue.apply_to("zed --classic")),
    ];

    let prompt = format!(
        "Configure default behavior for {}\n{}",
        blue.apply_to("zed <path>"),
        console::style("You can change this later in Oxed settings"),
    );

    let selection = dialoguer::Select::new()
        .with_prompt(&prompt)
        .items(&items)
        .default(0)
        .interact()
        .ok()?;

    Some(if selection == 0 {
        cli::CliBehaviorSetting::ExistingWindow
    } else {
        cli::CliBehaviorSetting::NewWindow
    })
}

mod linux {
    use std::{
        env,
        ffi::OsString,
        io,
        os::unix::net::UnixDatagram,
        path::{Path, PathBuf},
        process::{self, ExitStatus},
    };

    use anyhow::{Context as _, anyhow};
    use cli::FORCE_CLI_MODE_ENV_VAR_NAME;
    use fork::Fork;

    use crate::{Detect, InstalledApp};

    struct App(PathBuf);

    impl Detect {
        pub fn detect(path: Option<&Path>) -> anyhow::Result<impl InstalledApp> {
            let path = if let Some(path) = path {
                path.to_path_buf().canonicalize()?
            } else {
                let cli = env::current_exe()?;
                let dir = cli.parent().context("no parent path for cli")?;

                // libexec is the standard, lib/oxed is for Arch (and other non-libexec distros),
                // ./oxed is for the target directory in development builds.
                let possible_locations = [
                    "../libexec/oxed-editor",
                    "../lib/oxed/oxed-editor",
                    "./oxed",
                ];
                possible_locations
                    .iter()
                    .find_map(|p| dir.join(p).canonicalize().ok().filter(|path| path != &cli))
                    .with_context(|| {
                        format!("could not find any of: {}", possible_locations.join(", "))
                    })?
            };

            Ok(App(path))
        }
    }

    impl InstalledApp for App {
        fn zed_version_string(&self) -> String {
            format!(
                "Oxed {}{}{} – {}",
                if *release_channel::RELEASE_CHANNEL_NAME == "stable" {
                    "".to_string()
                } else {
                    format!("{} ", *release_channel::RELEASE_CHANNEL_NAME)
                },
                option_env!("RELEASE_VERSION").unwrap_or_default(),
                match option_env!("ZED_COMMIT_SHA") {
                    Some(commit_sha) => format!(" {commit_sha} "),
                    None => "".to_string(),
                },
                self.0.display(),
            )
        }

        fn launch(&self, ipc_url: String, user_data_dir: Option<&str>) -> anyhow::Result<()> {
            let data_dir = user_data_dir
                .map(PathBuf::from)
                .unwrap_or_else(|| paths::data_dir().clone());

            let sock_path = data_dir.join(format!(
                "oxed-{}.sock",
                *release_channel::RELEASE_CHANNEL_NAME
            ));
            let sock = UnixDatagram::unbound()?;
            if sock.connect(&sock_path).is_err() {
                self.boot_background(ipc_url, user_data_dir)?;
            } else {
                sock.send(ipc_url.as_bytes())?;
            }
            Ok(())
        }

        fn run_foreground(
            &self,
            ipc_url: String,
            user_data_dir: Option<&str>,
        ) -> io::Result<ExitStatus> {
            let mut cmd = std::process::Command::new(self.0.clone());
            cmd.arg(ipc_url);
            if let Some(dir) = user_data_dir {
                cmd.arg("--user-data-dir").arg(dir);
            }
            cmd.status()
        }

        fn path(&self) -> PathBuf {
            self.0.clone()
        }
    }

    impl App {
        fn boot_background(
            &self,
            ipc_url: String,
            user_data_dir: Option<&str>,
        ) -> anyhow::Result<()> {
            let path = &self.0;

            match fork::fork() {
                Ok(Fork::Parent(_)) => Ok(()),
                Ok(Fork::Child) => {
                    unsafe { std::env::set_var(FORCE_CLI_MODE_ENV_VAR_NAME, "") };
                    if fork::setsid().is_err() {
                        eprintln!("failed to setsid: {}", std::io::Error::last_os_error());
                        process::exit(1);
                    }
                    if fork::close_fd().is_err() {
                        eprintln!("failed to close_fd: {}", std::io::Error::last_os_error());
                    }
                    let mut args: Vec<OsString> =
                        vec![path.as_os_str().to_owned(), OsString::from(ipc_url)];
                    if let Some(dir) = user_data_dir {
                        args.push(OsString::from("--user-data-dir"));
                        args.push(OsString::from(dir));
                    }
                    let error = exec::execvp(path.clone(), &args);
                    // if exec succeeded, we never get here.
                    eprintln!("failed to exec {:?}: {}", path, error);
                    process::exit(1)
                }
                Err(_) => Err(anyhow!(io::Error::last_os_error())),
            }
        }
    }
}
