"""The manifest field names in manifest_spec.py and in the store's zod schema
(`modManifestSchema` in packages/schemas/src/mod.ts) must not drift apart.

The two legitimately differ in places. Each difference is listed below with its
reason; a new difference that is not listed fails this test, and so does a listed
one that no longer exists.
"""

import re
from pathlib import Path

from rsmm.sdk import manifest_spec

MOD_TS = Path(__file__).resolve().parent.parent / "packages" / "schemas" / "src" / "mod.ts"

# In the zod schema, absent from manifest_spec.
ONLY_IN_SCHEMA = {
    "dependencies": "the store/API shape of the manifest's `requires` list (mod-id -> range map)",
    "translations": "store-only localized listing text; the manifest spec has no such table",
}

# In manifest_spec, absent from the zod schema.
ONLY_IN_SPEC = {
    "config": "top-level manifest table; the store schema covers the [mod] table only",
    "content": "top-level manifest table; the store schema covers the [mod] table only",
    "experiment": "top-level manifest table; the store schema covers the [mod] table only",
    "mod": "the [mod] table itself, whose keys the schema lists directly",
    "patch": "top-level manifest table; the store schema covers the [mod] table only",
    "provides": "top-level manifest table; the store schema covers the [mod] table only",
    "requires": "top-level manifest table; the store reads it as `dependencies`",
    "conflicts": "dependency-graph key the store does not index",
    "experimental": "apply-time opt-in the store does not show",
    "load_order": "apply-time ordering the store does not show",
    "loader_flags": "loader feature switches the store does not show",
    "multiplayer_scope": "apply/lint-time scope tag the store does not show",
    "priority": "apply-time ordering the store does not show",
    "recommends": "dependency-graph key the store does not index",
    "replaces": "dependency-graph key the store does not index",
    "restore_options": "apply-time game-option cleanup the store does not show",
    "suggests": "dependency-graph key the store does not index",
}


def _schema_keys() -> set[str]:
    src = MOD_TS.read_text(encoding="utf-8")
    start = src.index("export const modManifestSchema = z.object({")
    end = src.index("\n});", start)
    return set(re.findall(r"^  ([A-Za-z_]\w*):", src[start:end], re.M))


def test_manifest_fields_match_store_schema():
    schema = _schema_keys()
    spec = set(manifest_spec.MOD_FIELDS) | set(manifest_spec.TOP_LEVEL)
    assert schema, "parsed no keys from modManifestSchema; did mod.ts change shape?"
    assert schema - spec == set(ONLY_IN_SCHEMA), (
        "modManifestSchema keys that manifest_spec lacks changed: "
        f"{sorted((schema - spec) ^ set(ONLY_IN_SCHEMA))}")
    assert spec - schema == set(ONLY_IN_SPEC), (
        "manifest_spec names that modManifestSchema lacks changed: "
        f"{sorted((spec - schema) ^ set(ONLY_IN_SPEC))}")
