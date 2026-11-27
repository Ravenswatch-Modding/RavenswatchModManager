#pragma once

namespace rsmm {

// Custom status registration.
//
// Ravenswatch registers its 20 statuses as engine value definitions in code
// (StatusValueDefs_Register), and the status table's init
// (StatusSettingsGo_Init) deletes every table row whose id has no definition.
// A mod-declared status therefore needs a definition registered in the same
// manager, or its row is pruned and its id holds no value.
//
// Reads every enabled mod's `mods/<id>/statuses.json` (written by `rsmm apply`
// from the manifest's `status` content), validates each entry, and detours both
// engine functions. Whichever runs first registers every declared status, once
// per process, on the game thread. No declared status => no hooks, one plain
// log line. Returns true if at least one detour is live.
bool install_status_hooks();

} // namespace rsmm
