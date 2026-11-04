mod encrypted_password;

pub use encrypted_password::{EncryptedPassword, IKnowWhatIAmDoingAndIHaveReadTheDocs};

use smol::lock::Mutex;
use smol::net::unix::UnixListener;
use util::fs::make_file_executable;

use std::ffi::OsStr;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use futures::channel::{mpsc, oneshot};
use futures::{
    AsyncBufReadExt as _, AsyncWriteExt as _, FutureExt as _, SinkExt, StreamExt, io::BufReader,
    select_biased,
};
use gpui::{AsyncApp, BackgroundExecutor, Task};
use smol::fs;
use util::{ResultExt as _, maybe};

use util::{paths::PathExt, shell::ShellKind};

#[derive(PartialEq, Eq)]
pub enum AskPassResult {
    CancelledByUser,
    Timedout,
}

type PasswordPrompt = (
    String,
    oneshot::Sender<EncryptedPassword>,
    oneshot::Receiver<()>,
);

pub struct AskPassDelegate {
    tx: mpsc::UnboundedSender<PasswordPrompt>,
    executor: BackgroundExecutor,
    _task: Task<()>,
}

impl AskPassDelegate {
    pub fn new(
        cx: &mut AsyncApp,
        password_prompt: impl Fn(String, oneshot::Sender<EncryptedPassword>, &mut AsyncApp)
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self::new_with_cancellation(cx, move |prompt, response_sender, _cancellation, cx| {
            password_prompt(prompt, response_sender, cx)
        })
    }

    pub fn new_with_cancellation(
        cx: &mut AsyncApp,
        password_prompt: impl Fn(
            String,
            oneshot::Sender<EncryptedPassword>,
            oneshot::Receiver<()>,
            &mut AsyncApp,
        ) + Send
        + Sync
        + 'static,
    ) -> Self {
        let (tx, mut rx) = mpsc::unbounded::<PasswordPrompt>();
        let task = cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some((prompt, response_sender, cancellation)) = rx.next().await {
                password_prompt(prompt, response_sender, cancellation, cx);
            }
        });
        Self {
            tx,
            _task: task,
            executor: cx.background_executor().clone(),
        }
    }

    pub fn ask_password(&self, prompt: String) -> Task<Option<EncryptedPassword>> {
        let mut this_tx = self.tx.clone();
        self.executor.spawn(async move {
            let (response_sender, response_receiver) = oneshot::channel();
            let (cancellation_sender, cancellation_receiver) = oneshot::channel();
            this_tx
                .send((prompt, response_sender, cancellation_receiver))
                .await
                .ok()?;
            let _cancellation_sender = cancellation_sender;
            response_receiver.await.ok()
        })
    }
}

pub struct AskPassSession {
    askpass_task: PasswordProxy,
    askpass_opened_rx: Option<oneshot::Receiver<()>>,
    askpass_kill_master_rx: Option<oneshot::Receiver<()>>,
    executor: BackgroundExecutor,
}

const ASKPASS_SCRIPT_NAME: &str = "askpass.sh";

const GPG_WRAPPER_SCRIPT_NAME: &str = "gpg-wrapper.sh";

impl AskPassSession {
    /// This will create a new AskPassSession.
    /// You must retain this session until the master process exits.
    #[must_use]
    pub async fn new(executor: BackgroundExecutor, delegate: AskPassDelegate) -> Result<Self> {
        let (askpass_opened_tx, askpass_opened_rx) = oneshot::channel::<()>();

        let askpass_opened_tx = Arc::new(Mutex::new(Some(askpass_opened_tx)));

        let (askpass_kill_master_tx, askpass_kill_master_rx) = oneshot::channel::<()>();
        let kill_tx = Arc::new(Mutex::new(Some(askpass_kill_master_tx)));

        let get_password = {
            let executor = executor.clone();

            move |prompt| {
                let prompt = delegate.ask_password(prompt);
                let kill_tx = kill_tx.clone();
                let askpass_opened_tx = askpass_opened_tx.clone();
                executor.spawn(async move {
                    if let Some(askpass_opened_tx) = askpass_opened_tx.lock().await.take() {
                        askpass_opened_tx.send(()).ok();
                    }
                    if let Some(password) = prompt.await {
                        ControlFlow::Continue(Ok(password))
                    } else {
                        if let Some(kill_tx) = kill_tx.lock().await.take() {
                            kill_tx.send(()).ok();
                        }
                        ControlFlow::Break(())
                    }
                })
            }
        };
        let askpass_task = PasswordProxy::new(Box::new(get_password), executor.clone()).await?;

        Ok(Self {
            askpass_task,
            askpass_kill_master_rx: Some(askpass_kill_master_rx),
            askpass_opened_rx: Some(askpass_opened_rx),
            executor,
        })
    }

    // This will run the askpass task forever, resolving as many authentication requests as needed.
    // The caller is responsible for examining the result of their own commands and cancelling this
    // future when this is no longer needed. Note that this can only be called once, but due to the
    // drop order this takes an &mut, so you can `drop()` it after you're done with the master process.
    //
    // When `timeout` is provided, this resolves with `AskPassResult::Timedout` if no askpass prompt
    // has been opened within that duration. This is intended for connection establishment (e.g.
    // SSH), where "no prompt and no connection" indicates an unreachable host. Callers wrapping
    // commands that may legitimately run for a long time without prompting (e.g. git) should pass
    // `None` and rely on the command's own completion instead.
    pub async fn run(&mut self, timeout: Option<Duration>) -> AskPassResult {
        let askpass_opened_rx = self.askpass_opened_rx.take().expect("Only call run once");
        let askpass_kill_master_rx = self
            .askpass_kill_master_rx
            .take()
            .expect("Only call run once");
        let executor = self.executor.clone();
        let timer = async move {
            match timeout {
                Some(timeout) => executor.timer(timeout).await,
                None => std::future::pending().await,
            }
        };

        select_biased! {
            _ = askpass_opened_rx.fuse() => {
                // Note: this await can only resolve after we are dropped.
                askpass_kill_master_rx.await.ok();
                AskPassResult::CancelledByUser
            }

            _ = futures::FutureExt::fuse(timer) => {
                AskPassResult::Timedout
            }
        }
    }

    /// Returns the value to set as SSH_ASKPASS: the path to the generated shell script.
    pub fn script_path(&self) -> impl AsRef<OsStr> {
        self.askpass_task.script_path()
    }

    /// Path to a script suitable for git's `gpg.program`, routing GnuPG
    /// passphrase prompts through Zed's askpass UI. `None` if unavailable.
    pub fn gpg_wrapper_path(&self) -> Option<&std::path::Path> {
        return self.askpass_task.gpg_wrapper_path();
    }
}

pub struct PasswordProxy {
    _task: Task<()>,
    /// Path to the generated askpass script, set as SSH_ASKPASS.
    askpass_script_path: std::path::PathBuf,
    gpg_wrapper_script_path: Option<std::path::PathBuf>,
}

impl PasswordProxy {
    pub async fn new(
        mut get_password: Box<
            dyn FnMut(String) -> Task<ControlFlow<(), Result<EncryptedPassword>>>
                + 'static
                + Send
                + Sync,
        >,
        executor: BackgroundExecutor,
    ) -> Result<Self> {
        let temp_dir = tempfile::Builder::new().prefix("zed-askpass").tempdir()?;
        let askpass_socket = temp_dir.path().join("askpass.sock");
        let current_exec =
            std::env::current_exe().context("Failed to determine current zed executable path.")?;
        let askpass_program = current_exec.as_path();

        let askpass_script_path = temp_dir.path().join(ASKPASS_SCRIPT_NAME);

        let askpass_socket_path = askpass_socket.clone();

        // Create a gpg wrapper script that routes GnuPG passphrase prompts through
        // the same socket (and thus through Zed's askpass UI), using pinentry loopback mode. We compute the path
        // before the socket task takes ownership of `temp_dir`, and write the file
        // afterwards.
        let (gpg_wrapper_script_path, gpg_wrapper_script) =
            match generate_gpg_wrapper_script(askpass_program, &askpass_socket_path) {
                Ok(script) => (
                    Some(temp_dir.path().join(GPG_WRAPPER_SCRIPT_NAME)),
                    Some(script),
                ),
                Err(err) => {
                    log::warn!("could not create gpg askpass wrapper: {err:#}");
                    (None, None)
                }
            };

        let _task = executor.spawn(async move {
            maybe!(async move {
                let listener =
                    UnixListener::bind(&askpass_socket).context("creating askpass socket")?;

                while let Ok((mut stream, _)) = listener.accept().await {
                    let mut buffer = Vec::new();
                    let mut reader = BufReader::new(&mut stream);
                    if reader.read_until(b'\0', &mut buffer).await.is_err() {
                        buffer.clear();
                    }
                    let prompt = String::from_utf8_lossy(&buffer).into_owned();
                    let password = get_password(prompt).await;
                    match password {
                        ControlFlow::Continue(password) => {
                            if let Ok(password) = password
                                && let Ok(decrypted) =
                                    password.decrypt(IKnowWhatIAmDoingAndIHaveReadTheDocs)
                            {
                                stream.write_all(decrypted.as_bytes()).await.log_err();
                            }
                        }
                        ControlFlow::Break(()) => {
                            // note: we expect the caller to drop this task when it's done.
                            // We need to keep the stream open until the caller is done to avoid
                            // spurious errors from ssh.
                            std::future::pending::<()>().await;
                            drop(stream);
                        }
                    }
                }
                drop(temp_dir);
                Result::<_, anyhow::Error>::Ok(())
            })
            .await
            .log_err();
        });

        let askpass_script = generate_askpass_script(askpass_program, &askpass_socket_path)?;
        fs::write(&askpass_script_path, askpass_script)
            .await
            .with_context(|| format!("creating askpass script at {askpass_script_path:?}"))?;
        make_file_executable(&askpass_script_path)
            .await
            .with_context(|| {
                format!("marking askpass script executable at {askpass_script_path:?}")
            })?;

        // Write the gpg wrapper script (computed above) and mark it executable.
        let gpg_wrapper_script_path =
            if let Some((path, script)) = gpg_wrapper_script_path.zip(gpg_wrapper_script) {
                match async {
                    fs::write(&path, script)
                        .await
                        .with_context(|| format!("creating gpg wrapper script at {path:?}"))?;
                    make_file_executable(&path).await.with_context(|| {
                        format!("marking gpg wrapper script executable at {path:?}")
                    })?;
                    anyhow::Ok(())
                }
                .await
                {
                    Ok(()) => Some(path),
                    Err(err) => {
                        log::warn!("could not write gpg askpass wrapper: {err:#}");
                        None
                    }
                }
            } else {
                None
            };

        Ok(Self {
            _task,
            askpass_script_path,
            gpg_wrapper_script_path,
        })
    }

    pub fn script_path(&self) -> impl AsRef<OsStr> {
        &self.askpass_script_path
    }

    pub fn gpg_wrapper_path(&self) -> Option<&std::path::Path> {
        self.gpg_wrapper_script_path.as_deref()
    }
}

/// Runs Zed in netcat mode for use in askpass.
pub fn main(socket: &str) {
    use std::io::{self, Read};
    use std::process::exit;

    let mut buffer = Vec::new();
    if let Err(err) = io::stdin().read_to_end(&mut buffer) {
        eprintln!("Error reading from stdin: {}", err);
        exit(1);
    }

    connect_and_write_prompt(socket, buffer)
}

/// Runs Zed in askpass mode using prompts passed as arguments.
pub fn main_from_args(socket: &str, args: impl IntoIterator<Item = String>) {
    let prompt = args.into_iter().collect::<Vec<_>>().join("\0");
    connect_and_write_prompt(socket, prompt.into_bytes())
}

fn connect_and_write_prompt(socket: &str, mut buffer: Vec<u8>) {
    use std::io::{self, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::process::exit;

    let mut stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("Error connecting to socket {}: {}", socket, err);
            exit(1);
        }
    };

    if buffer.last() != Some(&b'\0') {
        buffer.push(b'\0');
    }

    if let Err(err) = stream.write_all(&buffer) {
        eprintln!("Error writing to socket: {}", err);
        exit(1);
    }

    let mut response = Vec::new();
    if let Err(err) = stream.read_to_end(&mut response) {
        eprintln!("Error reading from socket: {}", err);
        exit(1);
    }

    if let Err(err) = io::stdout().write_all(&response) {
        eprintln!("Error writing to stdout: {}", err);
        exit(1);
    }
}

/// Generates the askpass shell script.
fn generate_askpass_script(
    askpass_program: &std::path::Path,
    askpass_socket: &std::path::Path,
) -> Result<String> {
    let shell_kind = ShellKind::Posix;
    let askpass_program = shell_kind.prepend_command_prefix(
        askpass_program
            .to_str()
            .context("Askpass program is on a non-utf8 path")?,
    );
    let askpass_program = shell_kind
        .try_quote_prefix_aware(&askpass_program)
        .context("Failed to shell-escape Askpass program path")?;
    let askpass_socket = askpass_socket
        .try_shell_safe(shell_kind)
        .context("Failed to shell-escape Askpass socket path")?;
    let print_args = "printf '%s\\0' \"$@\"";
    let shebang = "#!/bin/sh";
    Ok(format!(
        "{shebang}\n{print_args} | {askpass_program} --askpass={askpass_socket} 2> /dev/null \n",
    ))
}

#[inline]
fn generate_gpg_wrapper_script(
    askpass_program: &std::path::Path,
    askpass_socket: &std::path::Path,
) -> Result<String> {
    let shell_kind = ShellKind::Posix;
    let gpg_program = find_gpg_program().context("could not find a gpg binary on PATH")?;
    let gpg_program = gpg_program
        .to_str()
        .context("gpg program is on a non-utf8 path")?;
    let gpg_program = shell_kind
        .try_quote_prefix_aware(gpg_program)
        .context("Failed to shell-escape gpg program path")?;

    let askpass_program = shell_kind.prepend_command_prefix(
        askpass_program
            .to_str()
            .context("Askpass program is on a non-utf8 path")?,
    );
    let askpass_program = shell_kind
        .try_quote_prefix_aware(&askpass_program)
        .context("Failed to shell-escape Askpass program path")?;
    let askpass_socket = askpass_socket
        .try_shell_safe(shell_kind)
        .context("Failed to shell-escape Askpass socket path")?;

    let prompt = shell_kind
        .try_quote_prefix_aware("Enter passphrase for your Git signing key:")
        .context("Failed to shell-escape gpg passphrase prompt")?;

    // The wrapper only intervenes when git asks gpg to *sign* (e.g. `gpg -bsau
    // <key>`); other invocations like `--verify` run unchanged. Signing runs in
    // three stages, most silent first, so every pinentry configuration works
    // without Zed getting in the way:
    //   1. `--pinentry-mode error`: succeeds only via gpg-agent's passphrase
    //      cache or an unprotected key; guaranteed to never prompt anywhere.
    //   2. Default pinentry mode: lets the configured pinentry run. GUI
    //      pinentries (e.g. pinentry-mac reading the macOS Keychain) need no
    //      TTY and can sign silently or show their native dialog, exactly like
    //      terminal git; TTY pinentries fail fast ("Inappropriate ioctl for
    //      device") because git spawns gpg without a TTY.
    //   3. Loopback mode with the passphrase from Zed's askpass modal (fd 3),
    //      for setups where gpg cannot prompt at all.
    //      (e.g. when gpg agent's `pinentry-program` is not configured)
    //
    // git streams the payload on stdin (readable once) and reads the signature
    // from stdout, so we buffer stdin to replay it into each attempt and buffer
    // each attempt's output, forwarding it only if it succeeds. The
    // passphrase goes to fd 3 via a pipe.
    Ok(format!(
        r#"#!/bin/sh
for arg in "$@"; do
    case "$arg" in
        # Long-form signing options.
        --sign|--detach-sign|--clearsign|--clear-sign) is_signing=1 ;;
        # Skip other long options so flags like `--status-fd` don't match below.
        --*) ;;
        # Short-flag clusters containing `s`, e.g. git's `-bsau`.
        -*s*) is_signing=1 ;;
    esac
done

# Not a signing request: run gpg as-is, leaving its prompting untouched.
if [ -z "${{is_signing}}" ]; then
    exec {gpg_program} "$@"
fi

# Signing. Buffer stdin (the payload) and each attempt's output
# so we can retry cleanly on failure without git seeing partial output.
tmpdir=$(mktemp -d) || exit 1
trap 'rm -rf "$tmpdir"' EXIT
payload="$tmpdir/payload"
signature="$tmpdir/signature"
status="$tmpdir/status"
cat > "$payload" || exit 1

# Stage 1: fully silent. Only succeeds if gpg-agent already has the passphrase
# cached or the key is unprotected; `--pinentry-mode error` forbids launching
# any pinentry, so this can never prompt anywhere.
if {gpg_program} --pinentry-mode error "$@" < "$payload" > "$signature" 2> "$status"; then
    cat "$status" >&2
    cat "$signature"
    exit 0
fi

# Stage 2: let gpg launch the configured pinentry, matching terminal git. GUI
# pinentries work without a TTY and may fetch the passphrase from an OS
# keychain silently (or show their native dialog); TTY pinentries fail fast
# with "Inappropriate ioctl for device" since gpg has no TTY here.
if {gpg_program} "$@" < "$payload" > "$signature" 2> "$status"; then
    cat "$status" >&2
    cat "$signature"
    exit 0
fi

# Stage 3: gpg cannot obtain the passphrase on its own. Ask Zed for it, then
# hand it to gpg on fd 3 using loopback mode so no pinentry/terminal is
# required.
passphrase=$(printf '%s\0' {prompt} | {askpass_program} --askpass={askpass_socket} 2>/dev/null)
printf '%s\n' "$passphrase" |
{gpg_program} --pinentry-mode loopback --passphrase-fd 3 "$@" 3<&0 < "$payload"
"#,
    ))
}

/// Finds the real `gpg` (or `gpg2`) executable on `PATH`.
#[inline]
fn find_gpg_program() -> Option<std::path::PathBuf> {
    ["gpg", "gpg2"]
        .into_iter()
        .find_map(|candidate| which::which(candidate).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    async fn dropping_password_request_cancels_prompt(cx: &mut TestAppContext) {
        let (prompt_sender, mut prompt_receiver) = mpsc::unbounded();
        let delegate = AskPassDelegate::new_with_cancellation(
            &mut cx.to_async(),
            move |_, response_sender, cancellation, _| {
                prompt_sender
                    .unbounded_send((response_sender, cancellation))
                    .expect("prompt receiver should remain open");
            },
        );

        let password_request = delegate.ask_password("Password:".to_string());
        let (response_sender, cancellation) = prompt_receiver
            .next()
            .await
            .expect("password prompt should be received");

        drop(password_request);

        assert!(cancellation.await.is_err());
        drop(response_sender);
    }
}
