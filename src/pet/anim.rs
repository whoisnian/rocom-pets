//! 动画采样、clip 间交叉淡化、蒙皮矩阵计算。
//!
//! 混合在 TRS 分量上做(旋转用 slerp),而不是在矩阵上做——矩阵插值会把旋转插成剪切。
//! 淡化是桌宠必需的:状态机随时会从 Idle 切到 Walk/Happy,硬切会看着一跳。

use glam::{Mat4, Quat, Vec3};

use super::model::{Channel, Clip, Interpolation, Model, Property, Skeleton, Trs};

/// 一帧的姿势:每个节点的局部变换。
#[derive(Clone)]
pub struct Pose {
    pub locals: Vec<Trs>,
}

impl Pose {
    pub fn bind(skeleton: &Skeleton) -> Self {
        Self {
            locals: skeleton.bind.clone(),
        }
    }

    /// 采样某个 clip 到本姿势上。没有通道的节点保持绑定姿势。
    pub fn sample(&mut self, skeleton: &Skeleton, clip: &Clip, time: f32) {
        self.locals.copy_from_slice(&skeleton.bind);
        for channel in &clip.channels {
            if channel.node >= self.locals.len() || channel.times.is_empty() {
                continue;
            }
            let local = &mut self.locals[channel.node];
            match channel.property {
                Property::Translation => local.translation = sample_vec3(channel, time),
                Property::Scale => local.scale = sample_vec3(channel, time),
                Property::Rotation => local.rotation = sample_quat(channel, time),
            }
        }
    }

    /// 把 `other` 以权重 `weight`(0 = 全是自己,1 = 全是 other)混进来。
    pub fn blend_from(&mut self, other: &Pose, weight: f32) {
        let w = weight.clamp(0.0, 1.0);
        for (a, b) in self.locals.iter_mut().zip(&other.locals) {
            a.translation = a.translation.lerp(b.translation, w);
            a.scale = a.scale.lerp(b.scale, w);
            a.rotation = a.rotation.slerp(b.rotation, w);
        }
    }

    /// 算出交给 GPU 的蒙皮矩阵:关节世界变换 × 逆绑定矩阵。
    pub fn joint_matrices(&self, skeleton: &Skeleton, out: &mut Vec<Mat4>) {
        let mut world = vec![Mat4::IDENTITY; self.locals.len()];
        for &node in &skeleton.order {
            let local = self.locals[node].matrix();
            world[node] = match skeleton.parents[node] {
                -1 => local,
                parent => world[parent as usize] * local,
            };
        }
        out.clear();
        for (joint, &node) in skeleton.joints.iter().enumerate() {
            out.push(world[node] * skeleton.inverse_bind[joint]);
        }
    }
}

/// 找到 `time` 落在哪两个关键帧之间,返回(前一帧下标, 插值系数)。
fn locate(channel: &Channel, time: f32) -> (usize, f32) {
    let times = &channel.times;
    if times.len() == 1 || time <= times[0] {
        return (0, 0.0);
    }
    if time >= times[times.len() - 1] {
        return (times.len() - 1, 0.0);
    }
    // 关键帧少(几十到一百多),线性找足够快,省掉二分的边界心智负担
    let mut i = 0;
    while i + 1 < times.len() && times[i + 1] <= time {
        i += 1;
    }
    let span = times[i + 1] - times[i];
    let f = if span <= 0.0 {
        0.0
    } else {
        (time - times[i]) / span
    };
    (i, f)
}

fn sample_vec3(channel: &Channel, time: f32) -> Vec3 {
    let (i, f) = locate(channel, time);
    let a = Vec3::new(
        channel.values[i][0],
        channel.values[i][1],
        channel.values[i][2],
    );
    if f == 0.0 || channel.interpolation == Interpolation::Step || i + 1 >= channel.values.len() {
        return a;
    }
    let b = Vec3::new(
        channel.values[i + 1][0],
        channel.values[i + 1][1],
        channel.values[i + 1][2],
    );
    a.lerp(b, f)
}

fn sample_quat(channel: &Channel, time: f32) -> Quat {
    let (i, f) = locate(channel, time);
    let a = Quat::from_array(channel.values[i]);
    if f == 0.0 || channel.interpolation == Interpolation::Step || i + 1 >= channel.values.len() {
        return a.normalize();
    }
    let b = Quat::from_array(channel.values[i + 1]);
    a.normalize().slerp(b.normalize(), f)
}

/// 播放器:当前 clip + 可选的上一段(淡出中),每帧算出蒙皮矩阵。
pub struct Player {
    current: usize,
    time: f32,
    /// 淡出中的上一段:(clip 下标, 停在哪一刻, 剩余淡化时间)
    previous: Option<(usize, f32, f32)>,
    fade_duration: f32,
    /// 是否剥掉根骨骼的水平位移。桌宠的位置由行为逻辑推进(速度取自 manifest 的
    /// speed_cm_s),动画里那份位移必须抵消,否则宠物会一边被程序推、一边自己走出画布。
    /// 垂直分量保留:跳跃类动作靠它起跳。
    pub strip_root_motion: bool,
    pose: Pose,
    scratch: Pose,
    pub matrices: Vec<Mat4>,
}

impl Player {
    pub fn new(model: &Model, clip: usize) -> Self {
        let pose = Pose::bind(&model.skeleton);
        Self {
            current: clip,
            time: 0.0,
            previous: None,
            fade_duration: 0.18,
            strip_root_motion: true,
            pose: pose.clone(),
            scratch: pose,
            matrices: Vec::new(),
        }
    }

    /// 状态机接进来后要用(Phase 3),先留着。
    #[allow(dead_code)]
    pub fn current(&self) -> usize {
        self.current
    }

    #[allow(dead_code)]
    pub fn time(&self) -> f32 {
        self.time
    }

    /// 切到另一段动作;同一段则什么都不做(避免状态机每帧重置动画)。
    pub fn play(&mut self, clip: usize) {
        if clip == self.current {
            return;
        }
        self.previous = Some((self.current, self.time, self.fade_duration));
        self.current = clip;
        self.time = 0.0;
    }

    /// 直接跳到某一时刻(离屏渲染取固定帧用)。
    pub fn seek(&mut self, time: f32) {
        self.time = time;
        self.previous = None;
    }

    pub fn advance(&mut self, model: &Model, dt: f32) {
        let duration = model.clips[self.current].duration.max(1e-4);
        self.time = (self.time + dt) % duration;
        if let Some((_, _, remaining)) = &mut self.previous {
            *remaining -= dt;
            if *remaining <= 0.0 {
                self.previous = None;
            }
        }
    }

    /// 采样当前状态,更新 `matrices`。
    pub fn update(&mut self, model: &Model) {
        let skeleton = &model.skeleton;
        self.pose
            .sample(skeleton, &model.clips[self.current], self.time);
        if let Some((prev, prev_time, remaining)) = self.previous {
            // remaining 从 fade_duration 递减到 0,故当前段的权重是 1 - remaining/duration
            let weight = 1.0 - (remaining / self.fade_duration).clamp(0.0, 1.0);
            self.scratch.sample(skeleton, &model.clips[prev], prev_time);
            // 以旧姿势为底,按权重混向新姿势
            std::mem::swap(&mut self.pose, &mut self.scratch);
            self.pose.blend_from(&self.scratch, weight);
        }
        if self.strip_root_motion {
            let root = skeleton.root_joint;
            let bind = skeleton.bind[root].translation;
            let local = &mut self.pose.locals[root];
            local.translation.x = bind.x;
            local.translation.z = bind.z;
        }
        self.pose.joint_matrices(skeleton, &mut self.matrices);
    }
}
