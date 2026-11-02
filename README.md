# Oxed

Oxed is a minimal, LSP-first code editor written in Rust. It is a fork of [Zed](https://github.com/zed-industries/zed) by Zed Industries, stripped down to what a focused editor needs:

- language support through Tree-sitter and language servers (LSP),
- appearance customization (themes, fonts, settings),
- git integration,
- extensions.

Oxed ships **without** AI features, account sign-in or collaboration, an integrated terminal, tasks, a debugger, telemetry, crash reporting, auto-update, or remote development.

## Building

Linux is the primary target. Install the system dependencies listed in `script/linux`, then:

```sh
cargo run              # debug build
script/bundle-oxed-linux   # release bundle in target/release/oxed-linux-<arch>.tar.gz
```

The `oxed` CLI opens files and folders in a running Oxed instance. User settings live in `~/.config/oxed`, separate from any Zed installation.

## Network access

Oxed does not sign in, send telemetry or check for updates. It still contacts:

- `api.zed.dev`, anonymously, for the extension registry,
- language server download hosts (e.g. GitHub releases) when a language server is installed.

## Syncing with Zed

Oxed tracks Zed's stable releases. The `upstream` remote points to `zed-industries/zed`.

```sh
git fetch upstream --tags
git merge v1.27.0          # next stable tag
cargo check -p zed         # also refreshes Cargo.lock
```

To keep merges small, removed features are cut where they are wired up rather than deleted from the tree:

- `crates/zed` no longer depends on or initializes their UI crates, which stay in the tree untouched;
- a few kept crates (`title_bar`, `onboarding`, `recent_projects`, `settings_ui`) hide their entry points;
- core crates that the editor is built on (`client`, `rpc`, `remote`, `terminal`, `dap`, `language_model`, ...) are still compiled but never connected or started: AI stays disabled, and keymap bindings and command palette entries of removed features are skipped;
- tests that exercise removed features are marked `#[ignore]` with the reason.

Oxed's own release tags use the `oxed-v*` prefix.

## License

Oxed is distributed under the GNU General Public License v3.0 or later, like the Zed editor it is derived from (see `LICENSE-GPL`). Some crates, such as GPUI, are licensed under Apache 2.0 (see `LICENSE-APACHE`). "Zed" is a trademark of Zed Industries; Oxed is not affiliated with or endorsed by Zed Industries.
