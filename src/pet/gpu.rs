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
    /// 纯特效层 [opacity, glow, additive(0/1), 有噪声贴图(0/1)]
    /// 有基色的 [alpha 是镂空遮罩(0/1), 线条提亮倍数, -, -]
    params: [f32; 4],
    /// 纯特效层 [遮罩是否 matcap(0/1), -, 有星点(0/1), 有 matcap(0/1)]
    /// 有基色的 [-, 是玻璃/纱(0/1), 有星点(0/1), 有 matcap(0/1)]
    flags: [f32; 4],
    /// [星点 u 平铺, v 平铺, 边缘光强度, 不透明度]
    star: [f32; 4],
    /// 星点着色(rgb)+ **星点层强度**(a,`Stick_Intensity`)
    star_color: [f32; 4],
    /// MatCap 着色(rgb,可能是 HDR)+ 备用
    matcap_color: [f32; 4],
    /// 自发光:`Emitter Color`(rgb,线性)+ `Emitter Intensity`(a)。a = 0 时整层不画。
    emissive: [f32; 4],
    // ⚠ 字段顺序必须和 pet.wgsl 的 `MaterialParams` 逐个对齐:uniform 是按偏移读的,
    // 顺序错了不会报错,只会静默取到旁边那个字段的值(rim/main 曾经就是这么对调的)。
    /// 边缘光颜色
    rim_color: [f32; 4],
    /// [边缘光衰减次数, 色带混入强度, -, 有色带(0/1)]
    extra: [f32; 4],
    /// 玻璃内部那层:[折射率, GlobalDepth, 闪烁速度, 有内部层(0/1)]
    interior: [f32; 4],
    /// 内部星光着色(rgb,HDR)+ 闪烁次数(a)
    interior_color: [f32; 4],
    /// 模型包围盒:最小角 + 尺寸(w = 最长边)
    bounds_min: [f32; 4],
    bounds_size: [f32; 4],
    /// 色带的 ID 遮罩:[区间下限, 区间上限, 有遮罩(0/1), -]
    mask_id: [f32; 4],
}

/// 本体贴图 alpha 里那层线条遮罩的**加性**强度。游戏里那些纹路(水灵身上的竖条、
/// 多数宠物的身体分块线)比底色亮一档。
///
/// **形状已经按汇编改对了**(罗隐 body shader 51377 第 99~103 行):
///     r1.w = saturate((基色.a − 0.04) × 1.1111)     ← 和不透明度用的是同一个重映射
///     mad r6.xyz, cb6[7].xyzx, r1.w, r6.xyzx         ← 往**固有色**里加 cb6[7] × 那个遮罩
/// 原来这里是 `× mix(1.0, 1.55, alpha)`(乘法、且用生 alpha),形状就不对 —— 那个 1.55
/// 还是在上游法线 bug 修好前对着截图挑的。
///
/// **只剩强度是标定的**:`cb6[7]` 那个颜色的名字还没解出来(这条 shader 的 V=112,
/// 全库没有材质带这个块),所以先取中性白 × 这个标量。17 只对照对它很不敏感
/// (0.0~0.35 之间四项指标几乎不动),取中间值。
/// **`cb6[7]` 已经定名,但它解释不了这一项 —— 别照着改。**
///
/// 把 51377(罗隐 body,cb6、`dcl cb6[148]`、V=112)配到 `MI_P_Object` 块 14
/// (V=112 / S=142 ⇒ 总槽 149 ≥ 148),`cb6[7]` 解出来是 **`Glow Color × Glow Intensity`**,
/// 也就是那一步是**发光层**:`Glow Color × Glow Intensity × saturate((基色.a − 0.04) × 1.1111)`。
/// 而 `Glow Intensity` 在采样过的每只宠物上都是根默认 **0**(罗隐/鸭吉吉/点点/暮星辰/
/// 水灵/火神全查过),全库只有 2 处实例覆盖 ⇒ **这一层实机基本不画**。
///
/// **但把这里归零反而更差**:15 只对照的调色板中位 0.077 → **0.090**,而且退步**只落在
/// 星光族与水系**(暮星辰 0.058 → **0.162**、水灵 0.097 → 0.115、幽星光 0.081 → 0.095),
/// 罗隐 / 鸭吉吉 / 点点 / 迪莫 / 魔力猫 这些走基础路径的**一个数都没变**。
/// (顺带:全库过曝 4 → 3。)
///
/// ⇒ **`LINE_BOOST` 补的不是发光层**,而是那几族「我们只做了近似的额外层」欠下的量。
/// 它的真实身份仍未查明。**注意配对没通过判据**:罗隐自己的材质 0 个冻结块,
/// 只能借父材质的布局,而「块的槽位里要有这个材质覆盖过的参数」这条验证不了。
const LINE_BOOST: f32 = 0.2;

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
    glass_pipeline: wgpu::RenderPipeline,
    /// (首索引, 数量, 材质序号)。分三批,后两批要在不透明层之后画:
    /// `draws` 不透明、`glass_draws` 有基色的半透(玻璃/纱)、`effect_draws` 纯特效层。
    draws: Vec<(u32, u32, usize)>,
    effect_draws: Vec<(u32, u32, usize)>,
    glass_draws: Vec<(u32, u32, usize)>,
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
                // 6 = 玻璃内部那颗星的四角星场;7 = 色带的 ID 遮罩
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
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
        // 内部星层要把模型空间位置归一化到包围盒
        let bmin = [model.bounds.0.x, model.bounds.0.y, model.bounds.0.z, 0.0];
        let extent = model.bounds.1 - model.bounds.0;
        let bsize = [
            extent.x,
            extent.y,
            extent.z,
            extent.x.max(extent.y).max(extent.z),
        ];
        let mut material_binds = Vec::new();
        for material in &model.materials {
            // 主贴图:普通材质是基色;特效层是遮罩(形状来源),缺了就用白图 = 常量 1
            let main = match (&material.base_color, &material.effect) {
                (Some(image), _) => image,
                (None, Some(effect)) => effect.mask.as_ref().unwrap_or(&white),
                (None, None) => &white,
            };
            let main_view = upload_texture(device, queue, &material.name, main);
            // 第二张贴图两种用途共用一个 binding(一个材质只会是其中一种):
            // 特效层是噪声(火焰的流动),有基色的是卷动色带(暮星辰环带的渐变)
            let second = match &material.effect {
                Some(effect) => effect.noise.as_ref(),
                None => material.flow.as_ref(),
            };
            let noise_view = upload_texture(device, queue, &material.name, second.unwrap_or(&white));
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
            let interior_view = upload_texture(
                device,
                queue,
                &material.name,
                material.interior.as_ref().unwrap_or(&white),
            );
            let mask_id_view = upload_texture(
                device,
                queue,
                &material.name,
                material.mask_id.as_ref().unwrap_or(&white),
            );
            let has = |v: bool| if v { 1.0 } else { 0.0 };
            let rgb = |c: [f32; 3]| [c[0], c[1], c[2], 0.0];
            let mask_id = |m: &super::model::Material| {
                [
                    m.mask_id_range[0],
                    m.mask_id_range[1],
                    has(m.mask_id.is_some()),
                    0.0,
                ]
            };
            let interior = |m: &super::model::Material| {
                [
                    m.refraction,
                    // **march 深度按汇编算,不再手挑。** 汇编里是
                    // `marchDist = |半包围盒| × 0.01 × GlobalDepth`,这里传 GlobalDepth(=100),
                    // 那个 0.01 与包围盒的部分在 shader 里做(它才有包围盒)。
                    m.refract_depth,
                    // 闪烁速度(`FlickerSpeed`);次数走 interior_color.a
                    m.flicker[0],
                    has(m.interior.is_some()),
                ]
            };
            let extra = |m: &super::model::Material| {
                [m.rim_power, m.flow_power, 0.0, has(m.flow.is_some())]
            };
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
                        // 纯特效层没有基色,不走玻璃分支
                        0.0,
                        has(material.star.is_some()),
                        has(material.matcap.is_some()),
                    ],
                    emissive: [0.0, 0.0, 0.0, 0.0],   // 纯特效层不走这一层
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
                        material.stick_intensity,
                    ],
                    matcap_color: rgb(material.matcap_color),
                    rim_color: rgb(material.rim_color),
                    extra: extra(material),
                    interior: interior(material),
                    interior_color: [
                        material.interior_color[0],
                        material.interior_color[1],
                        material.interior_color[2],
                        material.flicker[1],
                    ],
                    bounds_min: bmin,
                    bounds_size: bsize,
                    mask_id: mask_id(material),
                },
                // 有基色的材质:params.x/.z 说明 alpha 怎么解释
                // (x=1 镂空遮罩、z=1 不透明度,都为 0 则是线条遮罩)
                None => MaterialUniform {
                    tint: [1.0; 4],
                    flow: material.flow_uv,
                    params: [
                        has(material.cutout),
                        // alpha 恒定的贴图没有线条可提,这一项必须是**加性的空操作 = 0**,
                        // 否则整只宠物被均匀加亮。
                        // **改成加性时踩过**:这儿原来写 1.0(乘法的恒等元),换成加性后就变成
                        // 「加一整份白」——17 只对照的对比比从 0.96 崩到 0.23、亮度冲到 1.2。
                        if material.line_detail && !material.alpha_opacity {
                            LINE_BOOST
                        } else {
                            0.0
                        },
                        has(material.alpha_opacity),
                        // 星点层来自「假半透」族 ⇒ 着色走 star_color(= Color02),不是四段渐变
                        has(material.star_fake_trans),
                    ],
                    // flags.y = 1 表示「玻璃/纱」:fs_main 据此加 MatCap 高光与材质边缘光,
                    // 普通不透明宠物无条件叠这两样会整只发白
                    flags: [
                        0.0,
                        has(material.translucent),
                        has(material.star.is_some()),
                        has(material.matcap.is_some()),
                    ],
                    emissive: [
                        material.emissive[0],
                        material.emissive[1],
                        material.emissive[2],
                        material.emissive_intensity,
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
                        material.stick_intensity,
                    ],
                    matcap_color: rgb(material.matcap_color),
                    rim_color: rgb(material.rim_color),
                    extra: extra(material),
                    interior: interior(material),
                    interior_color: [
                        material.interior_color[0],
                        material.interior_color[1],
                        material.interior_color[2],
                        material.flicker[1],
                    ],
                    bounds_min: bmin,
                    bounds_size: bsize,
                    mask_id: mask_id(material),
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
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(&interior_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&mask_id_view),
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
                // 玻璃内部层的采样起点 (UV1.x, UV1.y, UV2.x),见 model.rs 的 `interior_pos`
                wgpu::VertexAttribute {
                    offset: 56,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x3,
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
        // 混合通道:只测深度不写(半透层之间不该互相挡),**剔背面**——不剔的话闭合壳
        // 正反两面都参与混合,转身时可见组合不断变化 → 看着在闪。薄片状的火焰/水膜
        // 少画一面无所谓。混合沿用预乘 alpha:输出 alpha=0 就是加色(dst + rgb),
        // 输出 alpha=不透明度就是普通半透,一条管线覆盖两种。
        let effect_pipeline = make_pipeline(
            "pet-effect",
            "vs_main",
            "fs_effect",
            Some(wgpu::Face::Back),
            false,
        );
        // 有基色的半透(暮星辰那两个球)和不透明本体是同一个片元函数,只是走混合通道
        let glass_pipeline = make_pipeline(
            "pet-glass",
            "vs_main",
            "fs_main",
            Some(wgpu::Face::Back),
            false,
        );

        // 需要混合的最后画(叠在本体之上)。判据是 `blended()` 而不是 `translucent`:
        // 标着 BLEND_Translucent 但不透明度就是 1 的(幽星光那两个球)输出和不透明一样,
        // 放进混合通道只会因为不写深度而互相盖不住 —— 两颗球绕着转就闪。
        let (blended, draws): (Vec<_>, Vec<_>) = model
            .primitives
            .iter()
            .map(|p| (p.first_index, p.index_count, p.material))
            .partition(|&(_, _, m)| model.materials[m].blended());
        // 混合通道里再分两种片元函数:有基色的走 fs_main,纯特效层走 fs_effect
        let (glass_draws, effect_draws): (Vec<_>, Vec<_>) = blended
            .into_iter()
            .partition(|&(_, _, m)| model.materials[m].effect.is_none());
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
            glass_pipeline,
            draws,
            effect_draws,
            glass_draws,
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
        // 混合层放最后:本体的深度已经写好,这里只测不写,叠在上面
        for (pipeline, batch) in [
            (&self.glass_pipeline, &self.glass_draws),
            (&self.effect_pipeline, &self.effect_draws),
        ] {
            if batch.is_empty() {
                continue;
            }
            pass.set_pipeline(pipeline);
            for &(first, count, material) in batch {
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
