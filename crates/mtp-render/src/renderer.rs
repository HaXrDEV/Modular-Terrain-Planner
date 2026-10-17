//! Meshes on the GPU, offscreen targets and the passes of a frame:
//! ground, battle map and grid; instanced tiles; ghosts; the selection mask;
//! and a composite that draws the outline. The result is a plain RGBA
//! texture the app shows in its UI, or reads back for tests and thumbnails.

use std::collections::HashMap;
use std::time::Instant;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};
use mtp_core::tile::TileKey;
use wgpu::util::DeviceExt;

use crate::scene::{self, DrawGroup, DrawList, DrawTile, Instance, MeshRef, TierSource};

const SAMPLES: u32 = 4;
/// Colours are shaded and stored as display (sRGB-encoded) values, like the
/// Python app, so the texture is plain unorm.
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const MASK: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    grid: [f32; 4],
    ground_color: [f32; 4],
    line_color: [f32; 4],
    image_rect: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CompositeParams {
    outline: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 3],
    normal: [f32; 3],
}

/// Colours of the scene for a theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub background: [f32; 3],
    pub ground: [f32; 3],
    pub lines: [f32; 3],
    pub outline: [f32; 4],
}

impl Palette {
    pub const DARK: Self = Self {
        background: [0.122, 0.122, 0.13],
        ground: [0.22, 0.22, 0.25],
        lines: [0.52, 0.52, 0.58],
        outline: [0.2, 0.62, 1.0, 1.0],
    };
    pub const LIGHT: Self = Self {
        background: [0.953, 0.953, 0.953],
        ground: [0.84, 0.84, 0.86],
        lines: [0.5, 0.5, 0.55],
        outline: [0.0, 0.42, 0.85, 1.0],
    };
}

/// Everything about a frame except the GPU resources.
pub struct Frame<'a> {
    pub view_proj: Mat4,
    pub grid: [u32; 2],
    pub tiles: &'a [DrawTile],
    pub ghosts: &'a [DrawTile],
    pub palette: Palette,
    /// Physical pixels per UI point, so outlines and hatching scale.
    pub pixels_per_point: f32,
    pub show_ground: bool,
    pub show_grid: bool,
    /// Battle map rectangle in cells, if one is set and loaded.
    pub image_rect: Option<[f32; 4]>,
    pub lod_bias: f32,
}

#[derive(Default, Clone, Debug)]
pub struct FrameStats {
    pub draw_calls: u32,
    pub triangles: u64,
    pub instances: u32,
    pub cull_ms: f32,
    pub encode_ms: f32,
    /// Tiers the app should upload for a sharper next frame.
    pub wanted: Vec<(TileKey, usize)>,
}

/// A view's offscreen textures: the viewport and each thumbnail own one.
pub struct Target {
    pub size: [u32; 2],
    msaa: wgpu::TextureView,
    _resolved: wgpu::TextureView,
    depth: wgpu::TextureView,
    mask: wgpu::TextureView,
    pub output: wgpu::Texture,
    pub output_view: wgpu::TextureView,
    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    params: wgpu::Buffer,
    composite_bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    instance_capacity: u64,
}

struct Tiers {
    triangles: Vec<u32>,
    gpu: Vec<Option<(wgpu::Buffer, u32)>>,
    /// Tiers too big for one vertex buffer on this device.
    too_big: Vec<bool>,
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    meshes: HashMap<TileKey, Tiers>,
    placeholder: (wgpu::Buffer, u32),
    tile_pipeline: wgpu::RenderPipeline,
    ghost_depth_pipeline: wgpu::RenderPipeline,
    ghost_pipeline: wgpu::RenderPipeline,
    mask_pipeline: wgpu::RenderPipeline,
    ground_pipeline: wgpu::RenderPipeline,
    image_pipeline: wgpu::RenderPipeline,
    grid_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    globals_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    image_layout: wgpu::BindGroupLayout,
    image_bind: wgpu::BindGroup,
    image_sampler: wgpu::Sampler,
    gpu_bytes: u64,
}

impl TierSource for Renderer {
    fn tiers(&self, key: &TileKey) -> Option<&[u32]> {
        self.meshes.get(key).map(|t| t.triangles.as_slice())
    }

    fn resident(&self, key: &TileKey, tier: usize) -> bool {
        self.meshes.get(key).and_then(|t| t.gpu.get(tier)).is_some_and(Option::is_some)
    }

    fn usable(&self, key: &TileKey, tier: usize) -> bool {
        self.meshes.get(key).and_then(|t| t.too_big.get(tier)).is_some_and(|big| !big)
    }
}

/// Notes tiers that are too big for the GPU on stderr (the renderer has no
/// logging of its own); the tile is drawn coarser, or as a box.
fn warn_too_big(key: &TileKey, triangles: &[u32], too_big: &[bool]) {
    let skipped: Vec<u32> = triangles.iter().zip(too_big).filter(|(_, b)| **b).map(|(t, _)| *t).collect();
    eprintln!("{key}: tiers of {skipped:?} triangles are too big for one GPU buffer and are skipped");
}

/// Unit cube, flat shaded: stands in for tiles whose STL isn't loaded.
fn unit_cube() -> Vec<Vertex> {
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        ([0.0, 0.0, 1.0], [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0]]),
        ([0.0, 0.0, -1.0], [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [1.0, 0.0, 0.0]]),
        ([0.0, -1.0, 0.0], [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]]),
        ([0.0, 1.0, 0.0], [[1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 1.0], [1.0, 1.0, 1.0]]),
        ([-1.0, 0.0, 0.0], [[0.0, 1.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 1.0]]),
        ([1.0, 0.0, 0.0], [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [1.0, 1.0, 1.0], [1.0, 0.0, 1.0]]),
    ];
    let mut out = Vec::with_capacity(36);
    for (n, q) in faces {
        for i in [0, 1, 2, 0, 2, 3] {
            out.push(Vertex { pos: q[i], normal: n });
        }
    }
    out
}

/// Expands an indexed mesh into flat-shaded triangles (STL tiles look right
/// with a normal per face, and STL normals can't be trusted).
fn flat_vertices(positions: &[[f32; 3]], indices: &[u32]) -> Vec<Vertex> {
    let mut out = Vec::with_capacity(indices.len());
    for tri in indices.as_chunks::<3>().0 {
        let [a, b, c] = tri.map(|i| Vec3::from(positions[i as usize]));
        let n = (b - a).cross(c - a).normalize_or_zero().to_array();
        for p in [a, b, c] {
            out.push(Vertex { pos: p.to_array(), normal: n });
        }
    }
    out
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let scene_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/scene.wgsl").into()),
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/composite.wgsl").into()),
        });
        let uniform = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture = |binding, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("battle map"),
            entries: &[
                texture(0, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite"),
            entries: &[texture(0, false), texture(1, false), uniform(2, wgpu::ShaderStages::FRAGMENT)],
        });
        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene"),
            bind_group_layouts: &[Some(&globals_layout), Some(&image_layout)],
            immediate_size: 0,
        });
        let composite_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("composite"),
            bind_group_layouts: &[Some(&composite_layout)],
            immediate_size: 0,
        });

        let vertex_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        let instance_attrs =
            wgpu::vertex_attr_array![2 => Float32x3, 3 => Float32, 4 => Float32x3, 5 => Float32, 6 => Float32x4];
        let buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &vertex_attrs,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<Instance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &instance_attrs,
            }),
        ];
        let depth = |write: bool, compare, bias| {
            Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(write),
                depth_compare: Some(compare),
                stencil: wgpu::StencilState::default(),
                bias,
            })
        };
        let no_bias = wgpu::DepthBiasState::default();
        let msaa = wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false };
        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);

        let pipeline = |label: &str,
                        vs: &str,
                        fs: &str,
                        vertex_buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
                        format,
                        blend,
                        writes,
                        depth_state,
                        multisample| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&scene_layout),
                vertex: wgpu::VertexState {
                    module: &scene_shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: vertex_buffers,
                },
                primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                depth_stencil: depth_state,
                multisample,
                fragment: Some(wgpu::FragmentState {
                    module: &scene_shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: writes })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        use wgpu::ColorWrites;
        use wgpu::CompareFunction::{Always, LessEqual};
        let all = ColorWrites::ALL;
        let tile_pipeline = pipeline(
            "tiles",
            "vs_tile",
            "fs_tile",
            &buffers,
            COLOR_FORMAT,
            None,
            all,
            depth(true, LessEqual, no_bias),
            msaa,
        );
        // Ghosts and faded levels are see-through, so they take two passes:
        // depth first, then colour only where each is nearest. Every pixel
        // shows one ghost surface, whatever order they come in (before,
        // overlaps changed colour as the camera moved). The bias keeps them
        // behind a tile in the same place.
        let ghost_bias = wgpu::DepthBiasState { constant: 4, slope_scale: 1.5, clamp: 0.0 };
        let ghost_depth_pipeline = pipeline(
            "ghost depth",
            "vs_tile",
            "fs_tile",
            &buffers,
            COLOR_FORMAT,
            None,
            ColorWrites::empty(),
            depth(true, LessEqual, ghost_bias),
            msaa,
        );
        let ghost_pipeline = pipeline(
            "ghosts",
            "vs_tile",
            "fs_tile",
            &buffers,
            COLOR_FORMAT,
            blend,
            all,
            depth(false, LessEqual, ghost_bias),
            msaa,
        );
        let mask_pipeline = pipeline(
            "selection",
            "vs_tile",
            "fs_mask",
            &buffers,
            MASK,
            None,
            all,
            None,
            wgpu::MultisampleState::default(),
        );
        // The ground layers are drawn in order with depth writes off, so they
        // never fight each other at a distance (the Python version needed a
        // polygon offset for that)
        let ground = depth(false, Always, no_bias);
        let ground_pipeline =
            pipeline("ground", "vs_ground", "fs_ground", &[], COLOR_FORMAT, None, all, ground.clone(), msaa);
        let image_pipeline =
            pipeline("battle map", "vs_image", "fs_image", &[], COLOR_FORMAT, blend, all, ground.clone(), msaa);
        let grid_pipeline = pipeline("grid", "vs_ground", "fs_grid", &[], COLOR_FORMAT, blend, all, ground, msaa);
        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite"),
            layout: Some(&composite_pl),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some("fs_composite"),
                compilation_options: Default::default(),
                targets: &[Some(COLOR_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });

        let cube = unit_cube();
        let placeholder = (
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("placeholder"),
                contents: bytemuck::cast_slice(&cube),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            cube.len() as u32,
        );
        let image_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("battle map"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let empty = Self::image_texture(&device, &queue, 1, 1, &[0, 0, 0, 0]);
        let image_bind = Self::image_bind_group(&device, &image_layout, &empty, &image_sampler);

        Self {
            device,
            queue,
            meshes: HashMap::new(),
            placeholder,
            tile_pipeline,
            ghost_depth_pipeline,
            ghost_pipeline,
            mask_pipeline,
            ground_pipeline,
            image_pipeline,
            grid_pipeline,
            composite_pipeline,
            globals_layout,
            composite_layout,
            image_layout,
            image_bind,
            image_sampler,
            gpu_bytes: 0,
        }
    }

    /// Tells the renderer a tile's tiers (triangle counts, finest first), so
    /// it can choose between them; nothing is uploaded yet.
    pub fn register_mesh(&mut self, key: TileKey, triangles: Vec<u32>) {
        let n = triangles.len();
        self.release_mesh(&key);
        // Each triangle becomes three flat-shaded vertices; a tier past the
        // device's buffer limit would make wgpu abort, so it is never used
        let limit = self.device.limits().max_buffer_size;
        let too_big: Vec<bool> =
            triangles.iter().map(|&t| u64::from(t) * 3 * size_of::<Vertex>() as u64 > limit).collect();
        if too_big.iter().any(|b| *b) {
            warn_too_big(&key, &triangles, &too_big);
        }
        self.meshes.insert(key, Tiers { triangles, gpu: (0..n).map(|_| None).collect(), too_big });
    }

    pub fn is_registered(&self, key: &TileKey) -> bool {
        self.meshes.contains_key(key)
    }

    /// Uploads one tier of a registered mesh.
    pub fn upload_tier(&mut self, key: &TileKey, tier: usize, positions: &[[f32; 3]], indices: &[u32]) {
        let Some(entry) = self.meshes.get_mut(key) else { return };
        if entry.too_big.get(tier).is_none_or(|big| *big) {
            return;
        }
        let Some(slot) = entry.gpu.get_mut(tier) else { return };
        let vertices = flat_vertices(positions, indices);
        let bytes = (vertices.len() * size_of::<Vertex>()) as u64;
        let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(key.stem()),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        if let Some((old, count)) = slot.replace((buffer, vertices.len() as u32)) {
            self.gpu_bytes -= u64::from(count) * size_of::<Vertex>() as u64;
            old.destroy();
        }
        self.gpu_bytes += bytes;
    }

    /// Frees a mesh's GPU memory and forgets its tiers.
    pub fn release_mesh(&mut self, key: &TileKey) {
        if let Some(entry) = self.meshes.remove(key) {
            for (buffer, count) in entry.gpu.into_iter().flatten() {
                self.gpu_bytes -= u64::from(count) * size_of::<Vertex>() as u64;
                buffer.destroy();
            }
        }
    }

    /// Keys of meshes on the GPU, for evicting those no longer placed.
    pub fn registered(&self) -> impl Iterator<Item = &TileKey> {
        self.meshes.keys()
    }

    /// Bytes of vertex data on the GPU.
    pub fn gpu_bytes(&self) -> u64 {
        self.gpu_bytes
    }

    fn image_texture(device: &wgpu::Device, queue: &wgpu::Queue, w: u32, h: u32, rgba: &[u8]) -> wgpu::Texture {
        let mips = mip_chain(w, h, rgba);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("battle map"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: mips.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, (mw, mh, data)) in mips.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(mw * 4), rows_per_image: Some(*mh) },
                wgpu::Extent3d { width: *mw, height: *mh, depth_or_array_layers: 1 },
            );
        }
        texture
    }

    fn image_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        texture: &wgpu::Texture,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("battle map"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        })
    }

    /// Largest battle map side the GPU takes; the app scales images down to it.
    pub fn max_texture_side(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    /// Sets the battle map image (RGBA, top row first), or clears it.
    pub fn set_ground_image(&mut self, image: Option<(u32, u32, &[u8])>) {
        let texture = match image {
            Some((w, h, rgba)) if w > 0 && h > 0 && rgba.len() == (w * h * 4) as usize => {
                Self::image_texture(&self.device, &self.queue, w, h, rgba)
            }
            _ => Self::image_texture(&self.device, &self.queue, 1, 1, &[0, 0, 0, 0]),
        };
        self.image_bind = Self::image_bind_group(&self.device, &self.image_layout, &texture, &self.image_sampler);
    }

    pub fn create_target(&self, size: [u32; 2]) -> Target {
        let d = &self.device;
        let size = [size[0].max(1), size[1].max(1)];
        let tex = |label, format, samples, usage| {
            d.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let attach = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let sampled = attach | wgpu::TextureUsages::TEXTURE_BINDING;
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let msaa = view(&tex("msaa", COLOR_FORMAT, SAMPLES, attach));
        let resolved = view(&tex("resolved", COLOR_FORMAT, 1, sampled));
        let depth = view(&tex("depth", DEPTH, SAMPLES, attach));
        let mask = view(&tex("mask", MASK, 1, sampled));
        let output = tex("output", COLOR_FORMAT, 1, sampled | wgpu::TextureUsages::COPY_SRC);
        let output_view = view(&output);
        let buffer = |label, size| {
            d.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let globals = buffer("globals", size_of::<Globals>() as u64);
        let globals_bind = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &self.globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let params = buffer("composite", size_of::<CompositeParams>() as u64);
        let composite_bind = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite"),
            layout: &self.composite_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&resolved) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&mask) },
                wgpu::BindGroupEntry { binding: 2, resource: params.as_entire_binding() },
            ],
        });
        let instance_capacity = 256;
        Target {
            size,
            msaa,
            _resolved: resolved,
            depth,
            mask,
            output,
            output_view,
            globals,
            globals_bind,
            params,
            composite_bind,
            instances: Self::instance_buffer(d, instance_capacity),
            instance_capacity,
        }
    }

    fn instance_buffer(d: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
        d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: capacity * size_of::<Instance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Culls, chooses detail, then encodes and submits a frame into
    /// `target.output`.
    pub fn render(&self, target: &mut Target, frame: &Frame<'_>) -> FrameStats {
        let t0 = Instant::now();
        let viewport = Vec2::new(target.size[0] as f32, target.size[1] as f32);
        let list = scene::build(frame.view_proj, viewport, frame.tiles, frame.ghosts, self, frame.lod_bias);
        let cull_ms = t0.elapsed().as_secs_f32() * 1000.0;
        let mut stats = self.encode(target, frame, &list);
        stats.cull_ms = cull_ms;
        stats.wanted = list.wanted;
        stats
    }

    fn encode(&self, target: &mut Target, frame: &Frame<'_>, list: &DrawList) -> FrameStats {
        let t0 = Instant::now();
        let needed = list.instances.len() as u64;
        if needed > target.instance_capacity {
            target.instance_capacity = needed.next_power_of_two();
            target.instances = Self::instance_buffer(&self.device, target.instance_capacity);
        }
        if !list.instances.is_empty() {
            self.queue.write_buffer(&target.instances, 0, bytemuck::cast_slice(&list.instances));
        }
        let p = frame.palette;
        let rgba = |c: [f32; 3]| [c[0], c[1], c[2], 1.0];
        let globals = Globals {
            view_proj: frame.view_proj.to_cols_array_2d(),
            grid: [
                frame.grid[0] as f32,
                frame.grid[1] as f32,
                if frame.show_grid { 1.0 } else { 0.0 },
                frame.pixels_per_point,
            ],
            ground_color: rgba(p.ground),
            line_color: rgba(p.lines),
            image_rect: frame.image_rect.unwrap_or_default(),
        };
        self.queue.write_buffer(&target.globals, 0, bytemuck::bytes_of(&globals));
        let params =
            CompositeParams { outline: p.outline, params: [(2.0 * frame.pixels_per_point).round(), 0.0, 0.0, 0.0] };
        self.queue.write_buffer(&target.params, 0, bytemuck::bytes_of(&params));

        let mut stats = FrameStats { instances: list.instances.len() as u32, ..Default::default() };
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let [r, g, b] = p.background.map(f64::from);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.msaa,
                    depth_slice: None,
                    resolve_target: Some(&target._resolved),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &target.globals_bind, &[]);
            pass.set_bind_group(1, &self.image_bind, &[]);
            if frame.show_ground {
                pass.set_pipeline(&self.ground_pipeline);
                pass.draw(0..6, 0..1);
                stats.draw_calls += 1;
            }
            if frame.image_rect.is_some() {
                pass.set_pipeline(&self.image_pipeline);
                pass.draw(0..6, 0..1);
                stats.draw_calls += 1;
            }
            if frame.show_grid {
                pass.set_pipeline(&self.grid_pipeline);
                pass.draw(0..6, 0..1);
                stats.draw_calls += 1;
            }
            pass.set_pipeline(&self.tile_pipeline);
            for g in &list.tiles {
                self.draw_group(&mut pass, target, g, &mut stats);
            }
            if !list.ghosts.is_empty() {
                for pipeline in [&self.ghost_depth_pipeline, &self.ghost_pipeline] {
                    pass.set_pipeline(pipeline);
                    for g in &list.ghosts {
                        self.draw_group(&mut pass, target, g, &mut stats);
                    }
                }
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("selection mask"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.mask,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &target.globals_bind, &[]);
            pass.set_bind_group(1, &self.image_bind, &[]);
            pass.set_pipeline(&self.mask_pipeline);
            for g in &list.selection {
                self.draw_group(&mut pass, target, g, &mut stats);
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.output_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.composite_pipeline);
            pass.set_bind_group(0, &target.composite_bind, &[]);
            pass.draw(0..3, 0..1);
            stats.draw_calls += 1;
        }
        self.queue.submit([encoder.finish()]);
        stats.encode_ms = t0.elapsed().as_secs_f32() * 1000.0;
        stats
    }

    /// Instances are bound by vertex-buffer offset instead of a base
    /// instance, which the GL backend supports everywhere.
    fn draw_group(&self, pass: &mut wgpu::RenderPass<'_>, target: &Target, g: &DrawGroup, stats: &mut FrameStats) {
        let (buffer, count) = match &g.mesh {
            MeshRef::Placeholder => (&self.placeholder.0, self.placeholder.1),
            MeshRef::Tier(key, tier) => {
                match self.meshes.get(key).and_then(|m| m.gpu.get(*tier)).and_then(Option::as_ref) {
                    Some((b, c)) => (b, *c),
                    None => (&self.placeholder.0, self.placeholder.1),
                }
            }
        };
        let stride = size_of::<Instance>() as u64;
        let from = u64::from(g.start) * stride;
        pass.set_vertex_buffer(0, buffer.slice(..));
        pass.set_vertex_buffer(1, target.instances.slice(from..from + u64::from(g.count) * stride));
        pass.draw(0..count, 0..g.count);
        stats.draw_calls += 1;
        stats.triangles += u64::from(count / 3) * u64::from(g.count);
    }

    /// Copies `target.output` to the CPU as tightly packed RGBA rows.
    pub fn read_output(&self, target: &Target) -> Vec<u8> {
        let [w, h] = target.size;
        let padded = (w * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(padded * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.output,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        if let Ok(data) = slice.get_mapped_range() {
            for row in data.chunks(padded as usize) {
                out.extend_from_slice(&row[..(w * 4) as usize]);
            }
        }
        out
    }

    pub fn wait_idle(&self) {
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
    }
}

/// Box-filtered mip levels, largest first, so a zoomed-out battle map
/// doesn't shimmer.
fn mip_chain(w: u32, h: u32, rgba: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let mut levels = vec![(w, h, rgba.to_vec())];
    while let Some((pw, ph, prev)) = levels.last()
        && (*pw > 1 || *ph > 1)
    {
        let (nw, nh) = ((pw / 2).max(1), (ph / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let mut sum = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let sx = (x * 2 + dx).min(pw - 1);
                        let sy = (y * 2 + dy).min(ph - 1);
                        sum += u32::from(prev[((sy * pw + sx) * 4 + c) as usize]);
                    }
                    next[((y * nw + x) * 4 + c) as usize] = (sum / 4) as u8;
                }
            }
        }
        levels.push((nw, nh, next));
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_chain_ends_at_one_pixel() {
        let levels = mip_chain(5, 3, &[255; 5 * 3 * 4]);
        let sizes: Vec<_> = levels.iter().map(|(w, h, _)| (*w, *h)).collect();
        assert_eq!(sizes, [(5, 3), (2, 1), (1, 1)]);
        assert!(levels.iter().all(|(_, _, d)| d.iter().all(|&v| v == 255)));
    }

    #[test]
    fn flat_vertices_carry_face_normals() {
        let v = flat_vertices(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], &[0, 1, 2]);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].normal, [0.0, 0.0, 1.0]);
    }
}
