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
    /// 秒。特效层的 UV 卷动靠它推进(火焰在流动)。
    time: f32,
    _pad: [f32; 3],
}

/// 每个材质一份的特效参数。**普通材质也占一份**(tint 全 1、flags=0),
/// 这样两条通道共用同一个 bind group 布局,少一套代码。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MaterialUniform {
    tint: [f32; 4],
    flow: [f32; 4],
    /// [opacity, glow, additive(0/1), 是否有噪声贴图(0/1)]
    params: [f32; 4],
    /// [遮罩是否 matcap(0/1), 有基色(0/1), 有星点(0/1), 有 matcap(0/1)]
    flags: [f32; 4],
    /// [星点 u 平铺, v 平铺, 边缘光强度, 不透明度]
    star: [f32; 4],
    /// 星点着色(rgb)+ 线条提亮(a)
    star_color: [f32; 4],
    /// MatCap 着色(rgb,可能是 HDR)+ 备用
    matcap_color: [f32; 4],
    /// 半透材质的整体着色
    main_color: [f32; 4],
    /// 边缘光颜色
    rim_color: [f32; 4],
}

/// 本体贴图 alpha 里那层线条遮罩的提亮倍数。游戏里那些纹路(水灵身上的竖条、
/// 多数宠物的身体分块线)比底色亮一档,这里用乘法近似,数值按实机截图对出来的。
const LINE_BOOST: f32 = 1.55;

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
    effect_pipeline: wgpu::RenderPipeline,
    /// (首索引, 数量, 材质序号);按「是不是特效层」分成两批,特效层要最后画。
    draws: Vec<(u32, u32, usize)>,
    effect_draws: Vec<(u32, u32, usize)>,
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
                // 特效层的噪声贴图;普通材质绑一张 1×1 白图占位
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 星点与 MatCap:游戏里几乎每个宠物材质都挂着这两张
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
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
            // **必须 Repeat,不能用 wgpu 默认的 ClampToEdge。** UE 的贴图默认是 wrap,
            // 而这些网格的 UV 大量落在 [0,1] 之外(水灵实测 u/v 都到 -1.0)。
            // 夹取会把区间外的全压到贴图边缘,图案摊平成一片纯色——
            // 水灵身上那一道道竖向浅色条纹就是这么丢的。
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        let white = super::model::Image {
            width: 1,
            height: 1,
            rgba: vec![255, 255, 255, 255],
        };
        let mut material_binds = Vec::new();
        for material in &model.materials {
            // 主贴图:普通材质是基色;特效层是遮罩(形状来源),缺了就用白图 = 常量 1
            let main = match (&material.base_color, &material.effect) {
                (Some(image), _) => image,
                (None, Some(effect)) => effect.mask.as_ref().unwrap_or(&white),
                (None, None) => &white,
            };
            let main_view = upload_texture(device, queue, &material.name, main);
            let noise = material
                .effect
                .as_ref()
                .and_then(|e| e.noise.as_ref())
                .unwrap_or(&white);
            let noise_view = upload_texture(device, queue, &material.name, noise);
            let star_view = upload_texture(
                device,
                queue,
                &material.name,
                material.star.as_ref().unwrap_or(&white),
            );
            let matcap_view = upload_texture(
                device,
                queue,
                &material.name,
                material.matcap.as_ref().unwrap_or(&white),
            );
            let has = |v: bool| if v { 1.0 } else { 0.0 };
            let uniform = match &material.effect {
                Some(effect) => MaterialUniform {
                    tint: effect.tint,
                    flow: effect.flow,
                    params: [
                        effect.opacity,
                        effect.glow,
                        has(effect.additive),
                        has(effect.noise.is_some()),
                    ],
                    flags: [
                        has(effect.mask_matcap),
                        0.0,
                        has(material.star.is_some()),
                        has(material.matcap.is_some()),
                    ],
                    star: [
                        material.star_tiling[0],
                        material.star_tiling[1],
                        material.rim_intensity,
                        effect.opacity,
                    ],
                    star_color: [
                        material.star_color[0],
                        material.star_color[1],
                        material.star_color[2],
                        LINE_BOOST,
                    ],
                    matcap_color: [
                        material.matcap_color[0],
                        material.matcap_color[1],
                        material.matcap_color[2],
                        0.0,
                    ],
                    rim_color: [
                        material.rim_color[0],
                        material.rim_color[1],
                        material.rim_color[2],
                        0.0,
                    ],
                    main_color: [
                        material.main_color[0],
                        material.main_color[1],
                        material.main_color[2],
                        0.0,
                    ],
                },
                // 有基色的材质:params.x 说明 alpha 怎么解释(1=镂空遮罩,0=线条遮罩)
                None => MaterialUniform {
                    tint: [1.0; 4],
                    flow: [0.0, 0.0, 1.0, 1.0],
                    params: [
                        has(material.cutout),
                        // alpha 恒定的贴图没有线条可提,提亮必须是空操作(1.0),
                        // 否则整只宠物被均匀调亮
                        if material.line_detail {
                            LINE_BOOST
                        } else {
                            1.0
                        },
                        0.0,
                        0.0,
                    ],
                    flags: [
                        0.0,
                        1.0,
                        has(material.star.is_some()),
                        has(material.matcap.is_some()),
                    ],
                    star: [
                        material.star_tiling[0],
                        material.star_tiling[1],
                        material.rim_intensity,
                        material.opacity,
                    ],
                    star_color: [
                        material.star_color[0],
                        material.star_color[1],
                        material.star_color[2],
                        if material.line_detail {
                            LINE_BOOST
                        } else {
                            1.0
                        },
                    ],
                    matcap_color: [
                        material.matcap_color[0],
                        material.matcap_color[1],
                        material.matcap_color[2],
                        0.0,
                    ],
                    rim_color: [
                        material.rim_color[0],
                        material.rim_color[1],
                        material.rim_color[2],
                        0.0,
                    ],
                    main_color: [
                        material.main_color[0],
                        material.main_color[1],
                        material.main_color[2],
                        0.0,
                    ],
                },
            };
            let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("pet-material-uniform"),
                contents: bytemuck::bytes_of(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            material_binds.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("pet-material"),
                layout: &material_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&main_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&noise_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&star_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&matcap_view),
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
        // depth_write:主通道写深度,特效通道只测不写(半透层之间不该互相挡)
        let make_pipeline =
            |label: &str, vs: &str, fs: &str, cull: Option<wgpu::Face>, depth_write: bool| {
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
                        cull_mode: cull,
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: DEPTH_FORMAT,
                        depth_write_enabled: Some(depth_write),
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
        let pipeline = make_pipeline("pet", "vs_main", "fs_main", Some(wgpu::Face::Back), true);
        // 描边画背面:外扩后的壳只有背面能露在本体之外
        let outline_pipeline = make_pipeline(
            "pet-outline",
            "vs_outline",
            "fs_outline",
            Some(wgpu::Face::Front),
            true,
        );
        // 特效层:**不剔面**(火焰/水壳是薄壳,正反两面都要看得见)、不写深度。
        // 混合沿用预乘 alpha —— shader 输出 alpha=0 就等价于加色(dst + rgb),
        // 输出 alpha=不透明度就是普通半透,一条管线覆盖火焰与水壳两种。
        let effect_pipeline = make_pipeline("pet-effect", "vs_main", "fs_effect", None, false);

        // 特效层最后画:它们要叠在本体之上
        // 半透的一律进特效通道:纯特效层(没有基色)和「有基色但 BLEND_Translucent」的
        // (暮星辰的裙子与那两个球)都在里面
        let (effect_draws, draws): (Vec<_>, Vec<_>) = model
            .primitives
            .iter()
            .map(|p| (p.first_index, p.index_count, p.material))
            .partition(|&(_, _, m)| {
                model.materials[m].effect.is_some() || model.materials[m].translucent
            });

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
            effect_pipeline,
            draws,
            effect_draws,
        })
    }

    /// 上传本帧的相机与蒙皮矩阵。
    pub fn update(
        &self,
        queue: &wgpu::Queue,
        view_proj: Mat4,
        light_dir: Vec3,
        outline_width: f32,
        time: f32,
        matrices: &[Mat4],
    ) {
        queue.write_buffer(
            &self.camera,
            0,
            bytemuck::bytes_of(&CameraUniform {
                view_proj: view_proj.to_cols_array_2d(),
                light_dir: light_dir.normalize().to_array(),
                outline_width,
                time,
                _pad: [0.0; 3],
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
        // 特效层放最后:本体的深度已经写好,这里只测不写,叠在上面
        if !self.effect_draws.is_empty() {
            pass.set_pipeline(&self.effect_pipeline);
            for &(first, count, material) in &self.effect_draws {
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
