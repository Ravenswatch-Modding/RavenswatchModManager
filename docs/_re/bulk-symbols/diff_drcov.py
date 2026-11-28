"""Diff two DynamoRIO drcov coverage logs against Ravenswatch.exe .pdata and symbols."""

from __future__ import annotations

import argparse
import bisect
import json
import os
import re
import struct
import sys
from pathlib import Path

DEFAULT_EXE = r"E:\Documents\Projects\Ravenswatch\RSMM\evidence\game-snapshots\e04db5c457dd\Ravenswatch-e04db5c457dd.exe"
DEFAULT_SYMBOLS = r"E:\Documents\Projects\Ravenswatch\RSMM\repo\RavenswatchModManager\docs\_re\out\symbols.json"
IMAGE_BASE = 0x140000000


def pick_log_file(path_str: str) -> Path:
    """Resolve a drcov file path or pick the largest drcov*.log inside a directory."""
    p = Path(path_str)
    if p.is_file():
        return p
    if p.is_dir():
        candidates = sorted(p.glob("drcov*.log"), key=lambda x: x.stat().st_size, reverse=True)
        if not candidates:
            raise FileNotFoundError(f"No drcov*.log files found in {p}")
        return candidates[0]
    raise FileNotFoundError(f"Path not found: {p}")


def parse_drcov(path: Path, target_module_name: str = "ravenswatch.exe") -> dict:
    """Parse a drcov v2/v3 log (binary or text BB table) for target module RVAs."""
    raw = path.read_bytes()
    bb_marker = b"BB Table: "
    idx = raw.find(bb_marker)
    if idx < 0:
        raise ValueError(f"Missing 'BB Table:' header in {path}")

    line_end = raw.find(b"\n", idx)
    if line_end < 0:
        raise ValueError(f"Malformed 'BB Table:' line in {path}")

    header_bytes = raw[: line_end + 1]
    body = raw[line_end + 1 :]
    header_text = header_bytes.decode("utf-8", errors="replace")

    version = 2
    m_ver = re.search(r"DRCOV VERSION:\s*(\d+)", header_text)
    if m_ver:
        version = int(m_ver.group(1))

    m_bb = re.search(r"BB Table:\s*(\d+)\s+bbs", header_text)
    bb_count = int(m_bb.group(1)) if m_bb else 0

    target_lower = target_module_name.lower()
    mod_segments: dict[int, tuple[int, int]] = {}
    mod_bases_by_path: dict[str, int] = {}
    raw_mods: list[tuple[int, int, int, str]] = []

    in_mod_table = False
    has_seg_offset = False
    for line in header_text.splitlines():
        if line.startswith("Module Table:"):
            in_mod_table = True
            continue
        if line.startswith("BB Table:"):
            in_mod_table = False
            continue
        if not in_mod_table:
            continue
        if line.startswith("Columns:"):
            has_seg_offset = "offset" in line.lower()
            continue
        parts = [p.strip() for p in line.split(",")]
        if len(parts) < 4 or not parts[0].isdigit():
            continue
        mod_id = int(parts[0])
        try:
            if has_seg_offset and len(parts) >= 8:
                # id, containing_id, start, end, entry, offset, ... , path
                seg_start = int(parts[2], 16)
                mod_offset = int(parts[5], 16)
                mod_path = parts[-1]
            else:
                # v2 or v3 short columns: id, start, end, entry, [checksum, timestamp,] path
                seg_start = int(parts[1], 16)
                mod_offset = 0
                mod_path = parts[-1]
        except ValueError:
            continue
        if mod_path.lower().endswith("\\" + target_lower) or mod_path.lower() == target_lower:
            norm = mod_path.lower()
            if norm not in mod_bases_by_path or seg_start < mod_bases_by_path[norm]:
                mod_bases_by_path[norm] = seg_start
            raw_mods.append((mod_id, seg_start, mod_offset, norm))

    for mod_id, seg_start, mod_offset, norm in raw_mods:
        base = mod_bases_by_path[norm]
        seg_rva = (seg_start - base) if version >= 3 else 0
        mod_segments[mod_id] = (seg_rva, mod_offset)

    is_text = body.lstrip().startswith(b"module[")
    bb_rvas: set[int] = set()
    total_bbs_target = 0

    if is_text:
        text_body = body.decode("utf-8", errors="replace")
        for m in re.finditer(r"module\[\s*(\d+)\]:\s*(0x[0-9a-fA-F]+),\s*(\d+)", text_body):
            mod_id = int(m.group(1))
            if mod_id not in mod_segments:
                continue
            start_off = int(m.group(2), 16)
            seg_rva, _ = mod_segments[mod_id]
            bb_rvas.add(seg_rva + start_off)
            total_bbs_target += 1
    else:
        entry_size = 8
        avail = len(body) // entry_size
        count = min(bb_count, avail) if bb_count > 0 else avail
        for offset in range(0, count * entry_size, entry_size):
            start_off, _size, mod_id = struct.unpack_from("<IHH", body, offset)
            if mod_id not in mod_segments:
                continue
            seg_rva, _ = mod_segments[mod_id]
            bb_rvas.add(seg_rva + start_off)
            total_bbs_target += 1

    return {
        "file": str(path),
        "version": version,
        "header_bb_count": bb_count,
        "target_mod_ids": sorted(mod_segments.keys()),
        "target_bb_hits": total_bbs_target,
        "target_unique_bb_rvas": bb_rvas,
    }


def load_pdata_ranges(exe_path: Path) -> list[tuple[int, int]]:
    """Read PE .pdata RUNTIME_FUNCTION (begin_va, end_va) pairs."""
    data = exe_path.read_bytes()
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    if data[e_lfanew : e_lfanew + 4] != b"PE\x00\x00":
        raise ValueError(f"Not a valid PE file: {exe_path}")
    num_sections = struct.unpack_from("<H", data, e_lfanew + 6)[0]
    opt_hdr_size = struct.unpack_from("<H", data, e_lfanew + 20)[0]
    sec_off = e_lfanew + 24 + opt_hdr_size

    pdata_raw = None
    pdata_size = 0
    for i in range(num_sections):
        off = sec_off + i * 40
        name = data[off : off + 8].rstrip(b"\x00").decode("ascii", errors="replace")
        vsize, _va, rsize, ptr_raw = struct.unpack_from("<IIII", data, off + 8)
        if name == ".pdata":
            pdata_raw = ptr_raw
            pdata_size = min(vsize, rsize)
            break

    if pdata_raw is None:
        raise ValueError(f"No .pdata section in {exe_path}")

    ranges: list[tuple[int, int]] = []
    for off in range(pdata_raw, pdata_raw + pdata_size - 11, 12):
        begin_rva, end_rva, _unwind = struct.unpack_from("<III", data, off)
        if begin_rva == 0:
            continue
        ranges.append((IMAGE_BASE + begin_rva, IMAGE_BASE + end_rva))
    ranges.sort()
    return ranges


def load_symbols(sym_path: Path) -> tuple[list[int], dict[int, str]]:
    """Load sorted Ghidra function start VAs and names from docs/_re/out/symbols.json."""
    syms = json.loads(sym_path.read_text(encoding="utf-8"))
    by_addr: dict[int, str] = {}
    for entry in syms:
        va = int(entry["addr"], 16)
        by_addr[va] = entry["name"]
    starts = sorted(by_addr.keys())
    return starts, by_addr


def map_rvas_to_functions(
    bb_rvas: set[int],
    pdata_ranges: list[tuple[int, int]],
    sym_starts: list[int],
    sym_names: dict[int, str],
) -> tuple[set[int], int]:
    """Map BB RVAs to containing Ghidra function start (or .pdata start if missing)."""
    pdata_begins = [r[0] for r in pdata_ranges]
    funcs: set[int] = set()
    unmapped = 0

    for rva in bb_rvas:
        va = IMAGE_BASE + rva
        # First check exact symbol interval via bisect_right on sym_starts
        si = bisect.bisect_right(sym_starts, va) - 1
        pi = bisect.bisect_right(pdata_begins, va) - 1
        matched = False
        if pi >= 0 and pdata_ranges[pi][0] <= va < pdata_ranges[pi][1]:
            p_start = pdata_ranges[pi][0]
            if p_start in sym_names:
                funcs.add(p_start)
                matched = True
            elif si >= 0 and sym_starts[si] <= p_start:
                # Chained .pdata fragment inside an earlier Ghidra function
                funcs.add(sym_starts[si])
                matched = True
            else:
                funcs.add(p_start)
                matched = True
        elif si >= 0 and va - sym_starts[si] < 0x10000:
            funcs.add(sym_starts[si])
            matched = True
        if not matched:
            unmapped += 1
    return funcs, unmapped


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("baseline", help="Baseline drcov log or directory")
    ap.add_argument("action", help="Action drcov log or directory")
    ap.add_argument("--exe", default=DEFAULT_EXE, help="Snapshot Ravenswatch.exe for .pdata")
    ap.add_argument("--symbols", default=DEFAULT_SYMBOLS, help="Ghidra docs/_re/out/symbols.json")
    ap.add_argument("--out", default="", help="Optional JSON output path")
    args = ap.parse_args()

    base_path = pick_log_file(args.baseline)
    act_path = pick_log_file(args.action)

    base_cov = parse_drcov(base_path)
    act_cov = parse_drcov(act_path)
    pdata_ranges = load_pdata_ranges(Path(args.exe))
    sym_starts, sym_names = load_symbols(Path(args.symbols))

    base_funcs, base_unmapped = map_rvas_to_functions(
        base_cov["target_unique_bb_rvas"], pdata_ranges, sym_starts, sym_names
    )
    act_funcs, act_unmapped = map_rvas_to_functions(
        act_cov["target_unique_bb_rvas"], pdata_ranges, sym_starts, sym_names
    )

    action_only = sorted(act_funcs - base_funcs)
    baseline_only = sorted(base_funcs - act_funcs)
    shared = len(act_funcs & base_funcs)

    print(f"Baseline log: {base_cov['file']} (v{base_cov['version']}, total_bbs={base_cov['header_bb_count']}, target_bbs={len(base_cov['target_unique_bb_rvas'])}, funcs={len(base_funcs)}, unmapped={base_unmapped})")
    print(f"Action   log: {act_cov['file']} (v{act_cov['version']}, total_bbs={act_cov['header_bb_count']}, target_bbs={len(act_cov['target_unique_bb_rvas'])}, funcs={len(act_funcs)}, unmapped={act_unmapped})")
    print(f"Shared functions: {shared}")
    print(f"Baseline-only functions: {len(baseline_only)}")
    print(f"Action-only functions (action - baseline): {len(action_only)}")

    action_named = [
        (f"0x{va:x}", sym_names.get(va, f"PDATA_{va:x}"))
        for va in action_only
        if not sym_names.get(va, "FUN_").startswith("FUN_")
    ]
    action_fun = [
        (f"0x{va:x}", sym_names.get(va, f"PDATA_{va:x}"))
        for va in action_only
        if sym_names.get(va, "FUN_").startswith("FUN_")
    ]

    print(f"Action-only named={len(action_named)}, unnamed FUN_={len(action_fun)}")
    print("\nTop named action-only functions:")
    for addr, name in action_named[:40]:
        print(f"  {addr}  {name}")
    print("\nTop unnamed action-only functions:")
    for addr, name in action_fun[:30]:
        print(f"  {addr}  {name}")

    if args.out:
        out_obj = {
            "baseline": {
                "file": base_cov["file"],
                "version": base_cov["version"],
                "header_bb_count": base_cov["header_bb_count"],
                "target_unique_bbs": len(base_cov["target_unique_bb_rvas"]),
                "unique_functions": len(base_funcs),
                "unmapped_bbs": base_unmapped,
            },
            "action": {
                "file": act_cov["file"],
                "version": act_cov["version"],
                "header_bb_count": act_cov["header_bb_count"],
                "target_unique_bbs": len(act_cov["target_unique_bb_rvas"]),
                "unique_functions": len(act_funcs),
                "unmapped_bbs": act_unmapped,
            },
            "shared_functions": shared,
            "baseline_only_count": len(baseline_only),
            "action_only_count": len(action_only),
            "action_only": [
                {"addr": f"0x{va:x}", "name": sym_names.get(va, f"PDATA_{va:x}")}
                for va in action_only
            ],
        }
        Path(args.out).write_text(json.dumps(out_obj, indent=1), encoding="utf-8")
        print(f"\nWrote diff JSON to {args.out}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
