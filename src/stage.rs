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

/// 需求值的时间尺度(秒)。桌宠的节奏要慢:困倦几分钟才攒满,不然一直在睡。
/// 这些值是「手感常量」,不是从游戏数据来的——游戏里没有对应概念(AI 行为树不移植)。
const SLEEPY_BUILD_SECS: f32 = 480.0;
/// 睡这么久就睡饱了。
const SLEEPY_RECOVER_SECS: f32 = 90.0;
/// 待机这么久就无聊到想动一动。
const BORED_BUILD_SECS: f32 = 6.0;
/// 走动/互动能消掉多快的无聊。
const BORED_RELIEF_SECS: f32 = 2.0;
/// 困倦超过这个值就去睡,低于 SLEEPY_WAKE 就醒。
const SLEEPY_SLEEP_AT: f32 = 0.85;
const SLEEPY_WAKE_AT: f32 = 0.25;
/// 无聊超过这个值就换个地方走走。
const BORED_WALK_AT: f32 = 0.6;
/// 无聊时不走动而是随手做个表情的概率。
const EMOTE_CHANCE: f32 = 0.35;

/// 指针悬在身上时,朝它侧一点身(不是完整转 90°,读起来像「瞥一眼」)。
/// 真正的视线跟随需要 LookAt BlendSpace(没导出),而且 Wayland 下输入区外根本收不到
/// 指针事件——想追全屏光标就得吃掉输入,不做。
const GLANCE_RATIO: f32 = 0.45;

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

/// 状态的可读名字,只用于日志(睡觉的三段也分开,便于确认作息真的走完了)。
fn activity_label(activity: &Activity) -> &'static str {
    match activity {
        Activity::Idle { .. } => "待机",
        Activity::Walk { .. } => "行走",
        Activity::Dragged => "被拎着",
        Activity::React { .. } => "反应",
        Activity::Sleeping(SleepPhase::Falling { .. }) => "入睡",
        Activity::Sleeping(SleepPhase::Asleep) => "睡着",
        Activity::Sleeping(SleepPhase::Waking { .. }) => "醒来",
    }
}

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
    /// 一次性反应/表情,播完 `remaining` 秒回到待机。
    React { remaining: f32 },
    /// 睡觉:三段式(入睡 → 循环 → 醒来),见 docs/design.md §5。
    Sleeping(SleepPhase),
}

/// 睡觉的三段。`Loop` 会一直循环到睡饱或被打扰。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SleepPhase {
    Falling { remaining: f32 },
    Asleep,
    Waking { remaining: f32 },
}

/// 宠物的内部需求,驱动「接下来干什么」。
#[derive(Debug, Clone, Copy, Default)]
pub struct Needs {
    /// 困倦 0..1。
    pub sleepiness: f32,
    /// 无聊 0..1。
    pub boredom: f32,
}

impl Needs {
    /// 需求值推进的倍速。`ROCOM_PETS_NEEDS_SPEED=20` 可以把作息压缩 20 倍,
    /// 用来在几十秒内看完「困→睡→醒」整套,而不是等八分钟。
    fn speed() -> f32 {
        static SPEED: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
        *SPEED.get_or_init(|| {
            std::env::var("ROCOM_PETS_NEEDS_SPEED")
                .ok()
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|v| *v > 0.0)
                .unwrap_or(1.0)
        })
    }

    fn tick(&mut self, dt: f32, activity: &Activity) {
        let dt = dt * Self::speed();
        match activity {
            Activity::Sleeping(_) => {
                self.sleepiness -= dt / SLEEPY_RECOVER_SECS;
                self.boredom = 0.0;
            }
            Activity::Idle { .. } => {
                self.sleepiness += dt / SLEEPY_BUILD_SECS;
                self.boredom += dt / BORED_BUILD_SECS;
            }
            // 走动与互动都算「有事做」,消无聊
            _ => {
                self.sleepiness += dt / SLEEPY_BUILD_SECS;
                self.boredom -= dt / BORED_RELIEF_SECS;
            }
        }
        self.sleepiness = self.sleepiness.clamp(0.0, 1.0);
        self.boredom = self.boredom.clamp(0.0, 1.0);
    }
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
    /// 内部需求(困倦/无聊),决定待机结束后干什么。
    pub needs: Needs,
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
    /// 睡觉三段式;缺 Start/End 就直接进/出 Loop。
    sleep_start: Option<usize>,
    sleep_loop: Option<usize>,
    sleep_end: Option<usize>,
    /// 待机时随手做的表情池(有哪个算哪个)。
    emotes: Vec<usize>,
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

    /// 待机结束后干什么:困了去睡,无聊了走动或做个表情,否则继续待机。
    fn choose_next(&mut self, pos_x: f32, max_x: f32) {
        if self.needs.sleepiness >= SLEEPY_SLEEP_AT && self.clips.sleep_loop.is_some() {
            self.start_sleep();
            return;
        }
        if self.needs.boredom >= BORED_WALK_AT {
            // 无聊到想动:多数时候换个地方走走,偶尔只是做个表情
            let emote = !self.clips.emotes.is_empty() && self.rng.next_f32() < EMOTE_CHANCE;
            if !emote {
                let target_x = self.rng.next_f32() * max_x;
                let far_enough = (target_x - pos_x).abs() > self.size.0 as f32 * 0.25;
                if let (Some(walk), true) = (self.clips.walk, far_enough) {
                    self.activity = Activity::Walk { target_x };
                    self.target_yaw = camera_yaw(target_x > pos_x);
                    self.player.play(walk);
                    self.needs.boredom = 0.0;
                    return;
                }
            }
            if let Some(&clip) = self.pick_emote() {
                self.player.play(clip);
                self.activity = Activity::React {
                    remaining: self.model.clips[clip].duration.max(0.3),
                };
                self.needs.boredom = 0.0;
                return;
            }
        }
        self.activity = Activity::Idle {
            remaining: 1.5 + self.rng.next_f32() * 3.0,
        };
    }

    fn pick_emote(&mut self) -> Option<&usize> {
        if self.clips.emotes.is_empty() {
            return None;
        }
        let index = (self.rng.next_f32() * self.clips.emotes.len() as f32) as usize;
        self.clips
            .emotes
            .get(index.min(self.clips.emotes.len() - 1))
    }

    /// 进入睡觉:有 SleepStart 就先播入睡,否则直接躺下。
    fn start_sleep(&mut self) {
        self.target_yaw = 0.0;
        match self.clips.sleep_start {
            Some(clip) => {
                self.player.play(clip);
                self.activity = Activity::Sleeping(SleepPhase::Falling {
                    remaining: self.model.clips[clip].duration.max(0.2),
                });
            }
            None => self.enter_asleep(),
        }
    }

    fn enter_asleep(&mut self) {
        if let Some(clip) = self.clips.sleep_loop {
            self.player.play(clip);
        }
        self.activity = Activity::Sleeping(SleepPhase::Asleep);
    }

    /// 醒来:有 SleepEnd 就先播,否则直接站起。被打扰时也走这里。
    fn wake_up(&mut self) {
        match self.clips.sleep_end {
            Some(clip) => {
                self.player.play(clip);
                self.activity = Activity::Sleeping(SleepPhase::Waking {
                    remaining: self.model.clips[clip].duration.max(0.2),
                });
            }
            None => {
                self.player.play(self.clips.idle);
                self.activity = Activity::Idle { remaining: 1.0 };
            }
        }
    }

    /// 睡觉三段式的推进。循环段一直睡到睡饱(困倦降到 SLEEPY_WAKE_AT)。
    fn tick_sleep(&mut self, phase: SleepPhase, dt: f32) {
        match phase {
            SleepPhase::Falling { remaining } => {
                let remaining = remaining - dt;
                if remaining > 0.0 {
                    self.activity = Activity::Sleeping(SleepPhase::Falling { remaining });
                } else {
                    self.enter_asleep();
                }
            }
            SleepPhase::Asleep => {
                if self.needs.sleepiness <= SLEEPY_WAKE_AT {
                    self.wake_up();
                }
            }
            SleepPhase::Waking { remaining } => {
                let remaining = remaining - dt;
                if remaining > 0.0 {
                    self.activity = Activity::Sleeping(SleepPhase::Waking { remaining });
                } else {
                    self.player.play(self.clips.idle);
                    self.activity = Activity::Idle { remaining: 1.0 };
                }
            }
        }
    }

    /// 是否在睡(含入睡/醒来过程)。
    pub fn is_sleeping(&self) -> bool {
        matches!(self.activity, Activity::Sleeping(_))
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
        let sleep_start = model.clip("SleepStart");
        let sleep_loop = model.clip("SleepLoop").or_else(|| model.clip("SleepStand"));
        let sleep_end = model.clip("SleepEnd");
        // 表情池:待机时偶尔来一个,让它看着不是只会站桩
        let emotes = ["Happy", "Sad", "Anger", "Show", "Relax", "Alert"]
            .iter()
            .filter_map(|name| model.clip(name))
            .collect();
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
            needs: Needs::default(),
            clips: Clips {
                idle,
                walk,
                startled,
                happy,
                afraid,
                sleep_start,
                sleep_loop,
                sleep_end,
                emotes,
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

/// 实体的稳定标识。**不是 `entities` 里的下标** —— 移除一只之后下标会滑动,
/// 而托盘的「移除这只」与掩码回读都跨帧持有标识。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EntityId(u64);

/// 在场的一只:角色本体 + 它自己的位置与拖动状态。
///
/// **拖动状态必须挂在实体上而不是 `Stage` 上**:多只同时在场时,拎起来的是被点中的
/// 那一只,其余照常待机。
pub struct Entity {
    id: EntityId,
    actor: Actor,
    /// 角色左上角在表面内的位置。
    pos: (f32, f32),
    /// 角色局部坐标下的覆盖矩形,只在角色变了才重算。
    coverage: Vec<Rect>,
    /// 按下时记下的「指针 - 角色左上角」偏移。
    drag_offset: Option<(f64, f64)>,
    /// 本次按下之后指针是否移动过(用来区分「点一下」与「拎起来拖」)。
    drag_moved: bool,
}

impl Entity {
    fn new(id: EntityId, actor: Actor, size: (u32, u32)) -> Self {
        let coverage = actor.coverage();
        let mut entity = Self {
            id,
            actor,
            pos: (0.0, 0.0),
            coverage,
            drag_offset: None,
            drag_moved: false,
        };
        entity.reset_position(size);
        entity
    }

    // 平台层还按「一只」渲染(Phase 5 第 2 步才改),这几个访问器先没人调。
    // 和 pack.rs 里那批 manifest 字段同一处理:照契约留着,比等到要用时再补更省事。
    #[allow(dead_code)]
    pub fn id(&self) -> EntityId {
        self.id
    }

    #[allow(dead_code)]
    pub fn actor(&self) -> &Actor {
        &self.actor
    }

    #[allow(dead_code)]
    pub fn pos(&self) -> (f32, f32) {
        self.pos
    }

    pub fn is_dragging(&self) -> bool {
        self.drag_offset.is_some()
    }

    /// 脚底在表面里的 y。**z 序就按它排**:脚底越靠下的越靠前(挡住后面的)。
    fn foot_y(&self) -> f32 {
        match &self.actor {
            Actor::Pet(pet) => self.pos.1 + pet.foot_offset,
            Actor::Sprite(sprite) => self.pos.1 + sprite.height as f32,
        }
    }

    /// 摆到初始位置:精灵居中(调试用),宠物站到屏幕底边中间。
    fn reset_position(&mut self, size: (u32, u32)) {
        let (w, h) = self.actor.size();
        self.pos.0 = (size.0 as f32 - w as f32) * 0.5;
        self.pos.1 = match self.actor {
            Actor::Sprite(_) => (size.1 as f32 - h as f32) * 0.5,
            Actor::Pet(_) => self.ground_y(size),
        };
        self.clamp_to_surface(size);
    }

    /// 宠物站立时左上角该在的 y:让脚底落在屏幕底边上方 GROUND_MARGIN 处。
    fn ground_y(&self, size: (u32, u32)) -> f32 {
        let foot = match &self.actor {
            Actor::Pet(pet) => pet.foot_offset,
            Actor::Sprite(sprite) => sprite.height as f32,
        };
        (size.1 as f32 - GROUND_MARGIN - foot).max(0.0)
    }

    fn clamp_to_surface(&mut self, size: (u32, u32)) {
        let (w, h) = self.actor.size();
        let max_x = (size.0 as f32 - w as f32).max(0.0);
        self.pos.0 = self.pos.0.clamp(0.0, max_x);
        match &self.actor {
            // 宠物的画布比它本身大(取景留了余量),按画布夹会把它顶离地面。
            // 真正该约束的是**脚底**留在屏幕内,画布超出边界让它被裁掉就好。
            Actor::Pet(pet) => {
                let min_y = -(h as f32 - pet.foot_offset);
                let max_y = size.1 as f32 - pet.foot_offset;
                self.pos.1 = self.pos.1.clamp(min_y.min(max_y), max_y);
            }
            Actor::Sprite(_) => {
                let max_y = (size.1 as f32 - h as f32).max(0.0);
                self.pos.1 = self.pos.1.clamp(0.0, max_y);
            }
        }
    }

    /// 这只自己想要的推进频率。行走/拖动/反应中位置本身在变,不看姿势也得跑满;
    /// 待机与睡觉交给姿势速度决定(睡着几乎不动 → 自动落到下限)。
    fn tick_hz(&self) -> f32 {
        match &self.actor {
            Actor::Pet(pet) => {
                let busy = self.drag_offset.is_some()
                    || matches!(
                        pet.activity,
                        Activity::Walk { .. } | Activity::React { .. } | Activity::Dragged
                    );
                if busy {
                    ACTIVE_HZ
                } else {
                    hz_for_motion(pet.player.motion())
                }
            }
            Actor::Sprite(_) => ACTIVE_HZ,
        }
    }

    /// 表面坐标是否落在这只的可见部分上。
    fn hit_test(&self, x: f64, y: f64) -> bool {
        let lx = (x - self.pos.0 as f64).floor() as i32;
        let ly = (y - self.pos.1 as f64).floor() as i32;
        self.actor.hit(lx, ly)
    }

    /// 这只在表面坐标下的输入矩形。
    fn input_rects(&self) -> impl Iterator<Item = Rect> + '_ {
        let (dx, dy) = (self.pos.0.round() as i32, self.pos.1.round() as i32);
        self.coverage.iter().map(move |r| r.translated(dx, dy))
    }
}

pub struct Stage {
    /// 在场的实体。**顺序即绘制顺序**(靠后的画在上面);命中测试另按脚底 y 取最上面那只。
    entities: Vec<Entity>,
    next_id: u64,
    /// 表面尺寸(逻辑像素)。
    size: (u32, u32),
    pointer: Option<(f64, f64)>,
    passthrough: bool,
}

impl Stage {
    pub fn new(actor: Actor, size: (u32, u32)) -> Self {
        let mut stage = Self {
            entities: Vec::new(),
            next_id: 0,
            size,
            pointer: None,
            passthrough: false,
        };
        stage.spawn(actor);
        stage
    }

    /// 放一只上台,返回它的标识。
    pub fn spawn(&mut self, actor: Actor) -> EntityId {
        let id = EntityId(self.next_id);
        self.next_id += 1;
        self.entities.push(Entity::new(id, actor, self.size));
        id
    }

    /// 撤掉一只。找不到就是 false(标识可能已经失效)。
    #[allow(dead_code)] // 托盘的「移除这只」在 Phase 5 第 7 步接
    pub fn despawn(&mut self, id: EntityId) -> bool {
        let before = self.entities.len();
        self.entities.retain(|e| e.id != id);
        self.entities.len() != before
    }

    #[allow(dead_code)] // 平台层按实体渲染时用(Phase 5 第 2 步)
    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    fn entity_mut(&mut self, id: EntityId) -> Option<&mut Entity> {
        self.entities.iter_mut().find(|e| e.id == id)
    }

    /// 命中测试:**取最上面的那一只**。z 序按脚底 y(越靠下越靠前),
    /// 脚底相同则取后加入的(绘制顺序里在上面)。
    pub fn pick(&self, x: f64, y: f64) -> Option<EntityId> {
        self.entities
            .iter()
            .filter(|e| e.hit_test(x, y))
            .max_by(|a, b| {
                a.foot_y()
                    .total_cmp(&b.foot_y())
                    .then(a.id.0.cmp(&b.id.0))
            })
            .map(|e| e.id)
    }

    // ── 单实体便利访问 ────────────────────────────────────────────
    // 平台层目前仍按「一只」写(见 design.md §9 Phase 5 第 1 步),这几个先落到
    // **第一只**上。等平台层改成按实体渲染后一并删掉。

    /// 第一只(平台层过渡用)。台上至少有一只是 `Stage::new` 保证的。
    fn primary(&self) -> &Entity {
        &self.entities[0]
    }

    fn primary_mut(&mut self) -> &mut Entity {
        &mut self.entities[0]
    }

    pub fn actor(&self) -> &Actor {
        &self.primary().actor
    }

    /// 换角色(切形态):尺寸与轮廓都变了,重算覆盖区并重新落地。
    pub fn replace_actor(&mut self, actor: Actor) {
        let size = self.size;
        let entity = self.primary_mut();
        entity.actor = actor;
        entity.coverage = entity.actor.coverage();
        entity.drag_offset = None;
        entity.drag_moved = false;
        entity.reset_position(size);
    }

    /// 只给测试用:直接改角色状态(比如把困倦顶到阈值,省去等几分钟)。
    #[cfg(test)]
    pub fn actor_mut_for_test(&mut self) -> &mut Actor {
        &mut self.primary_mut().actor
    }

    /// 角色左上角位置(表面局部逻辑像素)。
    pub fn actor_pos(&self) -> (f32, f32) {
        self.primary().pos
    }

    pub fn passthrough(&self) -> bool {
        self.passthrough
    }

    /// 有没有哪一只正被拎着。
    pub fn is_dragging(&self) -> bool {
        self.entities.iter().any(Entity::is_dragging)
    }

    /// 所有实体重新落地(改屏幕尺寸/切形态之后)。
    pub fn reset_position(&mut self) {
        let size = self.size;
        for entity in &mut self.entities {
            entity.reset_position(size);
        }
    }

    /// 当前该交给合成器的输入区(表面局部坐标)。穿透时为空。
    ///
    /// **取各实体的并集**。这里不做去重/合并:合成器接受重叠矩形,而实体之间本来
    /// 就很少叠在一起;真要压条目数,该压的是单只那 60~87 个格子。
    pub fn input_regions(&self) -> Vec<Rect> {
        if self.passthrough {
            return Vec::new();
        }
        self.entities.iter().flat_map(Entity::input_rects).collect()
    }

    /// 表面坐标是否落在**任何一只**的可见部分上(比输入区更精确,用于自己内部的判定)。
    #[allow(dead_code)] // 内部判定现在都走 `pick`;这条留给平台层的命中查询
    pub fn hit_test(&self, x: f64, y: f64) -> bool {
        self.pick(x, y).is_some()
    }

    pub fn handle(&mut self, event: StageEvent) -> Reaction {
        match event {
            StageEvent::Resized { width, height } => {
                if (width, height) == self.size {
                    return Reaction::NONE;
                }
                self.size = (width, height);
                let size = self.size;
                for entity in &mut self.entities {
                    if matches!(entity.actor, Actor::Pet(_)) {
                        entity.pos.1 = entity.ground_y(size);
                    }
                    entity.clamp_to_surface(size);
                }
                Reaction::BOTH
            }
            StageEvent::PointerMoved { x, y } => {
                self.pointer = Some((x, y));
                // 正被拎着的那一只跟着指针走;其余照常
                let size = self.size;
                if let Some(entity) = self.entities.iter_mut().find(|e| e.is_dragging()) {
                    let (ox, oy) = entity.drag_offset.expect("刚判过在拖");
                    let moved = ((x - ox) as f32 - entity.pos.0).abs()
                        + ((y - oy) as f32 - entity.pos.1).abs();
                    if moved > DRAG_THRESHOLD {
                        entity.drag_moved = true;
                        if let Actor::Pet(pet) = &mut entity.actor {
                            // 真被拎起来了才播害怕:轻点一下不该惊动它
                            if pet.activity != Activity::Dragged {
                                let len = pet.clip_seconds(PetReaction::PickedUp);
                                pet.react(PetReaction::PickedUp, len);
                                pet.activity = Activity::Dragged;
                            }
                        }
                    }
                    entity.pos = ((x - ox) as f32, (y - oy) as f32);
                    entity.clamp_to_surface(size);
                    return Reaction::BOTH;
                }
                // 没在拖:看看是不是在头上来回蹭
                self.feed_petting(x, y)
            }
            StageEvent::PointerPressed { x, y } => {
                self.pointer = Some((x, y));
                if self.passthrough {
                    return Reaction::NONE;
                }
                let Some(id) = self.pick(x, y) else {
                    return Reaction::NONE;
                };
                let Some(entity) = self.entity_mut(id) else {
                    return Reaction::NONE;
                };
                entity.drag_offset = Some((x - entity.pos.0 as f64, y - entity.pos.1 as f64));
                entity.drag_moved = false;
                if let Actor::Pet(pet) = &mut entity.actor {
                    // 睡着时被戳 → 醒过来(而不是原地受惊)
                    if pet.is_sleeping() {
                        pet.wake_up();
                    }
                }
                Reaction::REDRAW
            }
            StageEvent::PointerReleased | StageEvent::PointerLeft => {
                if event == StageEvent::PointerLeft {
                    self.pointer = None;
                    // 指针走了:把瞥过去的身子转回正面(每一只都要)
                    for entity in &mut self.entities {
                        if let Actor::Pet(pet) = &mut entity.actor {
                            if matches!(pet.activity, Activity::Idle { .. }) {
                                pet.target_yaw = 0.0;
                            }
                            pet.petting.reset();
                        }
                    }
                }
                let size = self.size;
                let Some(entity) = self.entities.iter_mut().find(|e| e.is_dragging()) else {
                    return Reaction::NONE;
                };
                entity.drag_offset = None;
                let clicked = !entity.drag_moved;
                entity.drag_moved = false;
                if let Actor::Pet(pet) = &mut entity.actor {
                    if clicked && !pet.is_sleeping() {
                        // 只是点了一下 → 受惊(正在醒来的那一下不算)
                        let len = pet.clip_seconds(PetReaction::Startled);
                        pet.react(PetReaction::Startled, len);
                    } else if !clicked {
                        // 拎着放下 → 落回地面(下落动画等有 JumpFall 再说)
                        pet.activity = Activity::Idle { remaining: 1.5 };
                        pet.player.play(pet.clips.idle);
                    }
                }
                if matches!(entity.actor, Actor::Pet(_)) {
                    entity.pos.1 = entity.ground_y(size);
                }
                Reaction::BOTH
            }
            StageEvent::TogglePassthrough => {
                self.passthrough = !self.passthrough;
                for entity in &mut self.entities {
                    entity.drag_offset = None;
                }
                Reaction::BOTH
            }
        }
    }

    /// 装上新回读到的轮廓掩码(见 pet/mask.rs),顺带刷新输入区。
    pub fn set_pet_mask(&mut self, mask: Mask) -> Reaction {
        let entity = self.primary_mut();
        if let Actor::Pet(pet) = &mut entity.actor {
            pet.mask = Some(mask);
            entity.coverage = entity.actor.coverage();
            return Reaction {
                redraw: false,
                regions_dirty: true,
            };
        }
        Reaction::NONE
    }

    /// 指针在宠物头部区域移动:来回蹭够次数就算摸头。**只喂给指针下面那一只**,
    /// 其余的把蹭计数清零(指针已经不在它们身上了)。
    fn feed_petting(&mut self, x: f64, y: f64) -> Reaction {
        let picked = if self.passthrough {
            None
        } else {
            self.pick(x, y)
        };
        for entity in &mut self.entities {
            if Some(entity.id) != picked {
                if let Actor::Pet(pet) = &mut entity.actor {
                    pet.petting.reset();
                }
            }
        }
        let Some(picked) = picked else {
            return Reaction::NONE;
        };
        let Some(entity) = self.entity_mut(picked) else {
            return Reaction::NONE;
        };
        let local_y = (y - entity.pos.1 as f64) as f32;
        let center_x = entity.pos.0 as f64 + entity.actor.size().0 as f64 * 0.5;
        let Actor::Pet(pet) = &mut entity.actor else {
            return Reaction::NONE;
        };
        // 指针在身上(不限头部)就侧一点身,像是在瞥它
        if matches!(pet.activity, Activity::Idle { .. }) {
            pet.target_yaw = camera_yaw(x > center_x) * GLANCE_RATIO;
        }
        // 摸头只认头部:身上蹭不算
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

    /// 下一次推进该隔多久。只有「正在播的动作几乎不动」时才降频(见 `hz_for_motion`)。
    /// **取各实体里最快的那一个**:一只在走、其余在睡时,整台仍要按走的那只推进。
    pub fn tick_interval(&self) -> Duration {
        let hz = self
            .entities
            .iter()
            .map(Entity::tick_hz)
            .fold(f32::MIN, f32::max)
            .max(hz_for_motion(0.0));
        Duration::from_secs_f32(1.0 / hz)
    }

    /// 推进时间:宠物的行为与动画。返回是否要重画/重设输入区。
    pub fn tick(&mut self, dt: f32) -> Reaction {
        let size = self.size;
        let mut reaction = Reaction::NONE;
        for index in 0..self.entities.len() {
            let before = match &self.entities[index].actor {
                Actor::Pet(pet) => Some(activity_label(&pet.activity)),
                _ => None,
            };
            let one = Self::tick_entity(&mut self.entities[index], dt, size);
            if let (Some(before), Actor::Pet(pet)) = (before, &self.entities[index].actor) {
                if before != activity_label(&pet.activity) {
                    log::debug!(
                        "宠物 → {}(困倦 {:.2} 无聊 {:.2})",
                        activity_label(&pet.activity),
                        pet.needs.sleepiness,
                        pet.needs.boredom
                    );
                }
            }
            reaction.redraw |= one.redraw;
            reaction.regions_dirty |= one.regions_dirty;
        }
        reaction
    }

    fn tick_entity(entity: &mut Entity, dt: f32, size: (u32, u32)) -> Reaction {
        let surface_width = size.0 as f32;
        let dragging = entity.drag_offset.is_some();
        let Actor::Pet(pet) = &mut entity.actor else {
            return Reaction::NONE;
        };

        pet.petting.tick(dt);
        pet.needs.tick(dt, &pet.activity);
        let mut moved = false;
        if !dragging {
            match pet.activity {
                Activity::Dragged => {
                    // 刚被放下
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
                Activity::Sleeping(phase) => pet.tick_sleep(phase, dt),
                Activity::Idle { remaining } => {
                    let remaining = remaining - dt;
                    if remaining > 0.0 {
                        pet.activity = Activity::Idle { remaining };
                    } else {
                        // 待机结束:按需求挑下一件事
                        let max_x = (surface_width - pet.size.0 as f32).max(0.0);
                        pet.choose_next(entity.pos.0, max_x);
                    }
                }
                Activity::Walk { target_x } => {
                    let delta = target_x - entity.pos.0;
                    let step = pet.walk_speed * dt;
                    if delta.abs() <= step {
                        entity.pos.0 = target_x;
                        pet.activity = Activity::Idle {
                            remaining: 1.5 + pet.rng.next_f32() * 3.0,
                        };
                        pet.target_yaw = 0.0;
                        pet.player.play(pet.clips.idle);
                    } else {
                        entity.pos.0 += step * delta.signum();
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
            entity.clamp_to_surface(size);
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
mod entity_tests {
    use super::*;

    /// 两只精灵:第二只放在第一只右下方一点,重叠一块。
    fn two_sprites() -> Stage {
        let mut stage = Stage::new(Actor::Sprite(Sprite::test_pattern(64)), (800, 600));
        stage.spawn(Actor::Sprite(Sprite::test_pattern(64)));
        stage
    }

    #[test]
    fn spawn_and_despawn_track_by_id() {
        let mut stage = two_sprites();
        assert_eq!(stage.entities().len(), 2);
        let second = stage.entities()[1].id();
        assert!(stage.despawn(second));
        assert_eq!(stage.entities().len(), 1);
        // 同一个标识不会再命中(下标滑动了也不会误伤别人)
        assert!(!stage.despawn(second));
    }

    #[test]
    fn input_regions_are_the_union() {
        let mut stage = two_sprites();
        // 两只错开:各自的圆心都必须在输入区里
        stage.entities[1].pos = (100.0, 100.0);
        let regions = stage.input_regions();
        let first = stage.entities[0].pos;
        let (fx, fy) = (first.0 as f64 + 32.0, first.1 as f64 + 32.0);
        assert!(regions.iter().any(|r| r.contains(fx, fy)));
        assert!(regions.iter().any(|r| r.contains(132.0, 132.0)));
    }

    #[test]
    fn pick_takes_the_topmost() {
        let mut stage = two_sprites();
        // 完全重叠;第二只脚底更靠下 ⇒ 它在上面
        stage.entities[0].pos = (100.0, 100.0);
        stage.entities[1].pos = (100.0, 110.0);
        let top = stage.entities[1].id();
        assert_eq!(stage.pick(132.0, 142.0), Some(top));
        // 把第一只挪到更下面,z 序随之翻转
        stage.entities[0].pos = (100.0, 120.0);
        let other = stage.entities[0].id();
        assert_eq!(stage.pick(132.0, 152.0), Some(other));
    }

    #[test]
    fn dragging_moves_only_the_picked_one() {
        let mut stage = two_sprites();
        stage.entities[0].pos = (100.0, 100.0);
        stage.entities[1].pos = (400.0, 100.0);
        let still = stage.entities[0].pos;
        stage.handle(StageEvent::PointerPressed { x: 432.0, y: 132.0 });
        stage.handle(StageEvent::PointerMoved { x: 482.0, y: 152.0 });
        assert_eq!(stage.entities[1].pos, (450.0, 120.0));
        assert_eq!(stage.entities[0].pos, still, "没被点中的那只不该动");
        assert!(stage.is_dragging());
        stage.handle(StageEvent::PointerReleased);
        assert!(!stage.is_dragging());
    }
}

#[cfg(test)]
mod pet_tests {
    use super::*;

    /// 一只测试宠物:200×200 的画布,脚底在 180,走速 100px/s。
    fn pet_stage() -> Stage {
        let model = Model::for_test(&[
            "Idle",
            "Walk",
            "Shock",
            "Happy",
            "Fear",
            "SleepStart",
            "SleepLoop",
            "SleepEnd",
        ]);
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
        // 逼它走起来:待机计时耗尽 + 无聊攒够才会挑目标点(中间可能先做几个表情)
        for _ in 0..600 {
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
        for _ in 0..600 {
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

#[cfg(test)]
mod behaviour_tests {
    use super::*;

    fn pet_stage() -> Stage {
        let model = Model::for_test(&[
            "Idle",
            "Walk",
            "Shock",
            "SleepStart",
            "SleepLoop",
            "SleepEnd",
        ]);
        let actor = Actor::Pet(PetActor::new(model, (200, 200), 180.0, 100.0, 99));
        Stage::new(actor, (1000, 600))
    }

    fn pet(stage: &Stage) -> &PetActor {
        match stage.actor() {
            Actor::Pet(pet) => pet,
            _ => panic!("不是宠物"),
        }
    }

    /// 推进 `seconds` 秒(按 30Hz 切片),中途 `stop` 成立就停。
    fn run(stage: &mut Stage, seconds: f32, stop: impl Fn(&Stage) -> bool) -> f32 {
        let dt = 1.0 / 30.0;
        let mut elapsed = 0.0;
        while elapsed < seconds {
            stage.tick(dt);
            elapsed += dt;
            if stop(stage) {
                break;
            }
        }
        elapsed
    }

    #[test]
    fn boredom_builds_while_idle_and_drains_while_busy() {
        let mut s = pet_stage();
        run(&mut s, 3.0, |_| false);
        let bored = pet(&s).needs.boredom;
        assert!(bored > 0.3, "待机该攒无聊,实际 {bored}");
        // 走起来之后无聊会被消掉
        run(&mut s, 60.0, |s| {
            matches!(pet(s).activity, Activity::Walk { .. })
        });
        run(&mut s, 1.0, |_| false);
        assert!(pet(&s).needs.boredom < bored, "动起来该消无聊");
    }

    #[test]
    fn sleeps_when_sleepy_and_runs_all_three_phases() {
        let mut s = pet_stage();
        // 直接把困倦顶到阈值,省去等 8 分钟
        match s.actor_mut_for_test() {
            Actor::Pet(pet) => pet.needs.sleepiness = 0.99,
            _ => unreachable!(),
        }
        run(&mut s, 30.0, |s| pet(s).is_sleeping());
        assert!(
            matches!(
                pet(&s).activity,
                Activity::Sleeping(SleepPhase::Falling { .. })
            ),
            "该先播入睡,实际 {:?}",
            pet(&s).activity
        );
        run(&mut s, 5.0, |s| {
            matches!(pet(s).activity, Activity::Sleeping(SleepPhase::Asleep))
        });
        assert_eq!(
            pet(&s).activity,
            Activity::Sleeping(SleepPhase::Asleep),
            "该进入睡眠循环"
        );

        // 睡饱了会自己醒:困倦降到 SLEEPY_WAKE_AT 以下
        run(&mut s, SLEEPY_RECOVER_SECS + 5.0, |s| !pet(s).is_sleeping());
        assert!(
            pet(&s).needs.sleepiness <= SLEEPY_WAKE_AT + 0.05,
            "睡够该不困了"
        );
        assert!(
            matches!(pet(&s).activity, Activity::Idle { .. }),
            "醒来该回待机"
        );
    }

    #[test]
    fn poking_wakes_it_up_instead_of_startling() {
        let mut s = pet_stage();
        match s.actor_mut_for_test() {
            Actor::Pet(pet) => pet.needs.sleepiness = 0.99,
            _ => unreachable!(),
        }
        run(&mut s, 30.0, |s| {
            matches!(pet(s).activity, Activity::Sleeping(SleepPhase::Asleep))
        });
        let (x, y) = {
            let (px, py) = s.actor_pos();
            (px as f64 + 100.0, py as f64 + 100.0)
        };
        s.handle(StageEvent::PointerPressed { x, y });
        assert!(
            matches!(
                pet(&s).activity,
                Activity::Sleeping(SleepPhase::Waking { .. })
            ),
            "戳一下该转入醒来,实际 {:?}",
            pet(&s).activity
        );
        s.handle(StageEvent::PointerReleased);
        assert!(
            !matches!(pet(&s).activity, Activity::React { .. }),
            "叫醒的那一下不该再算受惊"
        );
        run(&mut s, 5.0, |s| {
            matches!(pet(s).activity, Activity::Idle { .. })
        });
        assert!(matches!(pet(&s).activity, Activity::Idle { .. }));
    }

    #[test]
    fn hovering_makes_it_glance_at_the_pointer() {
        let mut s = pet_stage();
        let (px, py) = s.actor_pos();
        let center_x = px as f64 + 100.0;
        let y = py as f64 + 100.0;
        // 指针在右侧:朝右瞥(幅度小于完整转身)
        s.handle(StageEvent::PointerMoved {
            x: center_x + 40.0,
            y,
        });
        let right = pet(&s).target_yaw;
        assert!((right - camera_yaw(true) * GLANCE_RATIO).abs() < 1e-5);
        assert!(
            right.abs() < camera_yaw(true).abs(),
            "瞥一眼的幅度该小于完整转身"
        );
        // 指针到左侧
        s.handle(StageEvent::PointerMoved {
            x: center_x - 40.0,
            y,
        });
        assert!(pet(&s).target_yaw * right < 0.0, "换边该反向");
        // 指针离开 → 转回正面
        s.handle(StageEvent::PointerLeft);
        assert_eq!(pet(&s).target_yaw, 0.0);
    }

    #[test]
    fn sleep_pose_lets_the_frame_rate_drop() {
        // 合成模型的动作没有通道 → 睡着时姿势不动,帧率该落到下限
        let mut s = pet_stage();
        match s.actor_mut_for_test() {
            Actor::Pet(pet) => pet.needs.sleepiness = 0.99,
            _ => unreachable!(),
        }
        run(&mut s, 30.0, |s| {
            matches!(pet(s).activity, Activity::Sleeping(SleepPhase::Asleep))
        });
        // 刚切段时有 0.18s 交叉淡化,那段姿势在动、帧率理应还是满的;等淡化过去再看
        run(&mut s, 0.5, |_| false);
        assert_eq!(s.tick_interval(), Duration::from_secs_f32(1.0 / STILL_HZ));
    }
}
