"""rsmm update-editor: a signed channel swaps the editor's files in place."""

from __future__ import annotations

import hashlib
import io
import json
import zipfile

import pytest
from test_loader_update import _pubkey_text, _sign

from rsmm.engine import editor_update as EU


def _publish(remote, files: dict[str, bytes], *, build=5, sha=None):
    remote.mkdir(exist_ok=True)
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        for n, b in files.items():
            z.writestr(n, b)
    zbytes = buf.getvalue()
    name = f"rsmm-map-editor-{EU.platform_key()}.zip"
    (remote / name).write_bytes(zbytes)
    asset = {"name": name, "sha256": sha or hashlib.sha256(zbytes).hexdigest(), "size": len(zbytes)}
    raw = json.dumps({"build": build, "assets": {EU.platform_key(): asset}}).encode()
    (remote / EU.MANIFEST_NAME).write_bytes(raw)
    (remote / EU.SIGNATURE_NAME).write_text(_sign(raw), encoding="utf-8")
    return remote.as_uri()


@pytest.fixture
def install(tmp_path, monkeypatch):
    monkeypatch.setenv("RSMM_LOADER_UPDATE_PUBKEY", _pubkey_text())
    d = tmp_path / "editor"
    d.mkdir()
    (d / "rsmm-map-editor").write_bytes(b"old editor")
    (d / "rsmm").write_bytes(b"old cli")
    return d


def test_update_swaps_the_files_and_keeps_the_old_ones_aside(tmp_path, install):
    base = _publish(tmp_path / "ch", {"rsmm-map-editor": b"new editor", "rsmm": b"new cli"})
    st = EU.check(3, base)
    assert st["status"] == "available" and st["latest"] == 5
    assert EU.apply_update(install, st, base) == ["rsmm", "rsmm-map-editor"]
    assert (install / "rsmm-map-editor").read_bytes() == b"new editor"
    assert (install / "rsmm.old").read_bytes() == b"old cli"
    EU.clear_old(install)
    assert not (install / "rsmm.old").exists()
    assert EU.check(5, base)["status"] == "up_to_date"
    assert EU.check(0, base)["status"] == "dev_build"


@pytest.mark.parametrize("files,sha,err", [
    ({"rsmm-map-editor": b"x"}, "0" * 64, "sha256"),
    ({"rsmm-map-editor": b"x", "../evil": b"x"}, None, "unexpected"),
])
def test_a_bad_zip_changes_nothing(tmp_path, install, files, sha, err):
    base = _publish(tmp_path / "ch", files, sha=sha)
    with pytest.raises(EU.EditorUpdateError, match=err):
        EU.apply_update(install, EU.check(1, base), base)
    assert (install / "rsmm-map-editor").read_bytes() == b"old editor"


def test_a_folder_without_the_editor_is_never_written(tmp_path, install):
    base = _publish(tmp_path / "ch", {"rsmm-map-editor": b"x"})
    with pytest.raises(EU.EditorUpdateError, match="does not hold"):
        EU.apply_update(tmp_path, EU.check(1, base), base)


def test_an_unsigned_manifest_is_refused(tmp_path, install):
    base = _publish(tmp_path / "ch", {"rsmm-map-editor": b"x"})
    (tmp_path / "ch" / EU.MANIFEST_NAME).write_text('{"build": 99, "assets": {}}')
    with pytest.raises(EU.EditorUpdateError, match="signature"):
        EU.check(1, base)
