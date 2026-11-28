#!/usr/bin/env bash
# Publish map editor zips to the rolling `map-editor` release, behind a signed
# manifest. The editor's Help > Check for updates (and its check at start)
# reads it through `rsmm update-editor`; see src/rsmm/engine/editor_update.py.
#
#   scripts/publish_map_editor.sh <dir with rsmm-map-editor-{linux,windows}.zip> \
#       --build <N> [--notes "what changed"] [--dry-run]
#
# <N> must be the RSMM_EDITOR_BUILD the zipped binaries were compiled with
# (.github/workflows/map-editor.yml uses the run number for both).
# Signing is the loader channel's (scripts/publish_loader.sh): same key, same
# tauri signer, same unwrap to plain minisign text.
set -euo pipefail
cd "$(dirname "$0")/.."

TAG=map-editor
DIR="${1:?usage: $0 <dir> --build N [--notes text] [--dry-run]}"; shift
BUILD="" NOTES="" DRY_RUN=0
while [ $# -gt 0 ]; do
    case "$1" in
        --build) BUILD="$2"; shift 2 ;;
        --notes) NOTES="$2"; shift 2 ;;
        --dry-run) DRY_RUN=1; shift ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
[[ "$BUILD" =~ ^[1-9][0-9]*$ ]] || { echo "--build must be a positive number" >&2; exit 2; }

MAN="$DIR/map-editor.manifest.json"
SIG="$MAN.minisig"
python3 - "$DIR" "$MAN" "$BUILD" "$NOTES" <<'PYEOF'
import hashlib, json, subprocess, sys
from datetime import datetime, timezone
from pathlib import Path

d, out, build, notes = Path(sys.argv[1]), Path(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
assets = {}
for os_key in ("linux", "windows"):
    z = d / f"rsmm-map-editor-{os_key}.zip"
    if z.is_file():
        b = z.read_bytes()
        assets[os_key] = {"name": z.name, "sha256": hashlib.sha256(b).hexdigest(), "size": len(b)}
if not assets:
    sys.exit(f"no rsmm-map-editor-*.zip in {d}")
out.write_text(json.dumps({
    "build": build,
    "commit": subprocess.run(["git", "rev-parse", "--short", "HEAD"],
                             capture_output=True, text=True).stdout.strip() or None,
    "generated": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "notes": notes or None,
    "assets": assets,
}, indent=1) + "\n", encoding="utf-8")
print(f"manifest: build {build}, {sorted(assets)}")
PYEOF

if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
    # The secret passes verbatim as TAURI_PRIVATE_KEY (publish_loader.sh has the why).
    env TAURI_PRIVATE_KEY="$TAURI_SIGNING_PRIVATE_KEY" \
        TAURI_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" \
        npx --yes @tauri-apps/cli@2.1.0 signer sign "$MAN"
    base64 -d < "$MAN.sig" > "$SIG" 2>/dev/null || cp "$MAN.sig" "$SIG"
    rm -f "$MAN.sig"
elif command -v minisign >/dev/null; then
    minisign -Sm "$MAN" -x "$SIG"
else
    echo "ERROR: no signing key (TAURI_SIGNING_PRIVATE_KEY or minisign)" >&2; exit 1
fi
head -1 "$SIG" | grep -q 'untrusted comment' || { echo "ERROR: signature is not minisign text" >&2; exit 1; }

# The client's own check, against the key it ships, before anything goes up.
PYTHONPATH=src python3 - "$DIR" <<'PYEOF'
import sys
from pathlib import Path
from rsmm.engine import editor_update as EU
m = EU.fetch_manifest(Path(sys.argv[1]).resolve().as_uri())
print(f"ok: the editor's updater accepts build {m['build']}")
PYEOF

if [ "$DRY_RUN" = 1 ]; then
    echo "dry run: not uploading ($DIR)"; exit 0
fi
gh release view "$TAG" >/dev/null 2>&1 || gh release create "$TAG" --latest=false \
    --title "Map editor (rolling)" \
    --notes "Rolling, signed builds of the native map editor, read by its updater (\`rsmm update-editor\`). Assets are replaced in place; do not pin."
gh release upload "$TAG" --clobber "$DIR"/rsmm-map-editor-*.zip "$MAN" "$SIG"
echo "published map editor build $BUILD"
