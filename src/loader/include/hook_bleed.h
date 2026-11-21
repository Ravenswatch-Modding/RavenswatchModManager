#pragma once

namespace rsmm {

// Redistribute shortened (<5.0 s) Bleed modifiers across round(dur / 0.5 s)
// ring-buffer tick slots on oCDtRemoteDamageTarget, and zero residual slots
// when State Bleeding ends. Vanilla 5.0 s / -1.0 s bleeds are untouched.
void install_bleed_hooks();
void remove_bleed_hooks();

} // namespace rsmm
