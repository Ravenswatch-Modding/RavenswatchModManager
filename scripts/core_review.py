"""Owner-review gate for core changes (the `core review` CI job).

  required  --base SHA --head SHA          is owner review needed for this diff?
  approved  --repo O/N --pr N --head SHA   has an owner approved this head?
  match     --files-from FILE              which listed paths are core?
  owners    [--exclude LOGIN]              logins from .github/CODEOWNERS

Core paths live in .github/core-paths.txt (the glob rules are in that file's
header). Review is required when `contract_pins.py breaking` prints anything
or when core files change by more than EXEMPT_LINES lines in total.
Stdlib only.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CORE_PATHS = ROOT / ".github" / "core-paths.txt"
CODEOWNERS = ROOT / ".github" / "CODEOWNERS"
EXEMPT_LINES = 2

_LOGIN_RE = re.compile(r"^@([A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?)$")


# --- core paths --------------------------------------------------------------

def parse_patterns(text: str) -> list[str]:
    out = []
    for n, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("!"):
            raise ValueError(f"core-paths line {n}: negation is not supported: {line}")
        out.append(line)
    return out


def _glob_to_regex(pat: str) -> re.Pattern[str]:
    dir_only = pat.endswith("/")
    anchored = pat.startswith("/") or "/" in pat.rstrip("/")
    pat = pat.strip("/")
    i, out = 0, []
    while i < len(pat):
        if pat.startswith("**/", i):
            out.append("(?:.*/)?")
            i += 3
        elif pat.startswith("**", i):
            out.append(".*")
            i += 2
        elif pat[i] == "*":
            out.append("[^/]*")
            i += 1
        elif pat[i] == "?":
            out.append("[^/]")
            i += 1
        else:
            out.append(re.escape(pat[i]))
            i += 1
    body = "".join(out)
    prefix = "" if anchored else "(?:.*/)?"
    # A match on a directory covers everything below it.
    tail = "/.*$" if dir_only else "(?:/.*)?$"
    return re.compile(f"^{prefix}{body}{tail}")


def compile_patterns(patterns: list[str]) -> list[re.Pattern[str]]:
    return [_glob_to_regex(p) for p in patterns]


def is_core(path: str, regexes: list[re.Pattern[str]]) -> bool:
    path = path.replace("\\", "/")
    if path.startswith("./"):
        path = path[2:]
    return any(r.match(path) for r in regexes)


def load_core_regexes(path: Path | None = None) -> list[re.Pattern[str]]:
    return compile_patterns(parse_patterns((path or CORE_PATHS).read_text(encoding="utf-8")))


# --- diff size ---------------------------------------------------------------

def parse_numstat(raw: str) -> list[tuple[str, int]]:
    """``git diff --numstat -z --no-renames`` -> [(path, changed lines)].

    A binary file counts as more than the exemption allows.
    """
    out = []
    for rec in raw.split("\0"):
        if not rec.strip():
            continue
        added, deleted, path = rec.lstrip("\n").split("\t", 2)
        n = EXEMPT_LINES + 1 if added == "-" else int(added) + int(deleted)
        out.append((path, n))
    return out


def core_changes(numstat: list[tuple[str, int]],
                 regexes: list[re.Pattern[str]]) -> list[tuple[str, int]]:
    return [(p, n) for p, n in numstat if is_core(p, regexes)]


# --- owners and approval -----------------------------------------------------

def parse_owners(text: str) -> list[str]:
    """User logins named in CODEOWNERS (teams and e-mail owners are skipped)."""
    seen: dict[str, None] = {}
    for raw in text.splitlines():
        for tok in raw.split("#", 1)[0].split():
            m = _LOGIN_RE.match(tok)
            if m:
                seen.setdefault(m.group(1), None)
    return list(seen)


def read_owners(path: Path | None = None) -> list[str]:
    path = path or CODEOWNERS
    if not path.is_file():
        return []
    return parse_owners(path.read_text(encoding="utf-8"))


def approving_owners(reviews: list[dict], owners: list[str], author: str,
                     head: str) -> list[str]:
    """Owners (not the author) whose latest non-comment review approves ``head``."""
    owner_set = {o.lower() for o in owners}
    author = (author or "").lower()
    latest: dict[str, dict] = {}
    for rv in sorted(reviews, key=lambda r: r.get("id", 0)):
        user = ((rv.get("user") or {}).get("login") or "").lower()
        # PENDING reviews are drafts; COMMENTED never changes a verdict.
        if not user or rv.get("state") in ("COMMENTED", "PENDING"):
            continue
        latest[user] = rv
    return [
        user for user, rv in latest.items()
        if user in owner_set and user != author
        and rv.get("state") == "APPROVED" and rv.get("commit_id") == head
    ]


def load_paginated(raw: str) -> list[dict]:
    """``gh api --paginate`` prints one JSON array per page, back to back."""
    dec, idx, out = json.JSONDecoder(), 0, []
    raw = raw.strip()
    while idx < len(raw):
        val, idx = dec.raw_decode(raw, idx)
        out.extend(val if isinstance(val, list) else [val])
        while idx < len(raw) and raw[idx].isspace():
            idx += 1
    return out


# --- git / gh ----------------------------------------------------------------

def _run(cmd: list[str]) -> str:
    r = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", cwd=ROOT)
    if r.returncode != 0:
        sys.exit(f"{' '.join(cmd)} failed:\n{r.stderr.strip()}")
    return r.stdout


def _set_output(key: str, value: str) -> None:
    path = os.environ.get("GITHUB_OUTPUT")
    if path:
        with open(path, "a", encoding="utf-8") as fh:
            fh.write(f"{key}={value}\n")


# --- commands ----------------------------------------------------------------

def cmd_required(base: str, head: str) -> int:
    # Compare against where the branch left the base, like `git diff base...head`;
    # the contract snapshots are read from the working tree (the head checkout).
    fork = _run(["git", "merge-base", base, head]).strip()
    breaking = _run([sys.executable, str(ROOT / "scripts" / "contract_pins.py"),
                     "breaking", "--base", fork]).splitlines()
    raw = _run(["git", "diff", "--numstat", "-z", "--no-renames", f"{fork}...{head}"])
    core = core_changes(parse_numstat(raw), load_core_regexes())
    total = sum(n for _, n in core)
    reasons = []
    if breaking:
        reasons.append("breaking contract changes:")
        reasons += [f"  {ln}" for ln in breaking]
    if total > EXEMPT_LINES:
        reasons.append(f"core files changed by {total} lines (limit {EXEMPT_LINES}):")
        reasons += [f"  {p} ({n})" for p, n in core]
    _set_output("required", "true" if reasons else "false")
    if reasons:
        print("owner review required")
        print("\n".join(reasons))
    else:
        print(f"owner review not required: {total} core line(s) changed "
              f"(limit {EXEMPT_LINES}), nothing breaking")
        for p, n in core:
            print(f"  {p} ({n})")
    return 0


def cmd_approved(repo: str, pr: int, head: str, author: str | None) -> int:
    owners = read_owners()
    if author is None:
        author = _run(["gh", "api", f"repos/{repo}/pulls/{pr}", "--jq", ".user.login"]).strip()
    reviews = load_paginated(_run(["gh", "api", "--paginate", f"repos/{repo}/pulls/{pr}/reviews"]))
    ok = approving_owners(reviews, owners, author, head)
    if ok:
        print(f"approved at {head[:12]} by {', '.join(ok)}")
        return 0
    eligible = [f"@{o}" for o in owners if o.lower() != author.lower()]
    who = " or ".join(eligible) or "nobody (no owners in .github/CODEOWNERS)"
    print(f"owner review required: approval of {head[:12]} from {who} is needed. "
          f"The author (@{author}) cannot approve their own PR, and a new push "
          f"needs a fresh approval.")
    return 1


def cmd_match(files_from: Path) -> int:
    regexes = load_core_regexes()
    for line in files_from.read_text(encoding="utf-8").splitlines():
        if line.strip() and is_core(line.strip(), regexes):
            print(line.strip())
    return 0


def cmd_owners(exclude: str | None) -> int:
    for o in read_owners():
        if not exclude or o.lower() != exclude.lower():
            print(o)
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("required")
    p.add_argument("--base", required=True)
    p.add_argument("--head", required=True)
    p = sub.add_parser("approved")
    p.add_argument("--repo", required=True)
    p.add_argument("--pr", required=True, type=int)
    p.add_argument("--head", required=True)
    p.add_argument("--author", help="PR author login (default: read from the PR)")
    p = sub.add_parser("match")
    p.add_argument("--files-from", required=True, type=Path)
    p = sub.add_parser("owners")
    p.add_argument("--exclude", help="login to leave out (the PR author)")
    args = ap.parse_args(argv)
    if args.cmd == "required":
        return cmd_required(args.base, args.head)
    if args.cmd == "approved":
        return cmd_approved(args.repo, args.pr, args.head, args.author)
    if args.cmd == "match":
        return cmd_match(args.files_from)
    return cmd_owners(args.exclude)


if __name__ == "__main__":
    sys.exit(main())
