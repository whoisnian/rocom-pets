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
    /// 描边宽度的**全局倍率**(1 = 按材质里读出来的实机宽度)。宽度本身逐材质,
    /// 在 `MaterialUniform::outline` 里;这儿留一个倍率是为了桌宠那条取舍 ——
    /// 实机的描边只有 ~0.39 厘米、几乎看不见,而桌宠要在任意背景上认得出轮廓。
    outline_scale: f32,
    /// 秒。特效层的 UV 卷动靠它推进(火焰在流动)。
    time: f32,
    /// 是否选择原游戏的高材质质量排列。目标实机配置为 Low；对应的
    /// `M_P_Object_Trans` shader map 不含 StarStick 采样块。
    high_material_quality: f32,
    /// 表情:脸那两个材质的 UV 偏移(整格,见 persona.rs 的 `Expression`)。
    /// **放在相机这份 uniform 里**是因为它是**每只**的(同一个形态的两只可以不同表情),
    /// 而材质那份是按形态共享的 —— 那边只存「这是不是脸」。
    face_uv: [f32; 2],
    /// 当前蒙皮姿势的物体包围盒中心(xyz)与最长边(w)。FakeFulid 的目标 PS 从
    /// PrimitiveSceneData 读取的正是 ObjectWorldPositionAndRadius / ObjectBounds；
    /// 不能拿未蒙皮 POSITION 或绑定姿势盒替代，否则液面会跟着身体弯曲。
    object_bounds: [f32; 4],
    /// 网格脸要画第几张卡(1–8),已经退过档、保证这只身上有。
    /// 和 `face_uv` 一样是**每只**的,所以放这份 uniform 里。
    face_card: f32,
    /// vec4 要 16 字节对齐,`object_bounds` 之后只能整块地补。
    _pad: [f32; 3],
}

/// 画一帧要给的东西。**打包传**:拆成参数的话 `update` 要排到第八个,而它们
/// 每帧一起变。
pub struct FrameParams {
    pub view_proj: Mat4,
    pub light_dir: Vec3,
    /// 描边宽度的全局倍率;1 = 实机宽度(逐材质,见 `MaterialSpec::outline_width`)。
    pub outline_scale: f32,
    /// 秒;特效层的 UV 卷动靠它推进。
    pub time: f32,
    /// 是否选择原游戏的高材质质量排列；桌面与离屏默认复现实机的 Low。
    pub high_material_quality: bool,
    /// 表情:脸那两个材质的 UV 偏移(见 persona.rs 的 `Expression`)。
    pub face_uv: [f32; 2],
    /// 表情:网格脸要画第几张卡(见 persona.rs 的 `Expression::card`)。
    /// 这只没有这张卡时会自动退档,调用方不必管。
    pub face_card: u32,
}

/// 每个材质一份的特效参数。**普通材质也占一份**(tint 全 1、flags=0),
/// 这样两条通道共用同一个 bind group 布局,少一套代码。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MaterialUniform {
    tint: [f32; 4],
    flow: [f32; 4],
    /// 纯特效层 [opacity, glow, additive(0/1), 有噪声贴图(0/1)]
    /// 有基色的 [alpha 是镂空遮罩(0/1), 线条提亮倍数, alpha 是不透明度(0/1), 这是脸(0/1)]
    params: [f32; 4],
    /// 纯特效层 [遮罩是否 matcap(0/1), -, 有星点(0/1), 有 matcap(0/1)]
    /// 有基色的 [**这是脸**(0/1), 是玻璃/纱(0/1), 有星点(0/1), 有 matcap(0/1)]
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
    /// 高光方向偏移(xyz,glTF Y-up)+ `HighLightSpecPow`
    highlight: [f32; 4],
    /// `HighLight SpecCol`(rgb)+ `HighLight SpecInt`
    highlight_color: [f32; 4],
    /// [边缘光衰减次数, 色带混入强度, Rim Soft Edge, 有色带(0/1)]
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
    /// 假半透族星点层:[速度X, 速度Y, 强度, 是否用 UV0]
    noise_uv: [f32; 4],
    /// `M_ShuiMu_ByIn` 专用参数，逐项对应原 shader 71636。
    glassy_flow1: [f32; 4],
    glassy_flow2: [f32; 4],
    glassy_fresnel: [f32; 4],
    /// [GlassyNoiseSpeed, UVScale, Refract, Depth]
    glassy_noise: [f32; 4],
    /// [FresnelMaskPow, Offset, Smooth, TriPlannarBlendInt]
    glassy_mask: [f32; 4],
    /// `M_P_Object_Trans` 场景深度淡化:[距离(米),开启强度,-,-]
    depth_fade: [f32; 4],
    /// [XiaoYou, YutuEar, FakeFluid, MatcapMasked]；每项对应一个原生材质分支。
    /// **第五族 FairyBall 的开关不在这儿,在 `family11.w`** —— 这一行已经满了,而前四族
    /// 最多用到 `family10`,`family11` 全库恒为 0,正好拿来既当它的参数又当判据。
    family_flags: [f32; 4],
    xiaoyou_base1: [f32; 4],
    xiaoyou_base2: [f32; 4],
    xiaoyou_flow1: [f32; 4],
    xiaoyou_flow2: [f32; 4],
    xiaoyou_star_color: [f32; 4],
    xiaoyou_noise_flow: [f32; 4],
    xiaoyou_shape: [f32; 4],
    xiaoyou_star_uv: [f32; 4],
    /// 四套互斥的原生材质族共用参数区；解释由 family_flags.y/z/w 与 family11.w 决定。
    family0: [f32; 4],
    family1: [f32; 4],
    family2: [f32; 4],
    family3: [f32; 4],
    family4: [f32; 4],
    family5: [f32; 4],
    family6: [f32; 4],
    family7: [f32; 4],
    family8: [f32; 4],
    family9: [f32; 4],
    family10: [f32; 4],
    family11: [f32; 4],
    /// 描边:[沿法线外扩多少米, -, -, -]。**逐材质**(游戏里也是),见
    /// `pack::MaterialSpec::outline_width`。
    outline: [f32; 4],
}

/// 本体贴图 alpha 里那层曾经使用的加性白色补偿。
///
/// **形状已经按汇编改对了**(罗隐 body shader 51377 第 99~103 行):
///     r1.w = saturate((基色.a − 0.04) × 1.1111)     ← 和不透明度用的是同一个重映射
///     mad r6.xyz, cb6[7].xyzx, r1.w, r6.xyzx         ← 加上 cb6[7] × 那个遮罩
/// 原来这里是 `× mix(1.0, 1.55, alpha)`(乘法、且用生 alpha),形状就不对 —— 那个 1.55
/// 还是在上游法线 bug 修好前对着截图挑的。
///
/// **位置在 2026-08-08 改过一次**:原来加在固有色累加器上,而水灵本体那条
/// ES3.1/Low/LOD0 的 `M_P_Object`(资源 `BF0167AE…`,PS 68952)里,`Glow Color × Glow
/// Intensity × 遮罩`(第 145 行)与 `Emitter Color × Emitter Intensity × 遮罩`
/// (第 62~65 行)**都进发光累加器 r1**,第 268 行才和已着色的颜色相加。现在按汇编放在
/// `glow` 里(见 pet.wgsl)。值仍然是 0,所以这次搬家对渲图是零改动。
///
/// 把 51377(罗隐 body,cb6、`dcl cb6[148]`、V=112)配到 `MI_P_Object` 块 14
/// (V=112 / S=142 ⇒ 总槽 149 ≥ 148),`cb6[7]` 解出来是 **`Glow Color × Glow Intensity`**,
/// 也就是那一步是**发光层**:`Glow Color × Glow Intensity × saturate((基色.a − 0.04) × 1.1111)`。
/// 而 `Glow Intensity` 在采样过的每只宠物上都是根默认 **0**(罗隐/鸭吉吉/点点/暮星辰/
/// 水灵/火神全查过),全库只有 2 处实例覆盖 ⇒ **这一层实机基本不画**。
///
/// 因而通用路径必须保持为 0；星光族、水体缺失的亮度要在各自的原始材质分支中补齐，
/// 不能用一层全局白膜代偿。矮脚爬爬的低对比正是这层代偿在高 alpha 纹理上的副作用。
const LINE_BOOST: f32 = 0.0;

/// 旧包(没有 `outline_width` 字段)的描边宽度,单位**米**。
///
/// 取全库模态值:`0.01 × OutlineWidthPC(0.13) × MaxWidthScale(300)` 厘米 = 0.39 厘米。
/// 854 份 `_Ol` 里 `OutlineWidthPC = 0.13` 的有 848 份、`MaxWidthScale = 300` 的有 847 份,
/// 例外只有火源两份、呜呜 `_Fx` 一份和挂在别的根上的 3 份。
/// 推导见 `exporter/Materials.cs` 的 `OutlineWidthOf`。
const DEFAULT_OUTLINE_WIDTH: f32 = 0.0039;

/// 一只宠物的 GPU 资源(网格与贴图按形态共享,实例状态另说)。
pub struct PetGpu {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    joints: wgpu::Buffer,
    joint_capacity: usize,
    camera: wgpu::Buffer,
    /// 只在模型含 FakeFulid 时保留一份 CPU 顶点，用当前关节矩阵复原 UE 每帧更新的
    /// PrimitiveSceneData bounds。其他宠物不承担逐帧蒙皮包围盒的开销。
    bounds_vertices: Option<Vec<Vertex>>,
    /// 这只网格脸真有哪几张表情卡(见 `Model::face_cards`);不是网格脸就是空。
    face_cards: Vec<u32>,
    bind_bounds: [f32; 4],
    frame_bind: wgpu::BindGroup,
    depth_layout: wgpu::BindGroupLayout,
    material_binds: Vec<wgpu::BindGroup>,
    pipeline: wgpu::RenderPipeline,
    outline_pipeline: wgpu::RenderPipeline,
    paint_order_pipeline: wgpu::RenderPipeline,
    effect_pipeline: wgpu::RenderPipeline,
    glass_pipeline: wgpu::RenderPipeline,
    glassy_inner_pipeline: wgpu::RenderPipeline,
    /// (首索引, 数量, 材质序号)。`glassy_inner_draws` 是原材质本来就不透明、写深度的
    /// 流动内胆；其余批次的先后见 `Self::new` 的拆分说明。
    draws: Vec<(u32, u32, usize)>,
    effect_draws: Vec<(u32, u32, usize)>,
    glass_draws: Vec<(u32, u32, usize)>,
    inner_draws: Vec<(u32, u32, usize)>,
    glassy_inner_draws: Vec<(u32, u32, usize)>,
    /// 原材质为不透明、但不应套桌宠统一描边的专用内层（当前为 YutuEar）。
    special_opaque_draws: Vec<(u32, u32, usize)>,
    /// 要画描边的那些(逐材质按 `_Ol` 资产开关;半透里有 `_Ol` 的也在这儿)。
    outline_draws: Vec<(u32, u32, usize)>,
    /// 剔正面画的那些(幽火族的双层壳),见 `paint_order_pipeline`。
    paint_order_draws: Vec<(u32, u32, usize)>,
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
        let bind_center = (model.bounds.0 + model.bounds.1) * 0.5;
        let bind_extent = model.bounds.1 - model.bounds.0;
        let bind_bounds = [
            bind_center.x,
            bind_center.y,
            bind_center.z,
            bind_extent.max_element(),
        ];
        let bounds_vertices = model
            .materials
            .iter()
            .any(|material| material.fake_fluid.is_some())
            .then(|| model.vertices.clone());
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
                    // 顶点阶段也要读:描边宽度是逐材质的,`vs_outline` 从这里拿
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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
                // 6 = 玻璃内部那颗星的四角星场;7 = 色带的 ID 遮罩;
                // 8/9 = Low `MI_P_Object_Trans` 的 MaskTex / RampTex；10 是 RampTex
                // 在 cooked uniform expression 中声明的 clamp sampler。
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
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        // 半透明通道读取第一遍不透明几何留下的场景深度。原
        // `M_P_Object_Trans` 的 OpacityDepthDistance 就采这张纹理；不是果冻专用遮罩。
        let depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pet-scene-depth"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
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
        let clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pet-ramp-clamp-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
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
            let main = material
                .yutu_ear
                .as_ref()
                .and_then(|y| y.bubble.as_ref())
                .or_else(|| material.fake_fluid.as_ref().and_then(|f| f.mask.as_ref()))
                .or_else(|| {
                    material
                        .matcap_masked
                        .as_ref()
                        .and_then(|m| m.matcap.as_ref())
                })
                // FairyBall 的 MatCap 也走这个槽:它是那一族唯一的贴图,而通用
                // `matcap_tex` 槽不能借用 —— `material.matcap.is_some()` 还兼着
                // 「半透件要不要补一层描边壳」的判据(见下面 `outline_draws`)。
                .or_else(|| material.fairy_ball.as_ref().and_then(|f| f.matcap.as_ref()))
                .unwrap_or_else(|| match (&material.base_color, &material.effect) {
                    (Some(image), _) => image,
                    (None, Some(effect)) => effect.mask.as_ref().unwrap_or(&white),
                    (None, None) => &white,
                });
            let main_view = upload_texture(device, queue, &material.name, main);
            // 第二张贴图两种用途共用一个 binding(一个材质只会是其中一种):
            // 特效层是噪声(火焰的流动),有基色的是卷动色带(暮星辰环带的渐变)
            let second = material
                .yutu_ear
                .as_ref()
                .and_then(|y| y.distort.as_ref())
                .or_else(|| material.fake_fluid.as_ref().and_then(|f| f.lut.as_ref()))
                .or_else(|| material.xiaoyou.as_ref().and_then(|x| x.noise.as_ref()))
                .or(match &material.effect {
                    Some(effect) => effect.noise.as_ref(),
                    None => material.flow.as_ref(),
                });
            let noise_view =
                upload_texture(device, queue, &material.name, second.unwrap_or(&white));
            let star_view = upload_texture(
                device,
                queue,
                &material.name,
                material
                    .yutu_ear
                    .as_ref()
                    .and_then(|y| y.flow.as_ref())
                    .or(material.star.as_ref())
                    .unwrap_or(&white),
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
            let light_mask_view = upload_texture(
                device,
                queue,
                &material.name,
                material.light_mask.as_ref().unwrap_or(&white),
            );
            let ramp_view = upload_texture(
                device,
                queue,
                &material.name,
                material.ramp.as_ref().unwrap_or(&white),
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
                [
                    m.rim_power,
                    m.flow_power,
                    m.rim_soft_edge,
                    has(m.flow.is_some()),
                ]
            };
            let rim_color = |m: &super::model::Material| {
                [
                    m.rim_color[0],
                    m.rim_color[1],
                    m.rim_color[2],
                    m.force_default_opacity,
                ]
            };
            let highlight = |m: &super::model::Material| {
                [
                    m.highlight_offset[0],
                    m.highlight_offset[1],
                    m.highlight_offset[2],
                    m.highlight_power,
                ]
            };
            let highlight_color = |m: &super::model::Material| {
                [
                    m.highlight_color[0],
                    m.highlight_color[1],
                    m.highlight_color[2],
                    m.highlight_intensity,
                ]
            };
            let glassy_flow1 = material.glassy_inner.as_ref().map_or([0.0; 4], |g| g.flow1);
            let glassy_flow2 = material.glassy_inner.as_ref().map_or([0.0; 4], |g| g.flow2);
            let glassy_fresnel = material
                .glassy_inner
                .as_ref()
                .map_or([0.0; 4], |g| g.fresnel);
            let glassy_noise = material.glassy_inner.as_ref().map_or([0.0; 4], |g| g.noise);
            let glassy_mask = material.glassy_inner.as_ref().map_or([0.0; 4], |g| g.mask);
            let exact_object_trans = material.object_trans_low
                && material.light_mask.is_some()
                && material.ramp.is_some();
            let depth_fade = [
                material.depth_fade[0],
                material.depth_fade[1],
                has(exact_object_trans),
                material.object_trans_soft_edge,
            ];
            let family_flags = [
                has(material.xiaoyou.is_some()),
                has(material.yutu_ear.is_some()),
                has(material.fake_fluid.is_some()),
                has(material.matcap_masked.is_some()),
            ];
            let xiaoyou_base1 = material.xiaoyou.as_ref().map_or([0.0; 4], |x| x.base1);
            let xiaoyou_base2 = material.xiaoyou.as_ref().map_or([0.0; 4], |x| x.base2);
            let xiaoyou_flow1 = material.xiaoyou.as_ref().map_or([0.0; 4], |x| x.flow1);
            let xiaoyou_flow2 = material.xiaoyou.as_ref().map_or([0.0; 4], |x| x.flow2);
            let xiaoyou_star_color = material.xiaoyou.as_ref().map_or([0.0; 4], |x| x.star_color);
            let xiaoyou_noise_flow = material.xiaoyou.as_ref().map_or([0.0; 4], |x| x.noise_flow);
            let xiaoyou_shape = material.xiaoyou.as_ref().map_or([0.0; 4], |x| x.shape);
            let xiaoyou_star_uv = material.xiaoyou.as_ref().map_or([0.0; 4], |x| x.star_uv);
            let mut family = [[0.0; 4]; 12];
            if let Some(y) = &material.yutu_ear {
                family[0] = y.bubble_color;
                family[1] = y.flow_color;
                family[2] = y.fresnel_color;
                family[3] = y.inner_color;
                family[4] = y.overall_color;
                family[5] = y.ramp_color;
                family[6] = y.top_color;
                family[7] = y.bubble_shape;
                family[8] = y.flow_shape;
                family[9] = y.light_shape;
                family[10] = y.top_shape;
            } else if let Some(f) = &material.fake_fluid {
                family[0] = f.edge_color;
                family[1] = f.fresnel_color;
                family[2] = f.plane_color;
                family[3] = f.gradient1;
                family[4] = f.gradient2;
                family[5] = f.height_tiling;
                family[6] = f.plane_axis;
                family[7] = f.plane_center;
                family[8] = f.body_shape;
                family[9] = f.gradient_shape;
                family[10] = f.top_shape;
            } else if let Some(m) = &material.matcap_masked {
                family[0] = m.base_color;
                family[1] = m.light_ramp;
                family[2] = m.flat_emissive;
                family[3] = m.main_color;
                family[4] = m.selection_color;
                family[5] = m.rim_shape;
                family[6] = m.surface_shape;
            } else if let Some(f) = &material.fairy_ball {
                family[0] = f.base_color;
                family[1] = f.matcap_color;
                family[2] = f.rim_dark;
                family[3] = f.rim_light;
                family[4] = f.main_color;
                // 形状与开关同格:`shape.w` 恒为 1,着色器据此认这一族。
                family[11] = f.shape;
            }
            // 旧包没有 `outline_width` ⇒ 用全库模态值 0.13 × 300。这比旧包自己当年那条
            // 「包围盒对角线 × 0.004」更接近实机(魔力猫 1.79 厘米 vs 0.39 厘米)。
            let outline_width = material.outline_width.unwrap_or(DEFAULT_OUTLINE_WIDTH);
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
                    emissive: [0.0, 0.0, 0.0, 0.0], // 纯特效层不走这一层
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
                    rim_color: rim_color(material),
                    highlight: highlight(material),
                    highlight_color: highlight_color(material),
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
                    noise_uv: material.noise_uv,
                    glassy_flow1,
                    glassy_flow2,
                    glassy_fresnel,
                    glassy_noise,
                    glassy_mask,
                    depth_fade,
                    family_flags,
                    xiaoyou_base1,
                    xiaoyou_base2,
                    xiaoyou_flow1,
                    xiaoyou_flow2,
                    xiaoyou_star_color,
                    xiaoyou_noise_flow,
                    xiaoyou_shape,
                    xiaoyou_star_uv,
                    family0: family[0],
                    family1: family[1],
                    family2: family[2],
                    family3: family[3],
                    family4: family[4],
                    family5: family[5],
                    family6: family[6],
                    family7: family[7],
                    family8: family[8],
                    family9: family[9],
                    family10: family[10],
                    family11: family[11],
                    outline: [outline_width, 0.0, 0.0, 0.0],
                },
                // 有基色的材质:params.x/.z 说明 alpha 怎么解释
                // (x=1 镂空遮罩、z=1 不透明度,都为 0 则是线条遮罩)
                None => MaterialUniform {
                    // Low ObjectTrans 的尾部原样乘 `MainColor * MainBright`；普通基色
                    // 材质不读取 tint，因此可安全共用这个已有的 vec4。
                    tint: [
                        material.main_color[0] * material.main_bright,
                        material.main_color[1] * material.main_bright,
                        material.main_color[2] * material.main_bright,
                        1.0,
                    ],
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
                    // 普通不透明宠物无条件叠这两样会整只发白。
                    // flags.x 在这条路上原来是空的,拿来放「这是哪种脸」:
                    // 0 = 不是脸、1 = 图集脸(偏 UV)、2 = 网格脸(八张卡里只画一张)
                    flags: [
                        if material.face_cards {
                            2.0
                        } else {
                            has(material.face)
                        },
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
                    rim_color: rim_color(material),
                    highlight: highlight(material),
                    highlight_color: highlight_color(material),
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
                    noise_uv: material.noise_uv,
                    glassy_flow1,
                    glassy_flow2,
                    glassy_fresnel,
                    glassy_noise,
                    glassy_mask,
                    depth_fade,
                    family_flags,
                    xiaoyou_base1,
                    xiaoyou_base2,
                    xiaoyou_flow1,
                    xiaoyou_flow2,
                    xiaoyou_star_color,
                    xiaoyou_noise_flow,
                    xiaoyou_shape,
                    xiaoyou_star_uv,
                    family0: family[0],
                    family1: family[1],
                    family2: family[2],
                    family3: family[3],
                    family4: family[4],
                    family5: family[5],
                    family6: family[6],
                    family7: family[7],
                    family8: family[8],
                    family9: family[9],
                    family10: family[10],
                    family11: family[11],
                    outline: [outline_width, 0.0, 0.0, 0.0],
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
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(&light_mask_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: wgpu::BindingResource::TextureView(&ramp_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: wgpu::BindingResource::Sampler(&clamp_sampler),
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
        let translucent_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("pet-translucent"),
                bind_group_layouts: &[
                    Some(&frame_layout),
                    Some(&material_layout),
                    Some(&depth_layout),
                ],
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
                // 预蒙皮局部位置；原 VS 21175/31053 直接把解码位置传给折射材质。
                wgpu::VertexAttribute {
                    offset: 56,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x3,
                },
                // glTF `COLOR_0`；三套目标材质的目标 Low PS 都直接读取它。
                wgpu::VertexAttribute {
                    offset: 68,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        };
        // depth_write:主通道写深度,特效通道只测不写(半透层之间不该互相挡)
        let make_pipeline = |label: &str,
                             vs: &str,
                             fs: &str,
                             cull: Option<wgpu::Face>,
                             depth_write: bool,
                             reads_scene_depth: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(if reads_scene_depth {
                    &translucent_pipeline_layout
                } else {
                    &pipeline_layout
                }),
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
        let pipeline = make_pipeline(
            "pet",
            "vs_main",
            "fs_main",
            Some(wgpu::Face::Back),
            true,
            false,
        );
        // 描边画背面:外扩后的壳只有背面能露在本体之外
        let outline_pipeline = make_pipeline(
            "pet-outline",
            "vs_outline",
            "fs_outline",
            Some(wgpu::Face::Front),
            true,
            false,
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
            true,
        );
        // 有基色的半透(暮星辰那两个球)和不透明本体是同一个片元函数,只是走混合通道
        let glass_pipeline = make_pipeline(
            "pet-glass",
            "vs_main",
            "fs_glass",
            Some(wgpu::Face::Back),
            false,
            true,
        );
        // `M_ShuiMu_ByIn` 在原资产里是 BLEND_Opaque、alpha 恒 1。它必须写深度，且用自己
        // 的折射/三平面噪声片元函数；把它塞进通用半透 fs_effect 会同时改错颜色与遮挡。
        // **幽火那一族剔正面、留背面。** 每团幽火是「外壳套内壳」两层闭合几何,
        // 实机能同时看到两层:剔掉正面之后,看到的是外壳的**内表面**,而内壳(整体在外壳里面)
        // 的内表面比它更靠近相机,于是照常写深度就压在上面 —— 两层都在、各自不透明、
        // **与三角顺序无关**。判据与证据链见 `pack::MaterialSpec::paint_order`。
        let paint_order_pipeline = make_pipeline(
            "pet-paint-order",
            "vs_main",
            "fs_main",
            Some(wgpu::Face::Front),
            true,
            false,
        );

        let glassy_inner_pipeline = make_pipeline(
            "pet-glassy-inner",
            "vs_main",
            "fs_glassy_inner",
            Some(wgpu::Face::Back),
            true,
            false,
        );

        let all_draws = model
            .primitives
            .iter()
            .map(|p| (p.first_index, p.index_count, p.material));
        // 专用不透明内胆先从普通/混合两路里拿走；否则 `base_color=None` 会让它按纯特效层
        // 自动落进混合通道，与原材质的 BLEND_Opaque 相反。
        let (glassy_inner_draws, remaining): (Vec<_>, Vec<_>) =
            all_draws.partition(|&(_, _, m)| model.materials[m].glassy_inner.is_some());
        let (special_opaque_draws, remaining): (Vec<_>, Vec<_>) = remaining
            .into_iter()
            .partition(|&(_, _, m)| model.materials[m].yutu_ear.is_some());
        let (paint_order_draws, remaining): (Vec<_>, Vec<_>) = remaining
            .into_iter()
            .partition(|&(_, _, m)| model.materials[m].paint_order);
        // 需要混合的最后画(叠在本体之上)。判据是 `blended()` 而不是 `translucent`:
        // 标着 BLEND_Translucent 但不透明度就是 1 的(幽星光那两个球)输出和不透明一样,
        // 放进混合通道只会因为不写深度而互相盖不住 —— 两颗球绕着转就闪。
        let (blended, draws): (Vec<_>, Vec<_>) = remaining
            .into_iter()
            .partition(|&(_, _, m)| model.materials[m].blended());
        // 混合通道里再分两种片元函数:有基色的走 fs_main,纯特效层走 fs_effect
        let (glass_draws, effect_draws): (Vec<_>, Vec<_>) = blended
            .into_iter()
            .partition(|&(_, _, m)| model.materials[m].effect.is_none());
        // **非加色的特效层要画在玻璃层前面。** 混合通道只测深度不写,顺序就是遮挡关系；
        // 这批几何是内层(如春兔耳膜里的液体),外层玻璃应在它之后混合。果冻的内胆已由
        // `glassy_inner_draws` 按原 BLEND_Opaque 单独写深度，不再依赖这条经验排序。
        // **加色层仍然留在最后**:它是打在最上面的光(火花的火焰),而且加色本来就与顺序无关,
        // 挪到玻璃层前面反而会被玻璃的 alpha 衰减一道。
        let (inner_draws, effect_draws): (Vec<_>, Vec<_>) =
            effect_draws.into_iter().partition(|&(_, _, m)| {
                model.materials[m]
                    .effect
                    .as_ref()
                    .is_some_and(|e| !e.additive)
            });
        // **描边是逐材质开的**:游戏的 `Mat/` 目录里除了材质本体还并排放着 `MI_…_Ol`,
        // 有它才画描边(见 `pack::MaterialSpec::outline`)。小灵面身旁那两团幽火、
        // 克莱因龙的液面、春兔耳朵里那泡液体都没有,原来一律画上去是多出来的一圈暗边。
        //
        // **`MI_P_Object_Trans_MatCap` 那一族的半透玻璃件也要画,而且要画在写深度的这一遍。**
        // 幽星光/曜星光/暮星辰的那两颗球(`_Fx1`/`_Fx2`,各自都有 `_Ol`)就是这么成为
        // 实心球的:外扩的背面壳是闭合凸体,从相机看正好铺满整个圆盘,半透的玻璃壳再盖上去。
        //
        // **为什么只有这一族**:它的覆盖率整条链只剩 MatCap 那一路 —— 目标 Low PS 1355 里
        // `alpha = max(基色a重映射, 高光×SpecInt, MatCap.r)`,而实例的 `HighLight SpecInt = 0`、
        // 球那块 UV 的基色 alpha 中位与 p90 **都是 0.000**,`matcap26` 本身又是一张暗图
        // (均值 0.199)。也就是说玻璃壳自己顶多盖住两成,实机看到的那颗**实心红球**
        // 只能来自它背后的描边壳。而同样标着半透、也有 `_Ol` 的暮星辰裙子与春兔耳膜
        // **是画出来的不透明度图**(那块 UV 的基色 alpha 中位 0.537 / 0.378),实机里
        // 确实透得见背景 —— 给它们补一层不透明壳会把耳朵里那泡液体连同背景一起糊掉(试过)。
        let outline_draws: Vec<_> = draws
            .iter()
            .copied()
            .filter(|&(_, _, m)| model.materials[m].outline.unwrap_or(true))
            .chain(glass_draws.iter().copied().filter(|&(_, _, m)| {
                model.materials[m].outline == Some(true) && model.materials[m].matcap.is_some()
            }))
            .collect();
        Ok(Self {
            vertices,
            indices,
            joints,
            joint_capacity,
            camera,
            bounds_vertices,
            face_cards: model.face_cards.clone(),
            bind_bounds,
            frame_bind,
            depth_layout,
            material_binds,
            pipeline,
            outline_pipeline,
            paint_order_pipeline,
            effect_pipeline,
            glass_pipeline,
            glassy_inner_pipeline,
            draws,
            effect_draws,
            glass_draws,
            inner_draws,
            glassy_inner_draws,
            special_opaque_draws,
            outline_draws,
            paint_order_draws,
        })
    }

    /// 上传本帧的相机与蒙皮矩阵。
    pub fn update(&self, queue: &wgpu::Queue, frame: &FrameParams, matrices: &[Mat4]) {
        let object_bounds = self
            .bounds_vertices
            .as_deref()
            .and_then(|vertices| posed_object_bounds(vertices, matrices))
            .unwrap_or(self.bind_bounds);
        queue.write_buffer(
            &self.camera,
            0,
            bytemuck::bytes_of(&CameraUniform {
                view_proj: frame.view_proj.to_cols_array_2d(),
                light_dir: frame.light_dir.normalize().to_array(),
                outline_scale: frame.outline_scale,
                time: frame.time,
                high_material_quality: if frame.high_material_quality {
                    1.0
                } else {
                    0.0
                },
                face_uv: frame.face_uv,
                object_bounds,
                face_card: resolve_face_card(&self.face_cards, frame.face_card) as f32,
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

    /// 给半透明第二遍绑定第一遍生成的场景深度。
    pub fn bind_scene_depth(
        &self,
        device: &wgpu::Device,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pet-scene-depth"),
            layout: &self.depth_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            }],
        })
    }

    /// 第一遍：只画会写深度的材质。半透明外壳随后要采这份场景深度。
    pub fn draw_opaque(&self, pass: &mut wgpu::RenderPass<'_>, outline: bool) {
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.set_bind_group(0, &self.frame_bind, &[]);
        if outline && !self.outline_draws.is_empty() {
            pass.set_pipeline(&self.outline_pipeline);
            for &(first, count, material) in &self.outline_draws {
                pass.set_bind_group(1, &self.material_binds[material], &[]);
                pass.draw_indexed(first..first + count, 0, 0..1);
            }
        }
        pass.set_pipeline(&self.pipeline);
        for &(first, count, material) in &self.draws {
            pass.set_bind_group(1, &self.material_binds[material], &[]);
            pass.draw_indexed(first..first + count, 0, 0..1);
        }
        if !self.glassy_inner_draws.is_empty() {
            pass.set_pipeline(&self.glassy_inner_pipeline);
            for &(first, count, material) in &self.glassy_inner_draws {
                pass.set_bind_group(1, &self.material_binds[material], &[]);
                pass.draw_indexed(first..first + count, 0, 0..1);
            }
        }
        if !self.paint_order_draws.is_empty() {
            pass.set_pipeline(&self.paint_order_pipeline);
            for &(first, count, material) in &self.paint_order_draws {
                pass.set_bind_group(1, &self.material_binds[material], &[]);
                pass.draw_indexed(first..first + count, 0, 0..1);
            }
        }
        if !self.special_opaque_draws.is_empty() {
            pass.set_pipeline(&self.pipeline);
            for &(first, count, material) in &self.special_opaque_draws {
                pass.set_bind_group(1, &self.material_binds[material], &[]);
                pass.draw_indexed(first..first + count, 0, 0..1);
            }
        }
    }

    /// 第二遍：读取场景深度，画只测不写的半透明/加色层。
    pub fn draw_translucent(&self, pass: &mut wgpu::RenderPass<'_>, scene_depth: &wgpu::BindGroup) {
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.set_bind_group(0, &self.frame_bind, &[]);
        pass.set_bind_group(2, scene_depth, &[]);
        for (pipeline, batch) in [
            (&self.effect_pipeline, &self.inner_draws),
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

/// 用与顶点着色器完全相同的线性混合蒙皮计算本帧物体盒。FakeFulid 的 cooked PS
/// 通过 PrimitiveSceneData 读取当前 `ObjectWorldPositionAndRadius/ObjectBounds`，液面
/// 平面以那个中心为原点；这是材质输入，不是为某个模型拟合液位。
fn posed_object_bounds(vertices: &[Vertex], matrices: &[Mat4]) -> Option<[f32; 4]> {
    if vertices.is_empty() || matrices.is_empty() {
        return None;
    }
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for vertex in vertices {
        let total: f32 = vertex.weights.iter().sum();
        let weights = if total > 0.0001 {
            vertex.weights.map(|weight| weight / total)
        } else {
            vertex.weights
        };
        let mut skin = Mat4::ZERO;
        for (slot, weight) in weights.into_iter().enumerate() {
            if weight > 0.0 {
                let joint = vertex.joints[slot] as usize;
                if joint < matrices.len() {
                    skin += matrices[joint] * weight;
                }
            }
        }
        let position = skin.transform_point3(Vec3::from_array(vertex.pos));
        min = min.min(position);
        max = max.max(position);
    }
    if !min.is_finite() || !max.is_finite() {
        return None;
    }
    let center = (min + max) * 0.5;
    Some([center.x, center.y, center.z, (max - min).max_element()])
}

/// 想画的那张表情卡这只有没有;没有就退档(`cards` 是这只真有的卡号,升序)。
///
/// 退档顺序:**先退回 2 号**(网格脸的默认脸,见 `Expression::card`),再退到最小的那张。
/// 卡是按需做的,缺号不少见 —— 觅觅蝠一/三阶没有 1 号、蝴蝶陶陶三阶没有 5 号(困倦),
/// 它睡着时若照着 5 号剔就整张脸都不画了。
/// `cards` 为空(不是网格脸)时原样返回:着色器那条判据本来就不生效。
fn resolve_face_card(cards: &[u32], want: u32) -> u32 {
    if cards.is_empty() || cards.contains(&want) {
        return want;
    }
    if cards.contains(&2) {
        return 2;
    }
    cards[0]
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
///
/// 桌宠只绕 Y 转、画布也一定是正方的,所以这里没有俯仰与宽高比;
/// 网页预览要拖着看,走 [`orbit_view`]。
pub fn orthographic_view(bounds: (Vec3, Vec3), yaw: f32, padding: f32) -> Mat4 {
    orbit_view(bounds, yaw, 0.0, padding, 1.0, Vec3::ZERO)
}

/// 取景半径:包围盒最长边的一半,乘上余量。
///
/// 取最长边而不是对角线:对角线会把瘦高的模型框得过松,宠物在画面里缩成一小团。
/// 单独提出来是因为网页预览要拿它换算「拖一像素等于世界里多远」——**正交投影下
/// 画面高度正好是 `2 * radius`**,两处各写一遍迟早对不上。
pub fn framing_radius(bounds: (Vec3, Vec3), padding: f32) -> f32 {
    let extent = bounds.1 - bounds.0;
    extent.x.max(extent.y).max(extent.z) * 0.5 * padding
}

/// 观察角 → 相机朝向。`pitch` 在这里夹紧,调用方不必自己管。
pub fn orbit_rotation(yaw: f32, pitch: f32) -> glam::Quat {
    glam::Quat::from_rotation_y(yaw)
        * glam::Quat::from_rotation_x(pitch.clamp(-MAX_PITCH, MAX_PITCH))
}

/// 同上,外加**俯仰**与**画布宽高比** —— 网页预览那块 canvas 可以拖、也不一定是正方的。
///
/// `pitch` 正值是从上往下看。**夹在 ±80° 内**:到极点时 `look_at` 的上方向会和视线共线,
/// 矩阵直接退化成一片空白。宽高比只放宽横向,竖向那半径不动,于是不论画布多宽,
/// 宠物在画面里的**高度**是一样的 —— 拖窗口大小时它不会跟着忽大忽小。
///
/// `target` 是**世界坐标里的**轨道中心偏移(网页预览的平移)。存世界坐标而不是屏幕偏移,
/// 是因为平移完再转视角时,被推到一边的宠物应当待在原地,而不是跟着镜头甩。
pub fn orbit_view(
    bounds: (Vec3, Vec3),
    yaw: f32,
    pitch: f32,
    padding: f32,
    aspect: f32,
    target: Vec3,
) -> Mat4 {
    let (min, max) = bounds;
    let center = (min + max) * 0.5 + target;
    let radius = framing_radius(bounds, padding);
    let rotation = orbit_rotation(yaw, pitch);
    let eye = center + rotation * Vec3::new(0.0, 0.0, radius * 2.0);
    let view = glam::camera::rh::view::look_at_mat4(eye, center, Vec3::Y);
    let half_w = radius * aspect.max(0.01);
    // 深度范围用 wgpu 的 0..1(DirectX 约定),与管线的 Depth32Float + CompareFunction::Less 匹配
    let proj = glam::camera::rh::proj::directx::orthographic(
        -half_w,
        half_w,
        -radius,
        radius,
        0.01,
        // 俯仰会把相机推到包围盒的角上,近/远平面要按对角线留够,不然会削掉一块
        radius * 6.0,
    );
    proj * view
}

/// 俯仰的上限(弧度)。差 10° 到极点就停 —— 再上去 `look_at` 就退化了。
pub const MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2 * 8.0 / 9.0;

#[cfg(test)]
mod tests {
    use super::*;

    /// 缺卡要退档,而且不能退成「一张都不画」。
    #[test]
    fn a_missing_face_card_falls_back_instead_of_vanishing() {
        let full: Vec<u32> = (1..=8).collect();
        assert_eq!(resolve_face_card(&full, 5), 5, "有就用它");
        // 蝴蝶陶陶三阶缺 5 号(困倦):退回默认那张
        let no_sleepy = [1, 2, 3, 4, 6, 7, 8];
        assert_eq!(resolve_face_card(&no_sleepy, 5), 2);
        // 觅觅蝠一阶连 1 号都没有,但 2 号在,默认脸照样有
        let no_first = [2, 3, 4, 5, 6, 7, 8];
        assert_eq!(resolve_face_card(&no_first, 2), 2);
        // 连 2 号都没有的极端情况:退到最小的一张,而不是什么都不画
        assert_eq!(resolve_face_card(&[3, 6], 5), 3);
        // 不是网格脸:原样返回(着色器不看这个值)
        assert_eq!(resolve_face_card(&[], 2), 2);
    }

    fn skinned_vertex(pos: [f32; 3], joint: u16) -> Vertex {
        Vertex {
            pos,
            normal: [0.0, 1.0, 0.0],
            uv: [0.0; 2],
            joints: [joint, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
            local_pos: pos,
            color: [1.0; 4],
        }
    }

    /// 拖视角那两条约束:**俯仰要夹住**(到极点 `look_at` 会退化成一片空白),
    /// 而**宽高比只放宽横向** —— 不论画布多宽,宠物在画面里的高度不变。
    #[test]
    fn orbit_clamps_pitch_and_only_widens_horizontally() {
        let bounds = (Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0));
        // 竖直方向的投影比例不受宽高比影响
        let square = orbit_view(bounds, 0.0, 0.0, 1.0, 1.0, Vec3::ZERO);
        let wide = orbit_view(bounds, 0.0, 0.0, 1.0, 2.0, Vec3::ZERO);
        assert!((square.y_axis.y - wide.y_axis.y).abs() < 1e-6, "高度该一样");
        assert!(wide.x_axis.x.abs() < square.x_axis.x.abs(), "横向该放宽");

        // 俯仰给到超过 90° 也不能让矩阵烂掉(NaN / 全零)
        let over = orbit_view(bounds, 0.3, 3.0, 1.0, 1.5, Vec3::ZERO);
        assert!(over.to_cols_array().iter().all(|v| v.is_finite()));
        assert_eq!(
            over,
            orbit_view(bounds, 0.3, MAX_PITCH, 1.0, 1.5, Vec3::ZERO),
            "该夹到上限"
        );

        // 不给俯仰与宽高比时,就是原来那个正方取景
        assert_eq!(
            orthographic_view(bounds, 0.7, 1.15),
            orbit_view(bounds, 0.7, 0.0, 1.15, 1.0, Vec3::ZERO)
        );
    }

    #[test]
    fn posed_bounds_follow_skin_matrices() {
        let vertices = [
            skinned_vertex([-1.0, -2.0, -3.0], 0),
            skinned_vertex([1.0, 2.0, 3.0], 1),
        ];
        let matrices = [
            Mat4::from_translation(Vec3::new(2.0, 3.0, 4.0)),
            Mat4::from_translation(Vec3::new(-2.0, -1.0, 0.0)),
        ];

        assert_eq!(
            posed_object_bounds(&vertices, &matrices),
            Some([0.0, 1.0, 2.0, 2.0])
        );
        assert_eq!(posed_object_bounds(&[], &matrices), None);
    }

    /// 网页预览的缩放没有动相机,而是把取景余量按比例收紧(`web.rs` 里传的是
    /// `PADDING / zoom`)—— 投影是正交的,这么做和「拉近」等价。这条测试钉住那个比例:
    /// 余量减半,同一个点在裁剪空间里就该走到大约两倍远。
    #[test]
    fn tightening_the_padding_makes_the_pet_fill_more_of_the_frame() {
        let bounds = (Vec3::splat(-1.0), Vec3::splat(1.0));
        let ndc_y = |padding: f32| {
            let clip = orbit_view(bounds, 0.0, 0.0, padding, 1.0, Vec3::ZERO)
                * Vec3::new(0.0, 1.0, 0.0).extend(1.0);
            clip.y / clip.w
        };

        let wide = ndc_y(1.15);
        let tight = ndc_y(1.15 / 2.0);
        assert!(
            (tight / wide - 2.0).abs() < 0.01,
            "余量减半应当正好等于放大两倍,实得 {wide} → {tight}"
        );
    }

    /// 平移要**精确跟手**:把轨道中心沿屏幕上方推「一个画面高」(正交下就是 `2 * radius`),
    /// 原来在正中的那个点就该正好落到画面下边缘 —— NDC 里走 2.0。差一点都会表现成
    /// 「拖得比手快 / 比手慢」,而这正是 web.rs 里 `pan` 那个换算的依据。
    #[test]
    fn panning_one_screen_height_moves_the_subject_exactly_one_screen() {
        let bounds = (Vec3::splat(-1.0), Vec3::splat(1.0));
        let padding = 1.15;
        let radius = framing_radius(bounds, padding);
        let ndc_y = |target: Vec3| {
            let clip = orbit_view(bounds, 0.0, 0.0, padding, 1.0, target) * Vec3::ZERO.extend(1.0);
            clip.y / clip.w
        };

        assert!(ndc_y(Vec3::ZERO).abs() < 1e-6, "没平移时中心就在画面正中");
        let one_screen = ndc_y(Vec3::Y * 2.0 * radius);
        assert!(
            (one_screen + 2.0).abs() < 1e-5,
            "中心上移一个画面高,画面里那个点就该反向走过整整一屏(NDC 满程 2.0),实得 {one_screen}"
        );
    }
}
