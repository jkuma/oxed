# Oxed

Oxed is a minimal, LSP-first code editor written in Rust. It is a fork of [Zed](https://github.com/zed-industries/zed) by Zed Industries, stripped down to what a focused editor needs:

- language support through Tree-sitter and language servers (LSP),
- appearance customization (themes, fonts, settings),
- git integration,
- extensions.

Oxed ships **without** AI features, account sign-in or collaboration, an integrated terminal, tasks, a debugger, telemetry, crash reporting, auto-update, or remote development.

## What Oxed adds

- **Code navigation in a popup.** Go to definition, implementation, type definition and references open a popup below the cursor line, with one row per result showing the code and its project path. `Ctrl+Alt+click` on a symbol shows its implementations.
- **A JetBrains (GNOME) keymap**, offered at onboarding: the JetBrains keymap with the shortcuts JetBrains moves in its GNOME variant.
- **Git blame annotations.** "Annotate with Git Blame", in the editor and gutter context menus, shows the commit date and author of each line, tinted with the commit's color.
- **Merge conflict resolution.** Conflicted files in the git panel offer:
  - "Accept Yours" and "Accept Theirs", which resolve a file with one side's version; during a rebase, "Yours" is the commit being replayed;
  - "Resolve Automatically", which merges the file again from git's index and opens it with the conflicts it could resolve, for review before saving.

  The three-way merge engine lives in `crates/merge`.
- **A simpler layout.** The project, outline and git panels share the left dock, and a window shows a single project.

## Installing

```sh
curl -fsSL https://raw.githubusercontent.com/jkuma/oxed/main/script/install.sh | sh
```

The script installs the latest release into `~/.local/oxed.app`, links the `oxed` command into `~/.local/bin` and adds Oxed to the applications menu. Run it again to update Oxed. The release relies on the system's libraries: the script lists any that are missing.

- `curl … | OXED_VERSION=0.2.0 sh` installs a given release.
- `OXED_BUNDLE_PATH=target/release/oxed-linux-x86_64.tar.gz script/install.sh` installs a bundle built locally.
- `curl … | sh -s -- --uninstall` removes Oxed and keeps its settings and data.

## Building

Oxed runs on Linux only. Install the system dependencies listed in `script/linux`, then:

```sh
cargo run              # debug build
script/bundle-oxed-linux   # release bundle in target/release/oxed-linux-<arch>.tar.gz
```

The `oxed` CLI opens files and folders in a running Oxed instance. User settings live in `~/.config/oxed`, separate from any Zed installation.

## Network access

Oxed does not sign in, send telemetry or check for updates. It still contacts:

- `api.zed.dev`, anonymously, for the extension registry,
- language server download hosts (e.g. GitHub releases) when a language server is installed.

## Relationship with Zed

Oxed is a standalone fork: it does not merge Zed's releases. Code that Oxed does not need is removed rather than disabled.

Fixes and improvements from stable Zed releases are reviewed and ported one by one, adapted to the fork, with the `zed-upstream` skill in `.claude/skills/`. Its ledger records the decision taken for every Zed commit; Zed v1.27.0 is reviewed.

## License

Oxed is distributed under the GNU General Public License v3.0 or later, like the Zed editor it is derived from (see `LICENSE-GPL`). Some crates, such as GPUI, are licensed under Apache 2.0 (see `LICENSE-APACHE`). "Zed" is a trademark of Zed Industries; Oxed is not affiliated with or endorsed by Zed Industries.
