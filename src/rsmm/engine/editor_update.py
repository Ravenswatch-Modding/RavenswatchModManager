"""Self-update for the native map editor (``apps/map-editor``).

The editor ships as a folder: ``rsmm-map-editor[.exe]`` and the frozen
``rsmm[.exe]`` it reads game data through. ``.github/workflows/map-editor.yml``
(publish run) puts one zip per OS on the rolling ``map-editor`` release, next
to a manifest signed with the key that signs the loader channel and the
desktop bundles::

    {"build": 12, "commit": "abc1234", "generated": "...", "notes": "...",
     "assets": {"linux": {"name": "...zip", "sha256": "...", "size": 123},
                "windows": {...}}}

Each build carries its number (``RSMM_EDITOR_BUILD`` at compile time); 0 is a
developer build, which never updates itself. Fetching, the HTTPS-only rule in
a frozen build and signature checking are the loader channel's own
(``rsmm.engine.loader_update``).

Installing swaps the files in place: each old file is renamed to
``<name>.old`` first (Windows lets a running .exe be renamed, not replaced),
the new one moved in, and the ``.old`` files cleared on the next check.
"""

from __future__ import annotations

import io
import json
import os
import sys
import zipfile
from pathlib import Path

from rsmm.engine import loader_update as LU
from rsmm.engine.minisign import MinisignError
from rsmm.engine.minisign import verify as minisign_verify

MANIFEST_NAME = "map-editor.manifest.json"
SIGNATURE_NAME = "map-editor.manifest.json.minisig"
DEFAULT_REMOTE_BASE = (
    "https://github.com/Ravenswatch-Modding/RavenswatchModManager/releases/download/map-editor"
)
_MAX_ZIP = 256 << 20
#: The only files an update may write: the editor, the CLI beside it, the readme.
_ALLOWED = {"rsmm-map-editor", "rsmm-map-editor.exe", "rsmm", "rsmm.exe", "README.txt"}


class EditorUpdateError(Exception):
    pass


def remote_base() -> str:
    return os.environ.get("RSMM_EDITOR_UPDATE_BASE", "").strip() or DEFAULT_REMOTE_BASE


def platform_key() -> str:
    return "windows" if sys.platform == "win32" else "linux"


def fetch_manifest(base: str | None = None) -> dict:
    """The signed manifest, verified before it is parsed."""
    base = base or remote_base()
    try:
        raw = LU._fetch(f"{base}/{MANIFEST_NAME}", limit=1 << 20)
        sig = LU._fetch(f"{base}/{SIGNATURE_NAME}", limit=1 << 16)
    except LU.ChannelNotPublishedError:
        raise
    except LU.LoaderUpdateError as e:
        raise EditorUpdateError(str(e)) from e
    try:
        minisign_verify(raw, sig.decode("utf-8", "replace"), LU.public_key())
    except MinisignError as e:
        raise EditorUpdateError(f"manifest signature rejected: {e}") from e
    try:
        m = json.loads(raw.decode("utf-8"))
    except (ValueError, UnicodeDecodeError) as e:
        raise EditorUpdateError(f"manifest is not valid JSON: {e}") from e
    if not (isinstance(m, dict) and isinstance(m.get("build"), int)
            and isinstance(m.get("assets"), dict)):
        raise EditorUpdateError("manifest needs an integer 'build' and an 'assets' object")
    return m


def _editor_dir(d: Path) -> Path:
    """Only a folder that holds the editor is ever written to."""
    if not any((d / n).is_file() for n in ("rsmm-map-editor", "rsmm-map-editor.exe")):
        raise EditorUpdateError(f"{d} does not hold rsmm-map-editor")
    return d


def clear_old(d: Path) -> None:
    """Remove the ``.old`` files a previous update left (best effort: one
    still running on Windows stays until the next time). Only in an editor folder."""
    if not any((d / n).is_file() for n in ("rsmm-map-editor", "rsmm-map-editor.exe")):
        return
    for n in _ALLOWED:
        try:
            (d / f"{n}.old").unlink(missing_ok=True)
        except OSError:
            pass


def check(current: int, base: str | None = None) -> dict:
    """``{"status": up_to_date | available | dev_build | not_published, ...}``."""
    if current <= 0:
        return {"status": "dev_build", "current": current}
    try:
        m = fetch_manifest(base)
    except LU.ChannelNotPublishedError:
        return {"status": "not_published", "current": current}
    asset = m["assets"].get(platform_key())
    latest = m["build"]
    status = "available" if latest > current and isinstance(asset, dict) else "up_to_date"
    return {"status": status, "current": current, "latest": latest,
            "notes": m.get("notes"), "commit": m.get("commit"), "_manifest": m}


def apply_update(d: Path, state: dict, base: str | None = None) -> list[str]:
    """Download this OS's zip, check its hash and swap its files into ``d``.
    Returns the names written."""
    d = _editor_dir(d)
    asset = state["_manifest"]["assets"][platform_key()]
    name, want = asset.get("name"), asset.get("sha256")
    if not isinstance(name, str) or "/" in name or "\\" in name or not isinstance(want, str):
        raise EditorUpdateError("manifest asset needs a plain 'name' and a 'sha256'")
    try:
        data = LU._fetch(f"{base or remote_base()}/{name}", limit=_MAX_ZIP)
    except LU.LoaderUpdateError as e:
        raise EditorUpdateError(str(e)) from e
    if LU._sha256(data) != want.lower():
        raise EditorUpdateError(f"{name}: sha256 does not match the signed manifest")
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        members = {i.filename: i for i in z.infolist() if not i.is_dir()}
        bad = sorted(n for n in members if n not in _ALLOWED)
        if bad:
            raise EditorUpdateError(f"{name}: unexpected files {bad}")
        if not any(n.startswith("rsmm-map-editor") for n in members):
            raise EditorUpdateError(f"{name}: no editor in the zip")
        clear_old(d)
        # Everything lands beside its target first, so a full disk fails
        # before any file of the install is touched.
        staged = []
        for n, info in members.items():
            tmp = d / f"{n}.new"
            tmp.write_bytes(z.read(info))
            staged.append((n, tmp))
    for n, tmp in staged:
        dst = d / n
        if dst.exists():
            os.replace(dst, d / f"{n}.old")
        os.replace(tmp, dst)
        if n != "README.txt" and os.name != "nt":
            dst.chmod(0o755)
    return sorted(n for n, _ in staged)
