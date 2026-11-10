---
name: zed-upstream
description: Compare Oxed with the latest stable Zed release and port selected upstream commits into Oxed. Use when asked to check upstream Zed, sync or catch up with Zed, see what changed in a Zed release, or port a specific Zed fix or pull request.
argument-hint: "[target tag, e.g. v1.27.0]"
---

# Porting changes from Zed

Oxed is a standalone, Linux-only fork of Zed. Its root commit has exactly the tree of the Zed tag recorded as `fork_base` in `ledger.json`. Oxed never merges Zed: it ports selected commits from stable Zed releases, one Oxed commit per Zed commit, adapted to the fork.

## Tools

Run from the repository root:

```
python3 .claude/skills/zed-upstream/scripts/upstream.py <command>
```

| Command | Effect |
|---|---|
| `fetch` | Creates or updates the blobless reference clone (`$OXED_UPSTREAM_CLONE`, default `~/.cache/oxed-upstream/zed.git`), lists the latest stable tags, checks the fork base tree. |
| `triage <tag> --out <dir>` | Classifies every commit between `reviewed_up_to` and `<tag>`, records the automatic decisions, writes a Markdown and JSON report of the candidates in `<dir>`. |
| `show <sha>` | Prints an upstream commit limited to the files Oxed still has, and lists the others. |
| `port <sha>` | Applies an upstream commit to a clean Oxed working tree with a 3-way merge, then reports conflicts, `Cargo.lock` changes and lines that break the fork rules. |
| `record <sha> <decision> [oxed commit or reason]` | Records `ported`, `skipped`, `deferred`, `removed` or `already-applied`. |
| `advance <tag>` | Marks `<tag>` as reviewed once every commit up to it has a decision. |

`ledger.json` is the source of truth: a decided commit is never proposed again. Deferred commits are listed again in every triage report.

## Workflow

1. **Fetch.** Run `fetch`. The target is the argument, or else the latest stable tag (`vX.Y.Z`, never `-pre`). Stop if `reviewed_up_to` already is the target.
2. **Triage.** Run `triage <target> --out <scratchpad dir>`. The first run can take several minutes because the clone downloads file contents on demand, so run it in the background. Commits are classified automatically:
   - `removed`: every touched file is gone from Oxed, or lives under an upstream-only prefix (`.github/`, `docs/`, `legal/`, `tooling/`);
   - `skipped`: Zed version bumps;
   - `already-applied`: the change is already in Oxed;
   - everything else is a candidate.
3. **Review the candidates.** Read each one with `show <sha>`. Judge its value for Oxed and its conflict risk against the adaptation rules below. With more than about twenty candidates, split the reading between a few subagents. Give each one its list of shas, the adaptation rules and the `show` command, and ask for a one-line summary, a recommendation, the value and the risk.
4. **Let the user choose.** Present the candidates grouped by area (LSP, editor, project, git, UI, dependencies…). Give each one its PR number, its title, one sentence on what it changes and your recommendation. Port nothing before the user has chosen. Record what the user rejects with `record <sha> skipped "<reason>"`, and what they postpone with `deferred`.
5. **Port the chosen commits** in the order of the report, one at a time:
   1. `port <sha>`. It refuses to run on a dirty working tree.
   2. Resolve the conflicts and adapt the code to the rules below. Handle every "review before committing" finding.
   3. Redo upstream `Cargo.lock` changes with `cargo update -p <crate>@<old> --precise <new>`, only for crates in the Oxed lock. Never apply lock hunks.
   4. Run `cargo check -p <crate> --tests`, `cargo clippy --no-deps -p <crate>` and `cargo test -p <crate>` on every touched crate.
   5. Commit. The title is the upstream title without the PR number. The body is `Ported from zed-industries/zed@<sha> (#<PR>).`, then one line per part left out (for example "The macOS part is left out."), then the usual attribution trailer. Never amend.
   6. `record <sha> ported <oxed commit>`, then `rm -rf target/debug/incremental`.

   If a port turns out not worth it, check that `git status` only lists files from this port, revert them, and record the commit as `skipped` with the reason.
6. **Finish the batch.**
   - Run `cargo check --workspace --tests` and `cargo test -p zed --bin oxed`.
   - Run the full crate-by-crate test sweep, or `cargo test` on every touched crate.
   - If a port changes something visible, build the bundle with `script/bundle-oxed-linux` for a manual test.
   - Run `advance <target>` once every commit has a decision.
   - Commit `ledger.json` as "Record the review of Zed <target>".
   - Ask the user before pushing.

## Adaptation rules

**Never port code for features Oxed removed.** That covers:
- AI, agents and edit predictions;
- collaboration, accounts and channels, `client`, `rpc` and `proto`;
- telemetry and auto-update;
- remote projects, SSH and dev containers;
- the terminal, the debugger and tasks;
- Vim and Helix;
- macOS, Windows, FreeBSD and wasm code, except in `crates/extension_api`.

When a commit mixes such code with useful changes, drop that part and keep the rest. A `#[cfg(target_os = "macos")]` or `cfg(windows)` block compiles silently on Linux, so remove it rather than port it.

**These APIs diverged from Zed:**
- `Worktree` is a single struct that derefs to `LocalSnapshot`. There is no `as_local()` or `as_local_mut()`, and `local_snapshot()` returns the `LocalSnapshot`.
- `LspStore::as_local()` and `BufferStore` local accessors return the local state directly, not an `Option`.
- `PathStyle`, `SanitizedPath` and `RemotePathBuf` are gone. Use `std::path::Path` and the free functions of the `path` crate (`join_relative`, `join_path`, `normalize_separators`…).
- The `path!`, `uri!` and `line_endings!` macros are gone; write the literal value.
- Projects are local only: there is no `upstream_client`, `downstream_client`, `is_via_collab` or remote worktree.
- The URL schemes are `oxed://` and `oxed-cli://`, the project settings folder is `.oxed/`, and the user-visible name is "Oxed".
- There are no links to the Zed documentation.

**When an upstream symbol does not exist in Oxed,** `git log -S<symbol> --oneline` shows the commit that removed or renamed it.

Follow `CLAUDE.md` and `.rules`, in particular their "Oxed fork" section. That means no `unwrap()`, no `let _ =` on fallible calls, minimal comments, Linux only, and deleting rather than hiding.
