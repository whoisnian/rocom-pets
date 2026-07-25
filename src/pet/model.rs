//! 读宠物包里的 glb:网格、骨架、动画、材质。
//!
//! 包是导出器(exporter/)产出的:一个 glb 里装着「网格 + 蒙皮 + 全部逻辑动作」,
//! 贴图独立成 PNG 放在 `tex/`,材质名后缀(`_By/_Es/_Mh`)对应贴图 `T_*_<槽>_D`
//! (见 docs/design.md §1、§4.2)。这里只做加载与整形,不碰 GPU。

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use glam::{Mat4, Quat, Vec3};

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
    /// 绑定姿势的包围盒(米),用来摆相机与换算屏幕尺寸。
    pub bounds: (Vec3, Vec3),
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

        for primitive in mesh.primitives() {
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

            let name = primitive
                .material()
                .name()
                .unwrap_or("material")
                .to_string();
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
        Ok(Self {
            vertices,
            indices,
            primitives,
            materials,
            skeleton,
            clips,
            bounds,
        })
    }

    pub fn clip(&self, name: &str) -> Option<usize> {
        self.clips.iter().position(|c| c.name == name)
    }
}

/// 按材质名后缀找基色贴图:`MI_..._By` → `T_..._By_D.png`(见 docs/design.md §1)。
fn find_base_color(tex_dir: &Path, material_name: &str) -> Option<Image> {
    let slot = material_name.rsplit('_').next()?.to_ascii_lowercase();
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
    log::warn!("材质 {material_name} 找不到 {slot}_D 基色贴图,用白色兜底");
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
