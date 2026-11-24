"""Files a mod's own [[content]] emitted are new assets by design (apply
registers them in UsedRscList), so lint must not report them as "not in
asset_map". A hand-placed file at an unknown path still is: that is how a
typo in an override path shows up."""

from __future__ import annotations

import json
import re

from rsmm.cli import lint


def test_emitted_assets_are_not_reported_as_unknown_paths(tmp_path, capsys):
    d = tmp_path / "DemoHero"
    new = d / "assets" / "EntitySettings" / "Heroes" / "Hero_Common"
    new.mkdir(parents=True)
    (d / "manifest.toml").write_text('[mod]\nid = "DemoHero"\n')
    (new / "Hero_Demo.entity.ot.EntitySettingsResource.gen").write_bytes(b"\x00")
    (new / "Hero_Typo.entity.ot.EntitySettingsResource.gen").write_bytes(b"\x00")
    (d / ".rsmm_emitted.json").write_text(json.dumps(
        ["EntitySettings/Heroes/Hero_Common/Hero_Demo.entity.ot.EntitySettingsResource.gen"]))
    lint.lint_one(d)
    out = re.sub(r"\x1b\[[0-9;]*m", "", capsys.readouterr().out)
    unknown = [ln for ln in out.splitlines() if "not in asset_map" in ln]
    assert len(unknown) == 1 and "Hero_Typo" in unknown[0]
