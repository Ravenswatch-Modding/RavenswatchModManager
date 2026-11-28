"""`rsmm update-editor` — update the native map editor in place.

The map editor runs this (it ships beside the frozen ``rsmm``); see
``rsmm.engine.editor_update`` for the channel and the security model.

Usage:
  rsmm update-editor --dir <editor folder> --current <build> --check --json
  rsmm update-editor --dir <editor folder> --current <build> --json
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from rsmm.engine import editor_update as EU
from rsmm.engine.loader_update import LoaderUpdateError


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(
        prog="rsmm update-editor",
        description="update the native map editor from its rolling channel")
    ap.add_argument("--dir", type=Path, required=True, help="the folder rsmm-map-editor runs from")
    ap.add_argument("--current", type=int, required=True, help="the running editor's build number")
    ap.add_argument("--check", action="store_true", help="report only, do not install")
    ap.add_argument("--json", action="store_true", dest="as_json", help="one line of JSON")
    a = ap.parse_args(argv)

    try:
        EU.clear_old(a.dir)
        st = EU.check(a.current)
        if st["status"] == "available" and not a.check:
            st["installed"] = EU.apply_update(a.dir, st)
            st["status"] = "installed"
    except (EU.EditorUpdateError, LoaderUpdateError, OSError) as e:
        st = {"status": "error", "error": str(e)}
    out = {k: v for k, v in st.items() if not k.startswith("_")}
    if a.as_json:
        print(json.dumps(out))
    else:
        latest = f" (build {out['latest']})" if out.get("latest") else ""
        err = f": {out['error']}" if "error" in out else ""
        print(f"map editor: {out['status']}{latest}{err}")
    # --json answers in the body (the editor reads stdout either way).
    return 1 if st["status"] == "error" and not a.as_json else 0


if __name__ == "__main__":
    raise SystemExit(main())
