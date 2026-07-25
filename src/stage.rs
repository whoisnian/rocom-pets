//! 与平台无关的 stage 逻辑:角色在屏幕上的位置、拖动、命中测试、输入区,以及宠物的行为。
//!
//! 平台后端只负责「造表面 / 收事件 / 出帧 / 设输入区」,所有状态都在这里,
//! 这样 Wayland 与 Windows 两边的行为天然一致,也能脱离窗口系统做单元测试。

use std::time::Duration;

use crate::pet::mask::Mask;
use crate::pet::target::camera_yaw;
use crate::pet::{Model, Player};
use crate::sprite::{Rect, Sprite};

/// 后端喂进来的事件(坐标都是表面局部逻辑像素)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StageEvent {
    Resized { width: u32, height: u32 },
    PointerMoved { x: f64, y: f64 },
    PointerPressed { x: f64, y: f64 },
    PointerReleased,
    PointerLeft,
    TogglePassthrough,
}

/// 处理一个事件后,后端该做什么。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Reaction {
    /// 需要重画并提交一帧。
    pub redraw: bool,
    /// 输入区变了,后端要重新 `set_input_region` / 更新命中区域。
    pub regions_dirty: bool,
}

impl Reaction {
    const NONE: Self = Self {
        redraw: false,
        regions_dirty: false,
    };
    const REDRAW: Self = Self {
        redraw: true,
        regions_dirty: false,
    };
    const BOTH: Self = Self {
        redraw: true,
        regions_dirty: true,
    };
}

/// 输入区网格粒度与 alpha 阈值,见 `Sprite::coverage_rects`。
const REGION_CELL: u32 = 8;
const REGION_ALPHA_THRESHOLD: u8 = 8;

/// 宠物脚底离屏幕底边留的空隙(逻辑像素)。
const GROUND_MARGIN: f32 = 4.0;

/// 按下后指针移动超过这么多逻辑像素才算拖动,否则算点击。
const DRAG_THRESHOLD: f32 = 4.0;

/// 摸头判定:头部区域取包围盒上面这个比例。
const HEAD_ZONE: f32 = 0.45;
/// 在这段时间窗口内来回蹭够 REVERSALS 次就算「摸头」。
const PET_WINDOW: f32 = 1.2;
const PET_REVERSALS: u8 = 3;

/// 推进频率的上下限。
///
/// 降频**只看姿势实际变化速度**,不按状态硬分档:待机动画本身带明显起伏
/// (实测关节最大速度 ~6m/s,行走 ~4.7m/s),一律按「待机」降到 12Hz 会看着发顿——
/// 这是实机反馈改过来的。频率由 [`hz_for_motion`] 连续映射,睡觉那类近乎静止的动作
/// 自动落到下限,不需要给每段动作手工标注。
const ACTIVE_HZ: f32 = 30.0;
const STILL_HZ: f32 = 10.0;
/// 关节速度到多少就跑满帧(米/秒)。取 1.0:实测有动作的段都在 4m/s 以上,余量充足。
const FULL_RATE_MOTION: f32 = 1.0;

/// 姿势变化速度 → 推进频率。
fn hz_for_motion(motion: f32) -> f32 {
    (motion / FULL_RATE_MOTION * ACTIVE_HZ).clamp(STILL_HZ, ACTIVE_HZ)
}

/// 宠物当前在干什么。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Activity {
    /// 站着待机,`remaining` 秒后换个地方走走。
    Idle { remaining: f32 },
    /// 走向 `target_x`(左上角的目标 x)。
    Walk { target_x: f32 },
    /// 被鼠标拎着。
    Dragged,
    /// 一次性反应(受惊/开心…),播完 `remaining` 秒回到待机。
    React { remaining: f32 },
}

/// 宠物对鼠标的反应(播哪段一次性动作)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PetReaction {
    /// 被点了一下 → 受惊。
    Startled,
    /// 头被反复蹭 → 开心。
    Petted,
    /// 被拎起来 → 害怕。
    PickedUp,
}

/// 一只宠物:模型 + 播放器 + 屏幕上的表现状态。
pub struct PetActor {
    pub model: Model,
    pub player: Player,
    /// 屏幕上的显示尺寸(逻辑像素)。
    pub size: (u32, u32),
    /// 当前朝向角(弧度,绕 Y 轴;0 = 面向观察者,+π/2 = 朝屏幕右)。
    pub yaw: f32,
    target_yaw: f32,
    pub activity: Activity,
    /// 走路速度(逻辑像素/秒)。
    pub walk_speed: f32,
    /// 画布顶端到宠物脚底的距离(逻辑像素)。取景留了余量,脚底不在画布最下沿,
    /// 站地面时要按这个值对齐,否则宠物会悬空。
    pub foot_offset: f32,
    /// 轮廓掩码(异步回读而来,见 pet/mask.rs);还没到就退化成包围盒判定。
    pub mask: Option<Mask>,
    clips: Clips,
    petting: Petting,
    rng: Rng,
}

/// 摸头追踪:指针在头部区域来回蹭的次数。
#[derive(Default)]
struct Petting {
    last_x: Option<f64>,
    direction: i8,
    reversals: u8,
    window: f32,
}

impl Petting {
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// 喂一个头部区域内的指针 x;够次数就返回 true(算一次摸头)。
    fn feed(&mut self, x: f64) -> bool {
        let Some(last) = self.last_x.replace(x) else {
            self.window = PET_WINDOW;
            return false;
        };
        let delta = x - last;
        if delta.abs() < 2.0 {
            return false; // 抖动不算
        }
        let direction = if delta > 0.0 { 1 } else { -1 };
        if self.direction != 0 && direction != self.direction {
            self.reversals += 1;
        }
        self.direction = direction;
        self.window = PET_WINDOW;
        if self.reversals >= PET_REVERSALS {
            self.reset();
            return true;
        }
        false
    }

    fn tick(&mut self, dt: f32) {
        if self.last_x.is_none() {
            return;
        }
        self.window -= dt;
        if self.window <= 0.0 {
            self.reset();
        }
    }
}

/// 逻辑动作在 glb 里的下标;缺的就是 None,行为要能降级。
struct Clips {
    idle: usize,
    walk: Option<usize>,
    startled: Option<usize>,
    happy: Option<usize>,
    afraid: Option<usize>,
}

impl PetActor {
    /// 播一次性反应动作,播完回待机。缺对应动作就只改状态(至少行为语义还在)。
    fn react(&mut self, reaction: PetReaction, model_len: f32) {
        let clip = match reaction {
            PetReaction::Startled => self.clips.startled,
            PetReaction::Petted => self.clips.happy,
            PetReaction::PickedUp => self.clips.afraid,
        };
        if let Some(clip) = clip {
            self.player.play(clip);
            self.activity = Activity::React {
                remaining: model_len.max(0.3),
            };
        }
    }

    /// 某段动作的时长(秒)。
    fn clip_seconds(&self, reaction: PetReaction) -> f32 {
        let clip = match reaction {
            PetReaction::Startled => self.clips.startled,
            PetReaction::Petted => self.clips.happy,
            PetReaction::PickedUp => self.clips.afraid,
        };
        clip.map(|c| self.model.clips[c].duration).unwrap_or(0.0)
    }

    pub fn new(
        model: Model,
        size: (u32, u32),
        foot_offset: f32,
        walk_speed: f32,
        seed: u64,
    ) -> Self {
        // Idle 一定要有:没有 Idle 的包等于没法待机,退化成用第 0 段动作
        let idle = model.clip("Idle").unwrap_or(0);
        let walk = model.clip("Walk");
        // 反应动作:游戏里「摸头」在 INTERACTIONTREE_CONF 有对应动作键,但键→动作表的映射
        // 还没核实(见 design.md §5),所以先按语义挑:受惊 Shock、开心 Happy、害怕 Fear,
        // 缺哪个就退到 Alert / Show / Shock
        let startled = model.clip("Shock").or_else(|| model.clip("Alert"));
        let happy = model.clip("Happy").or_else(|| model.clip("Show"));
        let afraid = model.clip("Fear").or(startled);
        let player = Player::new(&model, idle);
        Self {
            model,
            player,
            size,
            yaw: 0.0,
            target_yaw: 0.0,
            activity: Activity::Idle { remaining: 2.0 },
            walk_speed,
            foot_offset,
            mask: None,
            clips: Clips {
                idle,
                walk,
                startled,
                happy,
                afraid,
            },
            petting: Petting::default(),
            rng: Rng::new(seed),
        }
    }
}

/// 舞台上的角色:调试用的测试精灵,或真宠物。
pub enum Actor {
    /// `--sprite` 调试模式:平台层的验收(S1)只需要一张软边贴图。
    Sprite(Sprite),
    Pet(PetActor),
}

impl Actor {
    /// 逻辑像素尺寸。
    pub fn size(&self) -> (u32, u32) {
        match self {
            Actor::Sprite(sprite) => (sprite.width, sprite.height),
            Actor::Pet(pet) => pet.size,
        }
    }

    /// 角色局部坐标 (lx, ly) 是否算命中。
    fn hit(&self, lx: i32, ly: i32) -> bool {
        match self {
            Actor::Sprite(sprite) => sprite.alpha_at(lx, ly) >= REGION_ALPHA_THRESHOLD,
            Actor::Pet(pet) => {
                if lx < 0 || ly < 0 || lx as u32 >= pet.size.0 || ly as u32 >= pet.size.1 {
                    return false;
                }
                match &pet.mask {
                    // 有轮廓掩码就按轮廓判:腿与尾之间的空隙可以点穿
                    Some(mask) => {
                        mask.hit(lx as f32 / pet.size.0 as f32, ly as f32 / pet.size.1 as f32)
                    }
                    // 掩码还没回读回来(头一两帧):先按包围盒,总比点不到好
                    None => true,
                }
            }
        }
    }

    /// 角色局部坐标下的输入区矩形。
    fn coverage(&self) -> Vec<Rect> {
        match self {
            Actor::Sprite(sprite) => sprite.coverage_rects(REGION_CELL, REGION_ALPHA_THRESHOLD),
            Actor::Pet(pet) => match &pet.mask {
                Some(mask) if !mask.is_empty() => mask.rects(pet.size),
                _ => vec![Rect {
                    x: 0,
                    y: 0,
                    w: pet.size.0,
                    h: pet.size.1,
                }],
            },
        }
    }
}

pub struct Stage {
    actor: Actor,
    /// 表面尺寸(逻辑像素)。
    size: (u32, u32),
    /// 角色左上角在表面内的位置。
    pos: (f32, f32),
    /// 角色局部坐标下的覆盖矩形,只在角色变了才重算。
    coverage: Vec<Rect>,
    pointer: Option<(f64, f64)>,
    /// 按下时记下的「指针 - 角色左上角」偏移。
    drag_offset: Option<(f64, f64)>,
    /// 本次按下之后指针是否移动过(用来区分「点一下」与「拎起来拖」)。
    drag_moved: bool,
    passthrough: bool,
}

impl Stage {
    pub fn new(actor: Actor, size: (u32, u32)) -> Self {
        let coverage = actor.coverage();
        let mut stage = Self {
            actor,
            size,
            pos: (0.0, 0.0),
            coverage,
            pointer: None,
            drag_offset: None,
            drag_moved: false,
            passthrough: false,
        };
        stage.reset_position();
        stage
    }

    pub fn actor(&self) -> &Actor {
        &self.actor
    }

    /// 角色左上角位置(表面局部逻辑像素)。
    pub fn actor_pos(&self) -> (f32, f32) {
        self.pos
    }

    pub fn passthrough(&self) -> bool {
        self.passthrough
    }

    pub fn is_dragging(&self) -> bool {
        self.drag_offset.is_some()
    }

    /// 摆到初始位置:精灵居中(调试用),宠物站到屏幕底边中间。
    pub fn reset_position(&mut self) {
        let (w, h) = self.actor.size();
        self.pos.0 = (self.size.0 as f32 - w as f32) * 0.5;
        self.pos.1 = match self.actor {
            Actor::Sprite(_) => (self.size.1 as f32 - h as f32) * 0.5,
            Actor::Pet(_) => self.ground_y(),
        };
        self.clamp_to_surface();
    }

    /// 宠物站立时左上角该在的 y:让脚底落在屏幕底边上方 GROUND_MARGIN 处。
    fn ground_y(&self) -> f32 {
        let foot = match &self.actor {
            Actor::Pet(pet) => pet.foot_offset,
            Actor::Sprite(sprite) => sprite.height as f32,
        };
        (self.size.1 as f32 - GROUND_MARGIN - foot).max(0.0)
    }

    fn clamp_to_surface(&mut self) {
        let (w, h) = self.actor.size();
        let max_x = (self.size.0 as f32 - w as f32).max(0.0);
        self.pos.0 = self.pos.0.clamp(0.0, max_x);
        match &self.actor {
            // 宠物的画布比它本身大(取景留了余量),按画布夹会把它顶离地面。
            // 真正该约束的是**脚底**留在屏幕内,画布超出边界让它被裁掉就好。
            Actor::Pet(pet) => {
                let min_y = -(h as f32 - pet.foot_offset);
                let max_y = self.size.1 as f32 - pet.foot_offset;
                self.pos.1 = self.pos.1.clamp(min_y.min(max_y), max_y);
            }
            Actor::Sprite(_) => {
                let max_y = (self.size.1 as f32 - h as f32).max(0.0);
                self.pos.1 = self.pos.1.clamp(0.0, max_y);
            }
        }
    }

    /// 当前该交给合成器的输入区(表面局部坐标)。穿透时为空。
    pub fn input_regions(&self) -> Vec<Rect> {
        if self.passthrough {
            return Vec::new();
        }
        let (dx, dy) = (self.pos.0.round() as i32, self.pos.1.round() as i32);
        self.coverage.iter().map(|r| r.translated(dx, dy)).collect()
    }

    /// 表面坐标是否落在角色的可见部分上(比输入区更精确,用于自己内部的判定)。
    pub fn hit_test(&self, x: f64, y: f64) -> bool {
        let lx = (x - self.pos.0 as f64).floor() as i32;
        let ly = (y - self.pos.1 as f64).floor() as i32;
        self.actor.hit(lx, ly)
    }

    pub fn handle(&mut self, event: StageEvent) -> Reaction {
        match event {
            StageEvent::Resized { width, height } => {
                if (width, height) == self.size {
                    return Reaction::NONE;
                }
                let grounded = matches!(self.actor, Actor::Pet(_));
                self.size = (width, height);
                if grounded {
                    self.pos.1 = self.ground_y();
                }
                self.clamp_to_surface();
                Reaction::BOTH
            }
            StageEvent::PointerMoved { x, y } => {
                self.pointer = Some((x, y));
                if let Some((ox, oy)) = self.drag_offset {
                    let moved =
                        ((x - ox) as f32 - self.pos.0).abs() + ((y - oy) as f32 - self.pos.1).abs();
                    if moved > DRAG_THRESHOLD {
                        self.drag_moved = true;
                        if let Actor::Pet(pet) = &mut self.actor {
                            // 真被拎起来了才播害怕:轻点一下不该惊动它
                            if pet.activity != Activity::Dragged {
                                let len = pet.clip_seconds(PetReaction::PickedUp);
                                pet.react(PetReaction::PickedUp, len);
                                pet.activity = Activity::Dragged;
                            }
                        }
                    }
                    self.pos = ((x - ox) as f32, (y - oy) as f32);
                    self.clamp_to_surface();
                    return Reaction::BOTH;
                }
                // 没在拖:看看是不是在头上来回蹭
                self.feed_petting(x, y)
            }
            StageEvent::PointerPressed { x, y } => {
                self.pointer = Some((x, y));
                if self.passthrough || !self.hit_test(x, y) {
                    return Reaction::NONE;
                }
                self.drag_offset = Some((x - self.pos.0 as f64, y - self.pos.1 as f64));
                self.drag_moved = false;
                Reaction::REDRAW
            }
            StageEvent::PointerReleased | StageEvent::PointerLeft => {
                if event == StageEvent::PointerLeft {
                    self.pointer = None;
                }
                if self.drag_offset.take().is_none() {
                    return Reaction::NONE;
                }
                let clicked = !self.drag_moved;
                self.drag_moved = false;
                if let Actor::Pet(pet) = &mut self.actor {
                    if clicked {
                        // 只是点了一下 → 受惊
                        let len = pet.clip_seconds(PetReaction::Startled);
                        pet.react(PetReaction::Startled, len);
                    } else {
                        // 拎着放下 → 落回地面(下落动画等有 JumpFall 再说)
                        pet.activity = Activity::Idle { remaining: 1.5 };
                        pet.player.play(pet.clips.idle);
                    }
                }
                self.pos.1 = match self.actor {
                    Actor::Pet(_) => self.ground_y(),
                    Actor::Sprite(_) => self.pos.1,
                };
                Reaction::BOTH
            }
            StageEvent::TogglePassthrough => {
                self.passthrough = !self.passthrough;
                self.drag_offset = None;
                Reaction::BOTH
            }
        }
    }

    /// 装上新回读到的轮廓掩码(见 pet/mask.rs),顺带刷新输入区。
    pub fn set_pet_mask(&mut self, mask: Mask) -> Reaction {
        if let Actor::Pet(pet) = &mut self.actor {
            pet.mask = Some(mask);
            self.coverage = self.actor.coverage();
            return Reaction {
                redraw: false,
                regions_dirty: true,
            };
        }
        Reaction::NONE
    }

    /// 指针在宠物头部区域移动:来回蹭够次数就算摸头。
    fn feed_petting(&mut self, x: f64, y: f64) -> Reaction {
        if self.passthrough || !self.hit_test(x, y) {
            if let Actor::Pet(pet) = &mut self.actor {
                pet.petting.reset();
            }
            return Reaction::NONE;
        }
        let local_y = (y - self.pos.1 as f64) as f32;
        let Actor::Pet(pet) = &mut self.actor else {
            return Reaction::NONE;
        };
        // 只认头部:身上蹭不算摸头
        if local_y > pet.size.1 as f32 * HEAD_ZONE {
            pet.petting.reset();
            return Reaction::NONE;
        }
        if !pet.petting.feed(x) {
            return Reaction::NONE;
        }
        // 正在被拎着或已经在反应中就不打断
        if matches!(pet.activity, Activity::Dragged | Activity::React { .. }) {
            return Reaction::NONE;
        }
        let len = pet.clip_seconds(PetReaction::Petted);
        pet.react(PetReaction::Petted, len);
        Reaction::REDRAW
    }

    /// 下一次推进该隔多久。只有「正在播的动作几乎不动」时才降频(见 STILL_MOTION)。
    pub fn tick_interval(&self) -> Duration {
        let hz = match &self.actor {
            Actor::Pet(pet) => {
                // 行走/拖动/反应中位置本身在变,不看姿势也得跑满
                let busy =
                    self.drag_offset.is_some() || !matches!(pet.activity, Activity::Idle { .. });
                if busy {
                    ACTIVE_HZ
                } else {
                    hz_for_motion(pet.player.motion())
                }
            }
            Actor::Sprite(_) => ACTIVE_HZ,
        };
        Duration::from_secs_f32(1.0 / hz)
    }

    /// 推进时间:宠物的行为与动画。返回是否要重画/重设输入区。
    pub fn tick(&mut self, dt: f32) -> Reaction {
        let surface_width = self.size.0 as f32;
        let dragging = self.drag_offset.is_some();
        let Actor::Pet(pet) = &mut self.actor else {
            return Reaction::NONE;
        };

        pet.petting.tick(dt);
        let mut moved = false;
        if !dragging {
            match pet.activity {
                Activity::Dragged => {
                    pet.activity = Activity::Idle { remaining: 1.0 };
                    pet.player.play(pet.clips.idle);
                }
                Activity::React { remaining } => {
                    let remaining = remaining - dt;
                    if remaining > 0.0 {
                        pet.activity = Activity::React { remaining };
                    } else {
                        pet.activity = Activity::Idle {
                            remaining: 1.0 + pet.rng.next_f32() * 2.0,
                        };
                        pet.player.play(pet.clips.idle);
                    }
                }
                Activity::Idle { remaining } => {
                    let remaining = remaining - dt;
                    if remaining > 0.0 {
                        pet.activity = Activity::Idle { remaining };
                    } else {
                        // 挑一个新去处:走不动(没有 Walk 动作)就继续待机
                        let max_x = (surface_width - pet.size.0 as f32).max(0.0);
                        let target_x = pet.rng.next_f32() * max_x;
                        let far_enough = (target_x - self.pos.0).abs() > pet.size.0 as f32 * 0.25;
                        match (pet.clips.walk, far_enough) {
                            (Some(walk), true) => {
                                pet.activity = Activity::Walk { target_x };
                                pet.target_yaw = camera_yaw(target_x > self.pos.0);
                                pet.player.play(walk);
                            }
                            _ => {
                                pet.activity = Activity::Idle {
                                    remaining: 1.5 + pet.rng.next_f32() * 3.0,
                                }
                            }
                        }
                    }
                }
                Activity::Walk { target_x } => {
                    let delta = target_x - self.pos.0;
                    let step = pet.walk_speed * dt;
                    if delta.abs() <= step {
                        self.pos.0 = target_x;
                        pet.activity = Activity::Idle {
                            remaining: 1.5 + pet.rng.next_f32() * 3.0,
                        };
                        pet.target_yaw = 0.0;
                        pet.player.play(pet.clips.idle);
                    } else {
                        self.pos.0 += step * delta.signum();
                    }
                    moved = true;
                }
            }
        }

        // 转身不硬切,朝目标角插过去(约 0.25s 转 90°)
        let turn = 6.0 * dt;
        let diff = pet.target_yaw - pet.yaw;
        pet.yaw += diff.clamp(-turn, turn);

        pet.player.advance(&pet.model, dt);
        pet.player.update(&pet.model);

        if moved {
            self.clamp_to_surface();
        }
        // 动画一直在动,所以每 tick 都要重画;位置变了才需要重设输入区
        Reaction {
            redraw: true,
            regions_dirty: moved,
        }
    }
}

/// 小号 xorshift:行为里只需要「随机挑个去处」,不值得为此引一个 rand 依赖。
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_f32(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        // 取高 24 位映射到 [0, 1)
        ((x >> 40) as f32) / (1u32 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage() -> Stage {
        Stage::new(Actor::Sprite(Sprite::test_pattern(64)), (800, 600))
    }

    #[test]
    fn starts_centered_and_regions_follow_actor() {
        let s = stage();
        assert_eq!(s.actor_pos(), (368.0, 268.0));
        let regions = s.input_regions();
        assert!(!regions.is_empty());
        // 输入区跟着角色平移:圆心必被覆盖,角色外必不被覆盖
        assert!(regions.iter().any(|r| r.contains(400.0, 300.0)));
        assert!(!regions.iter().any(|r| r.contains(10.0, 10.0)));
    }

    #[test]
    fn drag_moves_actor_and_dirties_regions() {
        let mut s = stage();
        let start = s.actor_pos();
        // 按在圆心上
        let hit = s.handle(StageEvent::PointerPressed { x: 400.0, y: 300.0 });
        assert!(hit.redraw && s.is_dragging());
        let moved = s.handle(StageEvent::PointerMoved { x: 450.0, y: 320.0 });
        assert_eq!(
            moved,
            Reaction {
                redraw: true,
                regions_dirty: true
            }
        );
        assert_eq!(s.actor_pos(), (start.0 + 50.0, start.1 + 20.0));
        s.handle(StageEvent::PointerReleased);
        assert!(!s.is_dragging());
    }

    #[test]
    fn press_on_transparent_pixel_is_ignored() {
        let mut s = stage();
        // 精灵包围盒左上角是圆外的透明像素
        let (px, py) = s.actor_pos();
        assert_eq!(
            s.handle(StageEvent::PointerPressed {
                x: px as f64,
                y: py as f64
            }),
            Reaction::NONE
        );
        assert!(!s.is_dragging());
    }

    #[test]
    fn passthrough_clears_regions_and_blocks_drag() {
        let mut s = stage();
        assert_eq!(s.handle(StageEvent::TogglePassthrough), Reaction::BOTH);
        assert!(s.passthrough());
        assert!(s.input_regions().is_empty());
        s.handle(StageEvent::PointerPressed { x: 400.0, y: 300.0 });
        assert!(!s.is_dragging());
    }

    #[test]
    fn actor_stays_inside_after_shrink() {
        let mut s = stage();
        s.handle(StageEvent::PointerPressed { x: 400.0, y: 300.0 });
        s.handle(StageEvent::PointerMoved { x: 790.0, y: 590.0 });
        s.handle(StageEvent::PointerReleased);
        s.handle(StageEvent::Resized {
            width: 300,
            height: 200,
        });
        let (x, y) = s.actor_pos();
        assert!(x + 64.0 <= 300.0 && y + 64.0 <= 200.0, "角色越界: {x},{y}");
    }

    #[test]
    fn sprite_actor_does_not_tick() {
        let mut s = stage();
        assert_eq!(s.tick(0.1), Reaction::NONE);
    }

    #[test]
    fn rng_stays_in_unit_range() {
        let mut rng = Rng::new(12345);
        for _ in 0..1000 {
            let v = rng.next_f32();
            assert!((0.0..1.0).contains(&v), "越界: {v}");
        }
    }
}

#[cfg(test)]
mod pet_tests {
    use super::*;

    /// 一只测试宠物:200×200 的画布,脚底在 180,走速 100px/s。
    fn pet_stage() -> Stage {
        let model = Model::for_test(&["Idle", "Walk", "Shock", "Happy", "Fear"]);
        let actor = Actor::Pet(PetActor::new(model, (200, 200), 180.0, 100.0, 7));
        Stage::new(actor, (1000, 600))
    }

    fn activity(stage: &Stage) -> Activity {
        match stage.actor() {
            Actor::Pet(pet) => pet.activity,
            _ => panic!("不是宠物"),
        }
    }

    /// 宠物中心附近的表面坐标(必落在包围盒内)。
    fn center(stage: &Stage) -> (f64, f64) {
        let (x, y) = stage.actor_pos();
        (x as f64 + 100.0, y as f64 + 100.0)
    }

    #[test]
    fn stands_on_the_ground_line() {
        let s = pet_stage();
        // 脚底(180)应落在屏幕底边上方 GROUND_MARGIN 处
        assert_eq!(s.actor_pos().1 + 180.0, 600.0 - GROUND_MARGIN);
    }

    #[test]
    fn click_without_moving_startles() {
        let mut s = pet_stage();
        let (x, y) = center(&s);
        s.handle(StageEvent::PointerPressed { x, y });
        // 没移动就松手 = 点了一下
        s.handle(StageEvent::PointerReleased);
        assert!(
            matches!(activity(&s), Activity::React { .. }),
            "点击应触发反应(受惊)"
        );
        // 反应播完回到待机
        for _ in 0..40 {
            s.tick(0.05);
        }
        assert!(
            matches!(activity(&s), Activity::Idle { .. }),
            "反应结束该回待机"
        );
    }

    #[test]
    fn dragging_picks_up_then_lands() {
        let mut s = pet_stage();
        let (x, y) = center(&s);
        s.handle(StageEvent::PointerPressed { x, y });
        s.handle(StageEvent::PointerMoved {
            x: x + 60.0,
            y: y - 120.0,
        });
        assert_eq!(activity(&s), Activity::Dragged, "移动超过阈值算拎起来");
        assert!(s.is_dragging());
        s.handle(StageEvent::PointerReleased);
        assert!(matches!(activity(&s), Activity::Idle { .. }));
        // 松手落回地面
        assert_eq!(s.actor_pos().1 + 180.0, 600.0 - GROUND_MARGIN);
    }

    #[test]
    fn rubbing_the_head_pets_it() {
        let mut s = pet_stage();
        let (x, y) = center(&s);
        let head_y = y - 60.0; // 落在上 45% 的头部区域
        // 只蹭一下不算:得来回换向够 PET_REVERSALS 次
        s.handle(StageEvent::PointerMoved { x, y: head_y });
        s.handle(StageEvent::PointerMoved {
            x: x + 30.0,
            y: head_y,
        });
        assert!(
            matches!(activity(&s), Activity::Idle { .. }),
            "单向划过不该算摸头"
        );
        for dx in [-30.0, 30.0, -30.0] {
            s.handle(StageEvent::PointerMoved {
                x: x + dx,
                y: head_y,
            });
        }
        assert!(
            matches!(activity(&s), Activity::React { .. }),
            "来回蹭够次数应触发反应(开心)"
        );
    }

    #[test]
    fn rubbing_the_body_does_not_count() {
        let mut s = pet_stage();
        let (x, y) = center(&s);
        let body_y = y + 60.0; // 头部区域之外
        for dx in [0.0, 30.0, -30.0, 30.0, -30.0, 30.0] {
            s.handle(StageEvent::PointerMoved {
                x: x + dx,
                y: body_y,
            });
        }
        assert!(
            matches!(activity(&s), Activity::Idle { .. }),
            "在身上蹭不算摸头"
        );
    }

    #[test]
    fn still_pose_ticks_slower_than_moving() {
        let mut s = pet_stage();
        // 合成模型没有动画通道 → 姿势纹丝不动,量过两帧后应当降频
        s.tick(0.05);
        s.tick(0.05);
        let still = s.tick_interval();
        assert!(
            still > Duration::from_secs_f32(1.0 / 20.0),
            "静止姿势该降频"
        );
        // 逼它走起来:待机计时耗尽后会挑目标点
        for _ in 0..100 {
            s.tick(0.1);
            if matches!(activity(&s), Activity::Walk { .. }) {
                break;
            }
        }
        assert!(
            matches!(activity(&s), Activity::Walk { .. }),
            "待机够久该开始走"
        );
        assert!(s.tick_interval() < still, "行走时该比静止更勤地推进");
    }

    #[test]
    fn walking_reaches_its_target_and_faces_that_way() {
        let mut s = pet_stage();
        for _ in 0..100 {
            s.tick(0.1);
            if matches!(activity(&s), Activity::Walk { .. }) {
                break;
            }
        }
        let Activity::Walk { target_x } = activity(&s) else {
            panic!("没走起来")
        };
        let going_right = target_x > s.actor_pos().0;
        // 朝向与目标方向一致(camera_yaw 的符号在 pet/target.rs 里有回归测试)
        match s.actor() {
            Actor::Pet(pet) => assert_eq!(pet.target_yaw, camera_yaw(going_right)),
            _ => panic!("不是宠物"),
        }
        for _ in 0..200 {
            s.tick(0.05);
            if matches!(activity(&s), Activity::Idle { .. }) {
                break;
            }
        }
        assert!((s.actor_pos().0 - target_x).abs() < 1.0, "该走到目标点");
    }
}

#[cfg(test)]
mod rate_tests {
    use super::*;

    /// 把「姿势速度 → 帧率」的映射钉住:待机实测 ~6m/s 必须跑满,
    /// 睡觉那类近乎静止的必须落到下限,中间连续过渡。
    #[test]
    fn frame_rate_follows_motion() {
        assert_eq!(hz_for_motion(0.0), STILL_HZ);
        assert_eq!(hz_for_motion(6.0), ACTIVE_HZ, "待机实测量级该跑满帧");
        assert_eq!(hz_for_motion(4.7), ACTIVE_HZ, "行走实测量级该跑满帧");
        let mid = hz_for_motion(0.5);
        assert!(
            mid > STILL_HZ && mid < ACTIVE_HZ,
            "中间值该连续过渡,实际 {mid}"
        );
    }
}
