//! The 3D view: wgpu, drawn offscreen and shown as an egui texture.
//!
//! One instanced draw per unique mesh+material ("part"). Instances are culled
//! on the CPU each time the camera or the scene changes: against the view
//! frustum, and by projected size, so a pebble a kilometre away costs nothing.
//! A chapter is ~360 parts and ~17.5k instances (~13M triangles unculled).

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use eframe::egui;
use eframe::egui_wgpu;
use eframe::wgpu;
use eframe::wgpu::util::DeviceExt;
use glam::{Mat4, Vec3, Vec4};

use crate::assets::{MeshData, TextureData, Vertex};
use crate::camera::Camera;
use crate::scene::{hex_linear, Environment, PartKey, Terrain, TerrainLook};

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const SAMPLES: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    sun_dir: [f32; 4],
    params: [f32; 4],
    /// The sun's view of the near cascade (round the focus) ...
    light_vp: [[f32; 4]; 4],
    /// ... and of the far one (the whole view).
    light_vp_far: [[f32; 4]; 4],
    /// rgb: sun colour x intensity
    sun_color: [f32; 4],
    /// rgb x ambient multiplier; top.w = horizon power
    amb_top: [f32; 4],
    amb_horizon: [f32; 4],
    amb_bottom: [f32; 4],
    fog_bottom: [f32; 4],
    fog_mid: [f32; 4],
    fog_top: [f32; 4],
    /// top, mid, bottom height; w = fog on
    fog_heights: [f32; 4],
    /// start, mid, end distance beyond the focus; w = camera focus distance
    fog_dist: [f32; 4],
    /// intensity, brightness
    fog_params: [f32; 4],
    /// x: point lights in use
    light_info: [f32; 4],
    /// Per light: position + radius, then colour x intensity.
    lights: [[f32; 4]; 2 * MAX_LIGHTS],
}

/// Point lights the Scene lights (the nearest to the camera).
pub const MAX_LIGHTS: usize = 16;

/// Shadow map edge in texels.
const SHADOW_SIZE: u32 = 4096;
/// Shadow cascades (Bevy's cascaded shadow maps, cut to two): a sharp one round
/// the focus and one over the whole view. ponytail: both draw every caster in
/// the far box; cull per cascade if the shadow pass shows up in frame times.
const CASCADES: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaterialU {
    color: [f32; 4],
    flags: [f32; 4],
    /// An effect sprite's particle parameters (`map_scene._particle_params`), else zero.
    fx: [[f32; 4]; 4],
}

/// `vfx:<texture>?<16 numbers>` -> the particle parameters the shader reads.
pub fn vfx_params(mesh: &str) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    if let Some((_, q)) = mesh.split_once('?') {
        for (i, v) in q.split(',').filter_map(|x| x.parse::<f32>().ok()).take(16).enumerate() {
            out[i / 4][i % 4] = v;
        }
    }
    out
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Instance {
    pub m: [[f32; 4]; 4],
    pub tint: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TerrainVertex {
    pos: [f32; 3],
    nrm: [f32; 3],
    col: [f32; 3],
    /// Cell-centre position in the terrain grid, 0..1 (samples the weight map).
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct TerrainSlotU {
    color: [f32; 4],
    /// uv scale, height mul, height offset, smooth
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct TerrainU {
    slots: [TerrainSlotU; 5],
    /// x: material ready, y: has vertex colour
    flags: [f32; 4],
    water_deep: [f32; 4],
    water_shallow: [f32; 4],
    water_banks: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub col: [f32; 4],
}

pub struct GpuMesh {
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    count: u32,
    pub center: Vec3,
    pub radius: f32,
    /// Bounding box in the mesh's own space (object snapping).
    pub lo: Vec3,
    pub hi: Vec3,
    /// CPU copy for picking.
    pub cpu: Arc<MeshData>,
    /// Built on the first pick.
    pub bvh: std::sync::OnceLock<crate::bvh::Bvh>,
}

struct GpuTexture {
    view: wgpu::TextureView,
    has_alpha: bool,
}

/// What the renderer needs per part from the scene, every frame it changes.
#[derive(Clone, Copy)]
pub struct DrawPart<'a> {
    pub key: &'a PartKey,
    /// World matrix + tint of every instance that should be drawn (before culling).
    pub instances: &'a [Instance],
}

struct PartMat {
    bind: wgpu::BindGroup,
}

struct Target {
    size: (u32, u32),
    msaa: wgpu::TextureView,
    resolve: wgpu::Texture,
    resolve_view: wgpu::TextureView,
    depth: wgpu::TextureView,
    egui_id: egui::TextureId,
}

pub struct Renderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    globals: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    mat_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: wgpu::TextureView,
    terrain_pipe: wgpu::RenderPipeline,
    mesh_pipe: wgpu::RenderPipeline,
    /// Terrain and mesh drawn as lines (`None`: the GPU cannot).
    /// The meshes' wireframe overlay (the dense ground grid stays solid).
    wire: Option<wgpu::RenderPipeline>,
    decal_pipe: wgpu::RenderPipeline,
    vfx_pipe: wgpu::RenderPipeline,
    line_pipe: wgpu::RenderPipeline,
    line_top_pipe: wgpu::RenderPipeline,
    terrain: Option<(wgpu::Buffer, wgpu::Buffer, u32)>,
    water: Option<(wgpu::Buffer, u32)>,
    water_pipe: wgpu::RenderPipeline,
    /// What the terrain uniform holds, so the material can be switched off
    /// (a tile edited alone stands on plain ground) and back on.
    terrain_u_cpu: TerrainU,
    /// Per cascade: the uniform copy its pass reads, its bind group and its layer.
    shadow_bufs: Vec<wgpu::Buffer>,
    shadow_bgs: Vec<wgpu::BindGroup>,
    shadow_layers: Vec<wgpu::TextureView>,
    shadow_terrain_pipe: wgpu::RenderPipeline,
    shadow_mesh_pipe: wgpu::RenderPipeline,
    terrain_bgl: wgpu::BindGroupLayout,
    terrain_bg: wgpu::BindGroup,
    terrain_u: wgpu::Buffer,
    map_sampler: wgpu::Sampler,
    flat_normal: wgpu::TextureView,
    /// The look waiting for its textures, and its weight / colour maps.
    look: Option<(TerrainLook, wgpu::TextureView, wgpu::TextureView)>,
    /// The live weight / colour map textures (a brush rewrites them).
    look_maps: Option<(wgpu::Texture, wgpu::Texture, usize)>,
    pub meshes: HashMap<String, GpuMesh>,
    textures: HashMap<(String, bool), Option<GpuTexture>>,
    mats: HashMap<PartKey, PartMat>,
    instance_buf: Option<wgpu::Buffer>,
    instance_cap: u64,
    line_buf: Option<wgpu::Buffer>,
    line_cap: u64,
    target: Option<Target>,
    /// Project thumbnails: each its own target, kept while the editor runs.
    thumbs: Vec<Target>,
    /// The Project preview window's target, redrawn in place.
    preview: Option<Target>,
    /// The split view's own target (View > Split view).
    side: Option<Target>,
    pub stats: Stats,
}

#[derive(Default, Clone, Copy)]
pub struct Stats {
    pub drawn_instances: usize,
    pub drawn_triangles: u64,
    pub draws: usize,
    /// Effect sprites on screen (the editor keeps redrawing while they move).
    pub vfx: usize,
}

pub struct Frame<'a> {
    pub camera: &'a Camera,
    pub parts: Vec<DrawPart<'a>>,
    pub lines: &'a [LineVertex],
    /// Lines drawn over everything (gizmos, cursor).
    pub top_lines: &'a [LineVertex],
    /// Hide instances smaller than this many pixels on screen.
    pub min_px: f32,
    pub haze: f32,
    pub exposure: f32,
    pub shadows: bool,
    pub env: &'a Environment,
    pub draw_water: bool,
    /// Leave the ground out (Project thumbnails).
    pub no_terrain: bool,
    /// Seconds, for the effects' particles (0 holds them still).
    pub time: f32,
    /// The Scene background from the theme, linear; `None` = the chapter's sky.
    pub sky: Option<[f32; 4]>,
    /// Point lights: `(position, radius m, linear colour x intensity)`.
    pub lights: &'a [(Vec3, f32, [f32; 3])],
    pub view_mode: ViewMode,
}

/// Godot's viewport display modes.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum ViewMode {
    #[default]
    Lit,
    /// Textures only, no light or fog.
    Unshaded,
    /// Light and shadow on grey: what the lighting does.
    Lighting,
    Normals,
    Wireframe,
}

impl ViewMode {
    pub const ALL: [ViewMode; 5] = [ViewMode::Lit, ViewMode::Unshaded, ViewMode::Lighting, ViewMode::Normals, ViewMode::Wireframe];

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Lit => "Lit",
            ViewMode::Unshaded => "Unshaded",
            ViewMode::Lighting => "Lighting only",
            ViewMode::Normals => "Normals",
            ViewMode::Wireframe => "Wireframe",
        }
    }
}

impl Renderer {
    pub fn new(rs: &egui_wgpu::RenderState) -> Renderer {
        let device = Arc::new(rs.device.clone());
        let queue = Arc::new(rs.queue.clone());
        let d = &*device;
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("map shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let globals = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_entry = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let globals_bgl = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[
                uniform_entry,
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        // The shadow pass writes the shadow map, so it binds the uniform alone.
        let shadow_bgl = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow globals"),
            entries: &[uniform_entry],
        });
        let shadow_tex = d.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow map"),
            size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: CASCADES },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_view = shadow_tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let shadow_layers: Vec<wgpu::TextureView> = (0..CASCADES)
            .map(|k| {
                shadow_tex.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: k,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let shadow_sampler = d.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let globals_bg = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&shadow_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&shadow_sampler) },
            ],
        });
        let shadow_bufs: Vec<wgpu::Buffer> = (0..CASCADES)
            .map(|_| {
                d.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("shadow globals"),
                    size: std::mem::size_of::<Globals>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let shadow_bgs: Vec<wgpu::BindGroup> = shadow_bufs
            .iter()
            .map(|b| {
                d.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("shadow globals"),
                    layout: &shadow_bgl,
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource: b.as_entire_binding() }],
                })
            })
            .collect();
        let mat_bgl = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, // effect particles read it in vs_billboard
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let mut terrain_entries: Vec<wgpu::BindGroupLayoutEntry> = (0..12).map(tex_entry).collect();
        for b in [12, 13] {
            terrain_entries.push(wgpu::BindGroupLayoutEntry {
                binding: b,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }
        terrain_entries.push(wgpu::BindGroupLayoutEntry {
            binding: 14,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        });
        let terrain_bgl = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain"),
            entries: &terrain_entries,
        });
        let map_sampler = d.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain maps"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let sampler = d.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("albedo"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let white = {
            let t = d.create_texture_with_data(
                &queue,
                &wgpu::TextureDescriptor {
                    label: Some("white"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: COLOR_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &[255, 255, 255, 255],
            );
            t.create_view(&Default::default())
        };
        let flat_normal = d
            .create_texture_with_data(
                &queue,
                &wgpu::TextureDescriptor {
                    label: Some("flat normal"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &[128, 128, 255, 255],
            )
            .create_view(&Default::default());
        let terrain_u = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain material"),
            size: std::mem::size_of::<TerrainU>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let terrain_layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain"),
            bind_group_layouts: &[Some(&globals_bgl), Some(&terrain_bgl)],
            immediate_size: 0,
        });

        let mesh_layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh"),
            bind_group_layouts: &[Some(&globals_bgl), Some(&mat_bgl)],
            immediate_size: 0,
        });
        let plain_layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("plain"),
            bind_group_layouts: &[Some(&globals_bgl)],
            immediate_size: 0,
        });

        let depth = |write: bool, test: bool| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(if test { wgpu::CompareFunction::GreaterEqual } else { wgpu::CompareFunction::Always }),
            stencil: Default::default(),
            bias: Default::default(),
        };
        let ms = wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false };
        let color = |blend: Option<wgpu::BlendState>| {
            [Some(wgpu::ColorTargetState { format: COLOR_FORMAT, blend, write_mask: wgpu::ColorWrites::ALL })]
        };

        let terrain_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3, 3 => Float32x2];
        let terrain_pipeline = |polygon_mode: wgpu::PolygonMode| d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain"),
            layout: Some(&terrain_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_terrain"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<TerrainVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &terrain_attrs,
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_terrain"),
                compilation_options: Default::default(),
                targets: &color(None),
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, polygon_mode, ..Default::default() },
            depth_stencil: Some(depth(true, true)),
            multisample: ms,
            multiview_mask: None,
            cache: None,
        });
        let terrain_pipe = terrain_pipeline(wgpu::PolygonMode::Fill);

        let water_pipe = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water"),
            layout: Some(&terrain_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_terrain"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<TerrainVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &terrain_attrs,
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_water"),
                compilation_options: Default::default(),
                targets: &color(Some(wgpu::BlendState::ALPHA_BLENDING)),
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(depth(false, true)),
            multisample: ms,
            multiview_mask: None,
            cache: None,
        });
        let vattrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
        let iattrs = wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4];
        let mesh_buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &vattrs,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Instance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &iattrs,
            }),
        ];
        let mesh_pipeline_poly = |label: &str, vs: &str, fs: &str, blend: Option<wgpu::BlendState>, write: bool, polygon_mode: wgpu::PolygonMode| {
            d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&mesh_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &mesh_buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &color(blend),
                }),
                primitive: wgpu::PrimitiveState { cull_mode: None, polygon_mode, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    // Decals sit on the ground: nudge them toward the camera.
                    bias: if write {
                        Default::default()
                    } else {
                        wgpu::DepthBiasState { constant: 4, slope_scale: 2.0, clamp: 0.0 }
                    },
                    ..depth(write, true)
                }),
                multisample: ms,
                multiview_mask: None,
                cache: None,
            })
        };
        let mesh_pipeline_vs = |label: &str, vs: &str, fs: &str, blend: Option<wgpu::BlendState>, write: bool| mesh_pipeline_poly(label, vs, fs, blend, write, wgpu::PolygonMode::Fill);
        // Wireframe view: only where the GPU draws polygons as lines (not GL).
        // One flat colour over the solid pass, depth tested (hidden edges stay
        // hidden) and nudged toward the camera (write = false adds the bias).
        let wire = d.features().contains(wgpu::Features::POLYGON_MODE_LINE).then(|| mesh_pipeline_poly("mesh wire", "vs_mesh", "fs_wire", Some(wgpu::BlendState::ALPHA_BLENDING), false, wgpu::PolygonMode::Line));
        let mesh_pipeline = |label: &str, fs: &str, blend: Option<wgpu::BlendState>, write: bool| mesh_pipeline_vs(label, "vs_mesh", fs, blend, write);
        let mesh_pipe = mesh_pipeline("mesh", "fs_mesh", None, true);
        // Effects glow: added onto what is behind them, never hiding it.
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
        };
        let vfx_pipe = mesh_pipeline_vs("vfx", "vs_billboard", "fs_vfx", Some(additive), false);
        let decal_pipe = mesh_pipeline("decal", "fs_decal", Some(wgpu::BlendState::ALPHA_BLENDING), false);

        let lattrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];
        let line_pipeline = |label: &str, test: bool| {
            d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&plain_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_line"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<LineVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &lattrs,
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_line"),
                    compilation_options: Default::default(),
                    targets: &color(Some(wgpu::BlendState::ALPHA_BLENDING)),
                }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::LineList, ..Default::default() },
                depth_stencil: Some(depth(false, test)),
                multisample: ms,
                multiview_mask: None,
                cache: None,
            })
        };
        let shadow_layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow"),
            bind_group_layouts: &[Some(&shadow_bgl)],
            immediate_size: 0,
        });
        let shadow_depth = wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.5, clamp: 0.0 },
        };
        let shadow_pipeline = |label: &str, vs: &str, buffers: &[Option<wgpu::VertexBufferLayout>]| {
            d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&shadow_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                fragment: None,
                primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                depth_stencil: Some(shadow_depth.clone()),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let pos_only = wgpu::vertex_attr_array![0 => Float32x3];
        let shadow_terrain_pipe = shadow_pipeline(
            "shadow terrain",
            "vs_shadow_terrain",
            &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<TerrainVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &pos_only,
            })],
        );
        let shadow_mesh_pipe = shadow_pipeline("shadow mesh", "vs_shadow_mesh", &mesh_buffers);
        let line_pipe = line_pipeline("lines", true);
        let line_top_pipe = line_pipeline("lines over", false);

        let terrain_bg = Self::terrain_bind(d, &terrain_bgl, &[&white, &white], &[&white; 5], &[&flat_normal; 5], &map_sampler, &sampler, &terrain_u);
        queue.write_buffer(&terrain_u, 0, bytemuck::bytes_of(&TerrainU::default()));
        Renderer {
            water: None,
            water_pipe,
            terrain_u_cpu: TerrainU::default(),
            shadow_bufs,
            shadow_bgs,
            shadow_layers,
            shadow_terrain_pipe,
            shadow_mesh_pipe,
            terrain_bgl,
            terrain_bg,
            terrain_u,
            map_sampler,
            flat_normal,
            look: None,
            look_maps: None,
            device,
            queue,
            globals,
            globals_bg,
            mat_bgl,
            sampler,
            white,
            terrain_pipe,
            mesh_pipe,
            wire,
            decal_pipe,
            vfx_pipe,
            line_pipe,
            line_top_pipe,
            terrain: None,
            meshes: HashMap::new(),
            textures: HashMap::new(),
            mats: HashMap::new(),
            instance_buf: None,
            instance_cap: 0,
            line_buf: None,
            line_cap: 0,
            target: None,
            thumbs: Vec::new(),
            preview: None,
            side: None,
            stats: Stats::default(),
        }
    }

    /// Drop a mesh / texture so the next request loads it again (a file changed).
    pub fn forget(&mut self, key: &str) {
        self.meshes.remove(key);
        self.textures.retain(|(p, _), _| p != key);
    }

    pub fn can_wireframe(&self) -> bool {
        self.wire.is_some()
    }

    pub fn clear_scene(&mut self) {
        self.terrain = None;
        self.mats.clear();
        self.look = None;
        self.water = None;
        self.terrain_u_cpu = TerrainU::default();
        self.queue.write_buffer(&self.terrain_u, 0, bytemuck::bytes_of(&TerrainU::default()));
    }

    #[allow(clippy::too_many_arguments)]
    fn terrain_bind(
        d: &wgpu::Device,
        bgl: &wgpu::BindGroupLayout,
        maps: &[&wgpu::TextureView; 2],
        alb: &[&wgpu::TextureView; 5],
        nrm: &[&wgpu::TextureView; 5],
        map_sampler: &wgpu::Sampler,
        sampler: &wgpu::Sampler,
        u: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        let mut entries: Vec<wgpu::BindGroupEntry> = Vec::new();
        for (i, v) in maps.iter().chain(alb.iter()).chain(nrm.iter()).enumerate() {
            entries.push(wgpu::BindGroupEntry { binding: i as u32, resource: wgpu::BindingResource::TextureView(v) });
        }
        entries.push(wgpu::BindGroupEntry { binding: 12, resource: wgpu::BindingResource::Sampler(map_sampler) });
        entries.push(wgpu::BindGroupEntry { binding: 13, resource: wgpu::BindingResource::Sampler(sampler) });
        entries.push(wgpu::BindGroupEntry { binding: 14, resource: u.as_entire_binding() });
        d.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("terrain"), layout: bgl, entries: &entries })
    }

    fn map_texture(&self, n: usize, data: &[u8], label: &str) -> (wgpu::Texture, wgpu::TextureView) {
        let t = self.device
            .create_texture_with_data(
                &self.queue,
                &wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: n as u32, height: n as u32, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                data,
            );
        let v = t.create_view(&Default::default());
        (t, v)
    }

    /// Rewrite the terrain's weight / colour maps after a paint stroke.
    pub fn update_look_maps(&mut self, weights: &[u8], color: Option<&[u8]>) {
        let Some((wtex, ctex, n)) = &self.look_maps else { return };
        let n = *n as u32;
        let write = |t: &wgpu::Texture, data: &[u8]| {
            if data.len() as u32 != n * n * 4 {
                return;
            }
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * n), rows_per_image: Some(n) },
                wgpu::Extent3d { width: n, height: n, depth_or_array_layers: 1 },
            );
        };
        write(wtex, weights);
        if let Some(data) = color {
            write(ctex, data);
        }
    }

    /// Start drawing the ground with the chapter's own material; it switches
    /// on once every texture it names has loaded (or failed).
    pub fn set_look(&mut self, look: TerrainLook) {
        let (wt, w) = self.map_texture(look.n, &look.weights, "terrain weights");
        let white = vec![255u8; look.n * look.n * 4];
        let (ct, c) = self.map_texture(look.n, look.color.as_deref().unwrap_or(&white), "terrain colour");
        self.look_maps = Some((wt, ct, look.n));
        self.look = Some((look, w, c));
    }

    /// Bind the look once its textures are in. True when it is drawing.
    pub fn finish_look(&mut self) -> bool {
        let Some((look, w, c)) = &self.look else { return false };
        let get = |p: &Option<String>| -> Option<Option<&wgpu::TextureView>> {
            match p {
                None => Some(None),
                Some(p) => self.textures.get(&(p.clone(), false)).map(|t| t.as_ref().map(|t| &t.view)),
            }
        };
        let mut alb: Vec<&wgpu::TextureView> = Vec::new();
        let mut nrm: Vec<&wgpu::TextureView> = Vec::new();
        for s in &look.slots {
            let (Some(a), Some(n)) = (get(&s.albedo), get(&s.normal)) else { return false };
            alb.push(a.unwrap_or(&self.white));
            nrm.push(n.unwrap_or(&self.flat_normal));
        }
        let alb: [&wgpu::TextureView; 5] = alb.try_into().ok().unwrap();
        let nrm: [&wgpu::TextureView; 5] = nrm.try_into().ok().unwrap();
        self.terrain_bg =
            Self::terrain_bind(&self.device, &self.terrain_bgl, &[w, c], &alb, &nrm, &self.map_sampler, &self.sampler, &self.terrain_u);
        let mut u = TerrainU::default();
        for (k, s) in look.slots.iter().enumerate() {
            u.slots[k] = TerrainSlotU {
                color: s.color,
                params: [s.uv_scale, s.height_mul_offset[0], s.height_mul_offset[1], s.smooth],
            };
        }
        u.flags = [1.0, look.color.is_some() as u8 as f32, 0.0, 0.0];
        if let Some(w) = &look.water {
            u.water_deep = w.deep;
            u.water_shallow = w.shallow;
            u.water_banks = w.banks;
        }
        self.terrain_u_cpu = u;
        self.queue.write_buffer(&self.terrain_u, 0, bytemuck::bytes_of(&u));
        self.look = None;
        true
    }

    pub fn set_terrain(&mut self, t: &Terrain) {
        let (v, col, idx) = t.mesh();
        let n = t.n;
        let verts: Vec<TerrainVertex> = v
            .iter()
            .zip(&col)
            .enumerate()
            .map(|(i, (v, c))| TerrainVertex { pos: v.pos, nrm: v.nrm, col: *c, uv: grid_uv(i, n) })
            .collect();
        let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain vb"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain ib"),
            contents: bytemuck::cast_slice(&idx),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.terrain = Some((vb, ib, idx.len() as u32));
        self.update_water(t);
    }

    /// Rebuild only the water surface (a water brush dab).
    pub fn update_water(&mut self, t: &Terrain) {
        let n = t.n;
        let verts: Vec<TerrainVertex> = (0..n * n)
            .map(|i| TerrainVertex { pos: t.vertex_pos(i / n, i % n).into(), nrm: [0.0, 1.0, 0.0], col: [0.0; 3], uv: grid_uv(i, n) })
            .collect();
        // Water: the cells under the painted water surface, at that height.
        // `col.x` carries the depth (water minus ground) for the shader.
        self.water = t.water.as_ref().and_then(|w| {
            let mut wv: Vec<TerrainVertex> = Vec::new();
            for r in 0..n - 1 {
                for c in 0..n - 1 {
                    let ids = [r * n + c, r * n + c + 1, (r + 1) * n + c, (r + 1) * n + c + 1];
                    if !ids.iter().any(|&i| w[i] > t.h[i] + 0.02) {
                        continue;
                    }
                    let vtx = |i: usize| {
                        let p = verts[i].pos;
                        TerrainVertex { pos: [p[0], w[i], p[2]], nrm: [0.0, 1.0, 0.0], col: [w[i] - t.h[i], 0.0, 0.0], uv: verts[i].uv }
                    };
                    for i in [ids[0], ids[1], ids[2], ids[1], ids[3], ids[2]] {
                        wv.push(vtx(i));
                    }
                }
            }
            (!wv.is_empty()).then(|| {
                let b = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("water vb"),
                    contents: bytemuck::cast_slice(&wv),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                (b, wv.len() as u32)
            })
        });
    }

    /// Draw the ground with the chapter's material (true) or plain (false).
    pub fn set_look_enabled(&mut self, on: bool) {
        let mut u = self.terrain_u_cpu;
        if !on {
            u.flags[0] = 0.0;
        }
        self.queue.write_buffer(&self.terrain_u, 0, bytemuck::bytes_of(&u));
    }

    /// Rewrite the terrain rows `r0..=r1` after a height edit.
    pub fn update_terrain_rows(&mut self, t: &Terrain, r0: usize, r1: usize) {
        let Some((vb, _, _)) = &self.terrain else { return };
        let n = t.n;
        let (v, colors) = t.mesh_rows(r0, r1);
        let rows: Vec<TerrainVertex> = v
            .iter()
            .zip(&colors)
            .enumerate()
            .map(|(i, (v, c))| TerrainVertex { pos: v.pos, nrm: v.nrm, col: *c, uv: grid_uv(r0 * n + i, n) })
            .collect();
        let off = (r0 * n * std::mem::size_of::<TerrainVertex>()) as u64;
        self.queue.write_buffer(vb, off, bytemuck::cast_slice(&rows));
    }

    pub fn has_mesh(&self, path: &str) -> bool {
        self.meshes.contains_key(path)
    }

    /// A loaded mesh's bounding sphere, in its own space.
    pub fn mesh_bounds(&self, path: &str) -> Option<(Vec3, f32)> {
        self.meshes.get(path).map(|m| (m.center, m.radius))
    }

    /// Render `f` into a texture of its own (a Project thumbnail) and leave
    /// the main view's target alone.
    pub fn render_thumb(&mut self, rs: &egui_wgpu::RenderState, size: (u32, u32), f: &Frame) -> egui::TextureId {
        let main = self.target.take();
        let id = self.render(rs, size, f);
        if let Some(t) = self.target.take() {
            self.thumbs.push(t);
        }
        self.target = main;
        id
    }

    /// Like `render_thumb`, but one target reused frame after frame.
    pub fn render_preview(&mut self, rs: &egui_wgpu::RenderState, size: (u32, u32), f: &Frame) -> egui::TextureId {
        let main = std::mem::replace(&mut self.target, self.preview.take());
        let id = self.render(rs, size, f);
        self.preview = std::mem::replace(&mut self.target, main);
        id
    }

    /// The split view: like `render_preview`, with a target of its own.
    pub fn render_side(&mut self, rs: &egui_wgpu::RenderState, size: (u32, u32), f: &Frame) -> egui::TextureId {
        let main = std::mem::replace(&mut self.target, self.side.take());
        let id = self.render(rs, size, f);
        self.side = std::mem::replace(&mut self.target, main);
        id
    }

    pub fn add_mesh(&mut self, path: String, m: MeshData) {
        let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh vb"),
            contents: bytemuck::cast_slice(&m.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh ib"),
            contents: bytemuck::cast_slice(&m.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let (center, radius, count) = (m.center, m.radius, m.indices.len() as u32);
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in &m.vertices {
            lo = lo.min(Vec3::from(v.pos));
            hi = hi.max(Vec3::from(v.pos));
        }
        if m.vertices.is_empty() {
            (lo, hi) = (Vec3::ZERO, Vec3::ZERO);
        }
        self.meshes.insert(path, GpuMesh { vb, ib, count, center, radius, lo, hi, cpu: Arc::new(m), bvh: Default::default() });
    }

    pub fn texture_known(&self, path: &str, swap: bool) -> bool {
        self.textures.contains_key(&(path.to_owned(), swap))
    }

    /// Record a texture; `None` marks one that failed (drawn untextured).
    pub fn add_texture(&mut self, path: String, swap: bool, t: Option<TextureData>) {
        let linear = is_linear(&path);
        let gpu = t.map(|t| {
            let (w, h, _) = t.levels[0];
            let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("albedo"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: t.levels.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: if linear { wgpu::TextureFormat::Rgba8Unorm } else { COLOR_FORMAT },
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            for (lvl, (lw, lh, data)) in t.levels.iter().enumerate() {
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &tex,
                        mip_level: lvl as u32,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    data,
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * lw), rows_per_image: Some(*lh) },
                    wgpu::Extent3d { width: *lw, height: *lh, depth_or_array_layers: 1 },
                );
            }
            GpuTexture { view: tex.create_view(&Default::default()), has_alpha: t.has_alpha }
        });
        self.textures.insert((path, swap), gpu);
    }

    /// The bind group for a part, once its texture (if any) is settled.
    fn material(&mut self, key: &PartKey) -> bool {
        if self.mats.contains_key(key) {
            return true;
        }
        let tex = match &key.texture {
            Some(p) => match self.textures.get(&(p.clone(), key.swap_rb)) {
                None => return false, // still loading
                Some(t) => t.as_ref(),
            },
            None => None,
        };
        // A normal map that failed to load just leaves the part smooth.
        let nrm = match &key.normal {
            Some(p) => match self.textures.get(&(p.clone(), key.normal_swap)) {
                None => return false,
                Some(t) => t.as_ref(),
            },
            None => None,
        };
        let color = key
            .color
            .as_deref()
            .and_then(hex_linear)
            .map(|c| [c[0], c[1], c[2], 1.0])
            .unwrap_or(if tex.is_some() { [1.0; 4] } else { [0.55, 0.55, 0.52, 1.0] });
        let u = MaterialU {
            color,
            flags: [tex.is_some() as u8 as f32, tex.map(|t| t.has_alpha && !key.decal && !key.vfx).unwrap_or(false) as u8 as f32, nrm.is_some() as u8 as f32, 0.0],
            fx: if key.vfx { vfx_params(&key.mesh) } else { [[0.0; 4]; 4] },
        };
        let ub = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("material"),
            contents: bytemuck::bytes_of(&u),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let view = tex.map(|t| &t.view).unwrap_or(&self.white);
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material"),
            layout: &self.mat_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: ub.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(nrm.map(|t| &t.view).unwrap_or(&self.flat_normal)) },
            ],
        });
        self.mats.insert(key.clone(), PartMat { bind });
        true
    }

    fn ensure_target(&mut self, rs: &egui_wgpu::RenderState, size: (u32, u32)) -> egui::TextureId {
        let size = (size.0.max(1), size.1.max(1));
        if let Some(t) = &self.target {
            if t.size == size {
                return t.egui_id;
            }
        }
        let d = &self.device;
        let ext = wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 };
        let tex = |label, format, samples, usage| {
            d.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: ext,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let msaa = tex("msaa", COLOR_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT).create_view(&Default::default());
        let resolve = tex(
            "resolve",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        let resolve_view = resolve.create_view(&Default::default());
        let depth = tex("depth", DEPTH_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT).create_view(&Default::default());
        let mut r = rs.renderer.write();
        let egui_id = match &self.target {
            Some(old) => {
                r.update_egui_texture_from_wgpu_texture(d, &resolve_view, wgpu::FilterMode::Linear, old.egui_id);
                old.egui_id
            }
            None => r.register_native_texture(d, &resolve_view, wgpu::FilterMode::Linear),
        };
        self.target = Some(Target { size, msaa, resolve, resolve_view, depth, egui_id });
        egui_id
    }

    /// Draw a frame into the offscreen target and return the egui texture showing it.
    pub fn render(&mut self, rs: &egui_wgpu::RenderState, size: (u32, u32), f: &Frame) -> egui::TextureId {
        let id = self.ensure_target(rs, size);
        let (w, h) = self.target.as_ref().unwrap().size;
        let aspect = w as f32 / h as f32;
        let cam = f.camera;
        let view = cam.view();
        let vp = cam.proj(aspect) * view;
        let env = f.env;
        let sun = env.sun_dir();
        // The sun's view: an orthographic box around what the camera looks at,
        // its centre snapped to whole shadow texels so edges don't crawl. The
        // far cascade covers the view; the near one a third of it, 3x sharper.
        let shadow_r = (cam.dist * 1.1).clamp(30.0, 700.0);
        let near_r = (cam.dist * 0.35).clamp(10.0, shadow_r);
        let (lf, lr) = (-sun, Vec3::Y.cross(-sun).normalize_or(Vec3::X));
        let lu = lf.cross(lr);
        let c = cam.target;
        let sun_box = |r: f32| -> (Mat4, Mat4) {
            let texel = 2.0 * r / SHADOW_SIZE as f32;
            let snap = |v: f32| (v / texel).round() * texel;
            let cl = Vec3::new(snap(c.dot(lr)), snap(c.dot(lu)), c.dot(lf));
            let (near, far) = (-2000.0f32, 2000.0f32);
            let view = Mat4::from_cols(
                Vec4::new(lr.x, lu.x, lf.x, 0.0),
                Vec4::new(lr.y, lu.y, lf.y, 0.0),
                Vec4::new(lr.z, lu.z, lf.z, 0.0),
                Vec4::new(-cl.x, -cl.y, -cl.z, 1.0),
            );
            let proj = Mat4::from_cols(
                Vec4::new(1.0 / r, 0.0, 0.0, 0.0),
                Vec4::new(0.0, 1.0 / r, 0.0, 0.0),
                Vec4::new(0.0, 0.0, 1.0 / (far - near), 0.0),
                Vec4::new(0.0, 0.0, -near / (far - near), 1.0),
            );
            (view, proj * view)
        };
        let (light_view, light_vp_far) = sun_box(shadow_r);
        let light_vp = sun_box(near_r).1;
        let g = Globals {
            view_proj: vp.to_cols_array_2d(),
            eye: cam.eye().extend(1.0).into(),
            sun_dir: sun.extend(0.0).into(),
            params: [f.haze, f.exposure, f.shadows as u8 as f32, f.time],
            light_vp: light_vp.to_cols_array_2d(),
            light_vp_far: light_vp_far.to_cols_array_2d(),
            sun_color: [env.sun_color[0] * env.sun_intensity, env.sun_color[1] * env.sun_intensity, env.sun_color[2] * env.sun_intensity, 0.0],
            amb_top: [env.amb_top[0] * env.amb_mul, env.amb_top[1] * env.amb_mul, env.amb_top[2] * env.amb_mul, env.amb_pow],
            amb_horizon: [env.amb_horizon[0] * env.amb_mul, env.amb_horizon[1] * env.amb_mul, env.amb_horizon[2] * env.amb_mul, 0.0],
            amb_bottom: [env.amb_bottom[0] * env.amb_mul, env.amb_bottom[1] * env.amb_mul, env.amb_bottom[2] * env.amb_mul, 0.0],
            fog_bottom: env.fog_bottom,
            fog_mid: env.fog_mid,
            fog_top: env.fog_top,
            fog_heights: [env.fog_heights[0], env.fog_heights[1], env.fog_heights[2], env.fog_on as u8 as f32],
            fog_dist: [env.fog_distances[0], env.fog_distances[1], env.fog_distances[2], (cam.eye() - cam.target).length()],
            fog_params: [env.fog_intensity, env.fog_brightness, 0.0, 0.0],
            light_info: [f.lights.len().min(MAX_LIGHTS) as f32, f.view_mode as u8 as f32, 0.0, 0.0],
            lights: {
                let mut l = [[0.0; 4]; 2 * MAX_LIGHTS];
                for (k, (p, r, c)) in f.lights.iter().take(MAX_LIGHTS).enumerate() {
                    l[2 * k] = [p.x, p.y, p.z, *r];
                    l[2 * k + 1] = [c[0], c[1], c[2], 0.0];
                }
                l
            },
        };
        self.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));
        // Each cascade's pass reads its own sun view through `light_vp`.
        for (k, vp) in [light_vp, light_vp_far].iter().enumerate() {
            let gk = Globals { light_vp: vp.to_cols_array_2d(), ..g };
            self.queue.write_buffer(&self.shadow_bufs[k], 0, bytemuck::bytes_of(&gk));
        }

        // ---- cull ------------------------------------------------------------
        let th = (cam.fov_y * 0.5).tan();
        let (ah, av) = ((th * aspect).atan(), cam.fov_y * 0.5);
        let (sh, ch) = ah.sin_cos();
        let (sv, cv) = av.sin_cos();
        let px_per_unit = h as f32 * 0.5 / th;
        let mut visible: Vec<Instance> = Vec::new();
        let mut draws: Vec<(usize, u32, u32, u8)> = Vec::new(); // part, first, count, pass (0 mesh, 1 decal, 2 vfx)
        let mut stats = Stats::default();
        for (pi, p) in f.parts.iter().enumerate() {
            let Some(mesh) = self.meshes.get(&p.key.mesh) else { continue };
            let (c, r) = (mesh.center, mesh.radius);
            let first = visible.len() as u32;
            for inst in p.instances {
                let m = Mat4::from_cols_array_2d(&inst.m);
                let wc = m.transform_point3(c);
                let scale = m.x_axis.truncate().length().max(m.y_axis.truncate().length()).max(m.z_axis.truncate().length());
                let wr = r * scale;
                let vc: Vec4 = view * wc.extend(1.0);
                let (x, y, z) = (vc.x, vc.y, vc.z);
                if cam.ortho {
                    // A box: half extents at any depth.
                    let hh = cam.half_height();
                    if x.abs() - wr > hh * aspect || y.abs() - wr > hh || z < crate::camera::NEAR - wr {
                        continue;
                    }
                    if inst.tint[3] == 0.0 && wr * (h as f32 * 0.5 / hh) < f.min_px {
                        continue;
                    }
                } else {
                    if z < crate::camera::NEAR - wr
                        || x * ch - z * sh > wr
                        || -x * ch - z * sh > wr
                        || y * cv - z * sv > wr
                        || -y * cv - z * sv > wr
                    {
                        continue;
                    }
                    if inst.tint[3] == 0.0 && wr * px_per_unit / z.max(0.1) < f.min_px {
                        continue;
                    }
                }
                visible.push(*inst);
            }
            let count = visible.len() as u32 - first;
            if count > 0 {
                draws.push((pi, first, count, if p.key.vfx { 2 } else { p.key.decal as u8 }));
                stats.drawn_instances += count as usize;
                if p.key.vfx {
                    stats.vfx += count as usize;
                }
                stats.drawn_triangles += count as u64 * (mesh.count / 3) as u64;
            }
        }
        // Shadow casters: everything (but decals) inside the sun's box,
        // including what is off screen but throws a shadow into it.
        let mut casters: Vec<(usize, u32, u32)> = Vec::new();
        if f.shadows {
            for (pi, p) in f.parts.iter().enumerate() {
                if p.key.decal || p.key.vfx {
                    continue;
                }
                let Some(mesh) = self.meshes.get(&p.key.mesh) else { continue };
                let first = visible.len() as u32;
                for inst in p.instances {
                    let m = Mat4::from_cols_array_2d(&inst.m);
                    let wc = m.transform_point3(mesh.center);
                    let scale = m.x_axis.truncate().length().max(m.y_axis.truncate().length()).max(m.z_axis.truncate().length());
                    let wr = mesh.radius * scale;
                    let lc = light_view.transform_point3(wc);
                    if lc.x.abs() - wr > shadow_r || lc.y.abs() - wr > shadow_r || wr < shadow_r * 0.004 {
                        continue;
                    }
                    visible.push(*inst);
                }
                let count = visible.len() as u32 - first;
                if count > 0 {
                    casters.push((pi, first, count));
                }
            }
        }
        // Materials need &mut self; settle them before encoding.
        draws.retain(|&(pi, ..)| self.material(f.parts[pi].key));
        stats.draws = draws.len();
        self.stats = stats;

        let bytes: &[u8] = bytemuck::cast_slice(&visible);
        if !bytes.is_empty() {
            if self.instance_cap < bytes.len() as u64 {
                self.instance_cap = (bytes.len() as u64).next_power_of_two();
                self.instance_buf = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("instances"),
                    size: self.instance_cap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            self.queue.write_buffer(self.instance_buf.as_ref().unwrap(), 0, bytes);
        }
        let line_bytes: Vec<u8> =
            [bytemuck::cast_slice::<_, u8>(f.lines), bytemuck::cast_slice::<_, u8>(f.top_lines)].concat();
        if !line_bytes.is_empty() {
            if self.line_cap < line_bytes.len() as u64 {
                self.line_cap = (line_bytes.len() as u64).next_power_of_two();
                self.line_buf = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("lines"),
                    size: self.line_cap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            self.queue.write_buffer(self.line_buf.as_ref().unwrap(), 0, &line_bytes);
        }

        // ---- encode ----------------------------------------------------------
        let t = self.target.as_ref().unwrap();
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("map frame") });
        for k in 0..CASCADES as usize {
            let mut sp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_layers[k],
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if f.shadows {
                sp.set_bind_group(0, &self.shadow_bgs[k], &[]);
                if let (Some((vb, ib, n)), false) = (&self.terrain, f.no_terrain) {
                    sp.set_pipeline(&self.shadow_terrain_pipe);
                    sp.set_vertex_buffer(0, vb.slice(..));
                    sp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    sp.draw_indexed(0..*n, 0, 0..1);
                }
                if let Some(ibuf) = &self.instance_buf {
                    sp.set_pipeline(&self.shadow_mesh_pipe);
                    sp.set_vertex_buffer(1, ibuf.slice(..));
                    for &(pi, first, count) in &casters {
                        let mesh = &self.meshes[&f.parts[pi].key.mesh];
                        sp.set_vertex_buffer(0, mesh.vb.slice(..));
                        sp.set_index_buffer(mesh.ib.slice(..), wgpu::IndexFormat::Uint32);
                        sp.draw_indexed(0..mesh.count, 0, first..first + count);
                    }
                }
            }
        }
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("map"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.msaa,
                    depth_slice: None,
                    resolve_target: Some(&t.resolve_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear({
                    // The sky behind the map: the fog's top colour, else a dusk grey.
                    let c = f.sky.unwrap_or(if f.env.fog_on { f.env.fog_top } else { [0.21, 0.26, 0.31, 1.0] });
                    wgpu::Color { r: c[0] as f64, g: c[1] as f64, b: c[2] as f64, a: 1.0 }
                }),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_bind_group(0, &self.globals_bg, &[]);
            let wire = self.wire.as_ref().filter(|_| f.view_mode == ViewMode::Wireframe);
            if let (Some((vb, ib, n)), false) = (&self.terrain, f.no_terrain) {
                rp.set_bind_group(1, &self.terrain_bg, &[]);
                rp.set_vertex_buffer(0, vb.slice(..));
                rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                rp.set_pipeline(&self.terrain_pipe);
                rp.draw_indexed(0..*n, 0, 0..1);
            }
            if let Some(ibuf) = &self.instance_buf {
                // Pass 3: the wireframe overlay on the solid meshes (kind 0).
                for pass in [0u8, 3, 1, 2] {
                    rp.set_pipeline(match pass {
                        0 => &self.mesh_pipe,
                        3 => match wire {
                            Some(m) => m,
                            None => continue,
                        },
                        1 => &self.decal_pipe,
                        _ => &self.vfx_pipe,
                    });
                    rp.set_vertex_buffer(1, ibuf.slice(..));
                    for &(pi, first, count, kind) in &draws {
                        if kind != pass % 3 {
                            continue;
                        }
                        let key = f.parts[pi].key;
                        let mesh = &self.meshes[&key.mesh];
                        rp.set_bind_group(1, &self.mats[key].bind, &[]);
                        rp.set_vertex_buffer(0, mesh.vb.slice(..));
                        rp.set_index_buffer(mesh.ib.slice(..), wgpu::IndexFormat::Uint32);
                        rp.draw_indexed(0..mesh.count, 0, first..first + count);
                    }
                }
            }
            if let (Some((wb, n)), true) = (&self.water, f.draw_water) {
                rp.set_pipeline(&self.water_pipe);
                rp.set_bind_group(1, &self.terrain_bg, &[]);
                rp.set_vertex_buffer(0, wb.slice(..));
                rp.draw(0..*n, 0..1);
            }
            if let Some(lb) = &self.line_buf {
                let n_depth = f.lines.len() as u32;
                let n_top = f.top_lines.len() as u32;
                rp.set_vertex_buffer(0, lb.slice(..));
                if n_depth > 0 {
                    rp.set_pipeline(&self.line_pipe);
                    rp.draw(0..n_depth, 0..1);
                }
                if n_top > 0 {
                    rp.set_pipeline(&self.line_top_pipe);
                    rp.draw(n_depth..n_depth + n_top, 0..1);
                }
            }
        }
        self.queue.submit([enc.finish()]);
        id
    }

    /// Read the last rendered frame back (for `--snapshot`).
    pub fn capture(&self) -> Option<image::RgbaImage> {
        let t = self.target.as_ref()?;
        let (w, h) = t.size;
        let row = (4 * w).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture"),
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &t.resolve, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let data = slice.get_mapped_range().ok()?;
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let start = (y * row) as usize;
            px.extend_from_slice(&data[start..start + (w * 4) as usize]);
        }
        image::RgbaImage::from_raw(w, h, px)
    }
}

fn grid_uv(i: usize, n: usize) -> [f32; 2] {
    let (r, c) = (i / n, i % n);
    [(c as f32 + 0.5) / n as f32, (r as f32 + 0.5) / n as f32]
}

/// Normal and MRA maps hold data, not colour: sample them without sRGB decode.
fn is_linear(path: &str) -> bool {
    let p = path.to_ascii_uppercase();
    p.contains("_NRM") || p.contains("_MRA") || p.contains("_MSK")
}

#[cfg(test)]
mod tests {
    #[test]
    fn shader_validates() {
        let m = naga::front::wgsl::parse_str(include_str!("shader.wgsl")).expect("shader.wgsl parses");
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&m).expect("shader.wgsl validates");
    }
}
