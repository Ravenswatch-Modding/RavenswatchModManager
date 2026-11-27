// Map editor shading: one sun, a sky/ground hemisphere, distance haze.
// Engine space (left-handed, Y up); reversed-Z depth.

struct Globals {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    sun_dir: vec4<f32>,   // xyz: direction TOWARD the sun
    params: vec4<f32>,    // x: haze density, y: exposure, z: shadows on
    light_vp: mat4x4<f32>,
    sun_color: vec4<f32>,
    amb_top: vec4<f32>,       // w: horizon power
    amb_horizon: vec4<f32>,
    amb_bottom: vec4<f32>,
    fog_bottom: vec4<f32>,
    fog_mid: vec4<f32>,
    fog_top: vec4<f32>,
    fog_heights: vec4<f32>,   // top, mid, bottom height; w: fog on
    fog_dist: vec4<f32>,      // start, mid, end beyond focus; w: focus distance
    fog_params: vec4<f32>,    // intensity, brightness
};
@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var t_shadow: texture_depth_2d;
@group(0) @binding(2) var s_shadow: sampler_comparison;

// Fraction of sunlight reaching `wp` (3x3 PCF over the sun's depth map).
fn sunlit(wp: vec3<f32>, n: vec3<f32>) -> f32 {
    if (g.params.z < 0.5) { return 1.0; }
    let p = g.light_vp * vec4<f32>(wp + n * 0.12, 1.0);
    let uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || p.z > 1.0) { return 1.0; }
    let texel = 1.0 / 4096.0;
    var sum = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            sum += textureSampleCompareLevel(t_shadow, s_shadow, uv + vec2<f32>(f32(x), f32(y)) * texel, p.z - 0.0008);
        }
    }
    return sum / 9.0;
}

fn aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

// The chapter's ambient: three colours over the hemisphere (oCDtAmbientLightingSettings).
fn ambient(n: vec3<f32>) -> vec3<f32> {
    let k = pow(abs(n.y), g.amb_top.w);
    if (n.y >= 0.0) {
        return mix(g.amb_horizon.rgb, g.amb_top.rgb, k);
    }
    return mix(g.amb_horizon.rgb, g.amb_bottom.rgb, k);
}

// The chapter's height fog (oCDtFogSettings): colour by world height between
// the bottom / mid / top heights, amount growing with distance past the
// camera's focus, alpha = strength.
fn fog_color(h: f32) -> vec4<f32> {
    let top = g.fog_heights.x;
    let mid = g.fog_heights.y;
    let bot = g.fog_heights.z;
    if (h >= mid) {
        return mix(g.fog_mid, g.fog_top, clamp((h - mid) / max(top - mid, 0.01), 0.0, 1.0));
    }
    return mix(g.fog_bottom, g.fog_mid, clamp((h - bot) / max(mid - bot, 0.01), 0.0, 1.0));
}

fn apply_fog(c: vec3<f32>, wp: vec3<f32>) -> vec3<f32> {
    if (g.fog_heights.w < 0.5) { return c; }
    let d = length(g.eye.xyz - wp) - g.fog_dist.w;
    let t = smoothstep(g.fog_dist.x, g.fog_dist.z, d);
    let fc = fog_color(wp.y);
    return mix(c, fc.rgb * g.fog_params.y, clamp(fc.a * t * g.fog_params.x, 0.0, 1.0));
}

fn shade(albedo: vec3<f32>, n_in: vec3<f32>, wp: vec3<f32>) -> vec3<f32> {
    let to_eye = g.eye.xyz - wp;
    var n = normalize(n_in);
    // Meshes are drawn double-sided with unknown winding: light the side we see.
    if (dot(n, to_eye) < 0.0) { n = -n; }
    let l = normalize(g.sun_dir.xyz);
    // Lambert: albedo / pi per unit of sun intensity, as a PBR light is.
    let direct = g.sun_color.rgb * (max(dot(n, l), 0.0) * sunlit(wp, n) / 3.14159);
    let lit = albedo * (ambient(n) + direct);
    return aces(apply_fog(lit, wp) * g.params.y);
}

// ---- terrain ------------------------------------------------------------------
// The chapter's own Dt_TerrainBlendFlavor5 material (engine/terrain_look.py): a
// base texture set and four layers blended by the painted weight map, each
// layer's albedo alpha a height that sharpens the blend.

struct TSlot {
    color: vec4<f32>,
    params: vec4<f32>,    // uv scale, height mul, height offset, smooth
};
struct TerrainU {
    slots: array<TSlot, 5>,
    flags: vec4<f32>,     // x: material ready, y: has vertex colour
    water_deep: vec4<f32>,
    water_shallow: vec4<f32>,
    water_banks: vec4<f32>,
};
@group(1) @binding(0) var t_weights: texture_2d<f32>;
@group(1) @binding(1) var t_vcolor: texture_2d<f32>;
@group(1) @binding(2) var t_alb0: texture_2d<f32>;
@group(1) @binding(3) var t_alb1: texture_2d<f32>;
@group(1) @binding(4) var t_alb2: texture_2d<f32>;
@group(1) @binding(5) var t_alb3: texture_2d<f32>;
@group(1) @binding(6) var t_alb4: texture_2d<f32>;
@group(1) @binding(7) var t_nrm0: texture_2d<f32>;
@group(1) @binding(8) var t_nrm1: texture_2d<f32>;
@group(1) @binding(9) var t_nrm2: texture_2d<f32>;
@group(1) @binding(10) var t_nrm3: texture_2d<f32>;
@group(1) @binding(11) var t_nrm4: texture_2d<f32>;
@group(1) @binding(12) var s_map: sampler;
@group(1) @binding(13) var s_rep: sampler;
@group(1) @binding(14) var<uniform> tm: TerrainU;

struct TIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) col: vec3<f32>,
    @location(3) uv: vec2<f32>,
};
struct TOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) wp: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) col: vec3<f32>,
    @location(3) uv: vec2<f32>,
};

@vertex
fn vs_terrain(v: TIn) -> TOut {
    var o: TOut;
    o.clip = g.view_proj * vec4<f32>(v.pos, 1.0);
    o.wp = v.pos;
    o.nrm = v.nrm;
    o.col = v.col;
    o.uv = v.uv;
    return o;
}

// Anti-tiling: a second sample of every texture, rotated and at another
// scale, blended in by low-frequency noise so the repeat stops reading as a
// grid (the game's ground is painted, not stamped).
fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}
fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash2(i), hash2(i + vec2<f32>(1.0, 0.0)), u.x),
               mix(hash2(i + vec2<f32>(0.0, 1.0)), hash2(i + vec2<f32>(1.0, 1.0)), u.x), u.y);
}
fn uv2(uv: vec2<f32>) -> vec2<f32> {
    // ~37 degrees and 0.61x: no common period with the first sample.
    let c = 0.7986;
    let s = 0.6018;
    return vec2<f32>(c * uv.x - s * uv.y, s * uv.x + c * uv.y) * 0.61 + vec2<f32>(0.37, 0.71);
}

// Height blending: every layer bids its painted weight times its texture
// height (albedo alpha) scaled by the material's multiply / offset; the
// highest bids win, softened over a band set by the layer's smoothness.
fn bid(w: f32, h: f32, p: vec4<f32>) -> f32 {
    return select(-1.0, w * (h * p.y + p.z), w > 0.004);
}

fn tnormal(n: vec4<f32>) -> vec3<f32> {
    return vec3<f32>(n.x * 2.0 - 1.0, n.y * 2.0 - 1.0, max(n.z * 2.0 - 1.0, 0.2));
}

@fragment
fn fs_terrain(i: TOut) -> @location(0) vec4<f32> {
    if (tm.flags.x < 0.5) {
        return vec4<f32>(shade(i.col, i.nrm, i.wp), 1.0);
    }
    let w = textureSample(t_weights, s_map, i.uv);
    let xz = i.wp.xz;
    // 0..1 blend between the two samples, changing every ~25 m.
    let k = smoothstep(0.3, 0.7, vnoise(xz * 0.04) * 0.7 + vnoise(xz * 0.13) * 0.3);
    let u0 = xz * tm.slots[0].params.x;
    let u1 = xz * tm.slots[1].params.x;
    let u2 = xz * tm.slots[2].params.x;
    let u3 = xz * tm.slots[3].params.x;
    let u4 = xz * tm.slots[4].params.x;
    let a0 = mix(textureSample(t_alb0, s_rep, u0), textureSample(t_alb0, s_rep, uv2(u0)), k);
    let a1 = mix(textureSample(t_alb1, s_rep, u1), textureSample(t_alb1, s_rep, uv2(u1)), k);
    let a2 = mix(textureSample(t_alb2, s_rep, u2), textureSample(t_alb2, s_rep, uv2(u2)), k);
    let a3 = mix(textureSample(t_alb3, s_rep, u3), textureSample(t_alb3, s_rep, uv2(u3)), k);
    let a4 = mix(textureSample(t_alb4, s_rep, u4), textureSample(t_alb4, s_rep, uv2(u4)), k);
    let n0 = tnormal(mix(textureSample(t_nrm0, s_rep, u0), textureSample(t_nrm0, s_rep, uv2(u0)), k));
    let n1 = tnormal(mix(textureSample(t_nrm1, s_rep, u1), textureSample(t_nrm1, s_rep, uv2(u1)), k));
    let n2 = tnormal(mix(textureSample(t_nrm2, s_rep, u2), textureSample(t_nrm2, s_rep, uv2(u2)), k));
    let n3 = tnormal(mix(textureSample(t_nrm3, s_rep, u3), textureSample(t_nrm3, s_rep, uv2(u3)), k));
    let n4 = tnormal(mix(textureSample(t_nrm4, s_rep, u4), textureSample(t_nrm4, s_rep, uv2(u4)), k));

    let b0 = a0.a * tm.slots[0].params.y + tm.slots[0].params.z;
    let b1 = bid(w.r, a1.a, tm.slots[1].params);
    let b2 = bid(w.g, a2.a, tm.slots[2].params);
    let b3 = bid(w.b, a3.a, tm.slots[3].params);
    let b4 = bid(w.a, a4.a, tm.slots[4].params);
    let top = max(max(max(b0, b1), max(b2, b3)), b4);
    let band = vec4<f32>(tm.slots[1].params.w, tm.slots[2].params.w, tm.slots[3].params.w, tm.slots[4].params.w);
    let d0 = 0.2;
    let k0 = max(b0 - top + d0, 0.0);
    let k1 = max(b1 - top + max(band.x, 0.05) + 0.1, 0.0);
    let k2 = max(b2 - top + max(band.y, 0.05) + 0.1, 0.0);
    let k3 = max(b3 - top + max(band.z, 0.05) + 0.1, 0.0);
    let k4 = max(b4 - top + max(band.w, 0.05) + 0.1, 0.0);
    let sum = max(k0 + k1 + k2 + k3 + k4, 1e-4);
    var c = (a0.rgb * tm.slots[0].color.rgb * k0 + a1.rgb * tm.slots[1].color.rgb * k1
           + a2.rgb * tm.slots[2].color.rgb * k2 + a3.rgb * tm.slots[3].color.rgb * k3
           + a4.rgb * tm.slots[4].color.rgb * k4) * (2.0 / sum);
    let tn = (n0 * k0 + n1 * k1 + n2 * k2 + n3 * k3 + n4 * k4) / sum;

    if (tm.flags.y > 0.5) {
        c = c * textureSample(t_vcolor, s_map, i.uv).rgb;
    }
    // Large, soft light/dark patches: breaks the evenness a tiled texture has.
    c = c * (0.86 + 0.28 * vnoise(xz * 0.018));
    // Tangent frame of a height field: X along +X, Y along +Z.
    let up = normalize(i.nrm);
    let tx = normalize(vec3<f32>(1.0, 0.0, 0.0) - up * up.x);
    let tz = normalize(cross(tx, up));
    let n = normalize(tx * tn.x + tz * tn.y + up * tn.z);
    return vec4<f32>(shade(c, n, i.wp), 1.0);
}

// ---- water ---------------------------------------------------------------------
// The chapter's water material colours: shallow near the banks, deep further
// out, a bank colour in the thin edge, and the sun's glint.
@fragment
fn fs_water(i: TOut) -> @location(0) vec4<f32> {
    let depth = max(i.col.x, 0.0);
    let deep = select(vec3<f32>(0.17, 0.25, 0.23), tm.water_deep.rgb, tm.water_deep.a > 0.0);
    let shallow = select(vec3<f32>(0.13, 0.19, 0.15), tm.water_shallow.rgb, tm.water_shallow.a > 0.0);
    let banks = select(vec3<f32>(0.57, 0.59, 0.23), tm.water_banks.rgb, tm.water_banks.a > 0.0);
    var c = mix(shallow, deep, smoothstep(0.0, 2.5, depth));
    c = mix(banks * 0.6, c, smoothstep(0.02, 0.25, depth));
    // Gentle ripples for the light to catch.
    let rip = vnoise(i.wp.xz * 0.6 + vec2<f32>(0.0, 0.0)) - 0.5;
    let n = normalize(vec3<f32>(rip * 0.15, 1.0, -rip * 0.12));
    let v = normalize(g.eye.xyz - i.wp);
    let h = normalize(normalize(g.sun_dir.xyz) + v);
    let spec = pow(max(dot(n, h), 0.0), 80.0) * 0.6;
    let lit = shade(c, n, i.wp) + g.sun_color.rgb * spec * 0.05;
    let alpha = clamp(0.55 + smoothstep(0.0, 1.5, depth) * 0.35, 0.0, 0.92);
    return vec4<f32>(lit, alpha);
}

// ---- instanced scenery ----------------------------------------------------------

struct Material {
    color: vec4<f32>,
    flags: vec4<f32>,     // x: has texture, y: alpha test
    // Effect sprites only (map_scene._particle_params):
    fx0: vec4<f32>,       // life s, speed m/s, share alive, end size x
    fx1: vec4<f32>,       // start size x, flipbook cells across, first frame, last frame
    fx2: vec4<f32>,       // emission box half extents x, y, z (m), pivot v
    fx3: vec4<f32>,       // x: turn range (rad)
};
@group(1) @binding(0) var t_albedo: texture_2d<f32>;
@group(1) @binding(1) var s_albedo: sampler;
@group(1) @binding(2) var<uniform> mat: Material;

struct MIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) m3: vec4<f32>,
    @location(7) tint: vec4<f32>,
};
struct MOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) wp: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tint: vec4<f32>,
};

@vertex
fn vs_mesh(v: MIn) -> MOut {
    let m = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let wp = m * vec4<f32>(v.pos, 1.0);
    var o: MOut;
    o.clip = g.view_proj * wp;
    o.wp = wp.xyz;
    o.nrm = (m * vec4<f32>(v.nrm, 0.0)).xyz;
    o.uv = v.uv;
    o.tint = v.tint;
    return o;
}

fn albedo(i: MOut) -> vec4<f32> {
    var c = mat.color;
    if (mat.flags.x > 0.5) {
        c = c * textureSample(t_albedo, s_albedo, i.uv);
    }
    return c;
}

@fragment
fn fs_mesh(i: MOut) -> @location(0) vec4<f32> {
    let a = albedo(i);
    if (mat.flags.y > 0.5 && a.a < 0.5) { discard; }
    var rgb = shade(a.rgb, i.nrm, i.wp);
    rgb = mix(rgb, i.tint.rgb, i.tint.a);
    return vec4<f32>(rgb, 1.0);
}

// An effect's sprite: the quad (x, z in -0.5..0.5) turned to face the camera,
// centred on the instance's origin, as wide as its x axis.
fn hash1(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

// One particle of an effect sprite (slot nrm.x of VFX_PARTICLES). It is born
// somewhere in the emitter's box, flies up a cone at the emitter's speed,
// grows from the start to the end size, plays its flipbook and fades in and
// out over its life; g.params.w is the time. The instance's x axis length is
// the particle size (object scale included).
@vertex
fn vs_billboard(v: MIn) -> MOut {
    let m = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let c = (m * vec4<f32>(0.0, 0.0, 0.0, 1.0)).xyz;
    let size = max(length(m[0].xyz), 1e-4);
    let k = v.nrm.x;
    let life = max(0.05, mat.fx0.x);
    let seed = hash1(c.xz + vec2<f32>(k * 17.0, k * 3.0));
    let phase = fract(g.params.w / life + k + seed);
    let r3 = vec3<f32>(hash1(vec2<f32>(seed, 1.0)), hash1(vec2<f32>(seed, 2.0)), hash1(vec2<f32>(seed, 3.0))) * 2.0 - 1.0;
    let dir = normalize(vec3<f32>(r3.x * 0.35, 1.0, r3.z * 0.35));
    let centre0 = c + r3 * mat.fx2.xyz + dir * mat.fx0.y * phase * life;
    var grow = size * mix(mat.fx1.x, mat.fx0.w, phase);
    if (k >= mat.fx0.z) { grow = 0.0; }   // fewer particles for a sparse effect
    let fwd = normalize(centre0 - g.eye.xyz);
    var right = cross(vec3<f32>(0.0, 1.0, 0.0), fwd);
    if (length(right) < 1e-3) { right = vec3<f32>(1.0, 0.0, 0.0); }
    right = normalize(right);
    let up = cross(fwd, right);
    // The pivot (a texture row) sits on the particle: a flame stands on its base.
    let centre = centre0 + up * (mat.fx2.w - 0.5) * grow;
    let spin = (seed - 0.5) * 2.0 * mat.fx3.x;
    let q = vec2<f32>(v.pos.x * cos(spin) - v.pos.z * sin(spin), v.pos.x * sin(spin) + v.pos.z * cos(spin));
    // pos.z = +0.5 is the texture's top row (uv.y = 0).
    let wp = centre + (right * q.x + up * q.y) * grow;
    var uv = v.uv;
    let cells = max(1.0, mat.fx1.y);
    if (cells > 1.0) {
        let frames = max(1.0, mat.fx1.w - mat.fx1.z + 1.0);
        let f = min(mat.fx1.w, mat.fx1.z + floor(phase * frames));
        uv = (uv + vec2<f32>(f % cells, floor(f / cells))) / cells;
    }
    var o: MOut;
    o.clip = g.view_proj * vec4<f32>(wp, 1.0);
    o.wp = wp;
    // fs_vfx reads the fade from nrm.x: in at birth, out at death.
    o.nrm = vec3<f32>(sin(phase * 3.14159), 0.0, 0.0);
    o.uv = uv;
    o.tint = v.tint;
    return o;
}

@fragment
fn fs_vfx(i: MOut) -> @location(0) vec4<f32> {
    let a = albedo(i);
    // Mask textures carry their shape in the colour; take the dimmer of the two
    // as coverage so an opaque square never shows as a block.
    let cover = min(a.a, max(a.r, max(a.g, a.b)));
    let rgb = mix(a.rgb, i.tint.rgb, i.tint.a);
    return vec4<f32>(rgb * g.params.y, cover * 0.45 * clamp(i.nrm.x, 0.0, 1.0));
}

@fragment
fn fs_decal(i: MOut) -> @location(0) vec4<f32> {
    let a = albedo(i);
    var rgb = shade(a.rgb, vec3<f32>(0.0, 1.0, 0.0), i.wp);
    rgb = mix(rgb, i.tint.rgb, i.tint.a);
    return vec4<f32>(rgb, a.a);
}

// ---- overlay lines (selection boxes, brush ring, slot markers) -------------------

struct LIn {
    @location(0) pos: vec3<f32>,
    @location(1) col: vec4<f32>,
};
struct LOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) col: vec4<f32>,
};

@vertex
fn vs_line(v: LIn) -> LOut {
    var o: LOut;
    o.clip = g.view_proj * vec4<f32>(v.pos, 1.0);
    o.col = v.col;
    return o;
}

@fragment
fn fs_line(i: LOut) -> @location(0) vec4<f32> {
    return i.col;
}

// ---- shadow pass (depth only, from the sun) ------------------------------------

@vertex
fn vs_shadow_terrain(@location(0) pos: vec3<f32>) -> @builtin(position) vec4<f32> {
    return g.light_vp * vec4<f32>(pos, 1.0);
}

@vertex
fn vs_shadow_mesh(
    @location(0) pos: vec3<f32>,
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) m3: vec4<f32>,
) -> @builtin(position) vec4<f32> {
    let m = mat4x4<f32>(m0, m1, m2, m3);
    return g.light_vp * (m * vec4<f32>(pos, 1.0));
}
