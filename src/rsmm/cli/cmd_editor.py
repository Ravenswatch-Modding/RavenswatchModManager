"""`rsmm editor` — items, talents, heroes and abilities in one local page.

    rsmm editor                      open on the Items tab
    rsmm editor --tab abilities      ...or on another tab
    rsmm editor --port 9000 --no-browser

``item-editor``, ``talent-editor`` and ``ability-editor`` are aliases that open
it on their tab (see ``_dispatch.LEGACY``). The editors live in
``rsmm.cli.editor``. Maps are edited in the native map editor
(``apps/map-editor``), which drives the ``/map/api`` routes over ``--stdio``.
"""

from __future__ import annotations

import sys

from rsmm.cli.editor.server import run


def main(argv: list[str] | None = None) -> int:
    return run(argv)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
