#!/usr/bin/env python3
"""Phase 3.1: Identify statically linked libraries, versions, and PE MSVC toolset."""

from __future__ import annotations

import json
import re
import struct
from collections import Counter
from pathlib import Path

REPO = Path(r"E:\Documents\Projects\Ravenswatch\RSMM\repo\RavenswatchModManager")
SNAP_EXE = Path(r"E:\Documents\Projects\Ravenswatch\RSMM\evidence\game-snapshots\e04db5c457dd\Ravenswatch-e04db5c457dd.exe")
STR_PATH = REPO / "docs" / "_re" / "out" / "strings.json"
VFT_PATH = REPO / "docs" / "_re" / "out_new" / "vftables.jsonl"


def parse_rich_header(data: bytes) -> dict:
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    opt_off = e_lfanew + 24
    major_lnk, minor_lnk = struct.unpack_from("<BB", data, opt_off + 2)
    stub = data[0x80:e_lfanew]
    rich_idx = stub.find(b"Rich")
    entries = []
    if rich_idx >= 0 and rich_idx + 8 <= len(stub):
        xor_key = struct.unpack_from("<I", stub, rich_idx + 4)[0]
        # Find DanS ^ xor_key
        dans_bytes = struct.pack("<I", 0x536E6144 ^ xor_key)
        dans_idx = stub.find(dans_bytes, 0, rich_idx)
        if dans_idx >= 0:
            pos = dans_idx + 16  # skip DanS + 3 zero dwords
            while pos + 8 <= rich_idx:
                comp_id = struct.unpack_from("<I", stub, pos)[0] ^ xor_key
                count = struct.unpack_from("<I", stub, pos + 4)[0] ^ xor_key
                prod_id = comp_id >> 16
                build = comp_id & 0xFFFF
                entries.append((prod_id, build, count))
                pos += 8
    return {
        "linker_version": f"{major_lnk}.{minor_lnk}",
        "rich_entries": entries,
    }


def main() -> int:
    data = SNAP_EXE.read_bytes()
    rich = parse_rich_header(data)
    print(f"=== PE Linker & Rich Header ===")
    print(f"Linker version: {rich['linker_version']}")
    builds = Counter()
    for prod_id, build, count in rich["rich_entries"]:
        if build:
            builds[build] += count
    print("Top Rich compiler builds (build -> obj_count):", builds.most_common(12))

    vfts = [json.loads(ln) for ln in VFT_PATH.open(encoding="utf-8")]
    ns_counts = Counter()
    for v in vfts:
        sym = v["sym"]
        if sym.startswith(("oC", "oI", "oe::", "o_")):
            ns_counts["oEngine/Game"] += 1
        elif "::" in sym:
            ns_counts[sym.split("::", 1)[0]] += 1
        elif sym.startswith("bt"):
            ns_counts["Bullet (bt*)"] += 1
        else:
            ns_counts["Other/Global"] += 1
    print("\n=== Vtable Namespace Counts (total 6026) ===")
    for k, c in ns_counts.most_common(25):
        print(f"  {k}: {c}")

    strs = json.loads(STR_PATH.read_text(encoding="utf-8"))
    vals = [s["value"] for s in strs]
    print(f"\n=== String Evidence ({len(vals)} total strings) ===")

    checks = [
        ("MSVC CRT / STL", r"Microsoft Visual C\+\+|minkernel\\crts\\ucrt|VCRUNTIME"),
        ("Stormancer", r"ThirdParty-Ext\\Stormancer|Stormancer::|Stormancer_ConnectFail|stormancer\.rpc"),
        ("cpprestsdk / pplx", r"cpprestsdk/|pplx::"),
        ("RakNet", r"ThirdParty-Ext\\RakNet|RAKNET_STARTED|ID_CONNECTION_REQUEST_ACCEPTED"),
        ("Bullet Physics", r"_SHAPE_PROXYTYPE|Bullet physic|btKinematic"),
        ("Recast / Detour", r"Perforce\\Ext\\Detour|\\Recast\.cpp|\\Detour"),
        ("Crashpad / mini_chromium", r"crashpad|mini_chromium|\.\./\.\./client/crash|\.\./\.\./util/"),
        ("RapidJSON", r"rapidjson::"),
        ("msgpack-c", r"msgpack::|application/msgpack"),
        ("RxCpp", r"rxcpp::"),
        ("ctti", r"ctti::pretty_function"),
        ("Dear ImGui", r"ImGui"),
        ("Dynamic DLLs", r"^(EOSSDK-Win64-Shipping|fmod|fmodstudio|steam_api64|libScePad|xaudio2_9redist)\.dll$"),
    ]
    for label, pat in checks:
        rx = re.compile(pat, re.I)
        hits = sorted({v for v in vals if rx.search(v)})
        print(f"\n[{label}] ({len(hits)} unique strings)")
        for h in hits[:8]:
            print(f"  {h!r}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
