//! 宠物的 GPU 侧:顶点/索引缓冲、蒙皮矩阵、贴图,以及 toon + 描边两条管线。
//!
//! 蒙皮在顶点着色器里做(CPU 只算每关节一个矩阵),多实体时也只多上传一小块矩阵。
//! 描边是第二遍绘制:法线外扩 + 只画背面,顺序是先描边后本体(靠深度测试盖住)。

use anyhow::Result;
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use super::model::{Model, Vertex};

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    light_dir: [f32; 3],
    outline_width: f32,
}

/// 一只宠物的 GPU 资源(网格与贴图按形态共享,实例状态另说)。
pub struct PetGpu {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    joints: wgpu::Buffer,
    joint_capacity: usize,
    camera: wgpu::Buffer,
    frame_bind: wgpu::BindGroup,
    material_binds: Vec<wgpu::BindGroup>,
    pipeline: wgpu::RenderPipeline,
    outline_pipeline: wgpu::RenderPipeline,
    draws: Vec<(u32, u32, usize)>,
}

impl PetGpu {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &Model,
        target_format: wgpu::TextureFormat,
    ) -> Result<Self> {
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("pet-vertices"),
            contents: bytemuck::cast_slice(&model.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("pet-indices"),
            contents: bytemuck::cast_slice(&model.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let joint_capacity = model.skeleton.joints.len().max(1);
        let joints = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pet-joints"),
            size: (joint_capacity * size_of::<Mat4>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pet-camera"),
            size: size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pet-frame"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pet-material"),
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
            ],
        });

        let frame_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pet-frame"),
            layout: &frame_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: joints.as_entire_binding(),
                },
            ],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pet-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let mut material_binds = Vec::new();
        for material in &model.materials {
            let view = match &material.base_color {
                Some(image) => upload_texture(device, queue, &material.name, image),
                // 贴图缺失时用白色,至少形体还能看
                None => upload_texture(
                    device,
                    queue,
                    &material.name,
                    &super::model::Image {
                        width: 1,
                        height: 1,
                        rgba: vec![255, 255, 255, 255],
                    },
                ),
            };
            material_binds.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("pet-material"),
                layout: &material_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            }));
        }

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pet"),
            source: wgpu::ShaderSource::Wgsl(include_str!("pet.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pet"),
            bind_group_layouts: &[Some(&frame_layout), Some(&material_layout)],
            immediate_size: 0,
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 24,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 32,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Uint16x4,
                },
                wgpu::VertexAttribute {
                    offset: 40,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        };
        let make_pipeline = |label: &str, vs: &str, fs: &str, cull: wgpu::Face| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[Some(vertex_layout.clone())],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: Some(cull),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = make_pipeline("pet", "vs_main", "fs_main", wgpu::Face::Back);
        // 描边画背面:外扩后的壳只有背面能露在本体之外
        let outline_pipeline =
            make_pipeline("pet-outline", "vs_outline", "fs_outline", wgpu::Face::Front);

        let draws = model
            .primitives
            .iter()
            .map(|p| (p.first_index, p.index_count, p.material))
            .collect();

        Ok(Self {
            vertices,
            indices,
            joints,
            joint_capacity,
            camera,
            frame_bind,
            material_binds,
            pipeline,
            outline_pipeline,
            draws,
        })
    }

    /// 上传本帧的相机与蒙皮矩阵。
    pub fn update(
        &self,
        queue: &wgpu::Queue,
        view_proj: Mat4,
        light_dir: Vec3,
        outline_width: f32,
        matrices: &[Mat4],
    ) {
        queue.write_buffer(
            &self.camera,
            0,
            bytemuck::bytes_of(&CameraUniform {
                view_proj: view_proj.to_cols_array_2d(),
                light_dir: light_dir.normalize().to_array(),
                outline_width,
            }),
        );
        let count = matrices.len().min(self.joint_capacity);
        let flat: Vec<[[f32; 4]; 4]> = matrices[..count]
            .iter()
            .map(|m| m.to_cols_array_2d())
            .collect();
        queue.write_buffer(&self.joints, 0, bytemuck::cast_slice(&flat));
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, outline: bool) {
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.set_bind_group(0, &self.frame_bind, &[]);
        for stage in 0..if outline { 2 } else { 1 } {
            pass.set_pipeline(if stage == 0 && outline {
                &self.outline_pipeline
            } else {
                &self.pipeline
            });
            for &(first, count, material) in &self.draws {
                pass.set_bind_group(1, &self.material_binds[material], &[]);
                pass.draw_indexed(first..first + count, 0, 0..1);
            }
        }
    }
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    image: &super::model::Image,
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &image.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(image.width * 4),
            rows_per_image: Some(image.height),
        },
        wgpu::Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        },
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// 正交相机:桌宠是贴在桌面上的小人,透视没有意义,正交还免了远近缩放的麻烦。
///
/// `bounds` 是**绑定姿势**的包围盒,`yaw` 是绕 Y 轴的观察角(0 = 从 +Z 看;宠物朝 +Z,故 0 是正面)。
/// `padding` 要留出余量:跳跃/伸展类动作会超出绑定姿势的包围盒(实测 Happy 会高出一截)。
pub fn orthographic_view(bounds: (Vec3, Vec3), yaw: f32, padding: f32) -> Mat4 {
    let (min, max) = bounds;
    let center = (min + max) * 0.5;
    let extent = max - min;
    // 取最长边而不是对角线:对角线会把瘦高的模型框得过松,宠物在画面里缩成一小团
    let radius = extent.x.max(extent.y).max(extent.z) * 0.5 * padding;
    let eye = center + glam::Quat::from_rotation_y(yaw) * Vec3::new(0.0, 0.0, radius * 2.0);
    let view = glam::camera::rh::view::look_at_mat4(eye, center, Vec3::Y);
    // 深度范围用 wgpu 的 0..1(DirectX 约定),与管线的 Depth32Float + CompareFunction::Less 匹配
    let proj = glam::camera::rh::proj::directx::orthographic(
        -radius,
        radius,
        -radius,
        radius,
        0.01,
        radius * 4.0,
    );
    proj * view
}
