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

/// 顶点布局:位置/法线/UV/关节索引/权重。与 pet.wgsl 的 `@location` 一一对应。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub joints: [u16; 4],
    pub weights: [f32; 4],
    /// 玻璃内部层的采样起点 `(UV1.x, UV1.y, UV2.x)`。
    ///
    /// **这三个分量是从 shader 里查出来的,不是挑的**:反汇编 `MI_P_Object_Trans_MatCap` 的
    /// 片元着色器,内部层的起点是 `r4.xy = v2.zw; r4.z = v3.x`;再解 DXBC 的 `ISGN` 签名段,
    /// `v2` = TEXCOORD0、`v3` = TEXCOORD1,而 UE 把材质的 UV 两两打包进插值器
    /// (TEXCOORD0 = UV0.xy + UV1.xy,TEXCOORD1 = UV2.xy + UV3.xy)—— 于是就是这三个。
    ///
    /// 实测幽星光那两颗球 UV1 恒为 0、UV2.x 每颗球一个区间,所以起点几乎是**每颗球一个常量**,
    /// 空间变化全来自「折射方向 × 深度」。这正好解释实机为什么每颗球都稳定居中一颗星、
    /// 而且两颗球各是星和圆点(起点不同 → 落在星场的不同格)。
    /// 之前我拿模型空间位置当起点,画出来就是「一颗被拉伸的星贴在表面」。
    pub interior_pos: [f32; 3],
}

/// 一段网格:对应一个材质槽(宠物一般 2–3 个:本体/眼/嘴)。
pub struct Primitive {
    pub first_index: u32,
    pub index_count: u32,
    pub material: usize,
}

pub struct Material {
    pub name: String,
    /// 基色贴图(RGBA8),路径来自 manifest 的材质表;读失败才是 None,渲染时用白色兜底。
    pub base_color: Option<Image>,
    /// 贴图 alpha 是**镂空遮罩**(眼/嘴的表情图集)还是**线条遮罩**(本体的纹路)。
    pub cutout: bool,
    /// alpha 里是否真的有线条信息(见 `alpha_has_detail`);否则提亮要关掉。
    pub line_detail: bool,
    /// 材质标了 `BLEND_Translucent`:要叠 MatCap 高光、边缘光按混色算。
    ///
    /// **注意不等于「要混合」**:本作有一批材质标着 `BLEND_Translucent` 但不透明度就是 1
    /// (幽星光那两个球),它们的输出与不透明完全一样,却因为不写深度而互相盖不住 ——
    /// 两颗球绕着转、谁在前只由索引序决定,于是转身时前后关系突然对调,看着就是在闪。
    /// 真正需要混合的判据是 `blended()`。
    pub translucent: bool,
    pub opacity: f32,
    /// 星点 / MatCap 两张附加贴图与它们的着色,以及边缘光。
    pub star: Option<Image>,
    pub star_tiling: [f32; 2],
    pub star_color: [f32; 3],
    /// 星点层强度(`Stick_Intensity`)。
    pub stick_intensity: f32,
    pub matcap: Option<Image>,
    pub matcap_color: [f32; 3],
    pub rim_color: [f32; 3],
    pub rim_intensity: f32,
    pub rim_power: f32,
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
    /// 特效层的画法(火焰/水壳/光晕)。`None` = 普通不透明材质,走主通道。
    pub effect: Option<EffectMaterial>,
}

impl Material {
    /// 这一片是否真的需要混合(→ 在不透明层之后画、不写深度)。纯特效层永远要;
    /// 有基色的两种情况要:不透明度真的小于 1,或者**基色 alpha 就是不透明度**
    /// (`alpha_opacity`,静态开关 `Opacity or OpacityMask` 点名的那 11 个)。
    /// 标着半透、不透明度是 1、alpha 又只是纹路遮罩的,当不透明画 ——
    /// 输出一模一样却不会闪(见 `translucent`)。
    pub fn blended(&self) -> bool {
        self.effect.is_some() || (self.translucent && self.opacity < 1.0) || self.alpha_opacity
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
        let bytes = std::fs::read(glb_path).with_context(|| format!("读不到 {glb_path:?}"))?;
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
            // UV1 / UV2:玻璃内部层的采样起点(见 `Vertex::interior_pos`)
            let uv1: Vec<[f32; 2]> = reader
                .read_tex_coords(1)
                .map(|it| it.into_f32().collect())
                .unwrap_or_else(|| vec![[0.0, 0.0]; positions.len()]);
            let uv2: Vec<[f32; 2]> = reader
                .read_tex_coords(2)
                .map(|it| it.into_f32().collect())
                .unwrap_or_else(|| vec![[0.0, 0.0]; positions.len()]);
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
                    interior_pos: [uv1[i][0], uv1[i][1], uv2[i][0]],
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
                let effect = spec.base_color.is_none().then(|| EffectMaterial {
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
                    cutout: spec.mask_alpha,
                    line_detail,
                    translucent: spec.translucent,
                    opacity: spec.opacity,
                    // 星点/matcap 的 alpha 原样保留:形状全在 alpha 里
                    star: spec.star.as_deref().and_then(|p| load_texture(p, true)),
                    star_tiling: spec.star_tiling,
                    star_color: spec.star_color,
                    stick_intensity: spec.stick_intensity,
                    matcap: spec.matcap.as_deref().and_then(|p| load_texture(p, true)),
                    matcap_color: spec.matcap_color,
                    rim_color: spec.rim_color,
                    rim_intensity: spec.rim_intensity,
                    rim_power: spec.rim_power,
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
                        it.map(|v| [v[0], v[1], v[2], 0.0]).collect::<Vec<_>>(),
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

        let bounds = bind_pose_bounds(&vertices, &skeleton);
        let motion_bounds = animated_bounds(&vertices, &skeleton, &clips, bounds);
        Ok(Self {
            vertices,
            indices,
            primitives,
            materials,
            skeleton,
            clips,
            bounds,
            motion_bounds,
        })
    }

    pub fn clip(&self, name: &str) -> Option<usize> {
        self.clips.iter().position(|c| c.name == name)
    }
}

/// 按材质表给的路径读基色贴图。**alpha 原样保留**,怎么解释交给 shader:
///
/// - 眼/嘴的表情图集:alpha 是真遮罩,按阈值剔(`mask_alpha = true`);
/// - 本体:alpha 是**线条/细节遮罩**——RGB 是完整的固有色图集,alpha 里画着身上的纹路
///   (水灵身上那一道道竖向浅色条纹就在 alpha 里,白线正好压在纹路上)。
///   曾经把它刷成 255「省事」,结果纹路全丢;拿它当不透明度剔像素更糟,身体会被啃掉。
fn load_texture(path: &Path, _mask_alpha: bool) -> Option<Image> {
    let img = match image::open(path) {
        Ok(img) => img,
        Err(e) => {
            log::warn!("贴图 {path:?} 读取失败: {e}");
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
            for i in 0..parents.len() {
                if !done[i] {
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
    let (mut min, mut max) = bind;
    if vertices.is_empty() {
        return (min, max);
    }
    let bind_longest = (bind.1 - bind.0).max_element().max(1e-4);
    let limit = bind_longest * MAX_POSE_GROWTH;
    let drift_limit = (bind.1.y - bind.0.y).max(1e-4) * MAX_POSE_CENTER_DRIFT;
    let mut pose = Pose::bind(skeleton);
    let mut matrices = Vec::new();
    let root_bind = skeleton.bind[skeleton.root_joint].translation;

    // 先把每个姿势的盒子都量出来,**筛选放到第二遍**(见下面为什么不能拿绑定盒当基准)
    let mut sampled: Vec<(Vec3, Vec3)> = Vec::new();
    for clip in clips {
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
            }
        }
    }
    if sampled.is_empty() {
        return (min, max);
    }

    // **基准取「各姿势中心的中位数」,不是绑定盒中心。**
    //
    // 原来拿绑定盒中心当基准,对**浮游**宠物是灾难:叮叮卯的绑定盒只有 13.8 cm 高,而所有
    // 动作都把它悬到 y ≈ 0.78 m —— 偏移 0.7 m 远超 `drift_limit`(= 绑定盒高 × 1.0),
    // 于是**每一帧都被丢掉**,`motion_bounds` 退回绑定盒,相机框住原点附近的一小块,
    // 整只渲不出来(全库 4 个资产 / 6 个包栽在这儿,渲出来是全透明的空图)。
    //
    // 换成中位数后:整体一致的偏移不再被当成异常(中位数就是那个偏移),而个别跑飞的姿势
    // (Run/Walk 的 root 位移没被剥干净时,能差出几米)照样被剔掉 —— 守卫的本意保住了。
    let mut cx: Vec<f32> = sampled.iter().map(|(a, b)| (a.x + b.x) * 0.5).collect();
    let mut cy: Vec<f32> = sampled.iter().map(|(a, b)| (a.y + b.y) * 0.5).collect();
    let mut cz: Vec<f32> = sampled.iter().map(|(a, b)| (a.z + b.z) * 0.5).collect();
    for v in [&mut cx, &mut cy, &mut cz] {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    }
    let mid = sampled.len() / 2;
    let base = Vec3::new(cx[mid], cy[mid], cz[mid]);

    let mut rejected = 0usize;
    let mut accepted = 0usize;
    for (pose_min, pose_max) in &sampled {
        let drift = ((*pose_min + *pose_max) * 0.5 - base).abs().max_element();
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
        log::warn!("取景包围盒:{rejected} 个姿势全被丢掉,退回中位姿势");
        return sampled[mid];
    }
    log::debug!(
        "取景包围盒 = 绑定盒的 {:.2} 倍",
        (max - min).max_element() / bind_longest
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
