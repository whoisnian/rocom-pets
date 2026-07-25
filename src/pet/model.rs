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
    /// 把各动作采样一遍取到的包围盒,`bounds` 的超集。**画布与相机取景用这个**:
    /// 伸手、张翅、小跳的姿势会明显超出绑定姿势,只按 `bounds` 取景会把肢体裁掉。
    /// 实测(120 个抽样形态 × Idle/Happy/Show/Walk 各查一次):按绑定盒取景 11 个被裁
    /// (阿米亚特/波波拉肉眼可见)→ 按这个盒子取景剩 1 个;代价是画布面积平均 1.64 倍。
    pub motion_bounds: (Vec3, Vec3),
}

impl Model {
    /// 不带材质表的载入(旧包 / 只给了一个 glb 路径时):贴图退回命名约定猜。
    pub fn load(glb_path: &Path) -> Result<Self> {
        Self::load_with_materials(glb_path, &HashMap::new())
    }

    /// `materials` 是 manifest 的 `[forms.materials]`:glb 材质名 → 画什么。
    /// 空表就退回按贴图命名约定猜(`_By/_Es/_Mh` ↔ `T_*_<槽>_D`),那是旧行为。
    pub fn load_with_materials(
        glb_path: &Path,
        materials_spec: &HashMap<String, PackMaterial>,
    ) -> Result<Self> {
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
        // 没有材质表时才用几何占比猜特效层(旧包兜底)
        let guess_effects = materials_spec.is_empty() && should_drop_effect_layers(&mesh);

        for primitive in mesh.primitives() {
            let material_name = primitive
                .material()
                .name()
                .unwrap_or("material")
                .to_string();
            match materials_spec.get(&material_name) {
                // 有材质表:**没有基色就是纯特效层**(火焰/水壳/光晕的固有色是 shader 算的,
                // 材质里根本没有 BaseTex/EyeTex)。这比按几何占比或贴图亮度猜靠谱得多。
                Some(spec) if spec.base_color.is_none() => {
                    log::debug!("跳过特效层材质 {material_name}(材质没有基色参数)");
                    continue;
                }
                Some(_) => {}
                None if guess_effects && is_effect_slot(&material_name) => {
                    log::debug!("跳过特效层材质 {material_name}(旧包,按几何占比猜)");
                    continue;
                }
                None => {}
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
                let base_color = match materials_spec.get(&name) {
                    // 材质表给了确切的贴图与 alpha 语义,不用再猜
                    Some(spec) => spec
                        .base_color
                        .as_deref()
                        .and_then(|path| load_texture(path, spec.mask_alpha)),
                    None => find_base_color(&tex_dir, &name),
                };
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
    let mut used_body_texture = is_body_slot(&slot);
    let mut image = load_slot_texture(tex_dir, &slot).or_else(|| {
        if is_body_slot(&slot) {
            return None;
        }
        let fallback = load_slot_texture(tex_dir, "by");
        if fallback.is_some() {
            log::debug!("材质 {material_name} 没有 {slot}_D 贴图,退用本体贴图");
            used_body_texture = true;
        }
        fallback
    })?;
    // **用了本体贴图就要把 alpha 刷成不透明。** `_By_D` 的 alpha 不是不透明度,是美术塞的
    // 遮罩通道:813 张里 160 张通过率 <95%、60 张 <5%,拿去做 alpha 测试会把身体啃掉
    // (火花 4.8% → 只剩眼睛,迪莫 0.39% → 整只消失)。而叠加片(眼/嘴)自己的贴图是**带透明
    // 背景的表情图集**,那儿的 alpha 是真遮罩,必须留着剔——菊花梨的眼睛不剔就是一块方糊。
    // 于是差别放在载入这一步,shader 里只留一个统一的 alpha 测试。
    //
    // 判据是「**最终用的是哪张贴图**」而不是「材质槽叫什么」:非本体槽缺贴图时会退用本体贴图,
    // 那张一样是遮罩 alpha。火神踩过这个坑——它一身肌肉是 Fx 层做的、Fx 槽没自己的贴图,
    // 按槽名判就漏掉了,整个身体被 alpha 测试剔光,只剩翅膀角和尾巴。
    if used_body_texture {
        for pixel in image.rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
    }
    Some(image)
}

/// 是不是本体槽:`By`、`By1`、`By2`…(数字后缀是同一只宠物拆开的多张本体贴图)。
fn is_body_slot(slot: &str) -> bool {
    slot.strip_prefix("by")
        .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()))
}

/// `_Fx*` 槽占三角面的比例低于这个值就当装饰层丢掉,高于则当本体保留。
/// 全量统计:122 个带 Fx 的形态里,占比 <20% 的 59 个、>60% 的 24 个,中间空得很稀,
/// 阈值落在 40% 两边都不敏感。
const EFFECT_BODY_SHARE: f32 = 0.4;

/// 特效层是不是「装饰」级别(几何占比低)。
///
/// **这是旧包的兜底猜法,新包不走这条。** 新包的 manifest 带 `[forms.materials]`,
/// 导出器从游戏材质实例里读出「有没有 BaseTex/EyeTex」,没有就是纯特效层——那是确定的事实,
/// 不用猜。留着这条是为了还能读没有材质表的旧包。
///
/// 猜法本身也记一下当时的依据:`_Fx*` 槽的几何占比 <20% 的 59 个、>60% 的 24 个,
/// 中间很稀,所以按 40% 分「装饰 / 本体」。它对火花(78%,是本体)判对了,
/// 但对幽星光一阶判错了——那层壳占 79%、按占比该留,可它的贴图是黑底粉星点,
/// 不透明地画就是一坨黑盖住粉本体。**真相是材质里 `BaseTex` 指的是粉色本体贴图**,
/// 我把 `NoiseTex` 当成基色了;这类错误只有读材质才能避免。
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

/// 按材质表给的路径读基色贴图。
///
/// `mask_alpha` 决定 alpha 怎么处理,这是两类贴图的分水岭(见 pet.wgsl 里的说明):
/// - `true`(眼/嘴的表情图集):alpha 是真遮罩,原样留着让 shader 按阈值剔;
/// - `false`(本体):alpha 是美术塞的遮罩通道,**刷成不透明**,否则身体会被剔掉。
fn load_texture(path: &Path, mask_alpha: bool) -> Option<Image> {
    let img = match image::open(path) {
        Ok(img) => img,
        Err(e) => {
            log::warn!("贴图 {path:?} 读取失败: {e}");
            return None;
        }
    };
    let rgba = img.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    let mut rgba = rgba.into_raw();
    if !mask_alpha {
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
    }
    Some(Image {
        width,
        height,
        rgba,
    })
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
    let bind_center = (bind.0 + bind.1) * 0.5;
    let limit = bind_longest * MAX_POSE_GROWTH;
    let drift_limit = (bind.1.y - bind.0.y).max(1e-4) * MAX_POSE_CENTER_DRIFT;
    let mut pose = Pose::bind(skeleton);
    let mut matrices = Vec::new();
    let root_bind = skeleton.bind[skeleton.root_joint].translation;
    let mut rejected = 0usize;
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
            // 先单独量这个姿势,坏姿势整帧丢掉,不让它污染并集
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
            let drift = ((pose_min + pose_max) * 0.5 - bind_center)
                .abs()
                .max_element();
            if (pose_max - pose_min).max_element() > limit || drift > drift_limit {
                log::trace!(
                    "  丢掉 {} @{:.2}s:形体 {:.2}x、中心偏移 {:.2}(身高的 {:.0}%)",
                    clip.name,
                    time,
                    (pose_max - pose_min).max_element() / bind_longest,
                    drift,
                    drift / (bind.1.y - bind.0.y).max(1e-4) * 100.0
                );
                rejected += 1;
                continue;
            }
            min = min.min(pose_min);
            max = max.max(pose_max);
        }
    }
    if rejected > 0 {
        log::debug!(
            "取景包围盒:丢掉 {rejected}/{} 个姿势(形体超 {MAX_POSE_GROWTH} 倍或整只挪走)",
            clips.len() * BOUNDS_SAMPLES
        );
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
