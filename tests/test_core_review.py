"""scripts/core_review.py: glob matching, diff size, owner approval."""
from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location("core_review", ROOT / "scripts" / "core_review.py")
cr = importlib.util.module_from_spec(_spec)
sys.modules["core_review"] = cr
_spec.loader.exec_module(cr)

HEAD = "a" * 40
OLD = "b" * 40


def rx(*pats):
    return cr.compile_patterns(list(pats))


# --- globs ---------------------------------------------------------------

@pytest.mark.parametrize("pat,path,hit", [
    ("src/loader/lib/**", "src/loader/lib/rsmm.lua", True),
    ("src/loader/lib/**", "src/loader/lib/sub/x.lua", True),
    ("src/loader/lib/**", "src/loader/libx/rsmm.lua", False),
    ("src/loader/lib/**", "other/src/loader/lib/rsmm.lua", False),
    ("data/symbols.json", "data/symbols.json", True),
    ("data/symbols.json", "x/data/symbols.json", False),
    ("*.lua", "a/b/c.lua", True),          # no slash: any depth
    ("*.lua", "c.luax", False),
    ("/*.lua", "a/c.lua", False),          # leading slash: root only
    ("/*.lua", "c.lua", True),
    ("src/*.py", "src/a.py", True),
    ("src/*.py", "src/sub/a.py", False),   # * stops at "/"
    ("src/**/a.py", "src/a.py", True),     # **/ matches no directory
    ("src/**/a.py", "src/x/y/a.py", True),
    ("docs/", "docs/a/b.md", True),        # trailing slash: directory
    ("docs/", "docs", False),
    ("src/loader", "src/loader/lib/a.lua", True),  # directory name covers contents
    ("a?c", "abc", True),
    ("a?c", "a/c", False),
])
def test_glob(pat, path, hit):
    assert cr.is_core(path, rx(pat)) is hit


def test_backslash_and_dot_slash_paths_normalised():
    assert cr.is_core(".\\src\\loader\\lib\\x.lua", rx("src/loader/lib/**"))


def test_parse_patterns_skips_comments_and_rejects_negation():
    assert cr.parse_patterns("# c\n\n a/b \n") == ["a/b"]
    with pytest.raises(ValueError):
        cr.parse_patterns("!a/b\n")


def test_shipped_core_paths_load_and_match():
    r = cr.load_core_regexes()
    assert cr.is_core("src/loader/lib/rsmm.lua", r)
    assert cr.is_core("src/rsmm/cli/json_bridge.py", r)
    assert not cr.is_core("src/rsmm/engine/terrain.py", r)
    assert not cr.is_core("data/contracts/r_surface.txt", r)


# --- diff size -----------------------------------------------------------

def test_numstat_counts_added_plus_removed_and_binary():
    raw = "1\t1\tsrc/a.py\0-\t-\timg.png\0\n10\t0\tb\0"
    assert cr.parse_numstat(raw) == [("src/a.py", 2), ("img.png", 3), ("b", 10)]


def test_core_changes_filters_by_pattern():
    ns = [("src/loader/lib/a.lua", 2), ("README.md", 50)]
    assert cr.core_changes(ns, rx("src/loader/lib/**")) == [("src/loader/lib/a.lua", 2)]


# --- owners --------------------------------------------------------------

def test_parse_owners_logins_only():
    text = "# top\n* @Ovilli @Skigim\n/a/ @Ovilli @org/team me@x.io # @ghost\n"
    assert cr.parse_owners(text) == ["Ovilli", "Skigim"]


def test_missing_codeowners_means_no_owners(tmp_path):
    assert cr.read_owners(tmp_path / "CODEOWNERS") == []


# --- approval ------------------------------------------------------------

_id = iter(range(1, 1000))


def rv(user, state, commit=HEAD):
    return {"id": next(_id), "user": {"login": user}, "state": state, "commit_id": commit}


OWNERS = ["Ovilli", "Skigim"]


def test_owner_approval_of_head_passes():
    assert cr.approving_owners([rv("Ovilli", "APPROVED")], OWNERS, "someone", HEAD) == ["ovilli"]


def test_login_case_insensitive():
    assert cr.approving_owners([rv("ovilli", "APPROVED")], OWNERS, "x", HEAD)


def test_stale_sha_approval_fails():
    assert cr.approving_owners([rv("Ovilli", "APPROVED", OLD)], OWNERS, "x", HEAD) == []


def test_author_self_approval_fails():
    assert cr.approving_owners([rv("Skigim", "APPROVED")], OWNERS, "Skigim", HEAD) == []


def test_non_owner_approval_fails():
    assert cr.approving_owners([rv("rando", "APPROVED")], OWNERS, "x", HEAD) == []


def test_changes_requested_after_approval_cancels():
    reviews = [rv("Ovilli", "APPROVED"), rv("Ovilli", "CHANGES_REQUESTED")]
    assert cr.approving_owners(reviews, OWNERS, "x", HEAD) == []


def test_dismissed_after_approval_cancels():
    reviews = [rv("Ovilli", "APPROVED"), rv("Ovilli", "DISMISSED")]
    assert cr.approving_owners(reviews, OWNERS, "x", HEAD) == []


def test_comment_after_approval_keeps_it():
    reviews = [rv("Ovilli", "APPROVED"), rv("Ovilli", "COMMENTED")]
    assert cr.approving_owners(reviews, OWNERS, "x", HEAD) == ["ovilli"]


def test_approval_after_changes_requested_counts():
    reviews = [rv("Ovilli", "CHANGES_REQUESTED"), rv("Ovilli", "APPROVED")]
    assert cr.approving_owners(reviews, OWNERS, "x", HEAD) == ["ovilli"]


def test_other_owners_changes_requested_does_not_cancel_approval():
    reviews = [rv("Ovilli", "APPROVED"), rv("Skigim", "CHANGES_REQUESTED")]
    assert cr.approving_owners(reviews, OWNERS, "x", HEAD) == ["ovilli"]


def test_no_owners_fails_closed():
    assert cr.approving_owners([rv("Ovilli", "APPROVED")], [], "x", HEAD) == []


def test_deleted_user_review_ignored():
    r = {"id": 1, "user": None, "state": "APPROVED", "commit_id": HEAD}
    assert cr.approving_owners([r], OWNERS, "x", HEAD) == []


def test_paginated_output_parses():
    raw = json.dumps([{"id": 1}]) + json.dumps([{"id": 2}]) + "\n"
    assert cr.load_paginated(raw) == [{"id": 1}, {"id": 2}]


def test_cmd_approved_missing_codeowners_fails_closed(monkeypatch, capsys):
    monkeypatch.setattr(cr, "CODEOWNERS", Path("does-not-exist"))
    monkeypatch.setattr(cr, "_run", lambda cmd: json.dumps([rv("Ovilli", "APPROVED")]))
    assert cr.cmd_approved("o/n", 1, HEAD, "author") == 1
    assert "no owners" in capsys.readouterr().out


# --- required (real git repo) -------------------------------------------

def _git(cwd, *args):
    cfg = ["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"]
    subprocess.run(["git", *cfg, *args], cwd=cwd, check=True, capture_output=True)


@pytest.fixture
def repo(tmp_path, monkeypatch):
    (tmp_path / ".github").mkdir()
    (tmp_path / ".github" / "core-paths.txt").write_text("core/**\n", encoding="utf-8")
    (tmp_path / "scripts").mkdir()
    (tmp_path / "scripts" / "contract_pins.py").write_text("", encoding="utf-8")  # prints nothing
    (tmp_path / "core").mkdir()
    (tmp_path / "core" / "a.txt").write_text("1\n2\n3\n", encoding="utf-8")
    _git(tmp_path, "init", "-q", "-b", "main")
    _git(tmp_path, "add", "-A")
    _git(tmp_path, "commit", "-q", "-m", "base")
    monkeypatch.setattr(cr, "ROOT", tmp_path)
    monkeypatch.setattr(cr, "CORE_PATHS", tmp_path / ".github" / "core-paths.txt")
    return tmp_path


def _commit(repo, text):
    (repo / "core" / "a.txt").write_text(text, encoding="utf-8")
    _git(repo, "commit", "-q", "-am", "change")


def test_required_small_core_change_exempt(repo, capsys):
    _git(repo, "checkout", "-q", "-b", "pr")
    _commit(repo, "1\n2\nX\n")  # 1 added + 1 removed = 2
    assert cr.cmd_required("main", "pr") == 0
    assert "not required" in capsys.readouterr().out


def test_required_large_core_change(repo, capsys):
    _git(repo, "checkout", "-q", "-b", "pr")
    _commit(repo, "X\nY\nZ\n")  # 6
    assert cr.cmd_required("main", "pr") == 0
    out = capsys.readouterr().out
    assert out.startswith("owner review required") and "core/a.txt (6)" in out


def test_required_non_core_change_ignored(repo, capsys):
    _git(repo, "checkout", "-q", "-b", "pr")
    (repo / "other.txt").write_text("\n".join("x" * 10 for _ in range(50)), encoding="utf-8")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "other")
    cr.cmd_required("main", "pr")
    assert "not required" in capsys.readouterr().out


def test_required_when_pins_report_breaking(repo, capsys):
    (repo / "scripts" / "contract_pins.py").write_text(
        "print('r_surface: R.x removed')\n", encoding="utf-8")
    _git(repo, "checkout", "-q", "-b", "pr")
    _commit(repo, "1\n2\n3\n4\n")  # 1 line only
    cr.cmd_required("main", "pr")
    out = capsys.readouterr().out
    assert out.startswith("owner review required") and "R.x removed" in out
