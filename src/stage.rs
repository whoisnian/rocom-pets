//! 与平台无关的 stage 逻辑:角色在屏幕上的位置、拖动、命中测试、输入区,以及宠物的行为。
//!
//! 平台后端只负责「造表面 / 收事件 / 出帧 / 设输入区」,所有状态都在这里,
//! 这样 Wayland 与 Windows 两边的行为天然一致,也能脱离窗口系统做单元测试。

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

/// 宠物当前在干什么。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Activity {
    /// 站着待机,`remaining` 秒后换个地方走走。
    Idle { remaining: f32 },
    /// 走向 `target_x`(左上角的目标 x)。
    Walk { target_x: f32 },
    /// 被鼠标拎着。
    Dragged,
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
    clips: Clips,
    rng: Rng,
}

/// 逻辑动作在 glb 里的下标;缺的就是 None,行为要能降级。
struct Clips {
    idle: usize,
    walk: Option<usize>,
}

impl PetActor {
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
            clips: Clips { idle, walk },
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
            // 宠物暂时按包围盒判定:逐像素要回读离屏画布的 alpha,排在 Phase 2
            Actor::Pet(pet) => {
                lx >= 0 && ly >= 0 && (lx as u32) < pet.size.0 && (ly as u32) < pet.size.1
            }
        }
    }

    /// 角色局部坐标下的输入区矩形。
    fn coverage(&self) -> Vec<Rect> {
        match self {
            Actor::Sprite(sprite) => sprite.coverage_rects(REGION_CELL, REGION_ALPHA_THRESHOLD),
            Actor::Pet(pet) => {
                vec![Rect {
                    x: 0,
                    y: 0,
                    w: pet.size.0,
                    h: pet.size.1,
                }]
            }
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
                match self.drag_offset {
                    Some((ox, oy)) => {
                        self.pos = ((x - ox) as f32, (y - oy) as f32);
                        self.clamp_to_surface();
                        Reaction::BOTH
                    }
                    None => Reaction::NONE,
                }
            }
            StageEvent::PointerPressed { x, y } => {
                self.pointer = Some((x, y));
                if self.passthrough || !self.hit_test(x, y) {
                    return Reaction::NONE;
                }
                self.drag_offset = Some((x - self.pos.0 as f64, y - self.pos.1 as f64));
                if let Actor::Pet(pet) = &mut self.actor {
                    pet.activity = Activity::Dragged;
                }
                Reaction::REDRAW
            }
            StageEvent::PointerReleased | StageEvent::PointerLeft => {
                if event == StageEvent::PointerLeft {
                    self.pointer = None;
                }
                if self.drag_offset.take().is_none() {
                    return Reaction::NONE;
                }
                if let Actor::Pet(pet) = &mut self.actor {
                    // 松手就落回地面。真正的下落动画(Jump_Fall)排 Phase 2
                    pet.activity = Activity::Idle { remaining: 1.5 };
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

    /// 推进时间:宠物的行为与动画。返回是否要重画/重设输入区。
    pub fn tick(&mut self, dt: f32) -> Reaction {
        let surface_width = self.size.0 as f32;
        let dragging = self.drag_offset.is_some();
        let Actor::Pet(pet) = &mut self.actor else {
            return Reaction::NONE;
        };

        let mut moved = false;
        if !dragging {
            match pet.activity {
                Activity::Dragged => pet.activity = Activity::Idle { remaining: 1.0 },
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
                                pet.target_yaw = if target_x > self.pos.0 {
                                    std::f32::consts::FRAC_PI_2
                                } else {
                                    -std::f32::consts::FRAC_PI_2
                                };
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
