//! A chapter as the editor holds it: terrain heightfield and scenery objects,
//! decoded from the backend's JSON (`/map/api/terrain`, `/map/api/scenery`).

use anyhow::{anyhow, Context as _, Result};
use base64::Engine as _;
use glam::{Mat4, Quat, Vec3};
use serde_json::Value;

use crate::assets::Vertex;

fn b64(v: &Value) -> Result<Vec<u8>> {
    let s = v.as_str().ok_or_else(|| anyhow!("expected a base64 string"))?;
    base64::engine::general_purpose::STANDARD.decode(s).context("bad base64")
}

fn f32s(v: &Value) -> Result<Vec<f32>> {
    Ok(bytemuck::pod_collect_to_vec::<u8, f32>(&b64(v)?))
}

pub fn f32s_pub(v: &Value) -> Result<Vec<f32>> {
    f32s(v)
}

fn u32s(v: &Value) -> Result<Vec<u32>> {
    Ok(bytemuck::pod_collect_to_vec::<u8, u32>(&b64(v)?))
}

fn u16s(v: &Value) -> Result<Vec<u16>> {
    Ok(bytemuck::pod_collect_to_vec::<u8, u16>(&b64(v)?))
}

fn vec3(v: &Value) -> Result<Vec3> {
    let a = v.as_array().ok_or_else(|| anyhow!("expected [x, y, z]"))?;
    let f = |i: usize| a.get(i).and_then(Value::as_f64).map(|x| x as f32).ok_or_else(|| anyhow!("bad vec3"));
    Ok(Vec3::new(f(0)?, f(1)?, f(2)?))
}

// ---------------------------------------------------------------------------
// terrain
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Terrain {
    pub n: usize,
    pub min: Vec3,
    pub max: Vec3,
    pub h: Vec<f32>,
    pub path: Option<Vec<f32>>,
    pub block: Option<Vec<f32>>,
    /// Water surface height per cell (metres), where the chapter paints water.
    pub water: Option<Vec<f32>>,
    pub hmin: f32,
    pub hmax: f32,
}

impl Terrain {
    pub fn from_json(t: &Value) -> Result<Terrain> {
        let n = t["grid"].as_u64().ok_or_else(|| anyhow!("terrain: no grid"))? as usize;
        let quant16 = |o: &Value| -> Result<Vec<f32>> {
            let lo = o["min"].as_f64().unwrap_or(0.0) as f32;
            let hi = o["max"].as_f64().unwrap_or(0.0) as f32;
            Ok(u16s(&o["u16"])?.into_iter().map(|q| lo + q as f32 / 65535.0 * (hi - lo)).collect())
        };
        let quant8 = |o: &Value| -> Option<Vec<f32>> {
            b64(o).ok().map(|b| b.into_iter().map(|q| q as f32 / 255.0).collect())
        };
        let h = quant16(&t["height"])?;
        if h.len() != n * n {
            return Err(anyhow!("terrain: {} heights for a {n}x{n} grid", h.len()));
        }
        Ok(Terrain {
            n,
            min: vec3(&t["box_min"])?,
            max: vec3(&t["box_max"])?,
            hmin: t["height"]["min"].as_f64().unwrap_or(0.0) as f32,
            hmax: t["height"]["max"].as_f64().unwrap_or(0.0) as f32,
            path: t.get("path").and_then(quant8).filter(|v| v.len() == n * n),
            block: t.get("block").and_then(quant8).filter(|v| v.len() == n * n),
            water: t.get("water").filter(|w| w.is_object()).and_then(|w| quant16(w).ok()).filter(|v| v.len() == n * n),
            h,
        })
    }

    /// A flat square of ground, `half` metres each way: the stage a tile is
    /// edited on when it stands alone.
    pub fn flat(half: f32) -> Terrain {
        let n = 64;
        Terrain {
            n,
            min: Vec3::new(-half, -50.0, -half),
            max: Vec3::new(half, 50.0, half),
            h: vec![0.0; n * n],
            path: None,
            block: None,
            water: None,
            hmin: 0.0,
            hmax: 0.0,
        }
    }

    pub fn cell(&self) -> (f32, f32) {
        ((self.max.x - self.min.x) / self.n as f32, (self.max.z - self.min.z) / self.n as f32)
    }

    pub fn vertex_pos(&self, r: usize, c: usize) -> Vec3 {
        let (sx, sz) = self.cell();
        Vec3::new(self.min.x + (c as f32 + 0.5) * sx, self.h[r * self.n + c], self.min.z + (r as f32 + 0.5) * sz)
    }

    /// Ground height under (x, z), bilinear; None off the map.
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        let (sx, sz) = self.cell();
        let fc = (x - self.min.x) / sx - 0.5;
        let fr = (z - self.min.z) / sz - 0.5;
        let n = self.n as f32;
        if fc < 0.0 || fr < 0.0 || fc > n - 1.0 || fr > n - 1.0 {
            return None;
        }
        let (c0, r0) = (fc.floor() as usize, fr.floor() as usize);
        let (c1, r1) = ((c0 + 1).min(self.n - 1), (r0 + 1).min(self.n - 1));
        let (tc, tr) = (fc - c0 as f32, fr - r0 as f32);
        let at = |r: usize, c: usize| self.h[r * self.n + c];
        let a = at(r0, c0) + (at(r0, c1) - at(r0, c0)) * tc;
        let b = at(r1, c0) + (at(r1, c1) - at(r1, c0)) * tc;
        Some(a + (b - a) * tr)
    }

    /// The colour the browser editor gives the ground (`groundColor` in
    /// map.html, without the tint layer), in linear RGB.
    pub fn color(&self, i: usize) -> [f32; 3] {
        if self.hmin == self.hmax {
            // The plain stage a tile is edited on: a quiet dark floor.
            return [0.045, 0.05, 0.045];
        }
        let t = ((self.h[i] - self.hmin) / (self.hmax - self.hmin).max(1e-6)).clamp(0.0, 1.0);
        let (lo, mid, hi) = ([0.29, 0.36, 0.25], [0.50, 0.56, 0.36], [0.78, 0.76, 0.62]);
        let (a, b, f) = if t < 0.5 { (lo, mid, t * 2.0) } else { (mid, hi, (t - 0.5) * 2.0) };
        let mut c = [0.0f32; 3];
        for k in 0..3 {
            c[k] = a[k] + (b[k] - a[k]) * f;
        }
        let mix = |c: &mut [f32; 3], to: [f32; 3], w: f32| {
            for k in 0..3 {
                c[k] += (to[k] - c[k]) * w;
            }
        };
        if let Some(b) = &self.block {
            if b[i] > 0.05 {
                mix(&mut c, [0.36, 0.30, 0.32], b[i].min(1.0) * 0.6);
            }
        }
        if let Some(p) = &self.path {
            if p[i] > 0.05 {
                mix(&mut c, [0.85, 0.72, 0.47], p[i].min(1.0) * 0.85);
            }
        }
        c.map(|v| v.max(0.0).powf(2.2))
    }

    pub fn normal(&self, r: usize, c: usize) -> Vec3 {
        let n = self.n;
        let (sx, sz) = self.cell();
        let h = |r: usize, c: usize| self.h[r * n + c];
        let (c0, c1) = (c.saturating_sub(1), (c + 1).min(n - 1));
        let (r0, r1) = (r.saturating_sub(1), (r + 1).min(n - 1));
        let dx = (h(r, c1) - h(r, c0)) / ((c1 - c0).max(1) as f32 * sx);
        let dz = (h(r1, c) - h(r0, c)) / ((r1 - r0).max(1) as f32 * sz);
        Vec3::new(-dx, 1.0, -dz).normalize()
    }

    /// Vertices and linear colours of grid rows `r0..=r1`.
    pub fn mesh_rows(&self, r0: usize, r1: usize) -> (Vec<Vertex>, Vec<[f32; 3]>) {
        let n = self.n;
        let r1 = r1.min(n - 1);
        let mut v = Vec::with_capacity((r1 + 1 - r0) * n);
        let mut col = Vec::with_capacity((r1 + 1 - r0) * n);
        for r in r0..=r1 {
            for c in 0..n {
                let p = self.vertex_pos(r, c);
                v.push(Vertex { pos: p.into(), nrm: self.normal(r, c).into(), uv: [0.0, 0.0] });
                col.push(self.color(r * n + c));
            }
        }
        (v, col)
    }

    /// Grid vertices (row-major, `n * n`), their linear colours, and indices.
    pub fn mesh(&self) -> (Vec<Vertex>, Vec<[f32; 3]>, Vec<u32>) {
        let n = self.n;
        let (v, col) = self.mesh_rows(0, n - 1);
        let mut idx = Vec::with_capacity((n - 1) * (n - 1) * 6);
        for r in 0..n - 1 {
            for c in 0..n - 1 {
                let a = (r * n + c) as u32;
                let (b, d) = (a + 1, a + n as u32);
                let e = d + 1;
                idx.extend_from_slice(&[a, b, d, b, e, d]);
            }
        }
        (v, col, idx)
    }

    /// Where a ray first meets the ground, by marching then bisecting.
    pub fn raycast(&self, o: Vec3, d: Vec3, max_t: f32) -> Option<Vec3> {
        let (sx, sz) = self.cell();
        let step = sx.min(sz) * 0.75;
        let mut prev_t = 0.0;
        let mut t = 0.0;
        let above = |t: f32| {
            let p = o + d * t;
            self.height_at(p.x, p.z).map(|h| p.y >= h)
        };
        let mut was = above(0.0);
        while t < max_t {
            t += step;
            let now = above(t);
            if let (Some(true), Some(false)) = (was, now) {
                let (mut a, mut b) = (prev_t, t);
                for _ in 0..24 {
                    let m = (a + b) * 0.5;
                    if above(m) == Some(true) {
                        a = m
                    } else {
                        b = m
                    }
                }
                return Some(o + d * b);
            }
            if now.is_some() {
                was = now;
            }
            prev_t = t;
        }
        None
    }
}

// ---------------------------------------------------------------------------
// scenery
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PartKey {
    pub mesh: String,
    pub texture: Option<String>,
    pub swap_rb: bool,
    pub color: Option<String>,
    pub decal: bool,
    /// An effect's sprite: a camera-facing glowing quad (`map_scene.vfx_parts`).
    pub vfx: bool,
}

pub struct ScPart {
    pub key: PartKey,
    /// Shipped matrix of every instance.
    pub base: Vec<Mat4>,
    /// Object index each instance belongs to.
    pub owners: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xf {
    pub pos: Vec3,
    pub rot: Quat,
    pub scale: Vec3,
}

impl Xf {
    pub fn mat(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rot, self.pos)
    }
}

pub struct Scenery {
    pub levels: Vec<String>,
    pub entities: Vec<String>,
    pub obj_level: Vec<u16>,
    pub obj_id: Vec<u32>,
    pub obj_entity: Vec<u32>,
    pub obj_xf: Vec<Xf>,
    pub parts: Vec<ScPart>,
    /// Per object: (part index, instance index) of everything it draws.
    pub obj_instances: Vec<Vec<(u32, u32)>>,
    /// The generation recipe's tile spots (filled once the recipe arrives).
    pub spots: Vec<crate::recipe::Spot>,
}

impl Scenery {
    pub fn from_json(v: &Value) -> Result<Scenery> {
        let strs = |k: &str| -> Vec<String> {
            v[k].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()).unwrap_or_default()
        };
        let o = &v["objects"];
        let count = o["count"].as_u64().unwrap_or(0) as usize;
        let obj_level = u16s(&o["level"])?;
        let obj_id = u32s(&o["id"])?;
        let obj_entity = u32s(&o["entity"])?;
        let xf = f32s(&o["xf"])?;
        if obj_level.len() != count || obj_id.len() != count || obj_entity.len() != count || xf.len() != count * 10 {
            return Err(anyhow!("scenery: object arrays disagree with count {count}"));
        }
        let obj_xf = xf
            .chunks_exact(10)
            .map(|k| Xf {
                pos: Vec3::new(k[0], k[1], k[2]),
                rot: Quat::from_xyzw(k[3], k[4], k[5], k[6]).normalize(),
                scale: Vec3::new(k[7], k[8], k[9]),
            })
            .collect();
        let parts_v = v["parts"].as_array().ok_or_else(|| anyhow!("scenery: no parts"))?;
        let mats_v = v["matrices"].as_array().ok_or_else(|| anyhow!("scenery: no matrices"))?;
        let owners_v = v["owners"].as_array().ok_or_else(|| anyhow!("scenery: no owners"))?;
        let mut parts = Vec::with_capacity(parts_v.len());
        let mut obj_instances = vec![Vec::new(); count];
        for (pi, p) in parts_v.iter().enumerate() {
            let key = PartKey {
                mesh: p["mesh"].as_str().unwrap_or_default().to_owned(),
                texture: p["texture"].as_str().map(str::to_owned),
                swap_rb: p["swap_rb"].as_bool().unwrap_or(false),
                color: p["color"].as_str().map(str::to_owned),
                decal: p["decal"].as_bool().unwrap_or(false),
                vfx: p["vfx"].as_bool().unwrap_or(false),
            };
            let m = f32s(&mats_v[pi])?;
            let owners = u32s(&owners_v[pi])?;
            let base: Vec<Mat4> = m.chunks_exact(16).map(Mat4::from_cols_slice).collect();
            if base.len() != owners.len() {
                return Err(anyhow!("scenery: part {pi} has {} matrices, {} owners", base.len(), owners.len()));
            }
            for (j, &ow) in owners.iter().enumerate() {
                if let Some(list) = obj_instances.get_mut(ow as usize) {
                    list.push((pi as u32, j as u32));
                }
            }
            parts.push(ScPart { key, base, owners });
        }
        Ok(Scenery {
            levels: strs("levels"),
            entities: strs("entities"),
            obj_level,
            obj_id,
            obj_entity,
            obj_xf,
            parts,
            obj_instances,
            spots: Vec::new(),
        })
    }

    pub fn object_name(&self, i: usize) -> String {
        let e = self.entities.get(self.obj_entity[i] as usize).map(String::as_str).unwrap_or("?");
        e.rsplit('\\').next().unwrap_or(e).trim_end_matches(".entity.ot").to_owned()
    }

    pub fn instance_count(&self) -> usize {
        self.parts.iter().map(|p| p.base.len()).sum()
    }
}

/// `#rrggbb` to linear RGB.
pub fn hex_linear(s: &str) -> Option<[f32; 3]> {
    let s = s.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let ch = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok().map(|b| (b as f32 / 255.0).powf(2.2));
    Some([ch(0)?, ch(2)?, ch(4)?])
}

// ---------------------------------------------------------------------------
// terrain look (material + blend weights, from /map/api/terrain/look)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct LookSlot {
    /// Display name: the albedo texture's file name ("Ground_Grass").
    pub label: String,
    pub albedo: Option<String>,
    pub normal: Option<String>,
    pub uv_scale: f32,
    pub color: [f32; 4],
    pub height_mul_offset: [f32; 2],
    pub smooth: f32,
}

#[derive(Clone, Debug)]
pub struct WaterLook {
    pub deep: [f32; 4],
    pub shallow: [f32; 4],
    pub banks: [f32; 4],
}

#[derive(Clone)]
pub struct TerrainLook {
    pub water: Option<WaterLook>,
    /// The painted layer each weight channel is made from (what a brush paints).
    pub channel_sources: Vec<Option<String>>,
    /// The painted RGBA vertex-colour layer.
    pub color_layer: Option<String>,
    pub n: usize,
    /// base, R, G, B, A
    pub slots: Vec<LookSlot>,
    /// RGBA8 blend weight per cell (channel k weights slot k+1).
    pub weights: Vec<u8>,
    /// RGBA8 vertex colour per cell.
    pub color: Option<Vec<u8>>,
    /// World rectangle the in-game minimap picture covers: x0, z0, width, depth.
    pub minimap_rect: Option<[f32; 4]>,
}

impl TerrainLook {
    pub fn from_json(v: &Value) -> Result<TerrainLook> {
        let n = v["grid"].as_u64().ok_or_else(|| anyhow!("look: no grid"))? as usize;
        let f = |x: &Value, d: f32| x.as_f64().map(|y| y as f32).unwrap_or(d);
        let slots = ["base", "R", "G", "B", "A"]
            .iter()
            .map(|s| {
                let o = &v["slots"][*s];
                let col = o["color"].as_array();
                let c = |i: usize, d: f32| col.and_then(|a| a.get(i)).map(|x| f(x, d)).unwrap_or(d);
                let hm = o["height_mul_offset"].as_array();
                let h = |i: usize, d: f32| hm.and_then(|a| a.get(i)).map(|x| f(x, d)).unwrap_or(d);
                let albedo = o["albedo"].as_str().map(str::to_owned);
                let label = albedo
                    .as_deref()
                    .and_then(|a| a.rsplit('/').next())
                    .map(|f| f.trim_end_matches(".png").trim_start_matches("T_").trim_end_matches("_ALB").replace('_', " "))
                    .unwrap_or_else(|| s.to_string());
                LookSlot {
                    label,
                    albedo,
                    normal: o["normal"].as_str().map(str::to_owned),
                    uv_scale: f(&o["uv_scale"], 0.1),
                    color: [c(0, 0.5), c(1, 0.5), c(2, 0.5), c(3, 1.0)],
                    height_mul_offset: [h(0, 1.0), h(1, 0.0)],
                    smooth: f(&o["smooth"], 0.1),
                }
            })
            .collect();
        let weights = b64(&v["weights"])?;
        if weights.len() != n * n * 4 {
            return Err(anyhow!("look: {} weight bytes for {n}x{n}", weights.len()));
        }
        let color = b64(&v["color"]).ok().filter(|c| c.len() == n * n * 4);
        let channel_sources = v["channel_sources"].as_array().map(|a| a.iter().map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        let color_layer = v["color_layer"].as_str().map(str::to_owned);
        let c4 = |x: &Value| -> Option<[f32; 4]> {
            let a = x.as_array()?;
            Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32, 1.0])
        };
        let w = &v["water"];
        // The water material's colours are colour-picker values (sRGB bytes,
        // 0.172549 = 44/255): linear for the shader, or the water draws about
        // twice too bright (the "pale ravine").
        let lin = |c: [f32; 4]| [c[0].max(0.0).powf(2.2), c[1].max(0.0).powf(2.2), c[2].max(0.0).powf(2.2), c[3]];
        let water = w.is_object().then(|| WaterLook {
            deep: lin(c4(&w["deep"]).unwrap_or([0.17, 0.25, 0.23, 1.0])),
            shallow: lin(c4(&w["shallow"]).unwrap_or([0.13, 0.19, 0.15, 1.0])),
            banks: lin(c4(&w["banks"]).unwrap_or([0.57, 0.59, 0.23, 1.0])),
        });
        let minimap_rect = v["minimap_rect"].as_array().filter(|a| a.len() == 4).map(|a| std::array::from_fn(|i| a[i].as_f64().unwrap_or(0.0) as f32));
        Ok(TerrainLook { water, channel_sources, color_layer, n, slots, weights, color, minimap_rect })
    }

    pub fn textures(&self) -> Vec<String> {
        self.slots.iter().flat_map(|s| [s.albedo.clone(), s.normal.clone()]).flatten().collect()
    }
}

// ---------------------------------------------------------------------------
// environment (sun / ambient / fog, from /map/api/environment)
// ---------------------------------------------------------------------------

/// A chapter's daytime light, as the game data states it (render_look.py).
#[derive(Clone, Debug, PartialEq)]
pub struct Environment {
    pub sun_color: [f32; 3],
    pub sun_intensity: f32,
    /// 0..1, 1 = overhead.
    pub sun_zenith: f32,
    /// Degrees about +Y.
    pub sun_yaw: f32,
    pub amb_top: [f32; 3],
    pub amb_horizon: [f32; 3],
    pub amb_bottom: [f32; 3],
    pub amb_mul: f32,
    pub amb_pow: f32,
    pub fog_on: bool,
    pub fog_intensity: f32,
    pub fog_brightness: f32,
    pub fog_bottom: [f32; 4],
    pub fog_mid: [f32; 4],
    pub fog_top: [f32; 4],
    /// World heights of the top, mid and bottom fog colours.
    pub fog_heights: [f32; 3],
    /// Start, mid, end distance (metres beyond the camera's focus).
    pub fog_distances: [f32; 3],
}

impl Default for Environment {
    fn default() -> Self {
        Environment {
            sun_color: [1.0, 0.92, 0.75],
            sun_intensity: 8.0,
            sun_zenith: 0.7,
            sun_yaw: -57.0,
            amb_top: [0.45, 0.5, 0.55],
            amb_horizon: [0.3, 0.3, 0.28],
            amb_bottom: [0.05, 0.05, 0.05],
            amb_mul: 0.75,
            amb_pow: 1.0,
            fog_on: false,
            fog_intensity: 1.0,
            fog_brightness: 1.0,
            fog_bottom: [0.0; 4],
            fog_mid: [0.0; 4],
            fog_top: [0.0; 4],
            fog_heights: [5.0, 0.0, -15.0],
            fog_distances: [0.0, 20.0, 40.0],
        }
    }
}

impl Environment {
    pub fn from_json(v: &Value) -> Environment {
        let mut e = Environment::default();
        let f = |x: &Value| x.as_f64().map(|y| y as f32);
        let a = |x: &Value, n: usize| -> Option<Vec<f32>> {
            let arr = x.as_array()?;
            let out: Vec<f32> = arr.iter().filter_map(|y| y.as_f64().map(|z| z as f32)).collect();
            (out.len() >= n).then_some(out)
        };
        let s = &v["sun"];
        if let Some(c) = a(&s["color"], 3) {
            e.sun_color = [c[0], c[1], c[2]];
        }
        e.sun_intensity = f(&s["intensity"]).unwrap_or(e.sun_intensity);
        e.sun_zenith = f(&s["zenith"]).unwrap_or(e.sun_zenith);
        e.sun_yaw = f(&s["yaw"]).unwrap_or(e.sun_yaw);
        let am = &v["ambient"];
        if am.is_object() {
            let c3 = |k: &str, d: [f32; 3]| a(&am[k], 3).map(|c| [c[0], c[1], c[2]]).unwrap_or(d);
            e.amb_top = c3("top", e.amb_top);
            e.amb_horizon = c3("horizon", e.amb_horizon);
            e.amb_bottom = c3("bottom", e.amb_bottom);
            e.amb_mul = f(&am["mul"]).unwrap_or(e.amb_mul);
            e.amb_pow = f(&am["horizon_pow"]).unwrap_or(e.amb_pow).max(0.05);
        }
        let fg = &v["fog"];
        if fg.is_object() {
            e.fog_on = true;
            let c4 = |k: &str| a(&fg[k], 4).map(|c| [c[0], c[1], c[2], c[3]]).unwrap_or([0.0; 4]);
            e.fog_bottom = c4("bottom");
            e.fog_mid = c4("mid");
            e.fog_top = c4("top");
            e.fog_intensity = f(&fg["intensity"]).unwrap_or(1.0);
            e.fog_brightness = f(&fg["brightness"]).unwrap_or(1.0);
            if let Some(h) = a(&fg["heights"], 3) {
                e.fog_heights = [h[0], h[1], h[2]];
            }
            if let Some(d) = a(&fg["distances"], 3) {
                e.fog_distances = [d[0], d[1], d[2]];
            }
        }
        e
    }

    /// Unit vector toward the sun: yaw about +Y, elevation = zenith x 90 deg.
    pub fn sun_dir(&self) -> glam::Vec3 {
        let el = (self.sun_zenith.clamp(0.02, 1.0) * 90.0).to_radians();
        let yaw = self.sun_yaw.to_radians();
        glam::Vec3::new(-yaw.sin() * el.cos(), el.sin(), -yaw.cos() * el.cos()).normalize()
    }
}
