#!/usr/bin/env python3
"""Phase 4: Name registrars/meta_cb, event emitters, unique strings, and wrappers."""

from __future__ import annotations

import bisect
import hashlib
import json
import re
import struct
import sys
from collections import Counter, defaultdict
from pathlib import Path

REPO = Path(r"E:\Documents\Projects\Ravenswatch\RSMM\repo\RavenswatchModManager")
EV = Path(r"E:\Documents\Projects\Ravenswatch\RSMM\evidence\2026-11-26-bulk-symbols")
SNAP_EXE = Path(r"E:\Documents\Projects\Ravenswatch\RSMM\evidence\game-snapshots\e04db5c457dd\Ravenswatch-e04db5c457dd.exe")
CLASS_IDS = EV / "class_ids_e04db5c457dd.json"
EVENT_NAMES = EV / "event_names_e04db5c457dd.json"
DEC_PATH = REPO / "docs" / "_re" / "out_new" / "decompiled_new.jsonl"
SYM_PATH = REPO / "docs" / "_re" / "out" / "symbols.json"
STR_PATH = REPO / "docs" / "_re" / "out" / "strings.json"
RSMM_SYM_PATH = REPO / "data" / "symbols.json"

sys.path.insert(0, str(REPO / "scripts"))
import gen_function_patterns as gen  # noqa: E402

BLOCKLIST_CALLEES = {
    "atexit",
    "_guard_check_icall",
    "_Init_thread_header",
    "_Init_thread_footer",
    "free",
    "operator_delete",
    "operator_new",
    "memset",
    "memcpy",
    "memmove",
    "__security_check_cookie",
    "Crc32_TableInit",
    "__CxxFrameHandler4",
    "__GSHandlerCheck",
}

_PRI1_RX = re.compile(
    r"(?:^[A-Za-z_]\w*::[A-Za-z_~]\w*|\[[A-Za-z0-9_ -]{2,24}\]|__cdecl\b|__thiscall\b|"
    r"[A-Za-z]:\\|\.\./\.\./|\.(?:cpp|cc|c|h|hpp|ot|entity)\b)",
    re.I,
)
_FORMAT_ONLY_RX = re.compile(r"^(?:%[0-9.#*+-]*[diuoxXfFeEgGaAcspn]|\\s|\W)+$")
_SHADER_NOISE_RX = re.compile(r"(?:;\s*\n|\bfloat[234]\b|\bTexture2D\b|\bSV_Target\b)")
_PARAM_ARG_RX = re.compile(
    r"^(?:\([^)]+\)\s*)*[*&]?\s*(?:\([^)]+\)\s*)*param_\d+(?:\s*\+\s*(?:0x[0-9a-f]+|\d+))?$"
)


def sanitize_ghidra_name(name: str, max_len: int = 160) -> str:
    s = re.sub(r"\s+", "_", name.strip())
    s = re.sub(r"[`'\";/\x00-\x1f\x7f]", "_", s)
    if len(s) > max_len:
        h = hashlib.sha1(s.encode("utf-8")).hexdigest()[:8]
        s = f"{s[:max_len - 10]}_{h}"
    return s


def sanitize_str_anchor(text: str) -> str:
    s = re.sub(r"[^A-Za-z0-9_]+", "_", text)
    s = re.sub(r"_+", "_", s).strip("_")
    if len(s) > 56:
        s = s[:56].rstrip("_")
    return f"str_{s}" if s else ""


def is_good_str_anchor(text: str) -> tuple[int, str] | None:
    if text.startswith(".?") or "\n" in text and text.count("\n") > 2:
        return None
    if _SHADER_NOISE_RX.search(text) or _FORMAT_ONLY_RX.match(text):
        return None
    alpha = sum(1 for c in text if c.isalpha())
    if alpha < 5:
        return None
    if _PRI1_RX.search(text):
        return (1, text)
    if len(text) >= 8 and len(text) <= 120 and alpha >= 6:
        if any(c in text for c in (" ", ":", "_", ".")) or re.search(r"[a-z][A-Z]", text):
            return (2, text)
    return None


def main() -> int:
    data = SNAP_EXE.read_bytes()
    base, secs = gen.parse_pe(data)
    text_sec = secs[0]
    text_bytes = data[text_sec["raw_off"]:text_sec["raw_off"] + text_sec["raw_size"]]
    tva = base + text_sec["rva"]

    syms = json.loads(SYM_PATH.read_text(encoding="utf-8"))
    cur_name: dict[int, str] = {}
    sym_ranges: list[tuple[int, int, str]] = []
    name_to_va: dict[str, int] = {}
    for s in syms:
        va = int(s["addr"], 16)
        nm = s["name"]
        cur_name[va] = nm
        sym_ranges.append((va, va + int(s["size"]), nm))
        if not nm.startswith("FUN_"):
            name_to_va[nm] = va
    sym_ranges.sort()
    sym_starts = [r[0] for r in sym_ranges]

    def containing_fn(va: int) -> int | None:
        i = bisect.bisect_right(sym_starts, va) - 1
        if i >= 0:
            s_va, e_va, _ = sym_ranges[i]
            if s_va <= va < e_va:
                return s_va
        return None

    dec_code: dict[int, str] = {}
    for ln in DEC_PATH.open(encoding="utf-8"):
        d = json.loads(ln)
        dec_code[int(d["addr"], 16)] = d.get("code") or ""

    rsmm_vas = set()
    for s in json.loads(RSMM_SYM_PATH.read_text(encoding="utf-8"))["symbols"]:
        if s.get("va"):
            rsmm_vas.add(int(str(s["va"]), 16))

    fn_entries: dict[int, dict] = {}
    data_entries: dict[int, dict] = {}
    stats = Counter()

    def is_unnamed(va: int) -> bool:
        return cur_name.get(va, "").startswith("FUN_") and va not in fn_entries

    # ------------------------------------------------------------------
    # 4.3 Registrars & meta_cb0..2
    # ------------------------------------------------------------------
    class_tbl = json.loads(CLASS_IDS.read_text(encoding="utf-8"))["classes"]
    reg_fn_classes: dict[int, list[str]] = defaultdict(list)
    for cls, rec in class_tbl.items():
        rva = int(rec["registrar"], 16)
        fn_va = containing_fn(rva)
        if fn_va is not None:
            reg_fn_classes[fn_va].append(cls)

    cb_refs: dict[int, list[tuple[str, int]]] = defaultdict(list)  # target_va -> [(cls, slot)]
    cb_rx = re.compile(r"\[0x11\]\s*\+\s*(0x80|0x88|0x90)\)\s*=\s*([^;]+);")
    slot_map = {"0x80": 0, "0x88": 1, "0x90": 2}

    for fn_va, clss in sorted(reg_fn_classes.items()):
        if len(clss) != 1:
            stats["reg_skip_multi_class"] += 1
            continue
        cls = clss[0]
        rec = class_tbl[cls]
        if is_unnamed(fn_va):
            fn_entries[fn_va] = {
                "addr": f"0x{fn_va:x}",
                "name": sanitize_ghidra_name(f"{cls}::Register"),
                "kind": "function",
                "note": f"[anchor registrar] class id=0x{rec['id']:x} size={rec['size']}",
            }
            stats["reg_named"] += 1

        code = dec_code.get(fn_va, "")
        for off_hex, rhs in cb_rx.findall(code):
            cb_slot = slot_map[off_hex]
            rhs_s = rhs.strip()
            tgt_va: int | None = None
            m = re.search(r"(?:LAB_|FUN_)(1[0-9a-fA-F]{8})\b", rhs_s)
            if m:
                tgt_va = int(m.group(1), 16)
            else:
                # Could be an already-named function like oCFlagDef__ctor_or_dtor_3
                ident = rhs_s.lstrip("&").strip()
                tgt_va = name_to_va.get(ident) or name_to_va.get(ident.replace("__", "::"))
            if tgt_va is not None:
                cb_refs[tgt_va].append((cls, cb_slot))

    # Count jmp trampoline targets so we don't misname a shared _impl
    tramp_targets: dict[int, list[tuple[str, int, int]]] = defaultdict(list)
    for tgt_va, refs in sorted(cb_refs.items()):
        distinct_clss = {c for c, _ in refs}
        if len(distinct_clss) >= 10:
            if tgt_va not in cur_name or cur_name[tgt_va].startswith("FUN_"):
                slot0 = Counter(s for _, s in refs).most_common(1)[0][0]
                nm = "oCClassDescriptor::shared_meta_cb2_dtor" if tgt_va == 0x1401A1E60 else f"oCClassDescriptor::shared_meta_cb{slot0}_{tgt_va:x}"
                if tgt_va not in fn_entries:
                    fn_entries[tgt_va] = {
                        "addr": f"0x{tgt_va:x}",
                        "name": nm,
                        "kind": "function",
                        "note": f"[anchor registrar] shared meta_cb{slot0} across {len(distinct_clss)} classes",
                    }
                    stats["meta_cb_shared_named"] += 1
            continue
        if len(distinct_clss) > 1 or len(refs) != 1:
            stats["meta_cb_skip_shared"] += 1
            continue
        cls, cb_slot = refs[0]
        if tgt_va not in cur_name or cur_name[tgt_va].startswith("FUN_"):
            if tgt_va not in fn_entries:
                fn_entries[tgt_va] = {
                    "addr": f"0x{tgt_va:x}",
                    "name": sanitize_ghidra_name(f"{cls}::meta_cb{cb_slot}"),
                    "kind": "function",
                    "note": f"[anchor registrar] {cls} descriptor +0x88 -> +0x{0x80 + 8 * cb_slot:x}",
                }
                stats[f"meta_cb{cb_slot}_named"] += 1
        # Check for E9 rel32 jmp trampoline
        off = gen.va_to_offset(tgt_va, base, secs)
        if off is not None and off + 5 <= len(data) and data[off] == 0xE9:
            rel = struct.unpack_from("<i", data, off + 1)[0]
            real_va = tgt_va + 5 + rel
            tramp_targets[real_va].append((cls, cb_slot, tgt_va))

    for real_va, trefs in sorted(tramp_targets.items()):
        if len(trefs) != 1 or not is_unnamed(real_va):
            continue
        if real_va in cb_refs:
            continue
        cls, cb_slot, tramp_va = trefs[0]
        fn_entries[real_va] = {
            "addr": f"0x{real_va:x}",
            "name": sanitize_ghidra_name(f"{cls}::meta_cb{cb_slot}_impl"),
            "kind": "function",
            "note": f"[anchor registrar] target of {cls}::meta_cb{cb_slot} trampoline @0x{tramp_va:x}",
        }
        stats["meta_cb_impl_named"] += 1

    # ------------------------------------------------------------------
    # 4.1 Event emitters & initializers
    # ------------------------------------------------------------------
    ev_doc = json.loads(EVENT_NAMES.read_text(encoding="utf-8"))
    hex_to_ev = {v["va"].lower(): k for k, v in ev_doc.items()}
    glob_to_evs: dict[int, set[str]] = defaultdict(set)

    for fn_va, code in sorted(dec_code.items()):
        if "NamedEvent_Id_FromCrc(" not in code:
            continue
        ev_hits = {hex_to_ev[hx] for hx in re.findall(r"0x140f[0-9a-f]{5}", code) if hx in hex_to_ev}
        if len(ev_hits) != 1:
            continue
        ev_name = next(iter(ev_hits))
        if is_unnamed(fn_va):
            fn_entries[fn_va] = {
                "addr": f"0x{fn_va:x}",
                "name": sanitize_ghidra_name(f"InitEvent_{ev_name}"),
                "kind": "function",
                "note": f"[anchor event init] initializes CRC/global for event '{ev_name}'",
            }
            stats["event_init_named"] += 1
        for m in re.finditer(r"String_Assign\(&PTR_DAT_(1[0-9a-f]{8})\b", code):
            glob_to_evs[int(m.group(1), 16)].add(ev_name)
        for m in re.finditer(r"_DAT_(1[0-9a-f]{8})\s*=\s*uVar", code):
            g_va = int(m.group(1), 16)
            glob_to_evs[g_va].add(ev_name)
            if g_va not in rsmm_vas and g_va not in data_entries:
                data_entries[g_va] = {
                    "addr": f"0x{g_va:x}",
                    "name": f"g_EventId_{ev_name}",
                    "kind": "data",
                    "note": f"[anchor event global] {ev_name} id global initialized by 0x{fn_va:x}",
                }

    # Drop non-unique globals from data labels and lookup
    uniq_glob_ev: dict[int, str] = {}
    for g_va, evs in list(glob_to_evs.items()):
        if len(evs) == 1:
            uniq_glob_ev[g_va] = next(iter(evs))
        else:
            data_entries.pop(g_va, None)
    stats["event_data_labels"] = len(data_entries)

    for fn_va, code in sorted(dec_code.items()):
        if "NamedEvent_Dispatch(" not in code:
            continue
        matched_evs: set[str] = set()
        for m in re.finditer(r"(?:PTR_DAT_|DAT_)(1[0-9a-f]{8})\b", code):
            g_va = int(m.group(1), 16)
            if g_va in uniq_glob_ev:
                matched_evs.add(uniq_glob_ev[g_va])
        for hx in re.findall(r"0x140f[0-9a-f]{5}", code):
            if hx in hex_to_ev:
                matched_evs.add(hex_to_ev[hx])
        if len(matched_evs) == 1:
            ev_name = next(iter(matched_evs))
            if is_unnamed(fn_va):
                fn_entries[fn_va] = {
                    "addr": f"0x{fn_va:x}",
                    "name": sanitize_ghidra_name(f"Emit_{ev_name}"),
                    "kind": "function",
                    "note": f"[anchor event emitter] references {ev_name} and calls NamedEvent_Dispatch",
                }
                stats["event_emit_named"] += 1
            else:
                stats["event_emit_already_named"] += 1
        elif len(matched_evs) > 1:
            stats["event_emit_multi"] += 1

    # ------------------------------------------------------------------
    # 4.2 Log, assert, and error strings (str_<sanitised>)
    # ------------------------------------------------------------------
    strs = json.loads(STR_PATH.read_text(encoding="utf-8"))
    str_by_va = {int(s["addr"], 16): s["value"] for s in strs}
    str_xrefs: dict[int, set[int]] = defaultdict(set)
    for i in range(len(text_bytes) - 7):
        if text_bytes[i] in (0x48, 0x4C) and text_bytes[i + 1] == 0x8D and (text_bytes[i + 2] & 0xC7) == 0x05:
            tgt = tva + i + 7 + struct.unpack_from("<i", text_bytes, i + 3)[0]
            if tgt in str_by_va:
                fn = containing_fn(tva + i)
                if fn is not None:
                    str_xrefs[tgt].add(fn)

    fn_str_cands: dict[int, list[tuple[int, int, str]]] = defaultdict(list)
    for str_va, fns in str_xrefs.items():
        if len(fns) != 1:
            continue
        fn_va = next(iter(fns))
        if not is_unnamed(fn_va):
            continue
        good = is_good_str_anchor(str_by_va[str_va])
        if good is not None:
            pri, val = good
            fn_str_cands[fn_va].append((pri, str_va, val))

    for fn_va, cands in sorted(fn_str_cands.items()):
        cands.sort(key=lambda t: (t[0], -len(t[2]), t[1]))
        for pri, str_va, val in cands:
            nm = sanitize_str_anchor(val)
            if len(nm) >= 8:  # at least "str_" + 4 chars
                fn_entries[fn_va] = {
                    "addr": f"0x{fn_va:x}",
                    "name": nm,
                    "kind": "function",
                    "note": f"[anchor string pri={pri} @0x{str_va:x}] {val[:200]}",
                }
                stats[f"str_pri{pri}"] += 1
                break

    # ------------------------------------------------------------------
    # 4.4 Wrappers (<callee>_thunk), fixed-point loop
    # ------------------------------------------------------------------
    known_names: dict[int, str] = {
        va: nm for va, nm in cur_name.items() if not nm.startswith("FUN_")
    }
    for va, e in fn_entries.items():
        known_names[va] = e["name"]

    # Map decompiler C identifier (where :: renders as __) -> (va, canonical_name)
    c_ident_to_info: dict[str, tuple[int, str]] = {}
    ambig_c_idents: set[str] = set()

    def register_c_ident(va: int, nm: str):
        for key in (nm, nm.replace("::", "__")):
            if key in ambig_c_idents:
                continue
            prev = c_ident_to_info.get(key)
            if prev is not None and prev[0] != va:
                ambig_c_idents.add(key)
                c_ident_to_info.pop(key, None)
            else:
                c_ident_to_info[key] = (va, nm)

    for va, nm in known_names.items():
        register_c_ident(va, nm)

    # Precompute machine-code pure jmp rel32 thunks
    jmp_targets: dict[int, int] = {}
    for va, end_va, nm in sym_ranges:
        if not is_unnamed(va):
            continue
        off = gen.va_to_offset(va, base, secs)
        if off is None:
            continue
        sz = end_va - va
        if sz > 16:
            continue
        body = data[off:off + sz]
        # Skip optional endbr64 (F3 0F 1E FA) or nops (90 / 66 90)
        pos = 0
        if body.startswith(b"\xf3\x0f\x1e\xfa"):
            pos += 4
        while pos < len(body) and body[pos] == 0x90:
            pos += 1
        if pos + 5 <= len(body) and body[pos] == 0xE9:
            rest = body[pos + 5:]
            if all(b in (0xCC, 0x90) for b in rest):
                rel = struct.unpack_from("<i", body, pos + 1)[0]
                jmp_targets[va] = va + pos + 5 + rel

    # Precompute candidate single-call decompiles
    decompile_calls: dict[int, tuple[str, list[str]]] = {}
    call_stmt_rx = re.compile(
        r"^(?:return\s+)?(?:\([^)]+\)\s*)?([A-Za-z_~][\w:<>,~*&.]*)\s*\(([^;]*)\);$"
    )
    ctrl_rx = re.compile(r"\b(?:if|else|while|for|switch|goto)\b|[=?]")
    for va, code in dec_code.items():
        if not is_unnamed(va):
            continue
        i1 = code.find("{")
        i2 = code.rfind("}")
        if i1 < 0 or i2 <= i1:
            continue
        stmts = []
        for raw_ln in code[i1 + 1:i2].splitlines():
            ln = raw_ln.strip()
            if not ln or ln.startswith(("/*", "*", "//")) or ln in ("{", "}", "return;"):
                continue
            # Skip pure local variable declarations
            if ";" in ln and "(" not in ln and "=" not in ln:
                continue
            stmts.append(ln)
        if len(stmts) != 1:
            continue
        st = stmts[0]
        if ctrl_rx.search(st):
            continue
        m = call_stmt_rx.match(st)
        if not m:
            continue
        callee_tok, args_str = m.group(1), m.group(2).strip()
        if callee_tok.startswith("FUN_") or callee_tok in BLOCKLIST_CALLEES:
            continue
        args = [a.strip() for a in args_str.split(",")] if args_str else []
        if not all(_PARAM_ARG_RX.match(a) for a in args):
            continue
        decompile_calls[va] = (callee_tok, args)

    iteration = 0
    while True:
        iteration += 1
        added_this_round: list[tuple[int, str, str]] = []

        for va, tgt_va in sorted(jmp_targets.items()):
            if va in fn_entries:
                continue
            callee_nm = known_names.get(tgt_va)
            if not callee_nm or callee_nm in BLOCKLIST_CALLEES:
                continue
            new_nm = sanitize_ghidra_name(f"{callee_nm}_thunk")
            if new_nm.endswith("_thunk_thunk"):
                # Keep single _thunk suffix with chain depth note if already a thunk, or stack _thunk
                pass
            added_this_round.append((va, new_nm, f"[anchor wrapper iter={iteration}] tail-jmp to {callee_nm} @0x{tgt_va:x}"))

        for va, (callee_tok, _args) in sorted(decompile_calls.items()):
            if va in fn_entries or any(x[0] == va for x in added_this_round):
                continue
            info = c_ident_to_info.get(callee_tok)
            if info is None:
                continue
            tgt_va, callee_nm = info
            if callee_nm in BLOCKLIST_CALLEES or tgt_va == va:
                continue
            new_nm = sanitize_ghidra_name(f"{callee_nm}_thunk")
            added_this_round.append((va, new_nm, f"[anchor wrapper iter={iteration}] single-call forwarder to {callee_nm} @0x{tgt_va:x}"))

        if not added_this_round:
            break
        stats[f"thunk_iter{iteration}"] = len(added_this_round)
        stats["thunk_total"] += len(added_this_round)
        for va, new_nm, note in added_this_round:
            fn_entries[va] = {"addr": f"0x{va:x}", "name": new_nm, "kind": "function", "note": note}
            known_names[va] = new_nm
            register_c_ident(va, new_nm)

    # ------------------------------------------------------------------
    # Deduplicate names globally before emitting
    # ------------------------------------------------------------------
    taken = {nm for nm in cur_name.values() if not nm.startswith("FUN_")}
    name_seq: Counter[str] = Counter()
    out_list: list[dict] = []

    for va in sorted(fn_entries):
        e = dict(fn_entries[va])
        base_nm = e["name"]
        nm = base_nm
        if nm in taken:
            idx = max(2, name_seq[base_nm] + 1)
            while f"{base_nm}_{idx}" in taken:
                idx += 1
            nm = f"{base_nm}_{idx}"
            name_seq[base_nm] = idx
        else:
            name_seq[base_nm] = 1
        taken.add(nm)
        e["name"] = nm
        out_list.append(e)

    for va in sorted(data_entries):
        e = dict(data_entries[va])
        out_list.append(e)

    out_path = EV / "phase4_knowledge.json"
    out_path.write_text(json.dumps({"entries": out_list}, indent=1), encoding="utf-8")

    print(f"Wrote {out_path}: {len(fn_entries)} function entries + {len(data_entries)} data labels = {len(out_list)} total")
    for k, v in sorted(stats.items()):
        print(f"  {k}: {v}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
