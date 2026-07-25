//! 读宠物包里的 glb:网格、骨架、动画、材质。
//!
//! 包是导出器(exporter/)产出的:一个 glb 里装着「网格 + 蒙皮 + 全部逻辑动作」,
//! 贴图独立成 PNG 放在 `tex/`,材质名后缀(`_By/_Es/_Mh`)对应贴图 `T_*_<槽>_D`
//! (见 docs/design.md §1、§4.2)。这里只做加载与整形,不碰 GPU。

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use glam::{Mat4, Quat, Vec3};

use super::anim::Pose;

/// 顶点布局:位置/法线/UV/关节索引/权重。与 pet.wgsl 的 `@location` 一一对应。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

/// 一段网格:对应一个材质槽(宠物一般 2–3 个:本体/眼/嘴)。
pub struct Primitive {
    pub first_index: u32,
    pub index_count: u32,
    pub material: usize,
}

pub struct Material {
    pub name: String,
    /// 基色贴图(RGBA8),按命名约定从 `tex/` 找;找不到就是 None,渲染时用白色兜底。
    pub base_color: Option<Image>,
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
    /// 把所有动作都采样一遍取到的包围盒,`bounds` 的超集。**画布与相机取景用这个**:
    /// 伸手、张翅、跳跃的姿势会明显超出绑定姿势,只按 `bounds` 取景会把肢体裁掉
    /// (实测 120 个抽样形态里 11 个被裁,阿米亚特/波波拉肉眼可见)。
    pub motion_bounds: (Vec3, Vec3),
}

impl Model {
    /// `glb_path` 同级的 `tex/` 目录用于找贴图。
    pub fn load(glb_path: &Path) -> Result<Self> {
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
        let tex_dir = glb_path.parent().unwrap_or(Path::new(".")).join("tex");
        let drop_effects = should_drop_effect_layers(&mesh);

        for primitive in mesh.primitives() {
            let material_name = primitive
                .material()
                .name()
                .unwrap_or("material")
                .to_string();
            if drop_effects && is_effect_slot(&material_name) {
                log::debug!("跳过特效层材质 {material_name}");
                continue;
            }
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
                let base_color = find_base_color(&tex_dir, &name);
                materials.push(Material {
                    name: name.clone(),
                    base_color,
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

/// 按材质名后缀找基色贴图:`MI_..._By` → `T_..._By_D.png`(见 docs/design.md §1)。
///
/// 找不到本槽的贴图时退到本体槽(`By`):有些宠物的眼/特效槽指向**共享贴图**
/// (CommonTexture 里的眼睛图集之类),而共享贴图是哪张只写在材质实例的参数里,
/// 那份参数在本作解不出来(§1 的 OverflowException)。退到本体色至少是同色系,
/// 比留一块纯白好看;真要正确还得先把材质参数解出来。
fn find_base_color(tex_dir: &Path, material_name: &str) -> Option<Image> {
    let slot = material_name.rsplit('_').next()?.to_ascii_lowercase();
    load_slot_texture(tex_dir, &slot).or_else(|| {
        if slot == "by" {
            return None;
        }
        let fallback = load_slot_texture(tex_dir, "by");
        if fallback.is_some() {
            log::debug!("材质 {material_name} 没有 {slot}_D 贴图,退用本体贴图");
        }
        fallback
    })
}

/// `_Fx*` 槽占三角面的比例低于这个值就当装饰层丢掉,高于则当本体保留。
/// 全量统计:122 个带 Fx 的形态里,占比 <20% 的 59 个、>60% 的 24 个,中间空得很稀,
/// 阈值落在 40% 两边都不敏感。
const EFFECT_BODY_SHARE: f32 = 0.4;

/// 要不要丢掉这个形态的特效层。
///
/// `_Fx*` 槽有**两种完全不同的用途**,只能按几何占比区分:
///
/// - **装饰**(占比极低,几个三角的小面片):加色光晕、拖尾。靠游戏自研 shader 的加色/
///   半透混合才成立,我们只有不透明 toon 着色,照画就是几块凭空浮着的实心片 → 丢掉。
/// - **本体**(占比过半):火花 79%、幽星光 92%、小鼓象 91% —— 火焰/星光那一身就是 Fx 层做的,
///   丢了整只宠物只剩眼睛和配饰 → 必须留。
///
/// **已知渲不好的一类**:水蓝蓝这种半透水体,Fx 是内外两层壳(三角数一模一样)+ 一张噪声贴图,
/// 靠半透混合出水感。占比 79% 走「保留」分支,于是画成一团噪声;要正确得先支持半透材质,
/// 依赖材质实例参数解析(见 design.md 横向待办)。丢掉又会让它只剩蝴蝶结和脸,两头都不对。
fn should_drop_effect_layers(mesh: &gltf::Mesh) -> bool {
    let mut effect = 0usize;
    let mut body = 0usize;
    for primitive in mesh.primitives() {
        let name = primitive.material().name().unwrap_or("");
        // 顶点数就够比例判断,不必真去读索引
        let count = primitive
            .get(&gltf::Semantic::Positions)
            .map_or(0, |a| a.count());
        if is_effect_slot(name) {
            effect += count;
        } else {
            body += count;
        }
    }
    if effect == 0 {
        return false;
    }
    let share = effect as f32 / (effect + body) as f32;
    let drop = share < EFFECT_BODY_SHARE;
    log::debug!(
        "特效层占顶点 {:.0}% → {}",
        share * 100.0,
        if drop {
            "当装饰丢掉"
        } else {
            "当本体保留"
        }
    );
    drop
}

/// 材质槽是不是特效层(`MI_<资产>_Fx` / `_Fx1` / `_FX2` …)。
/// 只认 `Fx` 打头加可选数字;`Dynamic\d` 不算——那是身上会动的部件(布料/尾饰),
/// 属于宠物本体,实测渲出来是对的。
fn is_effect_slot(material_name: &str) -> bool {
    let Some(slot) = material_name.rsplit('_').next() else {
        return false;
    };
    let lower = slot.to_ascii_lowercase();
    lower
        .strip_prefix("fx")
        .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()))
}

fn load_slot_texture(tex_dir: &Path, slot: &str) -> Option<Image> {
    let entries = std::fs::read_dir(tex_dir).ok()?;
    let mut candidates: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    candidates.sort();
    for path in candidates {
        let stem = path.file_stem()?.to_string_lossy().to_string();
        let mut parts = stem.rsplitn(3, '_');
        let kind = parts.next().unwrap_or("");
        let tex_slot = parts.next().unwrap_or("");
        if kind == "D" && tex_slot.to_ascii_lowercase() == slot {
            match image::open(&path) {
                Ok(img) => {
                    let rgba = img.to_rgba8();
                    return Some(Image {
                        width: rgba.width(),
                        height: rgba.height(),
                        rgba: rgba.into_raw(),
                    });
                }
                Err(e) => log::warn!("贴图 {path:?} 读取失败: {e}"),
            }
        }
    }
    None
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
const BOUNDS_SAMPLES: usize = 5;

/// 把每段动作采样几帧、CPU 蒙皮一遍,取所有姿势的包围盒并集(含绑定姿势兜底)。
///
/// 水平位移按 `Player` 的规则剥掉(见 `anim.rs` 的 `strip_root_motion`):走跑动作的
/// root 位移由程序推进屏幕坐标,若算进包围盒会把画布撑到几米宽。
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
    let mut pose = Pose::bind(skeleton);
    let mut matrices = Vec::new();
    let root_bind = skeleton.bind[skeleton.root_joint].translation;
    for clip in clips {
        for step in 0..BOUNDS_SAMPLES {
            let time = clip.duration * step as f32 / (BOUNDS_SAMPLES - 1).max(1) as f32;
            pose.sample(skeleton, clip, time);
            let local = &mut pose.locals[skeleton.root_joint];
            local.translation.x = root_bind.x;
            local.translation.z = root_bind.z;
            pose.joint_matrices(skeleton, &mut matrices);
            for v in vertices {
                let mut skin = Mat4::ZERO;
                for i in 0..4 {
                    let w = v.weights[i];
                    if w > 0.0 {
                        skin += matrices[v.joints[i] as usize] * w;
                    }
                }
                let p = skin.transform_point3(Vec3::from(v.pos));
                min = min.min(p);
                max = max.max(p);
            }
        }
    }
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
