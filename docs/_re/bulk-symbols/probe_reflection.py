#!/usr/bin/env python3
"""Phase 5: 1-hour reflection probe (check registrar callbacks, serializers, and m_ strings)."""

from __future__ import annotations

import bisect
import json
import re
import struct
import sys
from collections import defaultdict
from pathlib import Path

REPO = Path(r"E:\Documents\Projects\Ravenswatch\RSMM\repo\RavenswatchModManager")
EV = Path(r"E:\Documents\Projects\Ravenswatch\RSMM\evidence\2026-11-26-bulk-symbols")
SNAP_EXE = Path(r"E:\Documents\Projects\Ravenswatch\RSMM\evidence\game-snapshots\e04db5c457dd\Ravenswatch-e04db5c457dd.exe")
CLASS_VERS = Path(r"E:\Documents\Projects\Ravenswatch\RSMM\evidence\game-snapshots\e04db5c457dd\class_versions.json")
DEC_PATH = REPO / "docs" / "_re" / "out_new" / "decompiled_new.jsonl"
SYM_PATH = REPO / "docs" / "_re" / "out" / "symbols.json"
STR_PATH = REPO / "docs" / "_re" / "out" / "strings.json"

sys.path.insert(0, str(REPO / "scripts"))
import gen_function_patterns as gen  # noqa: E402


def main() -> int:
    dec = {}
    dec_name = {}
    for ln in DEC_PATH.open(encoding="utf-8"):
        d = json.loads(ln)
        va = int(d["addr"], 16)
        dec[va] = d.get("code") or ""
        dec_name[va] = d.get("name") or ""

    syms = json.loads(SYM_PATH.read_text(encoding="utf-8"))
    sym_ranges = sorted((int(s["addr"], 16), int(s["addr"], 16) + int(s["size"]), s["name"]) for s in syms)
    sym_starts = [r[0] for r in sym_ranges]

    def containing_fn(va: int):
        i = bisect.bisect_right(sym_starts, va) - 1
        if i >= 0 and sym_ranges[i][0] <= va < sym_ranges[i][1]:
            return sym_ranges[i]
        return None

    # 1. Registrar callbacks + registration helper FUN_14050b970 / FUN_140573bc0
    print("=== 1. Registrar callbacks (oC2dAnimPictureDesc + 2 others) ===")
    for va in (0x140573C00, 0x14057FAE0, 0x1401A1E60, 0x140573BC0, 0x14050B970, 0x140243ED0, 0x140244970):
        print(f"\n--- 0x{va:x} ({dec_name.get(va, '?')}) ---")
        print(dec.get(va, "<not in decompiled_new.jsonl>")[:1200])

    # 2. Versioned class serialize paths
    print("\n=== 2. Versioned class serialize paths ===")
    cv = json.loads(CLASS_VERS.read_text(encoding="utf-8"))
    print(f"class_versions.json entries: {len(cv)}, samples: {list(cv.items())[:8]}")
    # Check CustomFlagFilter_Serialize + oC2dAnimPictureDesc::vf* / oCDtHeroProfileData::vf*
    for va, nm in sorted(dec_name.items()):
        if va == 0x140182160 or nm.startswith(("oC2dAnimPictureDesc::vf", "oCDtHeroProfileData::vf")):
            code = dec[va]
            if len(code.splitlines()) > 12:
                print(f"\n--- 0x{va:x} {nm} ---")
                print(code[:1200])
                if va != 0x140182160:
                    break

    # 3. Search strings.json for m_ member names & camelCase runs
    print("\n=== 3. m_* and camelCase member-name strings ===")
    strs = json.loads(STR_PATH.read_text(encoding="utf-8"))
    m_rx = re.compile(r"^m_[a-zA-Z0-9_]{3,}$")
    m_strs = sorted((int(s["addr"], 16), s["value"]) for s in strs if m_rx.match(s["value"]))
    print(f"Total m_* strings in strings.json: {len(m_strs)}")
    for va, val in m_strs:
        print(f"  0x{va:x}: {val}")

    # Check .rdata neighborhood around 0x1404b3480's referenced string (m_fVelocityScale) and all m_ strings
    data = SNAP_EXE.read_bytes()
    base, secs = gen.parse_pe(data)
    text_sec = secs[0]
    text = data[text_sec["raw_off"]:text_sec["raw_off"] + text_sec["raw_size"]]
    tva = base + text_sec["rva"]

    m_vas = {va: val for va, val in m_strs}
    m_xrefs: dict[int, set[int]] = defaultdict(set)
    for i in range(len(text) - 7):
        if text[i] in (0x48, 0x4C) and text[i + 1] == 0x8D and (text[i + 2] & 0xC7) == 0x05:
            tgt = tva + i + 7 + struct.unpack_from("<i", text, i + 3)[0]
            if tgt in m_vas:
                fn = containing_fn(tva + i)
                if fn:
                    m_xrefs[fn[0]].add(tgt)

    print(f"\nFunctions referencing m_* strings: {len(m_xrefs)}")
    for fn_va, tgt_vas in sorted(m_xrefs.items())[:8]:
        fn_nm = dec_name.get(fn_va, f"0x{fn_va:x}")
        print(f"\n  Function 0x{fn_va:x} ({fn_nm}) refs {[m_vas[v] for v in sorted(tgt_vas)]}:")
        print(dec.get(fn_va, "")[:1200])

    # Also check for camelCase clusters (>=5 consecutive camelCase strings within 0x100 bytes in strings.json)
    camel_rx = re.compile(r"^[a-z][a-zA-Z0-9]{2,40}$")
    camels = sorted((int(s["addr"], 16), s["value"]) for s in strs if camel_rx.match(s["value"]) and any(c.isupper() for c in s["value"]))
    runs = []
    cur = []
    for va, val in camels:
        if cur and va - cur[-1][0] <= 0x80:
            cur.append((va, val))
        else:
            if len(cur) >= 6:
                runs.append(cur)
            cur = [(va, val)]
    if len(cur) >= 6:
        runs.append(cur)
    print(f"\n=== Dense camelCase string runs (>=6 within 0x80 gap): {len(runs)} ===")
    for r in runs[:8]:
        print(f"  0x{r[0][0]:x}..0x{r[-1][0]:x} ({len(r)}): {[x[1] for x in r[:8]]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
