//! 读宠物包里的 glb:网格、骨架、动画、材质。
//!
//! 包是导出器(exporter/)产出的:一个 glb 里装着「网格 + 蒙皮 + 全部逻辑动作」,
//! 贴图独立成 PNG 放在 `tex/`,**哪个材质画哪张贴图由 manifest 的 `[forms.materials]` 指定**
//! (导出器从游戏材质实例里解出来,见 docs/design.md §1、§4.3)。
//! 这里只做加载与整形,不碰 GPU。

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use glam::{Mat4, Quat, Vec3};

use super::anim::Pose;
use crate::pack::Material as PackMaterial;

/// 顶点布局:位置/法线/UV/关节索引/权重/顶点色。与 pet.wgsl 的 `@location` 一一对应。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub joints: [u16; 4],
    pub weights: [f32; 4],
    /// 预蒙皮局部位置。玻璃内部/果冻内胆的折射起点都取它。
    ///
    /// 这里有配套顶点 shader 的直接证据，不再靠 `TEXCOORDn` 名字猜 UV 集：
    /// `M_ShuiMu_ByIn` 的 VS 21175 把解码后的局部位置写到 `o2.xy/o3.x`，PS 71636
    /// 原样读成起点；果冻外壳的 VS 31053 与 PS 68869 也是同一组写入/读取。
    /// 后三项才是输入法线 `ATTRIBUTE2.xyz`。旧代码把这些打包插值槽误认成 UV1/UV2。
    ///
    /// **注意这一层离对上实机还很远**:幽星光那两颗球实机是「红球 + 居中的大号黄色四角星
    /// /圆点」,而我们两种起点画出来都只是几点很淡的紫色斑 —— 换回 UV1/UV2 那版连斑都没有。
    /// 见 docs/design.md 的待办表。
    pub local_pos: [f32; 3],
    /// 游戏材质直接读取的 `COLOR_0`。小灵面 `M_P_Object_XiaoYou` 用
    /// `R*G*(1-A)` 控制流光/星点覆盖；兔耳液体用 R、FakeFluid 用 G。
    /// 以前加载器完全丢弃它，三个独立材质族都会因此失去身体或液面。
    pub color: [f32; 4],
}

/// 一段网格:对应一个材质槽(宠物一般 2–3 个:本体/眼/嘴)。
pub struct Primitive {
    pub first_index: u32,
    pub index_count: u32,
    pub material: usize,
}

pub struct Material {
    pub name: String,
    /// 脸(眼/嘴)那两个槽 —— 表情图集就贴在它们身上,见 pack.rs 的 `Material::face`。
    pub face: bool,
    /// 基色贴图(RGBA8),路径来自 manifest 的材质表;读失败才是 None,渲染时用白色兜底。
    pub base_color: Option<Image>,
    /// 贴图 alpha 是**镂空遮罩**(眼/嘴的表情图集)还是**线条遮罩**(本体的纹路)。
    pub cutout: bool,
    /// alpha 里是否真的有线条信息(见 `alpha_has_detail`);否则提亮要关掉。
    pub line_detail: bool,
    /// 材质标了 `BLEND_Translucent`:要叠 MatCap 高光、边缘光按混色算，且和 UE 一样
    /// 留在不写深度的混合通道。即使参数 Opacity 恰好为 1，也不能据此改成不透明材质：
    /// 莫比乌乌的外壳会挡住先画的内层液体。
    pub translucent: bool,
    /// 见 `pack::MaterialSpec::outline`。
    pub outline: Option<bool>,
    /// 见 `pack::MaterialSpec::paint_order`。
    pub paint_order: bool,
    pub opacity: f32,
    /// 星点 / MatCap 两张附加贴图与它们的着色,以及边缘光。
    pub star: Option<Image>,
    pub star_tiling: [f32; 2],
    /// 星点层来自「假半透」族:着色用 `star_color`(= `Color02`),不是四段渐变
    pub star_fake_trans: bool,
    pub star_color: [f32; 3],
    /// 星点层强度(`Stick_Intensity`)。
    pub stick_intensity: f32,
    pub matcap: Option<Image>,
    pub matcap_color: [f32; 3],
    pub rim_color: [f32; 3],
    pub rim_intensity: f32,
    /// 自发光色(线性)+ 强度;强度 0 = 不画。见 pack.rs。
    pub emissive: [f32; 3],
    pub emissive_intensity: f32,
    pub rim_power: f32,
    pub rim_soft_edge: f32,
    pub highlight_offset: [f32; 3],
    pub highlight_color: [f32; 3],
    pub highlight_power: f32,
    pub highlight_intensity: f32,
    pub force_default_opacity: f32,
    /// `M_P_Object_Trans` 的场景深度淡化:[距离(米),开启强度]。
    pub depth_fade: [f32; 2],
    /// 目标实机 ES3.1/Low、精确父材质 `MI_P_Object_Trans` 的局部着色输入。
    pub object_trans_low: bool,
    pub light_mask: Option<Image>,
    pub ramp: Option<Image>,
    pub object_trans_soft_edge: f32,
    pub main_color: [f32; 3],
    pub main_bright: f32,
    /// 假半透族星点层:[速度X, 速度Y, 强度, 是否用 UV0]。
    pub noise_uv: [f32; 4],
    /// 基色 alpha 是不透明度(见 `pack::Material::alpha_opacity`)。
    pub alpha_opacity: bool,
    /// 卷动色带:渐变图 + [u速度, v速度, u平铺, v平铺] + 混入强度。
    pub flow: Option<Image>,
    pub flow_uv: [f32; 4],
    pub flow_power: f32,
    /// 色带的 ID 遮罩与取值区间(见 `pack::Material::mask_id`)。
    pub mask_id: Option<Image>,
    pub mask_id_range: [f32; 2],
    /// 玻璃内部那颗星:四角星场贴图 + 着色 + 折射率 + march 深度。
    pub interior: Option<Image>,
    pub interior_color: [f32; 3],
    pub refraction: f32,
    /// march 深度(`GlobalDepth`)与闪烁 [速度, 次数];量纲见 `pack::Material`。
    pub refract_depth: f32,
    pub flicker: [f32; 2],
    /// `M_ShuiMu_ByIn` 的专用局部材质链；`noise.z` 保留资源中的
    /// `GlassyNoiseRefract` 原值，GPU 按 uniform preshader 求折射 eta。
    pub glassy_inner: Option<GlassyInner>,
    /// `MI_P_Object_XiaoYou` 的不透明专用材质链。
    pub xiaoyou: Option<XiaoYou>,
    /// `M_Gra_Yutu_Ear_Lighting` 的不透明内层液体。
    pub yutu_ear: Option<YutuEar>,
    /// `M_P_FakeFulid` 的半透明玻璃/液面。
    pub fake_fluid: Option<FakeFluid>,
    /// `M_P_MatCap_Masked` 的不透明 MatCap 外壳。
    pub matcap_masked: Option<MatcapMasked>,
    /// 特效层的画法(火焰/水壳/光晕)。`None` = 普通不透明材质,走主通道。
    pub effect: Option<EffectMaterial>,
}

impl Material {
    /// 这一片是否需要在不透明层之后混合（并保持不写深度）。纯特效层、UE 标记为
    /// `BLEND_Translucent` 的材质、以基色 alpha 为不透明度的材质，以及 FakeFluid 都要。
    pub fn blended(&self) -> bool {
        // UE 的 BLEND_Translucent 无论材质参数里的 Opacity 是否恰好为 1，都不写深度。
        // 内层液体必须先画、外层玻璃随后混合；把 opacity=1 的玻璃改进不透明通道会直接
        // 挡掉莫比乌乌的 Fx1。这是混合模式语义，不是按宠物做排序特判。
        self.effect.is_some() || self.translucent || self.alpha_opacity || self.fake_fluid.is_some()
    }
}

/// 特效层要用的东西:主色 + 卷动 + 遮罩/噪声贴图。参数解释见 `pack::Effect`。
pub struct EffectMaterial {
    pub tint: [f32; 4],
    pub opacity: f32,
    pub glow: f32,
    pub flow: [f32; 4],
    pub additive: bool,
    pub mask_matcap: bool,
    pub mask: Option<Image>,
    pub noise: Option<Image>,
}

#[derive(Clone, Copy)]
pub struct GlassyInner {
    pub flow1: [f32; 4],
    pub flow2: [f32; 4],
    pub fresnel: [f32; 4],
    /// [速度, UV 尺度, 折射率, 深度]
    pub noise: [f32; 4],
    /// [Fresnel 次数, 阈值起点, 过渡宽度, 三向混合强度]
    pub mask: [f32; 4],
}

pub struct XiaoYou {
    /// 目标 PS 的 t3；MainTex 与 StarTex 分别复用材质的 base_color / star。
    pub noise: Option<Image>,
    pub base1: [f32; 4],
    pub base2: [f32; 4],
    pub flow1: [f32; 4],
    pub flow2: [f32; 4],
    pub star_color: [f32; 4],
    pub noise_flow: [f32; 4],
    pub shape: [f32; 4],
    pub star_uv: [f32; 4],
}

pub struct YutuEar {
    pub bubble: Option<Image>,
    pub distort: Option<Image>,
    pub flow: Option<Image>,
    pub bubble_color: [f32; 4],
    pub flow_color: [f32; 4],
    pub fresnel_color: [f32; 4],
    pub inner_color: [f32; 4],
    pub overall_color: [f32; 4],
    pub ramp_color: [f32; 4],
    pub top_color: [f32; 4],
    pub bubble_shape: [f32; 4],
    pub flow_shape: [f32; 4],
    pub light_shape: [f32; 4],
    pub top_shape: [f32; 4],
}

pub struct FakeFluid {
    /// 目标 PS 的 t2/t3；FuildMask 是 sRGB 颜色资源，LUT 是线性数据资源。
    pub mask: Option<Image>,
    pub lut: Option<Image>,
    pub edge_color: [f32; 4],
    pub fresnel_color: [f32; 4],
    pub plane_color: [f32; 4],
    pub gradient1: [f32; 4],
    pub gradient2: [f32; 4],
    pub height_tiling: [f32; 4],
    pub plane_axis: [f32; 4],
    pub plane_center: [f32; 4],
    pub body_shape: [f32; 4],
    pub gradient_shape: [f32; 4],
    pub top_shape: [f32; 4],
}

pub struct MatcapMasked {
    pub matcap: Option<Image>,
    pub base_color: [f32; 4],
    pub light_ramp: [f32; 4],
    pub flat_emissive: [f32; 4],
    pub main_color: [f32; 4],
    pub selection_color: [f32; 4],
    pub rim_shape: [f32; 4],
    pub surface_shape: [f32; 4],
}

pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// 节点局部变换。分量存 TRS 而不是矩阵,因为动画混合要在 TRS 上做。
#[derive(Clone, Copy)]
pub struct Trs {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Trs {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

pub struct Skeleton {
    /// 每个节点的绑定局部变换。
    pub bind: Vec<Trs>,
    pub parents: Vec<i32>,
    /// 骨架根关节所在的节点:走跑动画的位移就挂在它上面(见 docs/spike-s3.md),
    /// 运行时要把这份位移剥掉,改由程序推进屏幕坐标,否则宠物会「走两份」。
    pub root_joint: usize,
    /// 拓扑序(父一定排在子之前),算世界变换时按这个顺序一遍过。
    pub order: Vec<usize>,
    /// skin.joints:关节序号 → 节点索引。
    pub joints: Vec<usize>,
    pub inverse_bind: Vec<Mat4>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Property {
    Translation,
    Rotation,
    Scale,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Interpolation {
    Linear,
    Step,
}

pub struct Channel {
    pub node: usize,
    pub property: Property,
    pub interpolation: Interpolation,
    pub times: Vec<f32>,
    /// 平移/缩放用前三个分量,旋转用四个。
    pub values: Vec<[f32; 4]>,
}

pub struct Clip {
    pub name: String,
    pub duration: f32,
    pub channels: Vec<Channel>,
}

pub struct Model {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub primitives: Vec<Primitive>,
    pub materials: Vec<Material>,
    pub skeleton: Skeleton,
    pub clips: Vec<Clip>,
    /// 绑定姿势的包围盒(米)。**只用来换算屏幕尺寸**(`height_cm` 对应的就是这个高度),
    /// 站姿高度必须稳定,不能跟着动作变。
    pub bounds: (Vec3, Vec3),
    /// 这份模型是从哪个 glb 读来的。**当缓存键用**:多实体共享网格与 GPU 资源时,
    /// 按 (包, 形态) 索引就是按它索引(见 wayland.rs 的 `models` / `pet_gpus`)。
    pub source: std::path::PathBuf,
    /// 把各动作采样一遍取到的包围盒,`bounds` 的超集。**画布与相机取景用这个**:
    /// 伸手、张翅、小跳的姿势会明显超出绑定姿势,只按 `bounds` 取景会把肢体裁掉。
    /// 实测(120 个抽样形态 × Idle/Happy/Show/Walk 各查一次):按绑定盒取景 11 个被裁
    /// (阿米亚特/波波拉肉眼可见)→ 按这个盒子取景剩 1 个;代价是画布面积平均 1.64 倍。
    pub motion_bounds: (Vec3, Vec3),
}

impl Model {
    /// `materials_spec` 是 manifest 的 `[forms.materials]`:glb 材质名 → 该画什么
    /// (基色贴图、alpha 语义)。**这是唯一的贴图来源**,导出器从游戏材质实例里解出来,
    /// 不再按贴图命名约定猜(猜法错 258 处,见 docs/design.md §1)。
    pub fn load(glb_path: &Path, materials_spec: &HashMap<String, PackMaterial>) -> Result<Self> {
        if materials_spec.is_empty() {
            bail!("{glb_path:?} 所属的包没有 [forms.materials](旧版导出的包),重导一次");
        }
        // 走 assets:包可能是解开的目录,也可能是一个 .rkpet(那时这条路径是虚拟的)
        let bytes = crate::assets::read(glb_path)?;
        let (doc, buffers, _images) =
            gltf::import_slice(&bytes).with_context(|| format!("解析 {glb_path:?} 失败"))?;
        let get = |b: gltf::Buffer| Some(buffers[b.index()].0.as_slice());

        // ── 骨架 ────────────────────────────────────────────────────
        let skin = doc
            .skins()
            .next()
            .context("glb 里没有 skin(不是蒙皮网格?)")?;
        let node_count = doc.nodes().count();
        let mut bind = vec![Trs::IDENTITY; node_count];
        let mut parents = vec![-1i32; node_count];
        for node in doc.nodes() {
            let (t, r, s) = node.transform().decomposed();
            bind[node.index()] = Trs {
                translation: Vec3::from(t),
                rotation: Quat::from_array(r),
                scale: Vec3::from(s),
            };
            for child in node.children() {
                parents[child.index()] = node.index() as i32;
            }
        }
        let order = topological_order(&parents);
        let joints: Vec<usize> = skin.joints().map(|j| j.index()).collect();
        let inverse_bind = match skin.reader(get).read_inverse_bind_matrices() {
            Some(iter) => iter.map(|m| Mat4::from_cols_array_2d(&m)).collect(),
            // 规范允许省略,此时视为单位矩阵
            None => vec![Mat4::IDENTITY; joints.len()],
        };
        if inverse_bind.len() != joints.len() {
            bail!(
                "inverseBindMatrices 数量({})与关节数({})不符",
                inverse_bind.len(),
                joints.len()
            );
        }
        // 根关节 = 父节点不在关节集合里的那个(通常就是 joints[0])
        let joint_set: std::collections::HashSet<usize> = joints.iter().copied().collect();
        let root_joint = joints
            .iter()
            .copied()
            .find(|&node| match parents[node] {
                -1 => true,
                parent => !joint_set.contains(&(parent as usize)),
            })
            .unwrap_or_else(|| joints.first().copied().unwrap_or(0));
        let skeleton = Skeleton {
            bind,
            parents,
            order,
            joints,
            inverse_bind,
            root_joint,
        };

        // ── 网格 ────────────────────────────────────────────────────
        // 蒙皮网格节点自身的变换按 glTF 规范忽略(蒙皮结果已在骨架空间)
        let mesh = doc
            .nodes()
            .find(|n| n.mesh().is_some() && n.skin().is_some())
            .and_then(|n| n.mesh())
            .context("找不到带 skin 的网格节点")?;

        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut primitives = Vec::new();
        let mut materials: Vec<Material> = Vec::new();
        let mut material_index = HashMap::new();

        for primitive in mesh.primitives() {
            let material_name = primitive
                .material()
                .name()
                .unwrap_or("material")
                .to_string();
            // 键在 Pack::load 里统一成了小写,见那边的说明
            let Some(spec) = materials_spec.get(&material_name.to_ascii_lowercase()) else {
                // manifest 与 glb 出自同一次导出,对不上就是包坏了;宁可少画一片也不猜
                log::warn!("材质 {material_name} 不在 manifest 的材质表里,跳过这一片");
                continue;
            };
            // 没有基色 = 纯特效层(火焰/水壳/光晕),不跳过了:走加色/半透的特效通道,
            // 主色与卷动参数都在材质里,见 `pack::Effect`。
            let reader = primitive.reader(get);
            let positions: Vec<[f32; 3]> =
                reader.read_positions().context("缺 POSITION")?.collect();
            let normals: Vec<[f32; 3]> = reader
                .read_normals()
                .map(|it| it.collect())
                .unwrap_or_else(|| vec![[0.0, 1.0, 0.0]; positions.len()]);
            let uvs: Vec<[f32; 2]> = reader
                .read_tex_coords(0)
                .map(|it| it.into_f32().collect())
                .unwrap_or_else(|| vec![[0.0, 0.0]; positions.len()]);
            let source_colors = reader.read_colors(0);
            let has_source_colors = source_colors.is_some();
            let mut colors: Vec<[f32; 4]> = source_colors
                .map(|it| it.into_rgba_f32().collect())
                // UE 顶点工厂在网格没有颜色缓冲时提供白色常量；这也是目标 PS 实际读到的值。
                .unwrap_or_else(|| vec![[1.0; 4]; positions.len()]);
            if has_source_colors {
                // 当前 CUE4Parse glTF 导出器把 FColor 的隐式 [0,1] 转换又除了一次 255。
                // 因而本应为 1 的通道在 GLB 中只有 1/255；这些通道正是原材质用来控制
                // XiaoYou/FakeFluid 等效果的顶点遮罩。只识别这种明确的特征，兼容早期
                // 已正确导出的包（其最大值会显著大于 1/255）。
                let max = colors
                    .iter()
                    .flat_map(|c| c.iter())
                    .copied()
                    .fold(0.0f32, f32::max);
                if max == 0.0 && (spec.yutu_ear.is_some() || spec.fake_fluid.is_some()) {
                    // 旧版 CUE4Parse 把“没有颜色缓冲”导成了显式全黑 COLOR_0；只在确认会读取
                    // 该通道的两个原生材质族上恢复 UE 的白色 vertex-factory 默认值。
                    colors.fill([1.0; 4]);
                } else if max > 0.0 && max <= 1.0 / 255.0 + 1.0e-6 {
                    for color in &mut colors {
                        for channel in color {
                            *channel = (*channel * 255.0).min(1.0);
                        }
                    }
                }
            }
            let joint_ids: Vec<[u16; 4]> = reader
                .read_joints(0)
                .context("缺 JOINTS_0")?
                .into_u16()
                .collect();
            let weights: Vec<[f32; 4]> = reader
                .read_weights(0)
                .context("缺 WEIGHTS_0")?
                .into_f32()
                .collect();

            let base = vertices.len() as u32;
            for i in 0..positions.len() {
                vertices.push(Vertex {
                    pos: positions[i],
                    normal: normals[i],
                    uv: uvs[i],
                    joints: joint_ids[i],
                    weights: weights[i],
                    local_pos: positions[i],
                    color: colors[i],
                });
            }
            let first_index = indices.len() as u32;
            let prim_indices: Vec<u32> = match reader.read_indices() {
                Some(it) => it.into_u32().map(|i| i + base).collect(),
                None => (0..positions.len() as u32).map(|i| i + base).collect(),
            };
            let index_count = prim_indices.len() as u32;
            indices.extend(prim_indices);

            let name = material_name;
            let material = *material_index.entry(name.clone()).or_insert_with(|| {
                let base_color = spec
                    .base_color
                    .as_deref()
                    .and_then(|path| load_texture(path, spec.mask_alpha));
                // 只有 alpha 真的有高低之分才启用纹路提亮(要在 base_color 被移动前算)
                let line_detail = base_color.as_ref().is_some_and(alpha_has_detail);
                // 特效层的遮罩/噪声贴图 alpha 原样保留:形状全靠它
                let effect = (spec.base_color.is_none()
                    && spec.yutu_ear.is_none()
                    && spec.fake_fluid.is_none()
                    && spec.matcap_masked.is_none())
                .then(|| EffectMaterial {
                    tint: spec.effect.tint,
                    opacity: spec.effect.opacity,
                    glow: spec.effect.glow,
                    flow: spec.effect.flow,
                    additive: spec.effect.additive(),
                    mask_matcap: spec.effect.mask_matcap,
                    mask: spec
                        .effect
                        .mask
                        .as_deref()
                        .and_then(|p| load_texture(p, true)),
                    noise: spec
                        .effect
                        .noise
                        .as_deref()
                        .and_then(|p| load_texture(p, true)),
                });
                materials.push(Material {
                    name: name.clone(),
                    base_color,
                    face: spec.face,
                    cutout: spec.mask_alpha,
                    line_detail,
                    translucent: spec.translucent,
                    outline: spec.outline,
                    paint_order: spec.paint_order,
                    opacity: spec.opacity,
                    // 星点/matcap 的 alpha 原样保留:形状全在 alpha 里
                    star: spec.star.as_deref().and_then(|p| load_texture(p, true)),
                    star_tiling: spec.star_tiling,
                    star_fake_trans: spec.star_fake_trans,
                    star_color: spec.star_color,
                    stick_intensity: spec.stick_intensity,
                    matcap: spec.matcap.as_deref().and_then(|p| load_texture(p, true)),
                    matcap_color: spec.matcap_color,
                    rim_color: spec.rim_color,
                    rim_intensity: spec.rim_intensity,
                    emissive: spec.emissive,
                    emissive_intensity: spec.emissive_intensity,
                    rim_power: spec.rim_power,
                    rim_soft_edge: spec.rim_soft_edge,
                    highlight_offset: spec.highlight_offset,
                    highlight_color: spec.highlight_color,
                    highlight_power: spec.highlight_power,
                    highlight_intensity: spec.highlight_intensity,
                    force_default_opacity: spec.force_default_opacity,
                    depth_fade: [spec.opacity_depth_distance * 0.01, spec.open_depth_distance],
                    object_trans_low: spec.object_trans_low,
                    // MaskTex 是数据贴图，保持线性字节；RampTex/BaseTex 是颜色贴图，
                    // shader 会按游戏原采样链显式做 sRGB 解码。
                    light_mask: spec
                        .light_mask
                        .as_deref()
                        .and_then(|p| load_texture(p, true)),
                    ramp: spec.ramp.as_deref().and_then(|p| load_texture(p, true)),
                    object_trans_soft_edge: spec.object_trans_soft_edge,
                    main_color: spec.main_color,
                    main_bright: spec.main_bright,
                    noise_uv: spec.noise_uv,
                    alpha_opacity: spec.alpha_opacity,
                    flow: spec.flow.as_deref().and_then(|p| load_texture(p, true)),
                    flow_uv: spec.flow_uv,
                    flow_power: spec.flow_power,
                    mask_id: spec.mask_id.as_deref().and_then(|p| load_texture(p, true)),
                    mask_id_range: spec.mask_id_range,
                    interior: spec.interior.as_deref().and_then(|p| load_texture(p, true)),
                    interior_color: spec.interior_color,
                    refraction: spec.refraction,
                    refract_depth: spec.refract_depth,
                    flicker: spec.flicker,
                    glassy_inner: spec.glassy_inner.as_ref().map(|g| GlassyInner {
                        flow1: g.flow1,
                        flow2: g.flow2,
                        fresnel: g.fresnel,
                        noise: g.noise,
                        mask: g.mask,
                    }),
                    xiaoyou: spec.xiaoyou.as_ref().map(|x| XiaoYou {
                        noise: spec
                            .effect
                            .noise
                            .as_deref()
                            .and_then(|p| load_texture(p, true)),
                        base1: x.base1,
                        base2: x.base2,
                        flow1: x.flow1,
                        flow2: x.flow2,
                        star_color: x.star_color,
                        noise_flow: x.noise_flow,
                        shape: x.shape,
                        star_uv: x.star_uv,
                    }),
                    yutu_ear: spec.yutu_ear.as_ref().map(|y| YutuEar {
                        bubble: y.bubble.as_deref().and_then(|p| load_texture(p, true)),
                        distort: y.distort.as_deref().and_then(|p| load_texture(p, true)),
                        flow: y.flow.as_deref().and_then(|p| load_texture(p, true)),
                        bubble_color: y.bubble_color,
                        flow_color: y.flow_color,
                        fresnel_color: y.fresnel_color,
                        inner_color: y.inner_color,
                        overall_color: y.overall_color,
                        ramp_color: y.ramp_color,
                        top_color: y.top_color,
                        bubble_shape: y.bubble_shape,
                        flow_shape: y.flow_shape,
                        light_shape: y.light_shape,
                        top_shape: y.top_shape,
                    }),
                    fake_fluid: spec.fake_fluid.as_ref().map(|f| FakeFluid {
                        mask: spec
                            .effect
                            .mask
                            .as_deref()
                            .and_then(|p| load_texture(p, true)),
                        lut: spec
                            .effect
                            .noise
                            .as_deref()
                            .and_then(|p| load_texture(p, true)),
                        edge_color: f.edge_color,
                        fresnel_color: f.fresnel_color,
                        plane_color: f.plane_color,
                        gradient1: f.gradient1,
                        gradient2: f.gradient2,
                        height_tiling: f.height_tiling,
                        plane_axis: f.plane_axis,
                        plane_center: f.plane_center,
                        body_shape: f.body_shape,
                        gradient_shape: f.gradient_shape,
                        top_shape: f.top_shape,
                    }),
                    matcap_masked: spec.matcap_masked.as_ref().map(|m| MatcapMasked {
                        matcap: m.matcap.as_deref().and_then(|p| load_texture(p, true)),
                        base_color: m.base_color,
                        light_ramp: m.light_ramp,
                        flat_emissive: m.flat_emissive,
                        main_color: m.main_color,
                        selection_color: m.selection_color,
                        rim_shape: m.rim_shape,
                        surface_shape: m.surface_shape,
                    }),
                    effect,
                });
                materials.len() - 1
            });
            primitives.push(Primitive {
                first_index,
                index_count,
                material,
            });
        }

        // ── 动画 ────────────────────────────────────────────────────
        let mut clips = Vec::new();
        // 被 `clamp_scale` 削掉的缩放关键帧数,整份模型统计一次(见那个函数的说明)
        let mut clamped = 0usize;
        for animation in doc.animations() {
            let mut channels = Vec::new();
            let mut duration = 0.0f32;
            for channel in animation.channels() {
                let reader = channel.reader(get);
                let times: Vec<f32> = reader.read_inputs().context("动画通道缺时间轴")?.collect();
                if let Some(&last) = times.last() {
                    duration = duration.max(last);
                }
                let interpolation = match channel.sampler().interpolation() {
                    gltf::animation::Interpolation::Step => Interpolation::Step,
                    // CubicSpline 我们不产出,退化成线性即可(退化只影响手工做的包)
                    _ => Interpolation::Linear,
                };
                use gltf::animation::util::ReadOutputs;
                let (property, values) = match reader.read_outputs().context("动画通道缺值")?
                {
                    ReadOutputs::Translations(it) => (
                        Property::Translation,
                        it.map(|v| [v[0], v[1], v[2], 0.0]).collect::<Vec<_>>(),
                    ),
                    ReadOutputs::Rotations(it) => {
                        (Property::Rotation, it.into_f32().collect::<Vec<_>>())
                    }
                    ReadOutputs::Scales(it) => (
                        Property::Scale,
                        it.map(|v| {
                            let (key, hit) = clamp_scale(v);
                            clamped += usize::from(hit);
                            key
                        })
                        .collect::<Vec<_>>(),
                    ),
                    ReadOutputs::MorphTargetWeights(_) => continue, // 形变目标先不做
                };
                channels.push(Channel {
                    node: channel.target().node().index(),
                    property,
                    interpolation,
                    times,
                    values,
                });
            }
            clips.push(Clip {
                name: animation.name().unwrap_or("(未命名)").to_string(),
                duration,
                channels,
            });
        }

        if vertices.is_empty() {
            // 曾经的表现是 wgpu 深处 panic「buffer slice can not be empty」,查半天才定位到
            // 材质名大小写对不上。这里直接说清楚。
            bail!(
                "{glb_path:?} 一片网格都没留下:{} 个材质全被跳过(材质表里查不到,或全是特效层)",
                mesh.primitives().len()
            );
        }

        // 自转的玻璃小件要压成平色(见 `flatten_spinning_parts`)。放在动画解析之后:
        // 判据要看骨骼在动作里到底转了多少。
        let spinning = spinning_joints(&skeleton, &clips);
        for prim in &primitives {
            if !materials[prim.material].translucent {
                continue;
            }
            let range = prim.first_index as usize..(prim.first_index + prim.index_count) as usize;
            flatten_spinning_parts(
                &mut vertices,
                &indices[range],
                materials[prim.material].base_color.as_ref(),
                &spinning,
            );
        }

        if clamped > 0 {
            log::debug!("{glb_path:?}:削掉 {clamped} 个过大的缩放关键帧");
        }

        let bounds = bind_pose_bounds(&vertices, &skeleton);
        let motion_bounds = animated_bounds(&vertices, &skeleton, &clips, bounds);
        Ok(Self {
            vertices,
            indices,
            primitives,
            materials,
            skeleton,
            clips,
            source: glb_path.to_path_buf(),
            bounds,
            motion_bounds,
        })
    }

    pub fn clip(&self, name: &str) -> Option<usize> {
        self.clips.iter().position(|c| c.name == name)
    }
}

/// 单根骨骼的缩放上限。
///
/// 这些骨架真的会「拉伸」——卡通挤压是动画的一部分,不能一律按 1.0 处理。但**孤立的
/// 坏帧**也真的有:喵喵 `Shock` 里脊椎的 X 缩放有一帧冲到 **4.90**(前后是 1.26 与 2.91),
/// 脖子同一段冲到 2.99。那不是画出来的 —— 同一条通道其余每一帧都满足 Y≡Z
/// (沿骨轴拉伸、径向等比挤压),偏偏这几帧不等,是压缩动画解出来的坏值。
///
/// 后果是看得见的:被点一下受惊时,喵喵的头会瞬间蹿出画布(取景盒按
/// [`MAX_POSE_GROWTH`] 筛过姿势,本来就装不下这种),用户报的就是这个。
///
/// 阈值 2.0 是量出来的:本地全部包(24 个形态、约 63 万个缩放分量)里 **99.35%**
/// 落在 [0.5, 2.0] 内,越界的集中在 Shock / CallOut / Relax 这几段的孤立帧上;
/// Idle 1.09、Alert 1.49、Sad 1.61、Anger 1.95 这些正常挤压全在阈值内,不受影响。
///
/// **只夹上限。** 往小了缩是有意义的 —— 动画师会把骨骼缩到接近 0 来藏部件,
/// 夹下限反而会让本该藏起来的东西冒出来。
const MAX_BONE_SCALE: f32 = 2.0;

/// 把一帧缩放夹进上限,返回(关键帧, 有没有动过)。第四个分量是填充,通道只用前三个。
fn clamp_scale(v: [f32; 3]) -> ([f32; 4], bool) {
    let key = [
        v[0].min(MAX_BONE_SCALE),
        v[1].min(MAX_BONE_SCALE),
        v[2].min(MAX_BONE_SCALE),
        0.0,
    ];
    (key, key[..3] != v[..])
}

/// 按材质表给的路径读基色贴图。/// 按材质表给的路径读基色贴图。**alpha 原样保留**,怎么解释交给 shader:
///
/// - 眼/嘴的表情图集:alpha 是真遮罩,按阈值剔(`mask_alpha = true`);
/// - 本体:alpha 是**线条/细节遮罩**——RGB 是完整的固有色图集,alpha 里画着身上的纹路
///   (水灵身上那一道道竖向浅色条纹就在 alpha 里,白线正好压在纹路上)。
///   曾经把它刷成 255「省事」,结果纹路全丢;拿它当不透明度剔像素更糟,身体会被啃掉。
fn load_texture(path: &Path, _mask_alpha: bool) -> Option<Image> {
    // 不用 `image::open`:包可能是 .rkpet,路径在文件系统里不存在(见 assets.rs)
    let bytes = match crate::assets::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            log::warn!("贴图读取失败: {e:#}");
            return None;
        }
    };
    let img = match image::load_from_memory(&bytes) {
        Ok(img) => img,
        Err(e) => {
            log::warn!("贴图 {path:?} 解不开: {e}");
            return None;
        }
    };
    let rgba = img.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    let rgba = rgba.into_raw();
    Some(Image {
        width,
        height,
        rgba,
    })
}

/// alpha 里到底有没有「线条」信息。
///
/// **不是每张本体贴图都有纹路层**:实测喵喵/鸭吉吉/治愈兔/大耳帽兜的 `By_D` alpha
/// 恒等于 1(100% 覆盖),那就没有任何线条可言;而水灵是 23% 覆盖,白线压在竖条纹上。
/// alpha 恒定时若还照着它提亮,等于把**整只宠物均匀调亮**——雪影娃娃就是这么被冲淡的。
/// 判据:alpha 要真的有高低之分,过高或过低的覆盖率都当「没信息」。
fn alpha_has_detail(image: &Image) -> bool {
    let total = image.rgba.len() / 4;
    if total == 0 {
        return false;
    }
    let high = image.rgba.chunks_exact(4).filter(|p| p[3] > 128).count();
    let share = high as f32 / total as f32;
    (0.02..0.90).contains(&share)
}

/// 哪些关节在动作里**沿一个方向转圈**(而不是来回摆)。
///
/// 判据是**净转动量**:把相邻两帧的相对旋转写成「轴 × 角」向量累加起来。一直朝一个方向转的
/// 会越加越大(幽星光那两颗球一个 Idle 净转 700° 以上),来回摆的正负相消、加不起来。
/// 不能用「转角绝对值累计」——翅膀扇十几个动作也能累到几千度,实测圣羽翼王会误判 71 件。
fn spinning_joints(skeleton: &Skeleton, clips: &[Clip]) -> std::collections::HashSet<u16> {
    /// 一整圈:低于这个的都算摆动。
    const SPIN_DEGREES: f32 = 360.0;
    let mut net: HashMap<usize, f32> = HashMap::new();
    for clip in clips {
        for channel in &clip.channels {
            if channel.property != Property::Rotation {
                continue;
            }
            let winding: Vec3 = channel
                .values
                .windows(2)
                .map(|w| {
                    let (from, to) = (Quat::from_array(w[0]), Quat::from_array(w[1]));
                    // q 与 -q 是同一朝向:先对齐符号,否则相对旋转会莫名多出半圈
                    let to = if from.dot(to) < 0.0 { -to } else { to };
                    let (axis, angle) = (to * from.inverse()).to_axis_angle();
                    axis * angle
                })
                .sum();
            let slot = net.entry(channel.node).or_insert(0.0);
            *slot = slot.max(winding.length().to_degrees());
        }
    }
    skeleton
        .joints
        .iter()
        .enumerate()
        .filter(|(_, node)| net.get(node).copied().unwrap_or(0.0) >= SPIN_DEGREES)
        .map(|(joint, _)| joint as u16)
        .collect()
}

/// 把「自转的玻璃小件」的 UV 钉成一点,于是整件一片平色。
///
/// **这是为了治那种「转起来在闪」。** 玻璃族里有一批小球是单骨骼刚体、还在动作里自转
/// (幽星光那两颗球一个 Idle 转两圈),而它们在基色图集里的 UV 落脚处横跨好几块**不相干**的
/// 色块 —— 实测幽星光球 A 那一片里有橙 (255,123,60)、奶油 (255,248,172)、粉 (222,125,201)、
/// 黄 (255,255,63):不是给球画的图,是刚好压在图集的几块拼缝上。逐像素采样再让它自转,
/// 亮度就在 101↔158 之间来回跳(实测幅度 57/255),而实机里这两颗球是一色的。
///
/// 判据要窄,三条都得满足:**材质是玻璃族** + **整件的顶点全压在同一根骨骼上**(= 刚体,
/// 自转时形体不变)+ **那根骨骼真的在动作里转满一圈以上**。少了最后一条会误伤一堆
/// 摆动的刚性小件(实测圣羽翼王的羽毛有 71 件、一窝蜂 8 件),它们的贴图是美术真画的。
/// 钉到哪个 UV:取「采出来的颜色最接近本件平均色」的那个顶点,比取包围盒中心更代表整件。
fn flatten_spinning_parts(
    vertices: &mut [Vertex],
    indices: &[u32],
    base_color: Option<&Image>,
    spinning: &std::collections::HashSet<u16>,
) {
    let Some(image) = base_color else { return };
    if image.width == 0 || image.height == 0 || spinning.is_empty() {
        return;
    }
    for part in connected_parts(indices) {
        // 刚体判据:全件同一根主骨骼,且主骨骼权重接近 1
        let joint_of = |v: usize| {
            let w = vertices[v].weights;
            let (best, weight) = (0..4).fold((0u16, 0.0), |acc, k| {
                if w[k] > acc.1 {
                    (vertices[v].joints[k], w[k])
                } else {
                    acc
                }
            });
            (weight > 0.99).then_some(best)
        };
        let Some(joint) = joint_of(part[0]) else {
            continue;
        };
        if !spinning.contains(&joint) || part.iter().any(|&v| joint_of(v) != Some(joint)) {
            continue;
        }

        let sample = |v: usize| {
            let uv = vertices[v].uv;
            // UE 的贴图是 wrap,UV 常落在 [0,1] 之外(见采样器那边的注释)
            let x = (uv[0].rem_euclid(1.0) * image.width as f32) as usize % image.width as usize;
            let y = (uv[1].rem_euclid(1.0) * image.height as f32) as usize % image.height as usize;
            let i = (y * image.width as usize + x) * 4;
            [
                image.rgba[i] as f32,
                image.rgba[i + 1] as f32,
                image.rgba[i + 2] as f32,
            ]
        };
        let colors: Vec<[f32; 3]> = part.iter().map(|&v| sample(v)).collect();
        let sum = colors
            .iter()
            .fold([0.0; 3], |a, c| [a[0] + c[0], a[1] + c[1], a[2] + c[2]]);
        let n = colors.len() as f32;
        let mean = [sum[0] / n, sum[1] / n, sum[2] / n];
        let pick = colors
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let d = |c: &[f32; 3]| (0..3).map(|k| (c[k] - mean[k]).powi(2)).sum::<f32>();
                d(a).total_cmp(&d(b))
            })
            .map(|(i, _)| part[i]);
        if let Some(pick) = pick {
            let uv = vertices[pick].uv;
            for &v in &part {
                vertices[v].uv = uv;
            }
        }
    }
}

/// 按「三角形共享顶点」把一片图元拆成互不相连的小件。
fn connected_parts(indices: &[u32]) -> Vec<Vec<usize>> {
    let mut parent: HashMap<u32, u32> = HashMap::new();
    fn find(parent: &mut HashMap<u32, u32>, mut x: u32) -> u32 {
        while let Some(&p) = parent.get(&x) {
            if p == x {
                break;
            }
            let grand = *parent.get(&p).unwrap_or(&p);
            parent.insert(x, grand);
            x = grand;
        }
        x
    }
    for &i in indices {
        parent.entry(i).or_insert(i);
    }
    for tri in indices.chunks_exact(3) {
        for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2])] {
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra != rb {
                parent.insert(ra, rb);
            }
        }
    }
    let mut groups: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    for &i in indices {
        if seen.insert(i) {
            let root = find(&mut parent, i);
            groups.entry(root).or_default().push(i as usize);
        }
    }
    groups.into_values().collect()
}

/// 父节点一定排在子节点之前的遍历序。
fn topological_order(parents: &[i32]) -> Vec<usize> {
    let mut order = Vec::with_capacity(parents.len());
    let mut done = vec![false; parents.len()];
    // 反复扫,直到所有「父已就绪」的节点都排完;最多扫深度次,骨架深度个位数
    while order.len() < parents.len() {
        let before = order.len();
        for i in 0..parents.len() {
            if done[i] {
                continue;
            }
            let ready = match parents[i] {
                -1 => true,
                p => done[p as usize],
            };
            if ready {
                done[i] = true;
                order.push(i);
            }
        }
        if order.len() == before {
            // 有环(不该发生):剩下的按原序补上,免得死循环
            for (i, settled) in done.iter().enumerate() {
                if !settled {
                    order.push(i);
                }
            }
            break;
        }
    }
    order
}

/// 每段动作采样几个时刻算包围盒。取 5 个:首尾加中间三点,够抓住伸展最大的那一帧,
/// 又不至于让载入变慢(最大的模型 1.2 万顶点 × 16 段 × 5 次 ≈ 100 万次蒙皮,实测几毫秒)。
const BOUNDS_SAMPLES: usize = 9;

/// 单个姿势允许比绑定姿势大多少倍(按最长边)。超过就当这段动作坏了,整个姿势不计入。
///
/// 正常伸展有个上限:张翅、伸手、跳起大概到 1.5–2 倍。而**借来的动画对不上骨架**时
/// (导出器的同族动画回退,见 design.md §9 Phase 4)会把某根骨头甩到几十倍远,
/// 那一个姿势就能把包围盒撑爆,取景于是把整只宠物缩成一条几像素宽的丝。
/// 宁可漏掉一段怪动作的伸展,也不能让正常动作全看不清。
const MAX_POSE_GROWTH: f32 = 2.5;

/// 姿势中心允许偏离绑定姿势中心多远(按绑定盒高度的比例)。超过就是「整只挪到别处去了」。
///
/// 这一条专治**召唤/落地类动作**:喵喵的 `CallOut` 是从 1.5m 高处掉下来,起始几帧整只猫
/// 悬在 y=1.44..2.47(其余动作都在 0..1.0),单帧形体明明只有 1.19 倍,可并集一下
/// 就把取景盒的高度从 0.8m 撑到 2.48m(3.09 倍),**每只宠物的画布都跟着白涨三倍**。
///
/// 整体平移是「宠物在屏幕上的位置」,该由程序挪画布(走路就是这么做的),不该让画布为它留空。
/// 代价说清楚:真把落地动作接进状态机时,得让程序驱动竖直偏移,而不是指望画布装得下。
///
/// 阈值取一整个身高:实测两类的间距很宽——**悬浮类宠物**是正常的,空空颅(幽灵)的 `Alert`
/// 常态浮在 45–56%,而**召唤落地**是 160–197%。一开始取 0.4 把空空颅的 Alert 也毙了,
/// 而 Alert 就在表情池里,于是运行时照样顶出画布;放到 1.0 两头都装得下。
const MAX_POSE_CENTER_DRIFT: f32 = 1.0;

/// 把每段动作采样几帧、CPU 蒙皮一遍,取所有姿势的包围盒并集(含绑定姿势兜底)。
///
/// 位移按 `Player` 的规则剥掉(见 `anim.rs` 的 `strip_root_motion`):走跑动作的
/// root 位移由程序推进屏幕坐标,若算进包围盒会把画布撑到几米宽。
/// root 之上的节点也可能带整体位移(喵喵 `CallOut` 就是),那种由中心偏移这条兜住。
fn animated_bounds(
    vertices: &[Vertex],
    skeleton: &Skeleton,
    clips: &[Clip],
    bind: (Vec3, Vec3),
) -> (Vec3, Vec3) {
    if vertices.is_empty() {
        return bind;
    }
    let bind_longest = (bind.1 - bind.0).max_element().max(1e-4);
    let limit = bind_longest * MAX_POSE_GROWTH;
    let drift_limit = (bind.1.y - bind.0.y).max(1e-4) * MAX_POSE_CENTER_DRIFT;
    let mut pose = Pose::bind(skeleton);
    let mut matrices = Vec::new();
    let root_bind = skeleton.bind[skeleton.root_joint].translation;

    // 先把每个姿势的盒子都量出来,**筛选放到第二遍**(见下面为什么不能拿绑定盒当基准)。
    // `clip_of` 记这一帧属于哪段动作 —— 基准要**按段**取中位数,见下面。
    let mut sampled: Vec<(Vec3, Vec3)> = Vec::new();
    let mut clip_of: Vec<usize> = Vec::new();
    for (ci, clip) in clips.iter().enumerate() {
        for step in 0..BOUNDS_SAMPLES {
            let time = clip.duration * step as f32 / (BOUNDS_SAMPLES - 1).max(1) as f32;
            pose.sample(skeleton, clip, time);
            // **必须和运行时剥得一模一样**:`Player::update` 只把 root 的 X/Z 归零、保留 Y
            // (跳跃要看得见腾空)。这里若连 Y 一起剥,量出来的盒子就比实际渲的低,
            // 带纵向起伏的动作(Happy/Show 的小跳)会顶出画布——踩过这个坑。
            let local = &mut pose.locals[skeleton.root_joint];
            local.translation.x = root_bind.x;
            local.translation.z = root_bind.z;
            pose.joint_matrices(skeleton, &mut matrices);
            let mut pose_min = Vec3::splat(f32::INFINITY);
            let mut pose_max = Vec3::splat(f32::NEG_INFINITY);
            for v in vertices {
                let mut skin = Mat4::ZERO;
                for i in 0..4 {
                    let w = v.weights[i];
                    if w > 0.0 {
                        skin += matrices[v.joints[i] as usize] * w;
                    }
                }
                let p = skin.transform_point3(Vec3::from(v.pos));
                pose_min = pose_min.min(p);
                pose_max = pose_max.max(p);
            }
            if pose_min.x.is_finite() {
                sampled.push((pose_min, pose_max));
                clip_of.push(ci);
            }
        }
    }
    if sampled.is_empty() {
        return bind; // 零动画形态:渲的就是绑定姿势
    }

    // **基准按「每段动作各自的姿势中心中位数」取,不是绑定盒中心、也不是全局中位数。**
    //
    // ① 拿**绑定盒中心**当基准,对**浮游**宠物是灾难:叮叮卯的绑定盒只有 13.8 cm 高,而所有
    //    动作都把它悬到 y ≈ 0.78 m —— 偏移远超 `drift_limit`(= 绑定盒高 × 1.0),
    //    于是**每一帧都被丢掉**,`motion_bounds` 退回绑定盒,相机框住原点附近,整只渲不出来。
    // ② 换成**全局**中位数仍不够:某一整段动作整体偏在别处(叮叮卯二阶的 Idle 就是),
    //    那一段会被整段丢掉,而运行时照样会播它 —— 盒子不覆盖它,渲出来还是空的。
    //    **守卫绝不能丢掉运行时真会显示的姿势。**
    // ③ 所以按**段内**中位数:整段一致的偏移不算异常(段内中位数就是那个偏移),
    //    只剔掉段内的离群帧(root 位移没剥干净时,尾部能差出几米)。
    let median3 = |idx: &[usize]| -> Vec3 {
        let mut c: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for &i in idx {
            let m = (sampled[i].0 + sampled[i].1) * 0.5;
            c[0].push(m.x);
            c[1].push(m.y);
            c[2].push(m.z);
        }
        for v in c.iter_mut() {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        }
        let k = idx.len() / 2;
        Vec3::new(c[0][k], c[1][k], c[2][k])
    };
    let mut per_clip: Vec<Vec<usize>> = vec![Vec::new(); clips.len()];
    for (i, &ci) in clip_of.iter().enumerate() {
        per_clip[ci].push(i);
    }
    let base_of: Vec<Vec3> = per_clip
        .iter()
        .map(|idx| {
            if idx.is_empty() {
                Vec3::ZERO
            } else {
                median3(idx)
            }
        })
        .collect();

    // **取景盒不拿绑定盒当种子。** 运行时显示的永远是某个动作的姿势,绑定姿势只在
    // 「零动画形态」那条路上出现(那时 `sampled` 为空,下面直接返回 `bind`)。
    // 而绑定盒可能是坏的:`Dem_JingJiLong2_001` 的绑定盒 y ∈ [−28.4, 3.2](31.6 米高,
    // 离群顶点),拿它当种子会把取景撑到 31 米,宠物缩成一个点 —— 渲出来就是空图。
    let (mut min, mut max) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    let mut rejected = 0usize;
    let mut accepted = 0usize;
    for (i, (pose_min, pose_max)) in sampled.iter().enumerate() {
        let drift = ((*pose_min + *pose_max) * 0.5 - base_of[clip_of[i]])
            .abs()
            .max_element();
        if (*pose_max - *pose_min).max_element() > limit || drift > drift_limit {
            rejected += 1;
            continue;
        }
        accepted += 1;
        min = min.min(*pose_min);
        max = max.max(*pose_max);
    }
    if rejected > 0 {
        log::debug!("取景包围盒:丢掉 {rejected}/{} 个姿势", sampled.len());
    }
    if accepted == 0 {
        // 仍然全否只可能是「每个姿势都比绑定盒大 2.5 倍以上」,那时绑定盒本身不可信,
        // 退回中位姿势那一帧,至少框得住。
        log::warn!("取景包围盒:{rejected} 个姿势全被丢掉,退回全部姿势的并集");
        let mut m0 = Vec3::splat(f32::INFINITY);
        let mut m1 = Vec3::splat(f32::NEG_INFINITY);
        for (a, b) in &sampled {
            m0 = m0.min(*a);
            m1 = m1.max(*b);
        }
        return (m0, m1);
    }
    log::debug!(
        "取景包围盒 = 绑定盒的 {:.2} 倍;绑定 {:?}..{:?} 取景 {:?}..{:?}",
        (max - min).max_element() / bind_longest,
        bind.0,
        bind.1,
        min,
        max
    );
    (min, max)
}

/// 绑定姿势下的顶点包围盒(蒙皮矩阵在绑定姿势是单位矩阵,直接取顶点即可)。
fn bind_pose_bounds(vertices: &[Vertex], _skeleton: &Skeleton) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for v in vertices {
        let p = Vec3::from(v.pos);
        min = min.min(p);
        max = max.max(p);
    }
    (min, max)
}

#[cfg(test)]
impl Model {
    /// 造一个只有骨架与空动作的模型:让 stage 的行为逻辑能脱离宠物包做单测。
    /// 单节点骨架 + 每段动作 1 秒,没有顶点(测试不碰 GPU)。
    pub fn for_test(clip_names: &[&str]) -> Self {
        let skeleton = Skeleton {
            bind: vec![Trs::IDENTITY],
            parents: vec![-1],
            order: vec![0],
            joints: vec![0],
            inverse_bind: vec![Mat4::IDENTITY],
            root_joint: 0,
        };
        let clips = clip_names
            .iter()
            .map(|name| Clip {
                name: (*name).to_string(),
                duration: 1.0,
                channels: Vec::new(),
            })
            .collect();
        Self {
            source: std::path::PathBuf::from("<test>"),
            vertices: Vec::new(),
            indices: Vec::new(),
            primitives: Vec::new(),
            materials: Vec::new(),
            skeleton,
            clips,
            bounds: (Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5)),
            // 合成模型没有顶点,动作包围盒就等于绑定姿势
            motion_bounds: (Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 孤立的坏帧要削掉,正常的挤压要留着。
    ///
    /// 喵喵 `Shock` 里那一帧(脊椎 X = 4.90)会把头拉出画布 —— 那是压缩动画解出来的
    /// 坏值,不是动画师画的(同一条通道其余帧都满足 Y≡Z,偏这几帧不等)。
    #[test]
    fn only_runaway_scale_keys_are_clamped() {
        // 那一帧的真实数值,来自 packs/喵喵 的 Shock
        let (key, hit) = clamp_scale([4.90, 0.98, 0.43]);
        assert!(hit);
        assert_eq!(key[0], MAX_BONE_SCALE);
        assert_eq!([key[1], key[2]], [0.98, 0.43], "只削大的那一头");

        // 正常的挤压(Anger 最大 1.95、Sad 1.61)一个字都不该改
        for v in [[1.95, 0.71, 0.71], [1.0, 1.0, 1.0], [0.63, 1.61, 1.61]] {
            let (key, hit) = clamp_scale(v);
            assert!(!hit, "{v:?} 不该被动");
            assert_eq!([key[0], key[1], key[2]], v);
        }

        // **下限不夹**:缩到接近 0 是动画师藏部件的手法,夹回去会让它冒出来
        let (key, hit) = clamp_scale([0.001, 0.001, 0.001]);
        assert!(!hit);
        assert_eq!(key[0], 0.001);
    }
}
