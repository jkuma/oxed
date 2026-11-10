#!/usr/bin/env python3
"""Compares Oxed with Zed releases and ports selected upstream commits.

Usage: upstream.py <command> [args]
  fetch                     create or update the reference clone of Zed
  triage <tag> [--out DIR]  classify the commits between the reviewed tag and <tag>
  show <sha>                print an upstream commit limited to files Oxed still has
  port <sha>                apply an upstream commit to the Oxed working tree
  lint                      check the staged and unstaged changes against the fork rules
  record <sha> <decision> [oxed commit | reason]
  advance <tag>             mark <tag> as reviewed once every commit has a decision

The reference clone lives in $OXED_UPSTREAM_CLONE, or ~/.cache/oxed-upstream/zed.git.
"""
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

UPSTREAM_URL = "https://github.com/zed-industries/zed"
SKILL_DIR = Path(__file__).resolve().parent.parent
LEDGER_PATH = SKILL_DIR / "ledger.json"
CLONE = Path(
    os.environ.get("OXED_UPSTREAM_CLONE", Path.home() / ".cache/oxed-upstream/zed.git")
)
MIN_FREE_BYTES = 5 * 1024**3
DECISIONS = ("ported", "skipped", "removed", "already-applied", "deferred")
STABLE_TAG = re.compile(r"^v(\d+)\.(\d+)\.(\d+)$")

# Upstream-only areas, treated as absent even when the directory exists in Oxed.
IGNORED_PREFIXES = (".github/", "docs/", "legal/", "tooling/")
# A commit that only keeps these, next to absent code, serves the absent code.
WORKSPACE_MANIFESTS = {"Cargo.toml", "Cargo.lock"}

# Added lines that compile on Linux but break the fork rules.
LINT_PATTERNS = [
    (re.compile(r"cfg!?\((?:not\()?(?:any\(|all\()?.*target_os"), "platform-specific cfg"),
    (re.compile(r"cfg!?\((?:not\()?(?:any\(|all\()?.*\bwindows\b"), "platform-specific cfg"),
    (re.compile(r"target_family\s*=\s*\"wasm\""), "wasm-specific cfg"),
    (re.compile(r"zed\.dev"), "link to zed.dev"),
    (re.compile(r"\bzed(?:-cli)?://"), "zed:// scheme (Oxed uses oxed://)"),
    (re.compile(r"\"[^\"]*\bZed\b[^\"]*\""), "user-visible Zed branding"),
]


def run(args, cwd=None, input=None, check=True):
    result = subprocess.run(
        args, cwd=cwd, input=input, capture_output=True, text=input is None or isinstance(input, str)
    )
    if check and result.returncode != 0:
        stderr = result.stderr if isinstance(result.stderr, str) else result.stderr.decode()
        sys.exit(f"error: {' '.join(map(str, args))}\n{stderr}")
    return result


def oxed_root():
    return Path(run(["git", "rev-parse", "--show-toplevel"], cwd=SKILL_DIR).stdout.strip())


def upstream(*args, check=True):
    return run(["git", "-C", str(CLONE), *args], check=check)


def oxed(*args, input=None, check=True):
    return run(["git", "-C", str(oxed_root()), *args], input=input, check=check)


def load_ledger():
    return json.loads(LEDGER_PATH.read_text())


def save_ledger(ledger):
    LEDGER_PATH.write_text(json.dumps(ledger, indent=2, sort_keys=True) + "\n")


def require_clone():
    if not (CLONE / "HEAD").exists():
        sys.exit(f"error: no reference clone at {CLONE}; run `upstream.py fetch` first")


def stable_tags():
    tags = upstream("tag", "-l", "v*").stdout.split()
    stable = [tag for tag in tags if STABLE_TAG.match(tag)]
    return sorted(stable, key=lambda tag: tuple(map(int, STABLE_TAG.match(tag).groups())))


def require_tag(tag):
    if upstream("rev-parse", "--verify", "--quiet", f"refs/tags/{tag}^{{commit}}", check=False).returncode:
        sys.exit(f"error: unknown tag {tag}; run `upstream.py fetch` first")


def cmd_fetch(_args):
    if not (CLONE / "HEAD").exists():
        CLONE.parent.mkdir(parents=True, exist_ok=True)
        free = shutil.disk_usage(CLONE.parent).free
        print(f"free space at {CLONE.parent}: {free / 1024**3:.0f} GiB")
        if free < MIN_FREE_BYTES:
            sys.exit("error: less than 5 GiB free, not cloning")
        run(["git", "init", "--bare", "--quiet", str(CLONE)])
        for key, value in [
            ("core.repositoryformatversion", "1"),
            ("extensions.partialClone", "origin"),
            ("remote.origin.url", UPSTREAM_URL),
            ("remote.origin.fetch", "+refs/tags/v*:refs/tags/v*"),
            ("remote.origin.tagOpt", "--no-tags"),
            ("remote.origin.promisor", "true"),
            ("remote.origin.partialclonefilter", "blob:none"),
        ]:
            upstream("config", key, value)
        print(f"created blobless clone at {CLONE}")
    upstream("fetch", "--quiet", "origin")
    tags = stable_tags()
    print("latest stable tags: " + ", ".join(tags[-5:]))

    ledger = load_ledger()
    fork_base = ledger["fork_base"]
    import_commit = oxed("rev-list", "--max-parents=0", "HEAD").stdout.split()[0]
    base_tree = upstream("rev-parse", f"{fork_base}^{{tree}}").stdout.strip()
    import_tree = oxed("rev-parse", f"{import_commit}^{{tree}}").stdout.strip()
    if base_tree != import_tree:
        sys.exit(f"error: {fork_base} tree {base_tree} differs from Oxed import tree {import_tree}")
    print(f"fork base {fork_base} matches Oxed import commit {import_commit[:10]}")
    print(f"reviewed up to {ledger['reviewed_up_to']}; latest stable is {tags[-1]}")


class OxedTree:
    """Paths of the Oxed HEAD, used to tell which upstream files still matter."""

    def __init__(self):
        self.files = set(oxed("ls-tree", "-r", "--name-only", "HEAD").stdout.splitlines())
        self.dirs = {str(parent) for path in self.files for parent in Path(path).parents}

    def keeps(self, status, path):
        if path.startswith(IGNORED_PREFIXES):
            return False
        if status != "A":
            return path in self.files
        parts = Path(path).parts
        owner = str(Path(*parts[:2])) if parts[0] == "crates" and len(parts) > 2 else str(Path(path).parent)
        return owner in self.dirs


def range_commits(base, target):
    """Commits of `target` missing from `base`, oldest first, with their changed files."""
    output = upstream(
        "log", "--cherry-pick", "--right-only", "--no-merges", "--no-renames", "--reverse",
        "--name-status", "--format=%x01%H%x1f%s%x1f%an%x1f%as", f"{base}...{target}",
    ).stdout
    commits = []
    for chunk in output.split("\x01")[1:]:
        header, _, body = chunk.partition("\n")
        sha, subject, author, date = header.split("\x1f")
        files = []
        for line in body.splitlines():
            if line.strip():
                status, _, path = line.partition("\t")
                files.append((status[0], path))
        commits.append({"sha": sha, "subject": subject, "author": author, "date": date, "files": files})
    return commits


def pr_number(subject):
    """The original pull request, also for cherry-picks titled `… (#A) (cherry-pick …) (#B)`."""
    match = re.search(r"\(#(\d+)\)", subject)
    return int(match.group(1)) if match else None


def clean_title(subject):
    return re.sub(r"\s*\((?:#\d+|cherry-pick[^)]*)\)", "", subject).strip()


def guess_kind(subject):
    lower = subject.lower()
    if re.search(r"\b(bump|upgrade|update)\b.*\b(dependenc|crate|version|sdk|taffy|tree-sitter)", lower):
        return "deps"
    if re.search(r"\b(fix|bug|crash|panic|prevent|don't|do not|correct|preserve|honor|handle|avoid|stop|restore)\b", lower):
        return "fix"
    if re.search(r"\b(refactor|clean ?up|move|rename|migrate|simplify|remove)\b", lower):
        return "refactor"
    return "feature"


def touched_areas(paths):
    areas = []
    for path in paths:
        parts = Path(path).parts
        area = parts[1] if parts[0] == "crates" and len(parts) > 1 else parts[0]
        if area not in areas:
            areas.append(area)
    return areas


def limited_patch(sha, paths):
    return upstream("format-patch", "-1", "--full-index", "--no-renames", "--stdout", sha, "--", *paths).stdout


def patch_size(patch):
    added = removed = 0
    for line in patch.splitlines():
        if line.startswith("+") and not line.startswith("+++"):
            added += 1
        elif line.startswith("-") and not line.startswith("---"):
            removed += 1
    return added, removed


def classify(commit, tree):
    kept = [path for status, path in commit["files"] if tree.keeps(status, path)]
    dropped = [path for _, path in commit["files"] if path not in kept]
    areas = ", ".join(touched_areas(dropped)[:6])
    if not kept:
        return "removed", f"touches only code absent from Oxed ({areas})", kept
    if dropped and set(kept) <= WORKSPACE_MANIFESTS:
        return "removed", f"only workspace manifests besides code absent from Oxed ({areas})", kept
    if re.match(r"^Bump Zed to v", commit["subject"]):
        return "skipped", "Zed version bump; Oxed versions independently", kept
    patch = limited_patch(commit["sha"], kept)
    if oxed("apply", "--reverse", "--check", "-", input=patch, check=False).returncode == 0:
        return "already-applied", "the change is already in Oxed", kept
    return "candidate", "", kept


def cmd_triage(args):
    if not args:
        sys.exit("usage: upstream.py triage <tag> [--out DIR]")
    target = args[0]
    out_dir = Path(args[args.index("--out") + 1]) if "--out" in args else Path(tempfile.gettempdir()) / "oxed-upstream"
    require_clone()
    require_tag(target)
    ledger = load_ledger()
    base = ledger["reviewed_up_to"]
    tree = OxedTree()
    commits = range_commits(base, target)
    counts = {}
    candidates = []
    for commit in commits:
        if commit["sha"] in ledger["commits"]:
            counts["already decided"] = counts.get("already decided", 0) + 1
            continue
        decision, reason, kept = classify(commit, tree)
        counts[decision] = counts.get(decision, 0) + 1
        if decision == "candidate":
            added, removed = patch_size(limited_patch(commit["sha"], kept))
            dropped = [path for _, path in commit["files"] if path not in kept]
            candidates.append({
                "sha": commit["sha"],
                "pr": pr_number(commit["subject"]),
                "title": commit["subject"],
                "date": commit["date"],
                "kind": guess_kind(commit["subject"]),
                "areas": touched_areas(kept),
                "added": added,
                "removed": removed,
                "dropped_files": dropped,
            })
        else:
            ledger["commits"][commit["sha"]] = {
                "pr": pr_number(commit["subject"]),
                "title": commit["subject"],
                "decision": decision,
                "reason": reason,
            }
    save_ledger(ledger)

    deferred = [
        {"sha": sha, **entry} for sha, entry in ledger["commits"].items() if entry["decision"] == "deferred"
    ]
    out_dir.mkdir(parents=True, exist_ok=True)
    stem = f"triage-{base}-{target}"
    (out_dir / f"{stem}.json").write_text(json.dumps({"base": base, "target": target, "candidates": candidates, "deferred": deferred}, indent=2) + "\n")
    lines = [f"# Zed {base} → {target}", "", f"{len(commits)} commits: " + ", ".join(f"{count} {name}" for name, count in sorted(counts.items())), ""]
    lines += ["| sha | PR | kind | areas | +/- | title | dropped files |", "|---|---|---|---|---|---|---|"]
    for candidate in candidates:
        lines.append(
            f"| {candidate['sha'][:10]} | {candidate['pr'] or ''} | {candidate['kind']} | {', '.join(candidate['areas'][:4])} "
            f"| +{candidate['added']}/-{candidate['removed']} | {candidate['title']} | {len(candidate['dropped_files'])} |"
        )
    if deferred:
        lines += ["", "## Still deferred", ""] + [f"- {entry['sha'][:10]} {entry['title']}: {entry['reason']}" for entry in deferred]
    (out_dir / f"{stem}.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    print(f"\nreport: {out_dir / (stem + '.md')}")


def commit_info(sha):
    require_clone()
    output = upstream("show", "--no-renames", "--name-status", "--format=%H%x1f%s", sha).stdout
    header, _, body = output.partition("\n")
    full_sha, subject = header.split("\x1f")
    files = [(line.split("\t")[0][0], line.split("\t", 1)[1]) for line in body.splitlines() if "\t" in line]
    return full_sha, subject, files


def cmd_show(args):
    if not args:
        sys.exit("usage: upstream.py show <sha>")
    sha, subject, files = commit_info(args[0])
    tree = OxedTree()
    kept = [path for status, path in files if tree.keeps(status, path)]
    dropped = [path for status, path in files if not tree.keeps(status, path)]
    print(f"{sha} {subject}")
    if dropped:
        print("files absent from Oxed: " + ", ".join(dropped))
    if kept:
        print(upstream("show", "--no-renames", "--format=", sha, "--", *kept).stdout)


def lockfile_versions(rev):
    result = upstream("show", f"{rev}:Cargo.lock", check=False)
    versions = {}
    for name, version in re.findall(r'\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"', result.stdout):
        versions.setdefault(name, set()).add(version)
    return versions


def lockfile_summary(sha):
    before, after = lockfile_versions(f"{sha}^"), lockfile_versions(sha)
    lines = []
    for name in sorted(set(before) | set(after)):
        old, new = before.get(name, set()), after.get(name, set())
        if old == new:
            continue
        removed, added = sorted(old - new), sorted(new - old)
        if len(removed) == 1 and len(added) == 1:
            lines.append(f"  {name} {removed[0]} -> {added[0]}   (cargo update -p {name}@{removed[0]} --precise {added[0]})")
        else:
            lines.append(f"  {name}: removed {removed or '-'}, added {added or '-'}")
    return lines


def seed_preimages(sha, files):
    """Copies the upstream base blobs into Oxed so `git apply --3way` finds them."""
    seeded = 0
    for status, path in files:
        if status == "A":
            continue
        blob = upstream("rev-parse", f"{sha}^:{path}").stdout.strip()
        if oxed("cat-file", "-e", blob, check=False).returncode == 0:
            continue
        content = run(["git", "-C", str(CLONE), "cat-file", "blob", blob], input=b"").stdout
        written = run(["git", "-C", str(oxed_root()), "hash-object", "-w", "--stdin"], input=content).stdout
        written = written.decode().strip() if isinstance(written, bytes) else written.strip()
        if written != blob:
            sys.exit(f"error: seeded blob for {path} hashed to {written}, expected {blob}")
        seeded += 1
    return seeded


def lint_added_lines(diff):
    findings = []
    path = None
    for line in diff.splitlines():
        if line.startswith("+++ "):
            path = line[6:] if line.startswith("+++ b/") else line[4:]
        elif line.startswith("+") and not line.startswith("+++"):
            for pattern, label in LINT_PATTERNS:
                if pattern.search(line):
                    findings.append(f"  {path}: {label}: {line[1:].strip()[:120]}")
    return findings


def report_lint():
    findings = lint_added_lines(oxed("diff", "--cached").stdout + "\n" + oxed("diff").stdout)
    if findings:
        print("review before committing (fork rules):")
        print("\n".join(findings))
    return findings


def cmd_lint(_args):
    if oxed("diff", "--name-only", "--diff-filter=U").stdout.strip():
        sys.exit("error: resolve and stage the conflicts first")
    if not report_lint():
        print("no fork rule findings")


def cmd_port(args):
    if not args:
        sys.exit("usage: upstream.py port <sha>")
    # The ledger collects decisions during a batch and is committed at its end.
    ledger = str(LEDGER_PATH.relative_to(oxed_root()))
    if oxed("status", "--porcelain", "--", ".", f":!{ledger}").stdout.strip():
        sys.exit("error: the Oxed working tree is not clean; commit or stash first")
    sha, subject, files = commit_info(args[0])
    tree = OxedTree()
    kept = [(status, path) for status, path in files if tree.keeps(status, path) and path != "Cargo.lock"]
    excluded = [path for status, path in files if (status, path) not in kept]
    print(f"porting {sha[:10]} {subject}")
    if excluded:
        print("excluded files: " + ", ".join(excluded))
    if any(path == "Cargo.lock" for _, path in files):
        summary = lockfile_summary(sha)
        print("Cargo.lock changes upstream (redo them with cargo, never apply the lock hunks):")
        print("\n".join(summary) if summary else "  (no version changes)")
    if not kept:
        print("nothing to apply outside Cargo.lock")
        return
    seeded = seed_preimages(sha, kept)
    if seeded:
        print(f"seeded {seeded} upstream base blob(s) into Oxed")
    patch = limited_patch(sha, [path for _, path in kept])
    result = oxed("apply", "--3way", "-", input=patch, check=False)
    print(result.stdout.strip() + ("\n" if result.stdout.strip() else "") + result.stderr.strip())
    conflicts = oxed("diff", "--name-only", "--diff-filter=U").stdout.split()
    if conflicts:
        print("conflicts to resolve: " + ", ".join(conflicts))
    report_lint()
    pr = pr_number(subject)
    title = clean_title(subject)
    reference = f"Ported from zed-industries/zed@{sha[:10]}" + (f" (#{pr})." if pr else ".")
    print(f"\ncommit message:\n{title}\n\n{reference}")
    if result.returncode != 0 and not conflicts:
        sys.exit("error: the patch did not apply; port it by hand")


def cmd_record(args):
    if len(args) < 2 or args[1] not in DECISIONS:
        sys.exit(f"usage: upstream.py record <sha> <{'|'.join(DECISIONS)}> [oxed commit | reason]")
    sha, subject, _ = commit_info(args[0])
    decision, value = args[1], " ".join(args[2:])
    entry = {"pr": pr_number(subject), "title": subject, "decision": decision, "reason": ""}
    if decision == "ported":
        if not value:
            sys.exit("error: give the Oxed commit that ports it")
        entry["oxed_commit"] = oxed("rev-parse", "--short", f"{value}^{{commit}}").stdout.strip()
    elif not value:
        sys.exit("error: give a reason")
    else:
        entry["reason"] = value
    ledger = load_ledger()
    ledger["commits"][sha] = entry
    save_ledger(ledger)
    print(f"recorded {sha[:10]} as {decision}")


def cmd_advance(args):
    if not args:
        sys.exit("usage: upstream.py advance <tag>")
    target = args[0]
    require_clone()
    require_tag(target)
    ledger = load_ledger()
    undecided = [commit for commit in range_commits(ledger["reviewed_up_to"], target) if commit["sha"] not in ledger["commits"]]
    if undecided:
        print(f"{len(undecided)} commit(s) still need a decision:")
        for commit in undecided:
            print(f"  {commit['sha'][:10]} {commit['subject']}")
        sys.exit(1)
    ledger["reviewed_up_to"] = target
    save_ledger(ledger)
    print(f"reviewed up to {target}")


COMMANDS = {
    "fetch": cmd_fetch,
    "triage": cmd_triage,
    "show": cmd_show,
    "port": cmd_port,
    "lint": cmd_lint,
    "record": cmd_record,
    "advance": cmd_advance,
}

if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] not in COMMANDS:
        sys.exit(__doc__)
    COMMANDS[sys.argv[1]](sys.argv[2:])
