#!/usr/bin/env python3
"""Contract pins: snapshots of the published names, so CI fails when one goes.

Three contracts are pinned under ``data/contracts/``:

  r_surface.txt     every dotted path of the Lua ``R.*`` API (``R.events.on``)
  manifest_ops.txt  ``<family>:<op>`` for the ability and terrain edit ops
  json_bridge.txt   ``abi <N>``, then each json_bridge command and argument

Additions are free. A removed or renamed name fails ``check`` / ``write``
unless ``deprecated.txt`` lists it as an alias for one release:

  <pin> <old> -> <new> until-loader <N>

Subcommands:
  write [--surface FILE]   regenerate the snapshots (refuses on a breaking removal)
  check [--surface FILE]   fail on any difference from the committed snapshots
  breaking --base <ref>    list breaking changes against a git ref (always exit 0)

``--surface FILE`` is the output of
``lua5.4 tests/lua/rsmm_spec.lua src/loader/lib --dump-surface``. Without it the
R surface is not generated or compared. Stdlib only.
"""

from __future__ import annotations

import argparse
import difflib
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CONTRACTS = ROOT / "data" / "contracts"
LOADER_VERSION = ROOT / "data" / "loader_version.json"

PINS = ("r_surface", "manifest_ops", "json_bridge")
_ALIAS_RE = re.compile(r"^(\w+) (.+?) -> (.+?) until-loader (\d+)$")


# --- parsing -----------------------------------------------------------------

def parse_lines(text: str) -> list[str]:
    return [ln.strip() for ln in text.splitlines() if ln.strip()]


def split_bridge(text: str) -> tuple[str | None, list[str]]:
    """``json_bridge.txt`` -> (abi line or None, the command lines)."""
    lines = parse_lines(text)
    if lines and lines[0].startswith("abi "):
        return lines[0], lines[1:]
    return None, lines


def parse_deprecated(text: str) -> list[dict]:
    """Alias entries; raises ValueError on a malformed line."""
    out = []
    for n, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        m = _ALIAS_RE.match(line)
        if not m or m.group(1) not in PINS:
            raise ValueError(
                f"deprecated.txt line {n}: expected "
                f"'<{'|'.join(PINS)}> <old> -> <new> until-loader <N>', got {line!r}")
        out.append({"pin": m.group(1), "old": m.group(2), "new": m.group(3),
                    "until": int(m.group(4))})
    return out


# --- the comparison rules (unit-tested) ---------------------------------------

def uncovered_removals(pin: str, old: list[str], new: list[str],
                       aliases: list[dict]) -> list[str]:
    """Names in ``old`` missing from ``new`` that no deprecated alias covers."""
    covered = {a["old"] for a in aliases if a["pin"] == pin}
    return sorted(set(old) - set(new) - covered)


def expired_aliases(aliases: list[dict], loader_version: int) -> list[dict]:
    return [a for a in aliases if a["until"] < loader_version]


def bridge_breaking(old_text: str, new_text: str, aliases: list[dict]) -> list[str]:
    """Breaking changes between two ``json_bridge.txt`` bodies."""
    old_abi, old_lines = split_bridge(old_text)
    new_abi, new_lines = split_bridge(new_text)
    out = []
    if old_abi != new_abi:
        out.append(f"json_bridge: {old_abi} -> {new_abi}")
    for name in uncovered_removals("json_bridge", old_lines, new_lines, aliases):
        out.append(f"json_bridge: {name} removed")
    return out


def write_refusals(committed: dict[str, str], new: dict[str, str],
                   aliases: list[dict]) -> list[str]:
    """Why ``write`` must refuse. A bridge line may only go with an abi bump."""
    out = []
    for pin, text in new.items():
        old = committed.get(pin)
        if old is None:
            continue
        if pin == "json_bridge":
            old_abi, old_lines = split_bridge(old)
            new_abi, new_lines = split_bridge(text)
            if old_abi == new_abi:
                out += [f"json_bridge: {n} removed, abi unchanged"
                        for n in uncovered_removals(pin, old_lines, new_lines, aliases)]
        else:
            out += [f"{pin}: {n} removed"
                    for n in uncovered_removals(pin, parse_lines(old),
                                                parse_lines(text), aliases)]
    return out


# --- generating the snapshots --------------------------------------------------

def _body(lines) -> str:
    return "\n".join(sorted(set(lines))) + "\n"


def gen_manifest_ops() -> str:
    sys.path.insert(0, str(ROOT / "src"))
    from rsmm.engine import ability_edit, terrain
    return _body([f"ability:{op}" for op in ability_edit._OPS]
                 + [f"terrain:{op}" for op in terrain.OPS])


def _walk_parser(parser: argparse.ArgumentParser, prefix: str, out: list[str]) -> None:
    for action in parser._actions:
        if isinstance(action, argparse._SubParsersAction):
            for name, sub in action.choices.items():
                cmd = f"{prefix} {name}".strip()
                out.append(cmd)
                _walk_parser(sub, cmd, out)
        elif action.option_strings:
            flag = max(action.option_strings, key=len)
            if flag not in ("-h", "--help"):
                out.append(f"{prefix} {flag}")
        else:
            out.append(f"{prefix} {action.dest}")


def gen_json_bridge() -> str:
    sys.path.insert(0, str(ROOT / "src"))
    from rsmm.cli import json_bridge
    lines: list[str] = []
    _walk_parser(json_bridge.build_parser(), "", lines)
    return f"abi {json_bridge.BRIDGE_ABI}\n" + _body(lines)


def generate(surface: Path | None) -> dict[str, str]:
    snaps = {"manifest_ops": gen_manifest_ops(), "json_bridge": gen_json_bridge()}
    if surface is not None:
        snaps["r_surface"] = _body(parse_lines(surface.read_text(encoding="utf-8")))
    return snaps


# --- IO ---------------------------------------------------------------------------

def _read(name: str) -> str | None:
    path = CONTRACTS / f"{name}.txt"
    return path.read_text(encoding="utf-8").replace("\r\n", "\n") if path.exists() else None


def _aliases() -> list[dict]:
    return parse_deprecated(_read("deprecated") or "")


def _loader_version() -> int:
    return int(json.loads(LOADER_VERSION.read_text(encoding="utf-8"))["loader_version"])


def _git_show(ref: str, rel: str) -> str:
    r = subprocess.run(["git", "-C", str(ROOT), "show", f"{ref}:{rel}"],
                       capture_output=True, text=True, encoding="utf-8")
    return r.stdout.replace("\r\n", "\n") if r.returncode == 0 else ""


# --- commands -----------------------------------------------------------------------

def cmd_write(surface: Path | None) -> int:
    snaps = generate(surface)
    committed = {p: t for p in snaps if (t := _read(p)) is not None}
    refusals = write_refusals(committed, snaps, _aliases())
    if refusals:
        print("refusing to write: a pinned name was removed without a deprecated entry.")
        for r in refusals:
            print(f"  {r}")
        print("Add '<pin> <old> -> <new> until-loader <N>' to data/contracts/deprecated.txt "
              "(for json_bridge, bump BRIDGE_ABI instead).")
        return 1
    CONTRACTS.mkdir(parents=True, exist_ok=True)
    for pin, text in snaps.items():
        (CONTRACTS / f"{pin}.txt").write_text(text, encoding="utf-8", newline="\n")
        print(f"wrote data/contracts/{pin}.txt")
    return 0


def cmd_check(surface: Path | None) -> int:
    snaps = generate(surface)
    failed = False
    if surface is None:
        print("r_surface.txt skipped (no --surface FILE)")
    try:
        aliases = _aliases()
    except ValueError as exc:
        print(exc)
        return 1
    for a in expired_aliases(aliases, _loader_version()):
        failed = True
        print(f"expired alias: {a['pin']} {a['old']} -> {a['new']} "
              f"(until-loader {a['until']} < loader_version {_loader_version()}); "
              "remove it from data/contracts/deprecated.txt")
    for pin, text in snaps.items():
        committed = _read(pin)
        if committed == text:
            continue
        failed = True
        diff = difflib.unified_diff((committed or "").splitlines(), text.splitlines(),
                                    f"committed {pin}.txt", f"generated {pin}.txt",
                                    lineterm="", n=0)
        print(f"data/contracts/{pin}.txt differs from the code:")
        print("\n".join(diff))
    if failed:
        flag = " --surface FILE" if surface is not None else ""
        print(f"\nIf the change is intended, run: python scripts/contract_pins.py write{flag}")
        return 1
    return 0


def cmd_breaking(base: str) -> int:
    aliases = _aliases()
    for pin in PINS:
        old = _git_show(base, f"data/contracts/{pin}.txt")
        new = _read(pin) or ""
        if not old:
            continue
        if pin == "json_bridge":
            out = bridge_breaking(old, new, aliases)
        else:
            out = [f"{pin}: {n} removed" for n in
                   uncovered_removals(pin, parse_lines(old), parse_lines(new), aliases)]
        for line in out:
            print(line)
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("write", "check"):
        p = sub.add_parser(name)
        p.add_argument("--surface", type=Path, help="output of rsmm_spec.lua --dump-surface")
    p = sub.add_parser("breaking")
    p.add_argument("--base", required=True, help="git ref to compare against")
    args = ap.parse_args(argv)
    if args.cmd == "write":
        return cmd_write(args.surface)
    if args.cmd == "check":
        return cmd_check(args.surface)
    return cmd_breaking(args.base)


if __name__ == "__main__":
    sys.exit(main())
