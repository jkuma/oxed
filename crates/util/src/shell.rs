use serde::{Deserialize, Serialize};
use std::{borrow::Cow, fmt, path::Path};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ShellKind {
    #[default]
    Posix,
    Csh,
    Tcsh,
    Rc,
    Fish,
    /// PowerShell 7.x
    Pwsh,
    Nushell,
    Xonsh,
    Elvish,
}

pub fn get_system_shell() -> String {
    std::env::var("SHELL").unwrap_or("/bin/sh".to_string())
}

impl fmt::Display for ShellKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShellKind::Posix => write!(f, "sh"),
            ShellKind::Csh => write!(f, "csh"),
            ShellKind::Tcsh => write!(f, "tcsh"),
            ShellKind::Fish => write!(f, "fish"),
            ShellKind::Pwsh => write!(f, "pwsh"),
            ShellKind::Nushell => write!(f, "nu"),
            ShellKind::Rc => write!(f, "rc"),
            ShellKind::Xonsh => write!(f, "xonsh"),
            ShellKind::Elvish => write!(f, "elvish"),
        }
    }
}

impl ShellKind {
    pub fn new(program: impl AsRef<Path>) -> Self {
        let program = program.as_ref();
        let program = program
            .file_stem()
            .unwrap_or_else(|| program.as_os_str())
            .to_string_lossy();

        match &*program {
            "pwsh" => ShellKind::Pwsh,
            "nu" => ShellKind::Nushell,
            "fish" => ShellKind::Fish,
            "csh" => ShellKind::Csh,
            "tcsh" => ShellKind::Tcsh,
            "rc" => ShellKind::Rc,
            "xonsh" => ShellKind::Xonsh,
            "elvish" => ShellKind::Elvish,
            "sh" | "bash" | "zsh" => ShellKind::Posix,
            // Some other shell detected, the user might install and use a
            // unix-like shell.
            _ => ShellKind::Posix,
        }
    }

    pub const fn command_prefix(&self) -> Option<char> {
        match self {
            ShellKind::Pwsh => Some('&'),
            ShellKind::Nushell => Some('^'),
            ShellKind::Posix
            | ShellKind::Csh
            | ShellKind::Tcsh
            | ShellKind::Rc
            | ShellKind::Fish
            | ShellKind::Xonsh
            | ShellKind::Elvish => None,
        }
    }

    pub fn prepend_command_prefix<'a>(&self, command: &'a str) -> Cow<'a, str> {
        match self.command_prefix() {
            Some(prefix) if !command.starts_with(prefix) => {
                Cow::Owned(format!("{prefix}{command}"))
            }
            _ => Cow::Borrowed(command),
        }
    }

    pub fn try_quote<'a>(&self, arg: &'a str) -> Option<Cow<'a, str>> {
        match self {
            ShellKind::Pwsh => Some(Self::quote_pwsh(arg)),
            ShellKind::Csh | ShellKind::Tcsh => Self::quote_csh(arg),
            ShellKind::Nushell => Self::quote_nushell(arg),
            ShellKind::Posix
            | ShellKind::Rc
            | ShellKind::Fish
            | ShellKind::Xonsh
            | ShellKind::Elvish => shlex::try_quote(arg).ok(),
        }
    }

    fn needs_quoting_powershell(s: &str) -> bool {
        s.is_empty()
            || s.chars().any(|c| {
                c.is_whitespace()
                    || matches!(
                        c,
                        '"' | '`'
                            | '$'
                            | '&'
                            | '|'
                            | '<'
                            | '>'
                            | ';'
                            | '('
                            | ')'
                            | '['
                            | ']'
                            | '{'
                            | '}'
                            | ','
                            | '\''
                            | '@'
                    )
            })
    }

    fn escape_powershell_quotes(s: &str) -> String {
        let mut result = String::with_capacity(s.len() + 4);
        result.push('\'');
        for c in s.chars() {
            if c == '\'' {
                result.push('\'');
            }
            result.push(c);
        }
        result.push('\'');
        result
    }

    pub fn quote_pwsh(arg: &str) -> Cow<'_, str> {
        if arg.is_empty() {
            return Cow::Borrowed("''");
        }

        if !Self::needs_quoting_powershell(arg) {
            return Cow::Borrowed(arg);
        }

        Cow::Owned(Self::escape_powershell_quotes(arg))
    }

    pub fn quote_csh(arg: &str) -> Option<Cow<'_, str>> {
        if arg.contains('\0') {
            return None;
        }
        if Self::is_unquoted_safe(arg) && !arg.starts_with('%') {
            return Some(Cow::Borrowed(arg));
        }
        if arg.is_empty() {
            return Some(Cow::Borrowed("''"));
        }
        let mut quoted = String::with_capacity(arg.len() + 2);
        let mut in_quotes = false;
        for character in arg.chars() {
            match character {
                '\'' | '!' | '\\' => {
                    if in_quotes {
                        quoted.push('\'');
                        in_quotes = false;
                    }
                    quoted.push('\\');
                    quoted.push(character);
                }
                character => {
                    if !in_quotes {
                        quoted.push('\'');
                        in_quotes = true;
                    }
                    if character == '\n' {
                        quoted.push('\\');
                    }
                    quoted.push(character);
                }
            }
        }
        if in_quotes {
            quoted.push('\'');
        }
        Some(Cow::Owned(quoted))
    }

    pub fn quote_nushell(arg: &str) -> Option<Cow<'_, str>> {
        if arg.contains('\0') {
            return None;
        }
        if Self::is_unquoted_safe(arg) {
            return Some(Cow::Borrowed(arg));
        }
        if !arg.contains('\'') {
            return Some(Cow::Owned(format!("'{arg}'")));
        }
        let mut quoted = String::with_capacity(arg.len() + 2);
        quoted.push('"');
        for character in arg.chars() {
            if character == '\\' || character == '"' {
                quoted.push('\\');
            }
            quoted.push(character);
        }
        quoted.push('"');
        Some(Cow::Owned(quoted))
    }

    fn is_unquoted_safe(arg: &str) -> bool {
        !arg.is_empty()
            && arg
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_./:,+@%".contains(&byte))
    }

    /// Quotes the given argument if necessary, taking into account the command prefix.
    ///
    /// In other words, this will consider quoting arg without its command prefix to not break the command.
    /// You should use this over `try_quote` when you want to quote a shell command.
    pub fn try_quote_prefix_aware<'a>(&self, arg: &'a str) -> Option<Cow<'a, str>> {
        if let Some(char) = self.command_prefix() {
            if let Some(arg) = arg.strip_prefix(char) {
                // we have a command that is prefixed
                for quote in ['\'', '"'] {
                    if let Some(arg) = arg
                        .strip_prefix(quote)
                        .and_then(|arg| arg.strip_suffix(quote))
                    {
                        // and the command itself is wrapped as a literal, that
                        // means the prefix exists to interpret a literal as a
                        // command. So strip the quotes, quote the command, and
                        // re-add the quotes if they are missing after requoting
                        let quoted = self.try_quote(arg)?;
                        return Some(if quoted.starts_with(['\'', '"']) {
                            Cow::Owned(self.prepend_command_prefix(&quoted).into_owned())
                        } else {
                            Cow::Owned(
                                self.prepend_command_prefix(&format!("{quote}{quoted}{quote}"))
                                    .into_owned(),
                            )
                        });
                    }
                }
                return self
                    .try_quote(arg)
                    .map(|quoted| Cow::Owned(self.prepend_command_prefix(&quoted).into_owned()));
            }
        }
        self.try_quote(arg).map(|quoted| match quoted {
            unquoted @ Cow::Borrowed(_) => unquoted,
            Cow::Owned(quoted) => Cow::Owned(self.prepend_command_prefix(&quoted).into_owned()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Examples
    // WSL
    // wsl.exe --distribution NixOS --cd /home/user -- /usr/bin/zsh -c "echo hello"
    // wsl.exe --distribution NixOS --cd /home/user -- /usr/bin/zsh -c "\"echo hello\"" | grep hello"
    // wsl.exe --distribution NixOS --cd ~ env RUST_LOG=info,remote=debug .zed_wsl_server/zed-remote-server-dev-build proxy --identifier dev-workspace-53
    // PowerShell from Nushell
    // nu -c overlay use "C:\Users\kubko\dev\python\39007\tests\.venv\Scripts\activate.nu"; ^"C:\Program Files\PowerShell\7\pwsh.exe" -C "C:\Users\kubko\dev\python\39007\tests\.venv\Scripts\python.exe -m pytest \"test_foo.py::test_foo\""
    // PowerShell from CMD
    // cmd /C \" \"C:\\\\Users\\\\kubko\\\\dev\\\\python\\\\39007\\\\tests\\\\.venv\\\\Scripts\\\\activate.bat\"& \"C:\\\\Program Files\\\\PowerShell\\\\7\\\\pwsh.exe\" -C \"C:\\\\Users\\\\kubko\\\\dev\\\\python\\\\39007\\\\tests\\\\.venv\\\\Scripts\\\\python.exe -m pytest \\\"test_foo.py::test_foo\\\"\"\"

    #[test]
    fn test_try_quote_nu_command() {
        let shell_kind = ShellKind::Nushell;
        assert_eq!(
            shell_kind.try_quote("'uname'").unwrap().into_owned(),
            "\"'uname'\"".to_string()
        );
        assert_eq!(
            shell_kind
                .try_quote_prefix_aware("'uname'")
                .unwrap()
                .into_owned(),
            "^\"'uname'\"".to_string()
        );
        assert_eq!(
            shell_kind.try_quote("^uname").unwrap().into_owned(),
            "'^uname'".to_string()
        );
        assert_eq!(
            shell_kind
                .try_quote_prefix_aware("^uname")
                .unwrap()
                .into_owned(),
            "^uname".to_string()
        );
        assert_eq!(
            shell_kind.try_quote("^'uname'").unwrap().into_owned(),
            "\"^'uname'\"".to_string()
        );
        assert_eq!(
            shell_kind
                .try_quote_prefix_aware("^'uname'")
                .unwrap()
                .into_owned(),
            "^'uname'".to_string()
        );
        assert_eq!(
            shell_kind.try_quote("'uname a'").unwrap().into_owned(),
            "\"'uname a'\"".to_string()
        );
        assert_eq!(
            shell_kind
                .try_quote_prefix_aware("'uname a'")
                .unwrap()
                .into_owned(),
            "^\"'uname a'\"".to_string()
        );
        assert_eq!(
            shell_kind.try_quote("^'uname a'").unwrap().into_owned(),
            "\"^'uname a'\"".to_string()
        );
        assert_eq!(
            shell_kind
                .try_quote_prefix_aware("^'uname a'")
                .unwrap()
                .into_owned(),
            "^'uname a'".to_string()
        );
        assert_eq!(
            shell_kind.try_quote("uname").unwrap().into_owned(),
            "uname".to_string()
        );
        assert_eq!(
            shell_kind
                .try_quote_prefix_aware("uname")
                .unwrap()
                .into_owned(),
            "uname".to_string()
        );
    }

    #[test]
    fn test_try_quote_csh() {
        for shell_kind in [ShellKind::Csh, ShellKind::Tcsh] {
            assert_eq!(shell_kind.try_quote("plain").unwrap(), "plain");
            assert_eq!(
                shell_kind.try_quote("--flag=a/b.c").unwrap(),
                "'--flag=a/b.c'"
            );
            assert_eq!(shell_kind.try_quote("=0").unwrap(), "'=0'");
            assert_eq!(shell_kind.try_quote("%agent").unwrap(), "'%agent'");
            assert_eq!(shell_kind.try_quote("50%").unwrap(), "50%");
            assert_eq!(shell_kind.try_quote("").unwrap(), "''");
            assert_eq!(
                shell_kind.try_quote("/home/bang!home/x").unwrap(),
                "'/home/bang'\\!'home/x'"
            );
            assert_eq!(shell_kind.try_quote("a\\!b c").unwrap(), "'a'\\\\\\!'b c'");
            assert_eq!(
                shell_kind.try_quote("O'Brien!").unwrap(),
                "'O'\\''Brien'\\!"
            );
            assert_eq!(shell_kind.try_quote("!").unwrap(), "\\!");
            assert_eq!(shell_kind.try_quote("\\'").unwrap(), "\\\\\\'");
            assert_eq!(
                shell_kind
                    .try_quote("/tmp/a\\'; echo INJECTED; true \\'")
                    .unwrap(),
                "'/tmp/a'\\\\\\''; echo INJECTED; true '\\\\\\'"
            );
            assert_eq!(
                shell_kind.try_quote("say \\\"hi\\\"").unwrap(),
                "'say '\\\\'\"hi'\\\\'\"'"
            );
            assert_eq!(
                shell_kind.try_quote("line\nbreak").unwrap(),
                "'line\\\nbreak'"
            );
            assert_eq!(
                shell_kind.try_quote("line\rbreak").unwrap(),
                "'line\rbreak'"
            );
            assert_eq!(
                shell_kind.try_quote("~/*$HOME `id`").unwrap(),
                "'~/*$HOME `id`'"
            );
            assert_eq!(shell_kind.try_quote("nul\0byte"), None);
        }
    }

    #[test]
    fn test_try_quote_nushell() {
        let shell_kind = ShellKind::Nushell;
        assert_eq!(shell_kind.try_quote("plain").unwrap(), "plain");
        assert_eq!(shell_kind.try_quote("").unwrap(), "''");
        assert_eq!(shell_kind.try_quote("=").unwrap(), "'='");
        assert_eq!(shell_kind.try_quote("=a").unwrap(), "'=a'");
        assert_eq!(shell_kind.try_quote("+=").unwrap(), "'+='");
        assert_eq!(
            shell_kind.try_quote("--flag=a/b.c").unwrap(),
            "'--flag=a/b.c'"
        );
        assert_eq!(
            shell_kind
                .try_quote("/home/a^b/.zed_server/server")
                .unwrap(),
            "'/home/a^b/.zed_server/server'"
        );
        assert_eq!(
            shell_kind.try_quote("$env.HOME (1 + 1) `id` a\\b").unwrap(),
            "'$env.HOME (1 + 1) `id` a\\b'"
        );
        assert_eq!(
            shell_kind.try_quote("O'Brien \\ \"x\" $y").unwrap(),
            "\"O'Brien \\\\ \\\"x\\\" $y\""
        );
        assert_eq!(shell_kind.try_quote("nul\0byte"), None);
    }

    #[test]
    fn test_quoted_arguments_survive_real_shells() {
        let arguments = [
            "SAFE; /usr/bin/printf INJECTED\r",
            "/tmp/a\\'; /usr/bin/printf INJECTED; true \\'",
            "a\\\\b \\\"c\\\" \\'d\\' \\$e \\!f",
            "line\nbreak",
            "bang!\nnew\\line",
            "O'Brien!",
            "a\\!b",
            "a\\\\!b",
            "!1x",
            "!",
            "!!",
            "!$ !* !:1 !-1 !# !?x?",
            "ending!",
            "trailing\\",
            "~/*$HOME `id` $(id) (1 + 1) \"quoted\"",
            "\t tab # hash",
            "=",
            "=a",
            "=0",
            "=1",
            "+=",
            "-=",
            "/=",
            "a=b",
            "--flag=a/b.c",
            "1..3",
            "/home/a^b/.zed_server/server",
            "钥匙🔑",
            "",
        ];
        let csh_settings = [
            "",
            "set backslash_quote\n",
            "set histchars = ''\n",
            "set histchars = \\!^\n",
            "set backslash_quote\nset histchars = ''\n",
            "set backslash_quote\nunset histchars\nset history = 100\n",
        ];
        let shells = [
            (
                "tcsh",
                ShellKind::Tcsh,
                "&&",
                &["-f"][..],
                &csh_settings[..],
            ),
            ("csh", ShellKind::Csh, "&&", &["-f"][..], &csh_settings[..]),
            (
                "nu",
                ShellKind::Nushell,
                ";",
                &["--no-config-file"][..],
                &[""][..],
            ),
        ];
        for (shell_name, shell_kind, separator, shell_options, settings) in shells {
            let Ok(shell) = which::which(shell_name) else {
                continue;
            };
            let shell = shell.to_string_lossy().into_owned();
            let printf = shell_kind.prepend_command_prefix("/usr/bin/printf");
            for (setting, argument) in settings
                .iter()
                .flat_map(|setting| arguments.iter().map(move |argument| (*setting, *argument)))
            {
                let quoted = shell_kind.try_quote(argument).unwrap();
                let inner = format!("{setting}cd /{separator} {printf} %s {quoted}");
                let mut inner_arguments = shell_options.to_vec();
                inner_arguments.extend(["-c", &inner]);
                let nested = format!(
                    "{setting}{} {}",
                    shell_kind.prepend_command_prefix(&shell),
                    inner_arguments
                        .iter()
                        .map(|inner_argument| shell_kind.try_quote(inner_argument).unwrap())
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                for script in [inner.clone(), nested] {
                    let output = smol::block_on(
                        smol::process::Command::new(&shell)
                            .args(shell_options)
                            .args(["-c", &script])
                            .env_clear()
                            .env("HOME", "/")
                            .env("PATH", "/usr/bin:/bin")
                            .output(),
                    )
                    .unwrap();
                    assert_eq!(
                        (
                            output.status.success(),
                            String::from_utf8_lossy(&output.stdout).into_owned(),
                            String::from_utf8_lossy(&output.stderr).into_owned(),
                        ),
                        (true, argument.to_owned(), String::new()),
                        "{shell} {script:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_try_quote_single_quote_paths() {
        let path_with_quote = r"C:\Temp\O'Brien\repo";
        let shlex_shells = [
            ShellKind::Posix,
            ShellKind::Fish,
            ShellKind::Csh,
            ShellKind::Tcsh,
            ShellKind::Rc,
            ShellKind::Xonsh,
            ShellKind::Elvish,
            ShellKind::Nushell,
        ];

        for shell_kind in shlex_shells {
            let quoted = shell_kind.try_quote(path_with_quote).unwrap().into_owned();
            assert_ne!(quoted, path_with_quote);
            assert_eq!(
                shlex::split(&quoted),
                Some(vec![path_with_quote.to_string()])
            );

            if shell_kind == ShellKind::Nushell {
                let prefixed = shell_kind.prepend_command_prefix(&quoted);
                assert!(prefixed.starts_with('^'));
            }
        }

        let quoted = ShellKind::Pwsh
            .try_quote(path_with_quote)
            .unwrap()
            .into_owned();
        assert!(quoted.starts_with('\''));
        assert!(quoted.ends_with('\''));
        assert!(quoted.contains("O''Brien"));
    }
}
