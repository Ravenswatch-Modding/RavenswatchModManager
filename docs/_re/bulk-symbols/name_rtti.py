#!/usr/bin/env python3
"""Phase 2: Name class methods, ctors/dtors, and interface methods from MSVC RTTI."""

from __future__ import annotations

import hashlib
import json
import re
import struct
import subprocess
import sys
from collections import Counter, defaultdict
from dataclasses import dataclass
from pathlib import Path

REPO = Path(r"E:\Documents\Projects\Ravenswatch\RSMM\repo\RavenswatchModManager")
EV = Path(r"E:\Documents\Projects\Ravenswatch\RSMM\evidence\2026-11-26-bulk-symbols")
SNAP_EXE = Path(r"E:\Documents\Projects\Ravenswatch\RSMM\evidence\game-snapshots\e04db5c457dd\Ravenswatch-e04db5c457dd.exe")
VFT_PATH = REPO / "docs" / "_re" / "out_new" / "vftables.jsonl"
DEC_PATH = REPO / "docs" / "_re" / "out_new" / "decompiled_new.jsonl"
SYM_PATH = REPO / "docs" / "_re" / "out" / "symbols.json"

sys.path.insert(0, str(REPO / "scripts"))
import gen_function_patterns as gen  # noqa: E402

NOISE_IFACE = re.compile(
    r"^(std|pplx|Concurrency|boost|Stormancer|rapidjson|msgpack)"
    r"|_Func_impl|_PPLTaskHandle|_TypeSelector|TaskHandle"
)
STUB_MIN_SHARE = 40
FAMILY_MAX = 60

_DEL_DTOR_RE = re.compile(
    r"(?:param_2|uParam2|_param_2)\s*&\s*1|&\s*1\b.*(?:operator_delete|free\()",
    re.DOTALL,
)
_LHS_THIS_STORE_RE = re.compile(r"(\*\s*(?:\([^)]+\)\s*)?\(?\s*param_1\b|param_1\s*\[)")


@dataclass
class VtableRec:
    sym: str
    vt_va: int
    this_off: int
    td_name: str
    base_tds: set[str]
    slots: list[tuple[int, int]]  # (slot_i, va_int)


def family_key(cls: str) -> str:
    base = cls.split("::")[-1]
    base = re.sub(r"^(oC|oI|oe|dt)+", "", base)
    for suffix in (
        "ConditionData", "ConditionSettings", "EntityCpntSettings",
        "EntityCpnt", "Settings", "Definition", "Data", "Manager",
        "Component", "Controller", "Resource", "Layer", "Event",
    ):
        if base.endswith(suffix) and len(base) > len(suffix):
            return suffix
    return base


def sanitize_class_name(sym: str) -> str:
    cls = sym[:-len("::vftable")] if sym.endswith("::vftable") else sym
    cls = re.sub(r"\s+", "_", cls)
    # Replace characters likely to cause Ghidra symbol-name issues while keeping C++ tokens.
    cls = re.sub(r"[`'\";/\x00-\x1f\x7f]", "_", cls)
    if len(cls) > 160:
        h = hashlib.sha1(cls.encode("utf-8")).hexdigest()[:8]
        cls = f"{cls[:150]}_{h}"
    return cls


def demangle_td_fallback(td_name: str) -> str:
    """Fallback demangler for abstract base TypeDescriptors with no vtable of their own."""
    s = td_name
    if s.startswith(".?AV") or s.startswith(".?AU"):
        s = s[4:]
    if s.endswith("@@"):
        s = s[:-2]
    parts = [p for p in s.split("@") if p]
    parts.reverse()
    return sanitize_class_name("::".join(parts) if parts else td_name)


def parse_rtti(exe_path: Path, vft_rows: list[dict]):
    data = exe_path.read_bytes()
    base, secs = gen.parse_pe(data)

    def read_u64(va: int) -> int | None:
        off = gen.va_to_offset(va, base, secs)
        return struct.unpack_from("<Q", data, off)[0] if off is not None and off + 8 <= len(data) else None

    def read_u32(rva: int) -> int | None:
        off = gen.va_to_offset(base + rva, base, secs)
        return struct.unpack_from("<I", data, off)[0] if off is not None and off + 4 <= len(data) else None

    def read_cstr(rva: int) -> str:
        off = gen.va_to_offset(base + rva, base, secs)
        if off is None:
            return ""
        end = data.find(b"\x00", off, off + 512)
        return data[off:end].decode("ascii", "replace") if end >= 0 else ""

    vts: list[VtableRec] = []
    td_to_cls: dict[str, str] = {}
    td_to_pri_off: dict[str, int] = {}
    base_tds_by_td: dict[str, set[str]] = defaultdict(set)

    for row in vft_rows:
        vt_va = int(row["addr"], 16)
        sym = row["sym"]
        col_va = read_u64(vt_va - 8)
        col_off = gen.va_to_offset(col_va, base, secs) if col_va else None
        if col_off is None:
            continue
        sig, this_off, _cd_off, td_rva, chd_rva = struct.unpack_from("<IIIII", data, col_off)
        if sig != 1:
            continue
        td_name = read_cstr(td_rva + 16)
        chd_off = gen.va_to_offset(base + chd_rva, base, secs)
        if chd_off is None:
            continue
        _chd_sig, _chd_attr, num_bases, base_arr_rva = struct.unpack_from("<IIII", data, chd_off)
        base_tds: set[str] = set()
        for i in range(min(num_bases, 64)):
            bd_rva = read_u32(base_arr_rva + i * 4)
            if not bd_rva:
                continue
            bd_td_rva = read_u32(bd_rva)
            if bd_td_rva is not None:
                base_tds.add(read_cstr(bd_td_rva + 16))
        slots = [(int(s["i"]), int(s["va"], 16)) for s in row.get("slots", [])]
        rec = VtableRec(sym=sym, vt_va=vt_va, this_off=this_off, td_name=td_name, base_tds=base_tds, slots=slots)
        vts.append(rec)
        base_tds_by_td[td_name].update(base_tds)
        cls = sanitize_class_name(sym)
        if td_name not in td_to_cls or this_off < td_to_pri_off[td_name]:
            td_to_cls[td_name] = cls
            td_to_pri_off[td_name] = this_off

    return vts, td_to_cls, base_tds_by_td


def extract_vft_rhs(rhs: str, known_syms: dict[str, VtableRec]) -> VtableRec | None:
    s = rhs.strip()
    # Strip leading C casts like `(undefined **)` without stripping inner template `(void)`
    while s.startswith("("):
        depth = 0
        end = -1
        for idx, ch in enumerate(s):
            if ch == "(":
                depth += 1
            elif ch == ")":
                depth -= 1
                if depth == 0:
                    end = idx
                    break
        if end < 0:
            break
        s = s[end + 1:].lstrip()
    if s.startswith("&"):
        s = s[1:].lstrip()
    idx = s.find("::vftable")
    if idx < 0:
        return None
    cand = s[:idx + len("::vftable")]
    return known_syms.get(cand)


def main() -> int:
    syms = json.loads(SYM_PATH.read_text(encoding="utf-8"))
    cur_name = {int(s["addr"], 16): s["name"] for s in syms}

    vft_rows = [json.loads(ln) for ln in VFT_PATH.open(encoding="utf-8")]
    vts, td_to_cls, base_tds_by_td = parse_rtti(SNAP_EXE, vft_rows)
    known_syms = {v.sym: v for v in vts}

    dec_code: dict[int, str] = {}
    for ln in DEC_PATH.open(encoding="utf-8"):
        d = json.loads(ln)
        dec_code[int(d["addr"], 16)] = d.get("code") or ""

    # 1. Slots pass
    slot_occ: dict[int, list[tuple[VtableRec, int]]] = defaultdict(list)
    slot_vts_seen: dict[int, set[int]] = defaultdict(set)
    for v in vts:
        for slot_i, va in v.slots:
            if (v.vt_va, slot_i) not in [(x.vt_va, si) for x, si in slot_occ[va]]:
                slot_occ[va].append((v, slot_i))
            slot_vts_seen[va].add(v.vt_va)

    entries: dict[int, dict] = {}
    stats = Counter()

    for va, occ in sorted(slot_occ.items()):
        if not cur_name.get(va, "").startswith("FUN_"):
            stats["slot_already_named"] += 1
            continue
        n_vts = len(slot_vts_seen[va])
        if n_vts > 50:
            stats["slot_skip_stub_gt50"] += 1
            continue
        if n_vts == 1:
            win_v, win_i = occ[0]
            stats["slot_unique"] += 1
        else:
            sharer_tds = {v.td_name for v, _ in occ}
            base_cands = [td for td in sharer_tds if all(td in v.base_tds for v, _ in occ)]
            if len(base_cands) != 1:
                stats["slot_skip_ambiguous_shared"] += 1
                continue
            win_td = base_cands[0]
            matching = [(v, i) for v, i in occ if v.td_name == win_td]
            win_v, win_i = min(matching, key=lambda t: (t[0].this_off, t[1]))
            stats["slot_shared_single_base"] += 1

        cls = td_to_cls.get(win_v.td_name, sanitize_class_name(win_v.sym))
        body = dec_code.get(va, "")
        is_del_dtor = (
            win_i == 0
            and body.count("\n") < 40
            and bool(_DEL_DTOR_RE.search(body))
        )
        suffix = "dtor_deleting" if is_del_dtor else f"vf{win_i}"
        if win_i == 0:
            stats["slot0_dtor_deleting" if is_del_dtor else "slot0_vf0"] += 1
        if win_v.this_off > 0:
            suffix = f"sub0x{win_v.this_off:x}_{suffix}"
        name = f"{cls}::{suffix}"
        note = f"[rtti slot] {win_v.sym} slot {win_i} (vts={n_vts})"
        if is_del_dtor:
            note += " [deleting dtor]"
        entries[va] = {"addr": f"0x{va:x}", "name": name, "kind": "function", "note": note}

    # 2. Constructors & destructors pass
    for va, code in sorted(dec_code.items()):
        if not cur_name.get(va, "").startswith("FUN_") or va in entries:
            continue
        if "::vftable" not in code:
            continue
        stored: list[VtableRec] = []
        seen_vt: set[int] = set()
        for ln in code.splitlines():
            if "::vftable" not in ln or "param_1" not in ln or "=" not in ln:
                continue
            lhs, rhs = ln.split("=", 1)
            if not _LHS_THIS_STORE_RE.search(lhs):
                continue
            vrec = extract_vft_rhs(rhs, known_syms)
            if vrec is not None and vrec.vt_va not in seen_vt:
                seen_vt.add(vrec.vt_va)
                stored.append(vrec)
        if not stored:
            continue
        stored_tds = {v.td_name for v in stored}
        cands = [td for td in stored_tds if stored_tds <= base_tds_by_td.get(td, set())]
        if len(cands) != 1:
            stats["ctor_skip_ambiguous"] += 1
            continue
        win_td = cands[0]
        cls = td_to_cls.get(win_td, demangle_td_fallback(win_td))
        sample_syms = ", ".join(v.sym.removesuffix("::vftable") for v in stored[:4])
        if len(stored) > 4:
            sample_syms += f", +{len(stored) - 4} more"
        entries[va] = {
            "addr": f"0x{va:x}",
            "name": f"{cls}::ctor_or_dtor",
            "kind": "function",
            "note": f"[rtti ctor/dtor] stores {len(stored)} vtable(s): {sample_syms}",
        }
        stats["ctor_or_dtor"] += 1

    # 3. Interface methods pass (mine_vtable_interfaces.py)
    iface_raw = subprocess.check_output(
        [sys.executable, str(REPO / "tools" / "mine_vtable_interfaces.py"), "--json", "--top", "1000"],
        cwd=str(REPO),
        text=True,
    )
    iface_rows = json.loads(iface_raw)
    want_ifaces = {(r["family"], int(r["slot"])): r for r in iface_rows}

    # Reconstruct non-stub implementations per (family, slot) identically to mine_vtable_interfaces.py
    share_cnt = Counter()
    for v in vts:
        for _, va in v.slots:
            share_cnt[va] += 1
    stubs = {va for va, n in share_cnt.items() if n >= STUB_MIN_SHARE}

    fams: dict[str, list[VtableRec]] = defaultdict(list)
    for v in vts:
        cls_raw = v.sym[:-len("::vftable")]
        if NOISE_IFACE.search(cls_raw):
            continue
        fams[family_key(cls_raw)].append(v)
    fams = {k: m for k, m in fams.items() if len(m) <= FAMILY_MAX}

    for (fam, slot), info in want_ifaces.items():
        members = fams.get(fam, [])
        impl_vts: dict[int, list[VtableRec]] = defaultdict(list)
        for m in members:
            if slot >= len(m.slots):
                continue
            _, va = m.slots[slot]
            if va in stubs:
                continue
            impl_vts[va].append(m)
        for va, sharers in impl_vts.items():
            ev_note = f"interface family={fam} slot={slot} behav={info['behaviour']} impls={info['implementations']}"
            if va in entries:
                entries[va]["note"] += f" | {ev_note}"
                stats["iface_enriched"] += 1
            elif cur_name.get(va, "").startswith("FUN_"):
                # Find unique most-derived common ancestor in ClassHierarchyDescriptor intersection
                common = set.intersection(*(s.base_tds for s in sharers))
                if not common:
                    stats["iface_skip_no_common_base"] += 1
                    continue
                most_derived = [td for td in common if common <= base_tds_by_td.get(td, set())]
                if len(most_derived) != 1:
                    stats["iface_skip_ambiguous_base"] += 1
                    continue
                win_td = most_derived[0]
                cls = td_to_cls.get(win_td, demangle_td_fallback(win_td))
                entries[va] = {
                    "addr": f"0x{va:x}",
                    "name": f"{cls}::vf{slot}",
                    "kind": "function",
                    "note": f"[rtti {ev_note}] common base across {len(sharers)} sharer(s)",
                }
                stats["iface_added"] += 1

    # 4. Ensure globally unique names (avoid Ghidra/corpus ambiguity on multiple ctors/dtors)
    name_counts: Counter[str] = Counter()
    existing_taken = {n for n in cur_name.values() if not n.startswith("FUN_")}
    out_entries: list[dict] = []
    for va in sorted(entries):
        e = dict(entries[va])
        base_nm = e["name"]
        name_counts[base_nm] += 1
        nm = base_nm
        if name_counts[base_nm] > 1 or nm in existing_taken:
            idx = name_counts[base_nm]
            while f"{base_nm}_{idx}" in existing_taken:
                idx += 1
            nm = f"{base_nm}_{idx}"
            name_counts[base_nm] = idx
        existing_taken.add(nm)
        e["name"] = nm
        out_entries.append(e)

    (EV / "rtti_knowledge.json").write_text(
        json.dumps({"entries": out_entries}, indent=1),
        encoding="utf-8",
    )

    # 5. Build representative 10-entry test subset
    test_picks: list[dict] = []
    used_addrs: set[str] = set()

    def pick_one(pred):
        for e in out_entries:
            if e["addr"] not in used_addrs and pred(e):
                used_addrs.add(e["addr"])
                test_picks.append(e)
                return True
        return False

    # 3 template classes (including nested with <, >, ,, (, ), *)
    pick_one(lambda e: "<" in e["name"] and "(" in e["name"] and "*" in e["name"])
    pick_one(lambda e: "<" in e["name"] and "," in e["name"])
    pick_one(lambda e: "<" in e["name"] and "::vf" in e["name"])
    # 2 oC* / oe::dt::* game slot methods
    pick_one(lambda e: e["name"].startswith("oe::dt::") and "::vf" in e["name"])
    pick_one(lambda e: e["name"].startswith("oC") and "::vf" in e["name"])
    # 2 dtor_deleting
    pick_one(lambda e: "dtor_deleting" in e["name"] and "::" in e["name"])
    pick_one(lambda e: "dtor_deleting" in e["name"] and "<" in e["name"])
    # 2 ctor_or_dtor
    pick_one(lambda e: "ctor_or_dtor" in e["name"] and e["name"].startswith("oC"))
    pick_one(lambda e: "ctor_or_dtor" in e["name"] and "<" in e["name"])
    # 1 secondary vtable sub0x...
    pick_one(lambda e: "::sub0x" in e["name"])
    while len(test_picks) < 10:
        if not pick_one(lambda _e: True):
            break

    (EV / "rtti_test10.json").write_text(
        json.dumps({"entries": test_picks}, indent=1),
        encoding="utf-8",
    )

    non_alnum = sorted({ch for e in out_entries for ch in e["name"] if not ch.isalnum()})
    print(f"Total emitted entries: {len(out_entries)}")
    for k, v in sorted(stats.items()):
        print(f"  {k}: {v}")
    print("Non-alnum chars in names:", "".join(non_alnum))
    print("Test10 entries:")
    for e in test_picks:
        print(f"  {e['addr']} -> {e['name']}  ({e['note'][:70]})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
