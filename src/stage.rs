//! 与平台无关的 stage 逻辑:角色在屏幕上的位置、拖动、命中测试、输入区,以及宠物的行为。
//!
//! 平台后端只负责「造表面 / 收事件 / 出帧 / 设输入区」,所有状态都在这里,
//! 这样 Wayland 与 Windows 两边的行为天然一致,也能脱离窗口系统做单元测试。

use std::sync::Arc;
use std::time::Duration;

use crate::act::{self, Beat, Script, Step};
use crate::persona::Persona;
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
    /// 又要重画又要重设输入区。平台层在「位置被外力改了」时用(比如召回)。
    pub const BOTH: Self = Self {
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

/// 放下之后的下落:重力与落地速度上限(逻辑像素)。
/// 用屏幕尺度的常数而不是从宠物尺寸推:掉落手感该和宠物多大无关。
const FALL_GRAVITY: f32 = 2600.0;
const FALL_MAX_SPEED: f32 = 1600.0;

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

/// **自己**出声的冷却(秒)。人点出来的那些不受它管。
///
/// 待机表情是宠物自己做的,按上面几个数算下来大约每 20~40 秒就有一个;做一次响一次的话,
/// 一只常驻桌面的宠物就是每半分钟叫你一嗓子 —— 这和「默认音量取 30%」是同一条产品约束
/// (见 audio.rs 的 `DEFAULT_VOLUME`)。所以自发的声音一分钟至多一次,
/// 而受惊/摸头/配置窗口点动作是**人要它出声**,该响就响。
const SELF_SPEAK_COOLDOWN: f32 = 60.0;

/// 待机时随手做的那几个表情:glb 里的动作名 → 配置窗口上显示的名字。
///
/// **这就是「表情池」的全集**。每只可以只开其中几个(见 `PetBuild::emotes`);
/// 一个都不开或者包里一个都没有,那只就只会站桩,行为上是允许的。
pub const EMOTES: &[(&str, &str)] = &[
    ("Happy", "开心"),
    ("Relax", "放松"),
    ("Show", "展示"),
    ("Anger", "生气"),
    ("Sad", "难过"),
    ("Fear", "害怕"),
];

/// 运行时会去找的**全部**动作:动作名 → 中文名。配置窗口按它算「动作覆盖率」。
///
/// **不是照 manifest 的 `[report]` 算的** —— 全库没有一个包写了那一节。
/// 这张表问的是另一个更有用的问题:*这只宠物在桌面上有哪些事做不了*。
/// 缺 `Run` 就只会走,缺 `SleepStart` 就直接躺下,缺 `JumpFall` 就落地没动作 ——
/// 都不是错误,只是少一点戏。
pub const RUNTIME_CLIPS: &[(&str, &str)] = &[
    ("Idle", "待机"),
    ("Walk", "行走"),
    ("Run", "奔跑"),
    ("JumpFall", "落地"),
    ("Shock", "受惊"),
    ("Happy", "开心"),
    ("Fear", "害怕"),
    ("Sad", "难过"),
    ("Anger", "生气"),
    ("Show", "展示"),
    ("Relax", "放松"),
    ("Alert", "警觉"),
    ("SleepStart", "入睡"),
    ("SleepLoop", "睡着"),
    ("SleepEnd", "醒来"),
    ("CallOut", "召唤"),
];

/// 找不到这段就退而求其次 —— **降级是行为的一部分**,不是补救:幽星光没有
/// `SleepLoop` 但有 `SleepStand`,它照样睡得着,不该记成「不会睡」。
///
/// 只此一份。以前 `PetActor::new`、`has_clip`、`play_clip` 各写各的,
/// 于是配置窗口的动作表说「睡着」能点(它按 `has_clip` 算),点下去运行时却报
/// 「这只没有这段」(它按 `model.clip` 找)。
fn fallbacks(name: &str) -> &'static [&'static str] {
    match name {
        "Shock" => &["Alert"],
        "Happy" => &["Show"],
        "Fear" => &["Shock", "Alert"],
        "SleepLoop" => &["SleepStand"],
        _ => &[],
    }
}

/// 这个形态做不做得了某段动作(降级也算有)。
pub fn has_clip(form: &crate::pack::Form, name: &str) -> bool {
    form.clip(name).is_some() || fallbacks(name).iter().any(|alt| form.clip(alt).is_some())
}

/// 在模型里找一段动作,找不到就按降级表退。
///
/// `pub` 是给下载站的预览用的(`web.rs`):网页上「有哪些动作能点」必须和桌面上
/// 「点下去有没有反应」是同一条判断,各写一份迟早对不上。
pub fn find_clip(model: &Model, name: &str) -> Option<usize> {
    model
        .clip(name)
        .or_else(|| fallbacks(name).iter().find_map(|alt| model.clip(alt)))
}

/// 在一层声音里找这段动作的音频,**走的是与动作同一张降级表** ——
/// 动作退到 Alert 的形态,声音也该退到 Alert。
///
/// 最后那一手是**认旧包**:2026-08-09 之前导的包,`[forms.voice]` 的键是另起的四个触发点名
/// (`happy`/`shock`/`callout`/`relax`),小写。四个都正好是动作名的小写形式,所以退一步查
/// 小写就能让下载过旧包的人不至于整只哑掉 —— 换键那天最坏的结果不该是「没报错、也没声音」。
fn pick_sound<'a, T>(layer: &'a std::collections::HashMap<String, T>, name: &str) -> Option<&'a T> {
    layer
        .get(name)
        .or_else(|| fallbacks(name).iter().find_map(|alt| layer.get(*alt)))
        .or_else(|| layer.get(&name.to_ascii_lowercase()))
}

/// 邻近感知:多少**身位**以内算「注意到旁边那只」。
///
/// 身位 = 两只显示高度的均值,**不能用绝对像素阈值** —— 同一台上 161px 的喵喵与 481px 的
/// 魔力猫,「挨着站」在像素上差了三倍。也**不能用画布尺寸**:画布带 1.64 倍取景余量
/// (第 4 步在跑动阈值上已经栽过一次),两只画布挨着时本体还隔着老远。
const NOTICE_DISTANCE: f32 = 2.0;
/// 同一对之间隔这么久才会再注意一次。没有它的话两只挨着站会没完没了地互相打招呼。
const NOTICE_COOLDOWN: f32 = 25.0;

/// 受惊之后往反方向逃开多少身位。逃到可走范围尽头太夸张,给个有限距离。
const FLEE_DISTANCE: f32 = 3.0;

/// 指针悬在身上时,朝它侧一点身(不是完整转 90°,读起来像「瞥一眼」)。
/// 真正的视线跟随需要 LookAt BlendSpace(没导出),而且 Wayland 下输入区外根本收不到
/// 指针事件——想追全屏光标就得吃掉输入,不做。
const GLANCE_RATIO: f32 = 0.45;

/// 状态的可读名字,只用于日志(睡觉的三段也分开,便于确认作息真的走完了)。
fn activity_label(activity: &Activity) -> &'static str {
    match activity {
        Activity::Idle { .. } => "待机",
        Activity::Walk { running: false, .. } => "行走",
        Activity::Walk { running: true, .. } => "奔跑",
        Activity::Falling { .. } => "下落",
        Activity::Dragged => "被拎着",
        Activity::React { .. } => "反应",
        Activity::Sleeping(SleepPhase::Falling { .. }) => "入睡",
        Activity::Sleeping(SleepPhase::Asleep) => "睡着",
        Activity::Sleeping(SleepPhase::Waking { .. }) => "醒来",
    }
}

/// 宠物当前在干什么。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Activity {
    /// 站着待机,`remaining` 秒后换个地方走走。
    Idle { remaining: f32 },
    /// 走向 `target_x`(左上角的目标 x)。`running` = 跑(远处才跑,见 `choose_next`)。
    Walk { target_x: f32, running: bool },
    /// 被放下之后往地面落。`speed` 是当前下落速度(px/s,受重力加速)。
    Falling { speed: f32 },
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

    /// `persona` 只缩放**攒**的那一侧:睡饱要多久、走一趟消多少无聊是手感,和脾气无关。
    fn tick(&mut self, dt: f32, activity: &Activity, persona: &Persona) {
        let dt = dt * Self::speed();
        let sleepy_rate = dt * persona.sleepy / SLEEPY_BUILD_SECS;
        match activity {
            Activity::Sleeping(_) => {
                self.sleepiness -= dt / SLEEPY_RECOVER_SECS;
                self.boredom = 0.0;
            }
            Activity::Idle { .. } => {
                self.sleepiness += sleepy_rate;
                self.boredom += dt * persona.bored / BORED_BUILD_SECS;
            }
            // 走动与互动都算「有事做」,消无聊
            _ => {
                self.sleepiness += sleepy_rate;
                self.boredom -= dt / BORED_RELIEF_SECS;
            }
        }
        self.sleepiness = self.sleepiness.clamp(0.0, 1.0);
        self.boredom = self.boredom.clamp(0.0, 1.0);
    }
}

// ── 感知与事件总线(design.md §6) ──────────────────────────────────
//
// 行为**只读感知快照**,不自己去摸 `Stage`:一是多实体下「旁边有谁」本来就得由 stage 汇总,
// 二是这样才能把「察觉到什么」与「于是做什么」拆开 —— 第 6 步的演出脚本要插在这中间,
// 它得能在意图落地**之前**看到它(比如宠物被戳了,正在跑的时间轴要能让位)。

/// 感知到的邻居。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Neighbor {
    pub id: EntityId,
    /// 脚底点之间的距离,单位是**身位**(见 [`NOTICE_DISTANCE`])。
    /// 用脚底而不是画布中心:两只站在同一条地面线上时,脚底距离才是「看着有多近」。
    pub distance: f32,
    /// 它在我右边?(转身朝向它用)
    pub on_right: bool,
}

/// 一只在某一 tick 看到的世界。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Perception {
    /// 最近的那一只;None = 台上就它一个。
    pub nearest: Option<Neighbor>,
    /// 指针在表面上的位置(逻辑像素);None = 指针不在表面上。
    pub pointer: Option<(f32, f32)>,
    /// 可走范围的右端(左端恒为 0),= 表面宽 − 自己的画布宽。
    pub max_x: f32,
}

/// 想做什么。**发出与执行分开**:发的时候只说意图,执行在 [`Stage::dispatch_intents`]。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Intent {
    pub from: EntityId,
    pub kind: IntentKind,
    /// 冲着谁;没有对象就是 None。
    pub target: Option<EntityId>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IntentKind {
    /// 注意到旁边那只:转过去打个招呼。
    Notice,
    /// 从 `from_x` 这个位置逃开(受惊后往反方向跑)。
    Flee { from_x: f32 },
}

// ── 声音 ────────────────────────────────────────────────────────
//
// `stage` **不碰音频设备**:这里只产出「放哪段字节、什么速率」,平台层交给 audio.rs。
// 行为逻辑因此照旧能脱离声卡做单元测试。

/// 一段待播的声音。
#[derive(Clone)]
pub struct SoundCue {
    /// 解好的样本。**共享**:同物种多实体只有一份(和模型一样)。
    pub pcm: Arc<crate::audio::Pcm>,
    /// 播放速率 = 变调。见 [`speed_for_cents`]。
    pub speed: f32,
}

/// 音分 → 播放速率。一个八度 = 1200 音分 = 两倍速。
///
/// Wwise 的 pitch 本身就是重采样(变调同时变速),所以调播放速率就是等价实现,
/// 不需要为每个音调预生成音频。
pub fn speed_for_cents(cents: f32) -> f32 {
    (cents / 1200.0).exp2()
}

/// 一个形态的声音库(加载时就解好的样本)。按形态共享。
///
/// **两层**,同一把键:`clips` 是嗓子发出来的叫声(跟着嗓音变调),`sfx` 是身体动静
/// (落地、扑翅、拖尾巴,**不变调**)。游戏里一段情绪就是这两条叠着放的,
/// 来自 `Pet_Vo_*` 与 `Pet_Action_*` 两族 Wwise 库。
pub struct VoiceBank {
    pub clips: std::collections::HashMap<String, Arc<crate::audio::Pcm>>,
    pub sfx: std::collections::HashMap<String, Arc<crate::audio::Pcm>>,
    pub cents_low: f32,
    pub cents_high: f32,
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

/// 造一只宠物要的东西。**用结构体而不是一串位置参数**:到第 6 步已经是 8 个,
/// 其中四个都是 `f32`(脚底、本体高、走速、跑速),位置传错了编译器也不会拦。
pub struct PetBuild {
    pub model: Arc<Model>,
    pub size: (u32, u32),
    pub foot_offset: f32,
    pub body_px: f32,
    pub walk_speed: f32,
    pub run_speed: f32,
    /// 形态 id(manifest 里 `[[forms]].id`)。演出脚本按它选角。
    pub form_id: i64,
    /// 叫声库;None = 这个形态没有(或者用户把声音关了)。
    pub voice: Option<Arc<VoiceBank>>,
    /// 脾气(见 persona.rs);不配就是「乖巧」= 基线行为。
    pub persona: Persona,
    /// 嗓音 −1~1;**None = 没设过,按 0(原调)来**。
    /// 不自动掷:同一个包的两只听着一样是正常的,想要不一样就在配置窗口里重掷。
    pub voice_value: Option<f32>,
    pub seed: u64,
}

/// 一只宠物:模型 + 播放器 + 屏幕上的表现状态。
pub struct PetActor {
    /// 网格/动画/贴图。**共享**:同物种多实体只有一份(见 design.md §9 Phase 5 第 2 步)。
    pub model: Arc<Model>,
    pub player: Player,
    /// 屏幕上的显示尺寸(逻辑像素)。
    pub size: (u32, u32),
    /// 当前朝向角(弧度,绕 Y 轴;0 = 面向观察者,+π/2 = 朝屏幕右)。
    pub yaw: f32,
    target_yaw: f32,
    pub activity: Activity,
    /// 走路速度(逻辑像素/秒)。
    pub walk_speed: f32,
    /// 跑速(逻辑像素/秒)。**要钳制**:动画反推出来的值中位 417cm/s、最大 1125cm/s
    /// (魔力猫那只 7.5m/s),照搬会让宠物瞬间横穿屏幕。见 wayland.rs 的 `run_speed_cm`。
    pub run_speed: f32,
    /// 画布顶端到宠物脚底的距离(逻辑像素)。取景留了余量,脚底不在画布最下沿,
    /// 站地面时要按这个值对齐,否则宠物会悬空。
    pub foot_offset: f32,
    /// 宠物本体的屏幕高度(逻辑像素)= `height_cm × scale × px_per_cm`。
    /// **和画布尺寸是两回事**(画布带 1.64 倍取景余量),距离阈值一律按它换算成身位。
    pub body_px: f32,
    /// 形态 id;演出脚本按它选角(见 act.rs)。
    pub form_id: i64,
    /// 脾气:一组作用在手感常量上的倍率(见 persona.rs)。
    pub persona: Persona,
    /// 正被演出脚本编排着。**外部打断会把它清掉**(受惊/摸头/被拎起都走 `react`
    /// 或直接设 `Dragged`),演出那边看见就收场 —— 打断语义只此一处,不散在各个分支里。
    acting: bool,
    voice: Option<Arc<VoiceBank>>,
    /// 这一只的嗓音属性 −1..1(游戏里是 −100~100)。同物种两只听着不一样,
    /// 和游戏里每只宠物各有一个 `voice` 值是同一个意思。
    ///
    /// 存档里写多少就是多少,不写就是 0;平台层存盘时把它收回 roster.toml
    /// (见 `PetBuild::voice_value`),不然同一只每次启动都换个嗓子。
    pub voice_value: f32,
    /// 待播的声音,由 `Stage::take_sounds` 收走。行为侧发声的地方分散在各处
    /// (`react`/`wake_up`/做表情/上台),挂在这儿就不必给每处都传一个出参。
    ///
    /// **一次最多两条**(叫声 + 动作音效),而不是一条:两层是一起响的。
    /// 再叫一声就整个换掉 —— 短音叠着放比排队自然,见 `audio::Audio::play`。
    pending_sounds: Vec<SoundCue>,
    /// 轮廓掩码(异步回读而来,见 pet/mask.rs);还没到就退化成包围盒判定。
    pub mask: Option<Mask>,
    /// 内部需求(困倦/无聊),决定待机结束后干什么。
    pub needs: Needs,
    /// 受惊后待逃的方向源(指针 x)。**不立刻跑**:先把受惊动作播完,
    /// 在 `React` 结束那一下才起跑,读起来才是「惊 → 逃」而不是「惊被跑打断」。
    flee_from: Option<f32>,
    /// 自发出声的冷却剩余(秒)。见 [`SELF_SPEAK_COOLDOWN`]。
    self_speak_cooldown: f32,
    /// 刚打过招呼的邻居与各自剩下的冷却(秒)。挨着站的两只不该没完没了地互相致意。
    notices: Vec<(EntityId, f32)>,
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
    /// 跑。**不是每只都有**(全库 463/539 个包有),缺了就一律用走。
    run: Option<usize>,
    /// 落地。原来不在导出器白名单里,全库一个都没有;补上之后仍是部分形态才有。
    jump_fall: Option<usize>,
    startled: Option<usize>,
    happy: Option<usize>,
    afraid: Option<usize>,
    /// 睡觉三段式;缺 Start/End 就直接进/出 Loop。
    sleep_start: Option<usize>,
    sleep_loop: Option<usize>,
    sleep_end: Option<usize>,
    /// 待机时随手做的表情池(有哪个算哪个),由性格定,见 persona.rs。
    emotes: Vec<usize>,
    /// 性格的默认表情在这只身上对应哪一段;None = 这个性格没有默认表情(或者包里缺)。
    default_emote: Option<usize>,
}

impl PetActor {
    /// 这只此刻该是哪张脸(见 persona.rs 的 `Expression`)。
    ///
    /// **正在播的那段动作说了算**,它没意见才用性格那张脸 —— 游戏里也是这样:
    /// 一只「哭哭眼」的幽星光生气时是生气眼、睡着时是困倦眼,性格给的只是它平时的样子。
    /// 按当前动作现算,不另存一份状态:换脸和换动作本来就是同一件事,
    /// 存两份就会有对不上的时候。
    pub fn face(&self) -> crate::persona::Expression {
        let clip = &self.model.clips[self.player.current()].name;
        crate::persona::face_for_clip(clip).unwrap_or(self.persona.face)
    }

    /// 播一次性反应动作,播完回待机。缺对应动作就只改状态(至少行为语义还在)。
    ///
    /// **一切外部打断都从这儿过**(受惊/摸头/被拎起),所以演出的「被打断」判定挂在这里:
    /// 人一伸手,正在演的那场就该让位。
    fn react(&mut self, reaction: PetReaction, model_len: f32) {
        self.acting = false;
        // 被拎起来不叫:拖动过程里指针一动就可能重入,叫起来会连成一串
        match reaction {
            PetReaction::Startled => {
                self.speak("Shock");
            }
            PetReaction::Petted => {
                self.speak("Happy");
            }
            PetReaction::PickedUp => {}
        }
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
            let emote_chance = (EMOTE_CHANCE * self.persona.emote).clamp(0.0, 1.0);
            let emote = !self.clips.emotes.is_empty() && self.rng.next_f32() < emote_chance;
            if !emote {
                let target_x = self.rng.next_f32() * max_x;
                let distance = (target_x - pos_x).abs();
                let far_enough = distance > self.size.0 as f32 * 0.25;
                // **远处才跑**:近距离用跑显得慌张,而且跑动画一两步就到、看着像抽搐。
                //
                // 阈值要按**可走范围**(`max_x`)取,这两条都踩过:
                // ① 按宠物画布取(「三个身位」)—— 画布带着取景余量、比宠物本身大 1.64 倍,
                //    水灵在 2560px 屏上画布就有 805px,三个身位 2415px 几乎整屏;
                // ② 按屏幕宽取 —— 可走范围只有 `max_x`(屏宽减画布宽),站在中间时
                //    最远也只能走 `max_x/2`,阈值取屏宽的三成五就已经够不着了。
                // 实测这两版跑动作**一次都不会触发**。四成可走范围 ≈ 两成的目标点会起跑。
                //
                // 性格在这个门槛上乘一个系数(活泼 0.6 更容易跑,高冷 2.0 基本不跑);
                // 乘出来超过 1.0 就等于「永远不跑」,那是「高冷」想要的效果,不必钳制。
                let threshold = max_x * 0.4 * self.persona.run;
                let running = distance > threshold && self.clips.run.is_some();
                let clip = if running {
                    self.clips.run
                } else {
                    self.clips.walk
                };
                if let (Some(clip), true) = (clip, far_enough) {
                    self.activity = Activity::Walk { target_x, running };
                    self.target_yaw = camera_yaw(target_x > pos_x);
                    self.player.play(clip);
                    self.needs.boredom = 0.0;
                    return;
                }
            }
            if let Some(&clip) = self.pick_emote() {
                self.player.play(clip);
                // 待机时随手做的表情**也出声**。原来只有受惊/摸头/醒来会响,于是一只
                // 自己在桌上生气、难过、展示的宠物是全程哑的 —— 而这几段在游戏里都有配音,
                // 只是当初导出器只导了四段(见 exporter/Audio.cs 那张表)
                let name = self.model.clips[clip].name.clone();
                self.speak_self(&name);
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

    /// 挑一个表情。**有默认表情的,一半时候做的就是它** ——
    /// 「默认表情」这四个字在游戏里指的是这只宠物平时那张脸,不是「唯一会做的表情」,
    /// 所以不是只做它,而是偏向它。
    fn pick_emote(&mut self) -> Option<&usize> {
        if self.clips.emotes.is_empty() {
            return None;
        }
        if self.clips.default_emote.is_some() && self.rng.next_f32() < 0.5 {
            return self.clips.default_emote.as_ref();
        }
        let index = (self.rng.next_f32() * self.clips.emotes.len() as f32) as usize;
        self.clips
            .emotes
            .get(index.min(self.clips.emotes.len() - 1))
    }

    /// 走到某个左上角 x,顺带转向那一侧。缺 `Run` 就走;连 `Walk` 都没有就走不了,
    /// 返回 false 让调用方兜底。
    fn walk_to(&mut self, target_x: f32, pos_x: f32, running: bool) -> bool {
        let running = running && self.clips.run.is_some();
        let clip = if running {
            self.clips.run
        } else {
            self.clips.walk
        };
        let Some(clip) = clip else {
            return false;
        };
        self.activity = Activity::Walk { target_x, running };
        self.target_yaw = camera_yaw(target_x > pos_x);
        self.player.play(clip);
        true
    }

    /// 受惊之后逃开:往远离 `from_x` 的那一侧跑 [`FLEE_DISTANCE`] 个身位。
    /// 缺 Run 就走,连 Walk 都没有就只能站着(至少别卡在反应状态里)。
    fn start_flee(&mut self, from_x: f32, pos_x: f32, max_x: f32) {
        let center = pos_x + self.size.0 as f32 * 0.5;
        // 正好被戳在正中间时往右跑;左右都行,不值得为此掷个骰子
        let away = if center >= from_x { 1.0 } else { -1.0 };
        let target_x = (pos_x + away * FLEE_DISTANCE * self.body_px).clamp(0.0, max_x);
        if self.walk_to(target_x, pos_x, true) {
            self.needs.boredom = 0.0;
        } else {
            // 连 Walk 都没有的形态:逃不了,至少别卡在反应状态里
            self.activity = Activity::Idle { remaining: 1.0 };
            self.player.play(self.clips.idle);
        }
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
        // 醒来播的是 `SleepEnd`,但那段没有配音 —— 出声用「放松」那条,和游戏里一样。
        // 睡醒也是它自己的事,同样受自发冷却管(被人吵醒时那一下点击已经先响过受惊了)
        self.speak_self("Relax");
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

    /// 排这段动作的声音:叫声 + 动作音效两层。缺哪层就少哪层,两层都没有就静默跳过。
    ///
    /// **按动作逻辑名取**(`Happy`/`Shock`/`CallOut`…),和 `[forms.clips]` 同一把键 ——
    /// 于是「播哪段动作」直接决定「出什么声」,不必再维护一张触发点 → 音频键的对照表。
    /// 取不到就走**动作那张降级表**([`fallbacks`]):没有 `Shock` 而有 `Alert` 的形态,
    /// 动作退到 Alert,声音也该跟着退到 Alert,两边同一套才不会各退各的。
    ///
    /// 少数几处「动作与声音不同名」的由调用方点名(醒来是 `SleepEnd` 配 `Relax`,
    /// 上台是只出声没有动作),所以这里收的是声音名而不是当前动作。
    ///
    /// 返回**这一下有没有真出声**,给 [`PetActor::speak_self`] 计冷却用。
    fn speak(&mut self, name: &str) -> bool {
        let Some(bank) = self.voice.as_ref() else {
            return false;
        };
        // voice ∈ [−1, 1] 在曲线两端之间插值:0 = 原声
        let cents = if self.voice_value >= 0.0 {
            bank.cents_high * self.voice_value
        } else {
            bank.cents_low * -self.voice_value
        };
        let mut cues = Vec::new();
        if let Some(pcm) = pick_sound(&bank.clips, name) {
            cues.push(SoundCue {
                pcm: Arc::clone(pcm),
                speed: speed_for_cents(cents),
            });
        }
        // 动作音效**不变调**:那条 RTPC 曲线是给嗓子的,650 个 `Pet_Action_*` 库里
        // 只有 1 个挂了它(见 exporter/Audio.cs 开头那张表)
        if let Some(pcm) = pick_sound(&bank.sfx, name) {
            cues.push(SoundCue {
                pcm: Arc::clone(pcm),
                speed: 1.0,
            });
        }
        if cues.is_empty() {
            return false;
        }
        self.pending_sounds = cues;
        true
    }

    /// 自己想出声(待机做表情、睡饱醒来)。**一分钟至多一次**,见 [`SELF_SPEAK_COOLDOWN`]。
    ///
    /// 冷却只拦自发的这一路:人点出来的走 [`PetActor::speak`],该响就响。
    /// **真出了声才计冷却** —— 这只没有这段声音的话,不该白占掉这一分钟。
    fn speak_self(&mut self, name: &str) {
        if self.self_speak_cooldown > 0.0 {
            return;
        }
        if self.speak(name) {
            self.self_speak_cooldown = SELF_SPEAK_COOLDOWN;
        }
    }

    /// 这只邻居还在冷却里吗。
    fn notice_ready(&self, id: EntityId) -> bool {
        !self.notices.iter().any(|(other, _)| *other == id)
    }

    fn remember_notice(&mut self, id: EntityId) {
        self.notices.push((id, NOTICE_COOLDOWN));
    }

    pub fn new(build: PetBuild) -> Self {
        let PetBuild {
            model,
            size,
            foot_offset,
            body_px,
            walk_speed,
            run_speed,
            form_id,
            voice,
            persona,
            voice_value,
            seed,
        } = build;
        // Idle 一定要有:没有 Idle 的包等于没法待机,退化成用第 0 段动作
        let idle = model.clip("Idle").unwrap_or(0);
        let walk = model.clip("Walk");
        let run = model.clip("Run");
        let jump_fall = model.clip("JumpFall");
        // 反应动作:游戏里「摸头」在 INTERACTIONTREE_CONF 有对应动作键,但键→动作表的映射
        // 还没核实(见 design.md §5),所以先按语义挑:受惊 Shock、开心 Happy、害怕 Fear,
        // 缺哪个就退到 Alert / Show / Shock
        let startled = find_clip(&model, "Shock");
        let happy = find_clip(&model, "Happy");
        let afraid = find_clip(&model, "Fear");
        let sleep_start = model.clip("SleepStart");
        let sleep_loop = find_clip(&model, "SleepLoop");
        let sleep_end = model.clip("SleepEnd");
        // 表情池**由性格定**(见 persona.rs:游戏的 NATURE_CONF + LLM_PET_BEHAVIOR_CONF)。
        // 包里没有的那几段自然就掉出去了。
        let emotes: Vec<usize> = persona
            .emote_pool()
            .iter()
            .filter_map(|name| model.clip(name))
            .collect();
        // 默认表情在池子里的位置(`emote_pool` 保证它排第一),给 `pick_emote` 加权用
        let default_emote = persona
            .default_emote
            .and_then(|name| model.clip(name))
            .filter(|clip| emotes.first() == Some(clip));
        let player = Player::new(&model, idle);
        let rng = Rng::new(seed);
        Self {
            model,
            player,
            size,
            yaw: 0.0,
            target_yaw: 0.0,
            activity: Activity::Idle { remaining: 2.0 },
            walk_speed,
            run_speed,
            foot_offset,
            body_px: body_px.max(1.0),
            form_id,
            persona,
            acting: false,
            voice,
            // 均匀取 −1..1:游戏里 voice 是逐只随机的属性,两端各 2% 才算「粗嗓门/婉转声」。
            // 不写就是 0 = 原调(以前这里是随机掷一个,那让同一只每次启动都换嗓子)
            voice_value: voice_value.unwrap_or(0.0),
            pending_sounds: Vec::new(),
            mask: None,
            needs: Needs::default(),
            flee_from: None,
            self_speak_cooldown: 0.0,
            notices: Vec::new(),
            clips: Clips {
                idle,
                walk,
                run,
                jump_fall,
                startled,
                happy,
                afraid,
                sleep_start,
                sleep_loop,
                sleep_end,
                emotes,
                default_emote,
            },
            petting: Petting::default(),
            rng,
        }
    }
}

/// 舞台上的角色:调试用的测试精灵,或真宠物。
///
/// 两个变体差着 600 多字节,但**不给 `Pet` 装箱**:每个实体只有一个 `Actor`,
/// 而它在每帧的 tick 与渲染里都要访问 —— 省下的那点内存换来的是一次堆间接。
#[allow(clippy::large_enum_variant)]
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
    /// 上台时给的落脚点(可走范围的百分之几)。**表面尺寸定下来之前摆了也白摆**
    /// (那时 `size` 还是 (1,1)),所以要留着,首次 configure 时重摆一遍。
    home: Option<f32>,
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
            home: None,
            drag_offset: None,
            drag_moved: false,
        };
        entity.reset_position(size);
        entity
    }

    /// 摆到可走范围的百分之几处(y 照旧落地)。
    fn set_home_fraction(&mut self, fraction: f32, size: (u32, u32)) {
        self.reset_position(size);
        let max_x = (size.0 as f32 - self.actor.size().0 as f32).max(0.0);
        self.pos.0 = fraction.clamp(0.0, 1.0) * max_x;
        self.clamp_to_surface(size);
    }

    pub fn id(&self) -> EntityId {
        self.id
    }

    pub fn actor(&self) -> &Actor {
        &self.actor
    }

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

    /// 脚底点(表面坐标)。距离一律按它算 —— 两只站在同一条地面线上时,
    /// 脚底之间的距离才是「看着有多近」;画布中心会被取景余量与身高差带偏。
    fn foot_point(&self) -> (f32, f32) {
        (self.pos.0 + self.actor.size().0 as f32 * 0.5, self.foot_y())
    }

    /// 本体的屏幕高度,用作「身位」的尺度。
    fn body_px(&self) -> f32 {
        match &self.actor {
            Actor::Pet(pet) => pet.body_px,
            Actor::Sprite(sprite) => sprite.height as f32,
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

    /// 表面坐标是否落在这只的可见部分上。
    fn hit_test(&self, x: f64, y: f64) -> bool {
        let lx = (x - self.pos.0 as f64).floor() as i32;
        let ly = (y - self.pos.1 as f64).floor() as i32;
        self.actor.hit(lx, ly)
    }

    /// 这只在表面坐标下的输入矩形。Wayland 交给 `set_input_region`,
    /// Windows 交给 `SetWindowRgn` —— 两边都是「这些矩形才吃鼠标」。
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
    /// 这一轮攒下的意图,由 [`Stage::dispatch_intents`] 统一落地。
    intents: Vec<Intent>,
    /// 正在演的那一场;同时只演一场(两只在演、第三只在旁边溜达是可以的)。
    performance: Option<Performance>,
    /// 每个脚本剩下的冷却(秒)。
    script_cooldown: Vec<(&'static str, f32)>,
    /// 拿到过真实表面尺寸没有。见 [`Stage::place`]。
    placed: bool,
    /// 目标帧率(配置里的 `fps`)。见 [`Stage::set_fps`]。
    hz: f32,
}

/// 正在跑的一场演出。
struct Performance {
    script: &'static Script,
    /// 各角色对应台上的哪一只,下标与 `script.cast` 对齐。
    cast: [EntityId; 2],
    /// 各自开演前站的位置(`GoHome` 要回到这儿)。
    home: [f32; 2],
    elapsed: f32,
    /// 下一拍在 `script.steps` 里的下标。
    next: usize,
}

impl Stage {
    /// 空台。**允许台上一只都没有**:托盘可以把最后一只也撤掉(见 design.md
    /// §9 Phase 5 第 7 步),这时输入区为空、每帧清成透明,程序还在跑。
    pub fn new(size: (u32, u32)) -> Self {
        Self {
            entities: Vec::new(),
            next_id: 0,
            size,
            pointer: None,
            passthrough: false,
            intents: Vec::new(),
            performance: None,
            script_cooldown: Vec::new(),
            placed: false,
            hz: crate::config::DEFAULT_FPS as f32,
        }
    }

    /// 改目标帧率(配置里的 `fps`)。台上不管在干什么都按这个推进。
    pub fn set_fps(&mut self, hz: f32) {
        self.hz = hz.max(1.0);
    }

    /// 收走这一轮攒下的声音。平台层每次 tick / 处理事件之后取一遍交给 audio.rs。
    pub fn take_sounds(&mut self) -> Vec<SoundCue> {
        let mut out = Vec::new();
        for entity in &mut self.entities {
            if let Actor::Pet(pet) = &mut entity.actor {
                out.append(&mut pet.pending_sounds);
            }
        }
        out
    }

    /// 放一只上台,返回它的标识。
    ///
    /// **错开摆**:`Entity::new` 一律摆在正中,于是托盘连加两只同物种的会精确重叠
    /// (第 5 步做邻近感知时撞见的 —— 距离恒为 0)。见 [`Stage::place`]。
    pub fn spawn(&mut self, actor: Actor) -> EntityId {
        self.spawn_at(actor, None)
    }

    /// 放一只上台,可以指定落脚点(可走范围的百分之几,见 [`Stage::home_fraction`])。
    ///
    /// 给了落脚点就不参与错开:那是「上次它自己站的地方」,再挪一下就白记了。
    pub fn spawn_at(&mut self, actor: Actor, home_x: Option<f32>) -> EntityId {
        let id = EntityId(self.next_id);
        self.next_id += 1;
        let mut entity = Entity::new(id, actor, self.size);
        match home_x {
            Some(fraction) => {
                log::debug!("记着上次的落脚点,回可走范围的 {:.0}% 处", fraction * 100.0);
                entity.set_home_fraction(fraction, self.size);
            }
            None => Self::place(&mut entity, self.entities.len(), self.size),
        }
        entity.home = home_x;
        self.entities.push(entity);
        id
    }

    /// 这一只现在站在可走范围的百分之几。撤下/存盘时读它写进阵容存档。
    ///
    /// 存**比例**而不是像素:换分辨率、换显示器之后像素值毫无意义,
    /// 而「靠左三成」在哪块屏上都说得通。
    pub fn home_fraction(&self, id: EntityId) -> Option<f32> {
        let entity = self.entities.iter().find(|e| e.id == id)?;
        let max_x = (self.size.0 as f32 - entity.actor.size().0 as f32).max(1.0);
        Some((entity.pos.0 / max_x).clamp(0.0, 1.0))
    }

    /// 把第 `index` 只摆到位:居中,再按次序左右轮流错开一个身位。
    ///
    /// **表面尺寸还没定下来时摆了也白摆**:stage 是先建再等 configure 的,那之前
    /// `size` 是 (1, 1),错开量会被 `clamp_to_surface` 整个吃掉 —— 实测两只 315px 的宠物
    /// 双双落在 x = 0,开演时「相隔 0.0 身位」。所以首次拿到真实尺寸时要重摆一遍。
    fn place(entity: &mut Entity, index: usize, size: (u32, u32)) {
        entity.reset_position(size);
        if index > 0 {
            let step = index.div_ceil(2) as f32;
            let side = if index % 2 == 1 { 1.0 } else { -1.0 };
            entity.pos.0 += side * step * entity.body_px();
            entity.clamp_to_surface(size);
        }
    }

    /// 全部重摆(首次拿到真实表面尺寸时)。记了落脚点的回它自己那儿,其余按次序错开。
    fn place_all(&mut self) {
        let size = self.size;
        for (index, entity) in self.entities.iter_mut().enumerate() {
            match entity.home {
                Some(fraction) => entity.set_home_fraction(fraction, size),
                None => Self::place(entity, index, size),
            }
        }
    }

    /// 召回:**不管记没记落脚点,全都摆回中间**,并把记下的那个作废。
    ///
    /// 召回的用途就是「它跑没影了,给我拉回来」,这时候还尊重旧位置就等于没召回。
    /// 记录也要一起清:留着的话下一次重建角色(改大小、切形态)又会把它拽回去。
    /// 之后它自己走到哪儿,哪儿就是新的落脚点。
    fn recall_all(&mut self) {
        let size = self.size;
        for (index, entity) in self.entities.iter_mut().enumerate() {
            entity.home = None;
            Self::place(entity, index, size);
        }
    }

    /// 让某一只出个声(平台层在「启用了一只」这类外部事件上调)。名字同 [`RUNTIME_CLIPS`]。
    pub fn speak(&mut self, id: EntityId, name: &str) {
        if let Some(Actor::Pet(pet)) = self.entity_mut(id).map(|e| &mut e.actor) {
            pet.speak(name);
        }
    }

    /// 手动播一段动作(配置窗口那张动作表点出来的)。
    ///
    /// 走的是**和受惊/摸头同一条路**(`React`):播完自己回待机,也会打断正在演的那场。
    /// 找不到这只、或者这个形态没这段动作,返回 false —— 界面据此把按钮置灰。
    pub fn play_clip(&mut self, id: EntityId, name: &str) -> bool {
        let Some(Actor::Pet(pet)) = self.entity_mut(id).map(|e| &mut e.actor) else {
            return false;
        };
        // 降级也算数,与配置窗口那张动作表的判断(`has_clip`)对齐
        let Some(clip) = find_clip(&pet.model, name) else {
            return false;
        };
        // 睡着的时候点一下动作,它该醒过来做 —— 不然看着像没反应
        pet.acting = false;
        pet.player.play(clip);
        // **按点的那个名字出声,不是按降级后播的那段**:两边共用同一张降级表,
        // 点「受惊」而形态只有 Alert 时,动作与声音会一起退到 Alert
        pet.speak(name);
        pet.activity = Activity::React {
            remaining: pet.model.clips[clip].duration.max(0.3),
        };
        true
    }

    /// 撤掉一只。找不到就是 false(标识可能已经失效)。
    pub fn despawn(&mut self, id: EntityId) -> bool {
        let before = self.entities.len();
        self.entities.retain(|e| e.id != id);
        self.entities.len() != before
    }

    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    fn entity_mut(&mut self, id: EntityId) -> Option<&mut Entity> {
        self.entities.iter_mut().find(|e| e.id == id)
    }

    /// 绘制顺序:**从后往前**(脚底 y 越小越先画,于是靠下的盖在上面)。
    /// 与 `pick` 的 z 序是同一套判据,只是方向相反。
    pub fn draw_order(&self) -> Vec<EntityId> {
        let mut order: Vec<&Entity> = self.entities.iter().collect();
        order.sort_by(|a, b| a.foot_y().total_cmp(&b.foot_y()).then(a.id.0.cmp(&b.id.0)));
        order.into_iter().map(|e| e.id).collect()
    }

    pub fn entity(&self, id: EntityId) -> Option<&Entity> {
        self.entities.iter().find(|e| e.id == id)
    }

    /// 命中测试:**取最上面的那一只**。z 序按脚底 y(越靠下越靠前),
    /// 脚底相同则取后加入的(绘制顺序里在上面)。
    pub fn pick(&self, x: f64, y: f64) -> Option<EntityId> {
        self.entities
            .iter()
            .filter(|e| e.hit_test(x, y))
            .max_by(|a, b| a.foot_y().total_cmp(&b.foot_y()).then(a.id.0.cmp(&b.id.0)))
            .map(|e| e.id)
    }

    /// 换掉某一只的角色(切形态):尺寸与轮廓都变了,重算覆盖区并重新落地。
    /// 标识不变 —— 托盘的插槽与掩码回读都还认着它。
    pub fn replace_actor(&mut self, id: EntityId, actor: Actor) -> bool {
        let size = self.size;
        let Some(entity) = self.entity_mut(id) else {
            return false;
        };
        entity.actor = actor;
        entity.coverage = entity.actor.coverage();
        entity.drag_offset = None;
        entity.drag_moved = false;
        // 换了角色画布尺寸就变了,位置必须重算。**记了落脚点的回它自己那儿** ——
        // 一律居中的话,改一次整体大小就把每只记下的位置抹成正中间,
        // 而那个「正中间」还会被当成新的落脚点存进阵容(实测就是这么丢的)。
        match entity.home {
            Some(fraction) => entity.set_home_fraction(fraction, size),
            None => entity.reset_position(size),
        }
        true
    }

    pub fn passthrough(&self) -> bool {
        self.passthrough
    }

    // ── 单实体便利访问(只给测试) ──────────────────────────────────
    // 平台层已经全部改成按实体走了(第 7 步删掉了最后几处调用)。测试里台上通常
    // 只有一只,这几个省去每次先取标识。

    #[cfg(test)]
    fn primary(&self) -> &Entity {
        &self.entities[0]
    }

    #[cfg(test)]
    fn primary_mut(&mut self) -> &mut Entity {
        &mut self.entities[0]
    }

    #[cfg(test)]
    pub fn actor(&self) -> &Actor {
        &self.primary().actor
    }

    /// 直接改角色状态(比如把困倦顶到阈值,省去等几分钟)。
    #[cfg(test)]
    pub fn actor_mut_for_test(&mut self) -> &mut Actor {
        &mut self.primary_mut().actor
    }

    #[cfg(test)]
    pub fn actor_pos(&self) -> (f32, f32) {
        self.primary().pos
    }

    /// 有没有哪一只正被拎着。
    #[cfg(test)]
    pub fn is_dragging(&self) -> bool {
        self.entities.iter().any(Entity::is_dragging)
    }

    /// 全部召回到屏幕中间。**也要错开**:三只叠在一起的「召回」等于把它们藏成一只。
    pub fn reset_position(&mut self) {
        self.recall_all();
    }

    /// 当前该交给合成器的输入区(表面局部坐标)。穿透时为空。
    ///
    /// **取各实体的并集**。这里不做去重/合并:合成器接受重叠矩形,而实体之间本来
    /// 就很少叠在一起;真要压条目数,该压的是单只那 60~87 个格子。
    #[allow(dead_code)] // 只有 Wayland 后端要;Windows 那边走 `shape_regions`
    pub fn input_regions(&self) -> Vec<Rect> {
        if self.passthrough {
            return Vec::new();
        }
        self.shape_regions()
    }

    /// 同样的矩形并集,但**不看穿透开关**。
    ///
    /// 给 Win32 的「窗口区域」用:那东西同时裁剪渲染,穿透时不能像 Wayland 的输入区
    /// 那样交空集 —— 交空集画面会跟着一起没。窗口的形状照旧按这些矩形来,「点不点得动」
    /// 交给 `WS_EX_TRANSPARENT`(见 platform/windows.rs 的 `update_window_region`)。
    #[allow(dead_code)] // 只有 Windows 后端要
    pub fn shape_regions(&self) -> Vec<Rect> {
        self.entities.iter().flat_map(Entity::input_rects).collect()
    }

    /// 表面坐标是否落在**任何一只**的可见部分上(比输入区更精确,用于自己内部的判定)。
    #[allow(dead_code)] // 内部判定现在都走 `pick`;这条留给平台层的命中查询
    pub fn hit_test(&self, x: f64, y: f64) -> bool {
        self.pick(x, y).is_some()
    }

    pub fn handle(&mut self, event: StageEvent) -> Reaction {
        let mut reaction = self.handle_event(event);
        // 事件里发出来的意图(比如受惊 → 想逃)也在这一轮落地
        let dispatched = self.dispatch_intents();
        reaction.redraw |= dispatched.redraw;
        reaction.regions_dirty |= dispatched.regions_dirty;
        reaction
    }

    fn handle_event(&mut self, event: StageEvent) -> Reaction {
        match event {
            StageEvent::Resized { width, height } => {
                if (width, height) == self.size {
                    return Reaction::NONE;
                }
                // 头一次拿到真实尺寸:之前那次摆位是按 (1, 1) 算的,全作废,重摆
                let first = !self.placed;
                self.size = (width, height);
                self.placed = true;
                if first {
                    self.place_all();
                    return Reaction::BOTH;
                }
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
                                pet.react(PetReaction::PickedUp, len); // 顺带清掉 acting
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
                let pointer_x = self.pointer.map(|(x, _)| x as f32);
                let Some(entity) = self.entities.iter_mut().find(|e| e.is_dragging()) else {
                    return Reaction::NONE;
                };
                let id = entity.id;
                entity.drag_offset = None;
                let clicked = !entity.drag_moved;
                entity.drag_moved = false;
                if let Actor::Pet(pet) = &mut entity.actor {
                    if clicked && !pet.is_sleeping() {
                        // 只是点了一下 → 受惊(正在醒来的那一下不算)
                        let len = pet.clip_seconds(PetReaction::Startled);
                        pet.react(PetReaction::Startled, len);
                        // 受惊之后要逃:发个意图,真跑起来在受惊动作播完那一下。
                        // 走总线而不是就地设状态 —— 第 6 步的演出脚本得能看见「它被戳了」
                        if let Some(from_x) = pointer_x {
                            self.intents.push(Intent {
                                from: id,
                                kind: IntentKind::Flee { from_x },
                                target: None,
                            });
                        }
                    } else if !clicked {
                        // 拎着放下 → 往地面落。**不再瞬移**:原来是直接把 y 设成地面线,
                        // 从半空松手会「啪」地闪下去。有 JumpFall 就播它,没有就用待机姿势落。
                        pet.activity = Activity::Falling { speed: 0.0 };
                        pet.player
                            .play(pet.clips.jump_fall.unwrap_or(pet.clips.idle));
                    }
                }
                // 已经在地面上(或者本来就不是宠物)就不用落
                let ground = entity.ground_y(size);
                let on_ground = entity.pos.1 >= ground;
                match &mut entity.actor {
                    Actor::Pet(pet) => {
                        if matches!(pet.activity, Activity::Falling { .. }) && on_ground {
                            pet.activity = Activity::Idle { remaining: 1.5 };
                            pet.player.play(pet.clips.idle);
                            entity.pos.1 = ground;
                        }
                    }
                    Actor::Sprite(_) => entity.pos.1 = ground,
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
    pub fn set_entity_mask(&mut self, id: EntityId, mask: Mask) -> Reaction {
        let Some(entity) = self.entity_mut(id) else {
            return Reaction::NONE;
        };
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
            if Some(entity.id) != picked
                && let Actor::Pet(pet) = &mut entity.actor
            {
                pet.petting.reset();
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

    /// 给第 `index` 只算一份感知快照。
    pub fn perceive(&self, index: usize) -> Perception {
        let me = &self.entities[index];
        let (my_x, my_y) = me.foot_point();
        let my_body = me.body_px();
        let nearest = self
            .entities
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, other)| {
                let (ox, oy) = other.foot_point();
                // 身位取两只的均值:大个子挨小个子时,谁算「近」不该只由其中一方说了算
                let unit = ((my_body + other.body_px()) * 0.5).max(1.0);
                Neighbor {
                    id: other.id,
                    distance: ((ox - my_x).powi(2) + (oy - my_y).powi(2)).sqrt() / unit,
                    on_right: ox >= my_x,
                }
            })
            .min_by(|a, b| a.distance.total_cmp(&b.distance));
        Perception {
            nearest,
            pointer: self.pointer.map(|(x, y)| (x as f32, y as f32)),
            max_x: (self.size.0 as f32 - me.actor.size().0 as f32).max(0.0),
        }
    }

    /// 把攒下的意图落地。**发出与执行分开**的那一半在这里 ——
    /// 第 6 步的演出脚本会插在这之前,先挑走它认得的那些。
    fn dispatch_intents(&mut self) -> Reaction {
        if self.intents.is_empty() {
            return Reaction::NONE;
        }
        let mut reaction = Reaction::NONE;
        for intent in std::mem::take(&mut self.intents) {
            let applied = match intent.kind {
                IntentKind::Notice => self.apply_notice(intent),
                IntentKind::Flee { from_x } => self.apply_flee(intent.from, from_x),
            };
            reaction.redraw |= applied;
        }
        reaction
    }

    /// 转过去朝着邻居,播一段打招呼的动作。目标不在了就当没发生过。
    fn apply_notice(&mut self, intent: Intent) -> bool {
        let Some(target) = intent.target else {
            return false;
        };
        let Some(their_x) = self.entity(target).map(|e| e.foot_point().0) else {
            return false;
        };
        let Some(entity) = self.entity_mut(intent.from) else {
            return false;
        };
        let my_x = entity.foot_point().0;
        let Actor::Pet(pet) = &mut entity.actor else {
            return false;
        };
        // 冷却按「打过招呼」记,不按「想打招呼」记:动作缺失时也别每帧重试
        pet.remember_notice(target);
        pet.target_yaw = camera_yaw(their_x > my_x);
        log::debug!("宠物 #{} 注意到 #{}", intent.from.0, target.0);
        let Some(clip) = pet
            .clips
            .happy
            .or_else(|| pet.clips.emotes.first().copied())
        else {
            return false;
        };
        pet.player.play(clip);
        pet.activity = Activity::React {
            remaining: pet.model.clips[clip].duration.max(0.3),
        };
        true
    }

    /// 记下要往哪边逃。真跑起来是在受惊动作播完那一下(见 `tick_entity`)。
    fn apply_flee(&mut self, from: EntityId, from_x: f32) -> bool {
        let Some(entity) = self.entity_mut(from) else {
            return false;
        };
        let Actor::Pet(pet) = &mut entity.actor else {
            return false;
        };
        pet.flee_from = Some(from_x);
        log::debug!("宠物 #{} 受惊,准备从 x={from_x:.0} 逃开", from.0);
        true
    }

    // ── 演出脚本(见 act.rs) ──────────────────────────────────────

    /// 两只之间的「身位」尺度。与 `perceive` 同一把尺:取两只本体高度的均值。
    fn body_unit(&self, a: EntityId, b: EntityId) -> f32 {
        let of = |id| self.entity(id).map(Entity::body_px).unwrap_or(1.0);
        ((of(a) + of(b)) * 0.5).max(1.0)
    }

    /// 这只现在能不能被拉去演:得是宠物、闲着、没在睡也没被拎着。
    fn free_to_act(&self, id: EntityId) -> bool {
        match self.entity(id).map(Entity::actor) {
            Some(Actor::Pet(pet)) => {
                matches!(pet.activity, Activity::Idle { .. }) && !pet.acting && !pet.is_sleeping()
            }
            _ => false,
        }
    }

    /// 选角:台上有没有这个脚本要的两只(按形态 id,取先找到的)。
    fn cast_for(&self, script: &Script) -> Option<[EntityId; 2]> {
        let find = |form_id: i64, skip: Option<EntityId>| {
            self.entities
                .iter()
                .find(|e| {
                    Some(e.id) != skip
                        && matches!(e.actor(), Actor::Pet(pet) if pet.form_id == form_id)
                })
                .map(|e| e.id)
        };
        let a = find(script.cast[0], None)?;
        // 第二个角色要**另一只**:同一形态在场两只时不能自己跟自己演
        let b = find(script.cast[1], Some(a))?;
        Some([a, b])
    }

    /// 没在演的时候,看看能不能开一场。
    fn try_start_performance(&mut self) {
        for script in act::SCRIPTS {
            if self.script_cooldown.iter().any(|(id, _)| *id == script.id) {
                continue;
            }
            let Some(cast) = self.cast_for(script) else {
                continue;
            };
            if !cast.iter().all(|id| self.free_to_act(*id)) {
                continue;
            }
            // 太远就不开演:第一拍是喊话,隔半个屏幕喊不合理;而且 `Approach` 的档期
            // 是按时间给的,起点太远根本走不到
            let (ax, _) = self.entity(cast[0]).expect("刚选出来").foot_point();
            let (bx, _) = self.entity(cast[1]).expect("刚选出来").foot_point();
            let distance = (ax - bx).abs() / self.body_unit(cast[0], cast[1]);
            if distance > script.max_distance {
                continue;
            }
            let home = [
                self.entity(cast[0]).expect("刚选出来").pos.0,
                self.entity(cast[1]).expect("刚选出来").pos.0,
            ];
            for id in cast {
                if let Some(Actor::Pet(pet)) = self.entity_mut(id).map(|e| &mut e.actor) {
                    pet.acting = true;
                }
            }
            log::info!("开演《{}》(相隔 {distance:.1} 身位)", script.name);
            self.performance = Some(Performance {
                script,
                cast,
                home,
                elapsed: 0.0,
                next: 0,
            });
            return;
        }
    }

    /// 推进正在演的那一场。返回是否要重画。
    fn tick_performance(&mut self, dt: f32) -> bool {
        for (_, remaining) in self.script_cooldown.iter_mut() {
            *remaining -= dt;
        }
        self.script_cooldown
            .retain(|(_, remaining)| *remaining > 0.0);

        let Some(perf) = self.performance.as_mut() else {
            self.try_start_performance();
            return self.performance.is_some();
        };
        // 有人被打断(受惊/摸头/拎起都会清掉 acting),或者干脆被撤下了 → 收场
        let cast = perf.cast;
        let script = perf.script;
        let still_acting = cast.iter().all(
            |id| matches!(self.entity(*id).map(Entity::actor), Some(Actor::Pet(pet)) if pet.acting),
        );
        if !still_acting {
            log::info!("《{}》被打断,收场", script.name);
            self.end_performance();
            return true;
        }

        let perf = self.performance.as_mut().expect("上面判过");
        perf.elapsed += dt;
        let elapsed = perf.elapsed;
        // 这一拍到点的全放出来。**允许打断自己**:上一段动作没播完就换下一件事是可以的
        let mut pending: Vec<Step> = Vec::new();
        while perf.next < script.steps.len() && script.steps[perf.next].at <= elapsed {
            pending.push(script.steps[perf.next]);
            perf.next += 1;
        }
        let home = perf.home;
        let done = elapsed >= script.length;
        for step in pending {
            let other = 1 - step.role;
            self.apply_beat(cast[step.role], cast[other], step.beat, home[step.role]);
        }
        if done {
            log::info!("《{}》演完", script.name);
            self.end_performance();
        }
        true
    }

    /// 收场:放开两位演员,记下冷却。
    fn end_performance(&mut self) {
        let Some(perf) = self.performance.take() else {
            return;
        };
        for id in perf.cast {
            if let Some(Actor::Pet(pet)) = self.entity_mut(id).map(|e| &mut e.actor) {
                pet.acting = false;
                // 停在哪儿就是哪儿,别把它们弹回原位。**还在走的让它走完**:
                // 收场时正走在回家路上的那只,半步停下会僵在半路
                if !matches!(pet.activity, Activity::Dragged | Activity::Walk { .. }) {
                    pet.activity = Activity::Idle { remaining: 1.0 };
                    pet.player.play(pet.clips.idle);
                }
            }
        }
        // 被打断的也要记冷却:不然人一松手它俩立刻又演一遍
        self.script_cooldown
            .push((perf.script.id, perf.script.cooldown));
    }

    /// 走一拍。
    fn apply_beat(&mut self, who: EntityId, other: EntityId, beat: Beat, home_x: f32) {
        let unit = self.body_unit(who, other);
        let Some(other_x) = self.entity(other).map(|e| e.foot_point().0) else {
            return;
        };
        let Some(entity) = self.entity_mut(who) else {
            return;
        };
        let my_x = entity.foot_point().0;
        let pos_x = entity.pos.0;
        let width = entity.actor.size().0 as f32;
        let Actor::Pet(pet) = &mut entity.actor else {
            return;
        };
        log::debug!("演出:#{} {beat:?}", who.0);
        match beat {
            Beat::Face => pet.target_yaw = camera_yaw(other_x > my_x),
            Beat::Play(name) => match pet.model.clip(name) {
                Some(clip) => {
                    pet.player.play(clip);
                    pet.activity = Activity::React {
                        remaining: pet.model.clips[clip].duration.max(0.3),
                    };
                }
                // 缺这段动作就跳过这一拍,整场照演 —— 全库动作覆盖不齐
                None => log::debug!("演出里缺 {name},跳过这一拍"),
            },
            Beat::Approach { gap, running } => {
                // 停在自己当前这一侧,不穿过对方
                let side = if my_x >= other_x { 1.0 } else { -1.0 };
                let target_center = other_x + side * gap * unit;
                pet.walk_to(target_center - width * 0.5, pos_x, running);
            }
            Beat::GoHome { running } => {
                pet.walk_to(home_x, pos_x, running);
            }
        }
    }

    /// 下一次推进该隔多久 —— 就是配置里那个目标帧率,**台上在干什么都不影响它**。
    ///
    /// 这里曾经按姿势变化速度自动降频(睡着的宠物落到 10Hz)。取消了:省下的那点
    /// CPU 换来的是「什么时候降、降到多少」全凭它自己判断,而帧率是用户看得见、
    /// 也说得出偏好的东西 —— 那就让配置里写多少就是多少。
    pub fn tick_interval(&self) -> Duration {
        Duration::from_secs_f32(1.0 / self.hz)
    }

    /// 推进时间:宠物的行为与动画。返回是否要重画/重设输入区。
    pub fn tick(&mut self, dt: f32) -> Reaction {
        let size = self.size;
        let mut reaction = Reaction::NONE;
        // 感知**先整台算完再推进**:边算边动的话,后面那几只看到的是同伴已经走过的位置,
        // 同一帧里的距离判定会不对称
        let perceptions: Vec<Perception> =
            (0..self.entities.len()).map(|i| self.perceive(i)).collect();
        for (entity, perception) in self.entities.iter_mut().zip(&perceptions) {
            let before = match &entity.actor {
                Actor::Pet(pet) => Some(activity_label(&pet.activity)),
                _ => None,
            };
            let one = Self::tick_entity(entity, dt, size, perception, &mut self.intents);
            if let (Some(before), Actor::Pet(pet)) = (before, &entity.actor)
                && before != activity_label(&pet.activity)
            {
                // 带上编号:多只在场时不带编号的日志根本读不出是谁在动
                log::debug!(
                    "宠物 #{} → {}(困倦 {:.2} 无聊 {:.2})",
                    entity.id.0,
                    activity_label(&pet.activity),
                    pet.needs.sleepiness,
                    pet.needs.boredom
                );
            }
            reaction.redraw |= one.redraw;
            reaction.regions_dirty |= one.regions_dirty;
        }
        let dispatched = self.dispatch_intents();
        reaction.redraw |= dispatched.redraw;
        reaction.regions_dirty |= dispatched.regions_dirty;
        // 演出**在个体行为之后**推进:这一拍下的指令要压过它们自己刚挑的事
        reaction.redraw |= self.tick_performance(dt);
        reaction
    }

    fn tick_entity(
        entity: &mut Entity,
        dt: f32,
        size: (u32, u32),
        perception: &Perception,
        intents: &mut Vec<Intent>,
    ) -> Reaction {
        let id = entity.id;
        let dragging = entity.drag_offset.is_some();
        // 地面线要在借出 `pet` 之前算好:它同时看 actor 与 size
        let ground = entity.ground_y(size);
        let Actor::Pet(pet) = &mut entity.actor else {
            return Reaction::NONE;
        };

        pet.petting.tick(dt);
        pet.needs.tick(dt, &pet.activity, &pet.persona);
        pet.self_speak_cooldown = (pet.self_speak_cooldown - dt).max(0.0);
        // 打招呼的冷却
        for (_, remaining) in pet.notices.iter_mut() {
            *remaining -= dt;
        }
        pet.notices.retain(|(_, remaining)| *remaining > 0.0);
        let mut moved = false;
        if !dragging {
            match pet.activity {
                Activity::Dragged => {
                    // 刚被放下
                    pet.activity = Activity::Idle { remaining: 1.0 };
                    pet.player.play(pet.clips.idle);
                }
                Activity::Falling { speed } => {
                    let speed = (speed + FALL_GRAVITY * dt).min(FALL_MAX_SPEED);
                    let next = entity.pos.1 + speed * dt;
                    if next >= ground {
                        entity.pos.1 = ground;
                        pet.activity = Activity::Idle { remaining: 1.0 };
                        pet.player.play(pet.clips.idle);
                    } else {
                        entity.pos.1 = next;
                        pet.activity = Activity::Falling { speed };
                    }
                    moved = true;
                }
                Activity::React { remaining } => {
                    let remaining = remaining - dt;
                    if remaining > 0.0 {
                        pet.activity = Activity::React { remaining };
                    } else if let Some(from_x) = pet.flee_from.take() {
                        // 受惊动作播完了 → 往反方向逃开。**用跑**(第 4 步欠的那条):
                        // 被吓到还慢悠悠走开说不过去;没有 Run 就退回 Walk,再没有就只能站着
                        pet.start_flee(from_x, entity.pos.0, perception.max_x);
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
                    } else if pet.acting {
                        // 正在演出里,两拍之间的空档:站着等下一拍,别自己跑去溜达
                        pet.activity = Activity::Idle { remaining: 0.5 };
                    } else if let Some(neighbor) = perception.nearest.filter(|n| {
                        n.distance <= NOTICE_DISTANCE * pet.persona.social && pet.notice_ready(n.id)
                    }) {
                        // 旁边站了个同伴 → 先打个招呼,再去忙自己的。
                        // 这里**只发意图**,转身与播动作在 `dispatch_intents` 里
                        intents.push(Intent {
                            from: id,
                            kind: IntentKind::Notice,
                            target: Some(neighbor.id),
                        });
                        // 意图没落地(比如对方这一帧被撤了)也不至于卡住:很快再挑一次
                        pet.activity = Activity::Idle { remaining: 0.5 };
                    } else {
                        // 待机结束:按需求挑下一件事
                        pet.choose_next(entity.pos.0, perception.max_x);
                    }
                }
                Activity::Walk { target_x, running } => {
                    let delta = target_x - entity.pos.0;
                    let speed = if running {
                        pet.run_speed
                    } else {
                        pet.walk_speed
                    };
                    let step = speed * dt;
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
    /// **先空转几轮**:xorshift 头一个输出被低位主导,种子相近时结果也相近。
    /// 第 6 步撞见过 —— 四只宠物用 7919 的倍数当种子,取到的嗓音属性全是 −1
    /// (听着一模一样)。运行时的种子取自纳秒时钟,本来就散;这是给「种子相近」兜底。
    fn new(seed: u64) -> Self {
        let mut rng = Self(seed | 1);
        for _ in 0..4 {
            rng.next_f32();
        }
        rng
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

/// 测试宠物的本体高度。画布是 200×200,本体比画布小(取景余量),取 120 与真实比例相当;
/// 于是一个身位 = 120px,`NOTICE_DISTANCE` 折合 240px。
#[cfg(test)]
const TEST_BODY_PX: f32 = 120.0;

/// 一份测试宠物的参数。要改哪项就 `PetBuild { form_id: 3758, ..test_build(m, 1) }`。
#[cfg(test)]
fn test_build(model: Arc<Model>, seed: u64) -> PetBuild {
    PetBuild {
        model,
        size: (200, 200),
        foot_offset: 180.0,
        body_px: TEST_BODY_PX,
        walk_speed: 100.0,
        run_speed: 250.0,
        form_id: 0,
        voice: None,
        persona: Persona::default(),
        voice_value: None,
        seed,
    }
}

#[cfg(test)]
mod home_tests {
    use super::*;

    /// 落脚点存的是**比例**,所以换了屏幕宽度还能回到「同样靠左三成」的地方。
    #[test]
    fn a_remembered_spot_survives_a_resize() {
        let mut stage = Stage::new((1000, 600));
        let model = Arc::new(Model::for_test(&["Idle", "Walk"]));
        let id = stage.spawn_at(Actor::Pet(PetActor::new(test_build(model, 1))), Some(0.25));
        // 可走范围 = 1000 − 画布 200 = 800,四分之一处是 200
        assert!((stage.entity(id).expect("在台上").pos().0 - 200.0).abs() < 1.0);
        assert!((stage.home_fraction(id).expect("该读得到") - 0.25).abs() < 0.01);

        // 换到更宽的屏:比例不变,像素跟着变
        stage.handle(StageEvent::Resized {
            width: 2000,
            height: 600,
        });
        assert!((stage.home_fraction(id).expect("该读得到") - 0.25).abs() < 0.01);
        assert!((stage.entity(id).expect("在台上").pos().0 - 450.0).abs() < 1.0);
    }

    /// 召回**不**尊重落脚点:它的用途就是「跑没影了,拉回来」。
    #[test]
    fn recall_ignores_the_remembered_spot() {
        let mut stage = Stage::new((1000, 600));
        let model = Arc::new(Model::for_test(&["Idle", "Walk"]));
        let id = stage.spawn_at(
            Actor::Pet(PetActor::new(test_build(model.clone(), 1))),
            Some(0.9),
        );
        stage.reset_position();
        let centred = stage.entity(id).expect("在台上").pos().0;
        assert!((centred - 400.0).abs() < 1.0, "该回到正中,实际 {centred}");

        // **记录也要作废**:留着的话下一次重建角色又会把它拽回 90% 去
        stage.replace_actor(id, Actor::Pet(PetActor::new(test_build(model, 2))));
        let after = stage.entity(id).expect("在台上").pos().0;
        assert!((after - 400.0).abs() < 1.0, "重建之后又跑回去了:{after}");
    }

    /// 重建角色(改整体大小、切形态)要回到记下的落脚点,而不是一律居中。
    ///
    /// 这条是实机逮到的:改一次整体大小,每只记下的位置就被抹成正中间,
    /// 而那个「正中间」还会被当成新的落脚点存回阵容 —— 记了等于没记。
    #[test]
    fn rebuilding_an_actor_returns_to_the_remembered_spot() {
        let mut stage = Stage::new((1000, 600));
        let model = Arc::new(Model::for_test(&["Idle", "Walk"]));
        let id = stage.spawn_at(
            Actor::Pet(PetActor::new(test_build(model.clone(), 1))),
            Some(0.9),
        );
        assert!((stage.home_fraction(id).expect("该读得到") - 0.9).abs() < 0.01);

        stage.replace_actor(id, Actor::Pet(PetActor::new(test_build(model, 2))));
        assert!(
            (stage.home_fraction(id).expect("该读得到") - 0.9).abs() < 0.01,
            "重建之后落脚点被抹掉了"
        );
    }

    /// 嗓音给了就用给的,没给才随机 —— 存档里存的就是掷出来那一次。
    #[test]
    fn a_saved_voice_value_is_used_verbatim() {
        let model = Arc::new(Model::for_test(&["Idle", "Walk"]));
        let pet = PetActor::new(PetBuild {
            voice_value: Some(-0.37),
            ..test_build(model.clone(), 1)
        });
        assert!((pet.voice_value + 0.37).abs() < 1e-6);
        // 不给就现掷一个,落在 −1..1
        let rolled = PetActor::new(test_build(model, 7));
        assert!((-1.0..=1.0).contains(&rolled.voice_value));
    }

    /// 覆盖率把「降级也算有」算进去 —— 否则只有 SleepStand 的那批会被误报成不会睡。
    #[test]
    fn coverage_counts_the_documented_fallbacks() {
        let mut clips = std::collections::HashMap::new();
        for name in ["Idle", "Walk", "SleepStand", "Alert", "Show"] {
            clips.insert(
                name.to_string(),
                crate::pack::Clip {
                    seconds: 1.0,
                    speed_cm_s: 0.0,
                },
            );
        }
        let form = crate::pack::Form::for_test(clips);
        // SleepLoop→SleepStand、Shock→Alert、Happy→Show、Fear→Alert 四个降级都该算有
        for name in ["SleepLoop", "Shock", "Happy", "Fear"] {
            assert!(has_clip(&form, name), "{name} 该按降级算有");
        }
        // 真没有的照样要报出来
        assert!(!has_clip(&form, "Run"));
        assert!(!has_clip(&form, "CallOut"));
    }

    /// 动作表说能点的,点下去就得真播出来。
    ///
    /// 幽星光只有 `SleepStand`:表格按 `has_clip` 算「睡着」能点,而 `play_clip`
    /// 以前拿 `model.clip` 直接找,于是点了只在日志里留一句「这只没有这段」。
    #[test]
    fn every_clip_the_table_offers_can_actually_be_played() {
        // 与上面那个形态同一套动作,只是换成运行时的模型
        let clips = ["Idle", "Walk", "SleepStand", "Alert", "Show"];
        let model = Arc::new(Model::for_test(&clips));
        let mut stage = Stage::new((1000, 600));
        let id = stage.spawn(Actor::Pet(PetActor::new(test_build(model, 3))));
        for name in ["SleepLoop", "Shock", "Happy", "Fear"] {
            assert!(stage.play_clip(id, name), "{name} 该按降级播得出来");
        }
        assert!(!stage.play_clip(id, "Run"), "真没有的还是得返回 false");
    }

    /// 眼睛跟着动作走:生气时是生气眼,睡着时是困倦眼,平时才是性格那张脸。
    #[test]
    fn the_face_follows_the_clip_being_played() {
        let model = Arc::new(Model::for_test(&["Idle", "Anger", "SleepStand"]));
        let mut stage = Stage::new((1000, 600));
        // 胆小 = 哭哭眼,拿它当「平时那张脸」才看得出被盖掉
        let timid = crate::persona::Persona::by_id("timid");
        let id = stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            persona: timid,
            ..test_build(model, 5)
        })));
        let face = |stage: &Stage| match stage.entity(id).map(|e| e.actor()) {
            Some(Actor::Pet(pet)) => pet.face().uv_offset(),
            _ => panic!("不是宠物"),
        };
        assert_eq!(face(&stage), timid.face.uv_offset(), "待机时是性格那张脸");
        assert!(stage.play_clip(id, "Anger"));
        assert_eq!(face(&stage), crate::persona::ANGRY.uv_offset());
        // 睡的那段只有 SleepStand,降级过去之后眼睛也得跟着变困
        assert!(stage.play_clip(id, "SleepLoop"));
        assert_eq!(face(&stage), crate::persona::SLEEPY.uv_offset());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage() -> Stage {
        let mut stage = Stage::new((800, 600));
        stage.spawn(Actor::Sprite(Sprite::test_pattern(64)));
        stage
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
        // 形状不跟着空:Win32 的窗口区域要拿它当形状用(见 platform/windows.rs)
        assert!(!s.shape_regions().is_empty());
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
        let mut stage = Stage::new((800, 600));
        for _ in 0..2 {
            stage.spawn(Actor::Sprite(Sprite::test_pattern(64)));
        }
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
    fn placement_waits_for_the_real_surface_size() {
        // stage 是先建再等 configure 的:那之前尺寸是 (1, 1),错开量会被整个夹掉。
        // 实测两只 315px 的宠物双双落在 x = 0,演出开场「相隔 0.0 身位」
        let mut stage = Stage::new((1, 1));
        for _ in 0..2 {
            stage.spawn(Actor::Sprite(Sprite::test_pattern(64)));
        }
        assert_eq!(
            stage.entities[0].pos.0, stage.entities[1].pos.0,
            "这时确实重叠"
        );
        stage.handle(StageEvent::Resized {
            width: 800,
            height: 600,
        });
        assert_ne!(
            stage.entities[0].pos.0, stage.entities[1].pos.0,
            "拿到真实尺寸就该重摆开"
        );
        // 之后的尺寸变化不再重摆(用户自己拖过的位置要留着)
        stage.entities[1].pos.0 = 700.0;
        stage.handle(StageEvent::Resized {
            width: 900,
            height: 600,
        });
        assert_eq!(stage.entities[1].pos.0, 700.0, "后续 resize 不该把它挪回去");
        // 召回也要错开:三只叠在一起的「召回」等于把它们藏成一只
        stage.spawn(Actor::Sprite(Sprite::test_pattern(64)));
        stage.reset_position();
        let xs: Vec<f32> = stage.entities.iter().map(|e| e.pos.0).collect();
        assert!(
            xs[0] != xs[1] && xs[1] != xs[2] && xs[0] != xs[2],
            "召回之后不该叠在一起: {xs:?}"
        );
    }

    #[test]
    fn empty_stage_is_a_valid_state() {
        // 托盘可以把最后一只也撤掉。那之后 stage 必须还能正常挨帧推进:
        // 输入区空 = 全穿透,点哪儿都不在,tick 也不该恐慌
        let mut stage = Stage::new((800, 600));
        assert!(stage.entities().is_empty());
        assert!(stage.input_regions().is_empty());
        assert_eq!(stage.pick(400.0, 300.0), None);
        assert_eq!(stage.tick(0.1), Reaction::NONE);
        assert!(stage.tick_interval() > Duration::ZERO, "空台的间隔不能是 0");
    }

    #[test]
    fn replace_actor_keeps_the_id_and_spares_the_others() {
        // 切形态换的是**那一只**:标识必须留着(托盘插槽与掩码回读都还认着它),
        // 同台其余的不能被动到 —— 早先那版 `replace_actor` 只认第一只,
        // 于是第二只永远换不了形态,而第一只会被别人的操作换掉
        let mut stage = two_sprites();
        stage.entities[1].pos = (100.0, 100.0);
        let target = stage.entities()[1].id();
        let untouched = stage.entities()[0].pos();

        assert!(stage.replace_actor(target, Actor::Sprite(Sprite::test_pattern(128))));
        assert_eq!(stage.entities().len(), 2);
        assert_eq!(stage.entities()[1].id(), target, "标识不该变");
        assert_eq!(
            stage.entity(target).expect("还在台上").actor().size(),
            (128, 128)
        );
        assert_eq!(stage.entities()[0].pos(), untouched, "不该动到别人");
        assert_eq!(stage.entities()[0].actor().size(), (64, 64));

        // 已经撤掉的标识:换不上,也不能误伤还在台上的
        assert!(stage.despawn(target));
        assert!(!stage.replace_actor(target, Actor::Sprite(Sprite::test_pattern(32))));
        assert_eq!(stage.entities()[0].actor().size(), (64, 64));
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
    fn same_form_entities_share_one_model() {
        let model = Arc::new(Model::for_test(&["Idle", "Walk"]));
        assert_eq!(Arc::strong_count(&model), 1);
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(test_build(Arc::clone(&model), 1))));
        stage.spawn(Actor::Pet(PetActor::new(test_build(Arc::clone(&model), 2))));
        // 两只在场,加上这里持有的那份 = 3;网格/动画/贴图只有一份
        assert_eq!(Arc::strong_count(&model), 3);
        let (Actor::Pet(a), Actor::Pet(b)) = (&stage.entities[0].actor, &stage.entities[1].actor)
        else {
            panic!("两只都该是宠物");
        };
        assert!(Arc::ptr_eq(&a.model, &b.model));
        // 撤掉一只,引用计数跟着降(缓存那边靠它判断能不能清)
        let second = stage.entities[1].id();
        assert!(stage.despawn(second));
        assert_eq!(Arc::strong_count(&model), 2);
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
            "Run",
            "Shock",
            "Happy",
            "Fear",
            "SleepStart",
            "SleepLoop",
            "SleepEnd",
        ]);
        let actor = Actor::Pet(PetActor::new(test_build(Arc::new(model), 7)));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(actor);
        stage
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
        // 受惊动作播完 → 逃开(**用跑**),而不是原地回待机
        let start_x = s.actor_pos().0;
        let mut fled_to = None;
        for _ in 0..40 {
            s.tick(0.05);
            if let Activity::Walk { running, target_x } = activity(&s) {
                assert!(running, "受惊逃跑该用跑");
                fled_to = Some(target_x);
                break;
            }
        }
        let target_x = fled_to.expect("受惊动作播完该起跑逃开");
        // 点的是画布正中,往哪边逃都行,但得逃出至少一个身位
        assert!(
            (target_x - start_x).abs() > TEST_BODY_PX,
            "逃跑目标该在一个身位之外,实际从 {start_x} 逃到 {target_x}"
        );
        // 逃到了就回常规状态(之后爱去哪去哪,不再断言位置)
        let mut arrived = false;
        for _ in 0..80 {
            s.tick(0.05);
            if (s.actor_pos().0 - target_x).abs() < 1.0 {
                arrived = true;
                break;
            }
        }
        assert!(arrived, "该跑到逃跑目标点");
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
        let lifted = s.actor_pos().1;
        s.handle(StageEvent::PointerReleased);
        // **松手不再瞬移**:先进入下落,一路掉到地面线才回待机
        assert!(
            matches!(activity(&s), Activity::Falling { .. }),
            "从半空松手该开始下落"
        );
        assert_eq!(s.actor_pos().1, lifted, "松手那一下不该跳位置");
        let ground = 600.0 - GROUND_MARGIN - 180.0;
        for _ in 0..120 {
            s.tick(1.0 / 60.0);
            if matches!(activity(&s), Activity::Idle { .. }) {
                break;
            }
            assert!(s.actor_pos().1 <= ground, "不该穿过地面");
        }
        assert!(
            matches!(activity(&s), Activity::Idle { .. }),
            "该落地回待机"
        );
        assert_eq!(s.actor_pos().1 + 180.0, 600.0 - GROUND_MARGIN);
    }

    #[test]
    fn far_target_runs_and_near_target_walks() {
        // 跑速比走速快,且只有远处才起跑
        let mut s = pet_stage();
        let max_x = 1000.0 - 200.0;
        match s.actor_mut_for_test() {
            Actor::Pet(pet) => {
                pet.needs.boredom = 1.0;
                // 近处:目标就在旁边 ⇒ 走(或原地),不该是跑
                pet.choose_next(0.0, 10.0);
                assert!(
                    !matches!(pet.activity, Activity::Walk { running: true, .. }),
                    "近距离不该起跑"
                );
                // 远处:反复挑几次,总会挑到超过三个身位的目标
                let mut saw_run = false;
                for _ in 0..40 {
                    pet.needs.boredom = 1.0;
                    pet.choose_next(0.0, max_x);
                    if matches!(pet.activity, Activity::Walk { running: true, .. }) {
                        saw_run = true;
                        break;
                    }
                }
                assert!(saw_run, "远处目标该起跑(测试模型带 Run)");
            }
            _ => panic!("该是宠物"),
        }
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

    /// 帧率**不随台上在干什么变**。以前静止时会自动降频,取消了 ——
    /// 用户选了 30 帧就该一直是 30 帧。
    #[test]
    fn the_frame_rate_ignores_what_the_pet_is_doing() {
        let mut s = pet_stage();
        s.set_fps(30.0);
        let want = Duration::from_secs_f32(1.0 / 30.0);
        // 合成模型没有动画通道 → 姿势纹丝不动,这正是以前会被降频的情形
        s.tick(0.05);
        s.tick(0.05);
        assert_eq!(s.tick_interval(), want, "站着不动也该按 30 帧推进");
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
        assert_eq!(s.tick_interval(), want, "走起来也还是 30 帧");
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
        let Activity::Walk { target_x, .. } = activity(&s) else {
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

/// 感知与事件总线(design.md §9 Phase 5 第 5 步)。
#[cfg(test)]
mod perception_tests {
    use super::*;

    /// 两只宠物,画布 200、本体 120px(一个身位)。
    fn two_pets() -> Stage {
        let model = Arc::new(Model::for_test(&["Idle", "Walk", "Run", "Happy"]));
        let mut stage = Stage::new((1000, 600));
        for seed in 1..=2 {
            stage.spawn(Actor::Pet(PetActor::new(test_build(
                Arc::clone(&model),
                seed,
            ))));
        }
        stage
    }

    #[test]
    fn distance_is_in_body_lengths_not_pixels() {
        // 关键判据:阈值必须随宠物尺寸缩放。同样隔 240px,
        // 一个身位 120 的看是 2 身位(算近),身位 60 的看是 4 身位(算远)
        let mut stage = two_pets();
        stage.entities[0].pos = (0.0, 400.0);
        stage.entities[1].pos = (240.0, 400.0);
        let near = stage.perceive(0).nearest.expect("旁边有一只");
        assert_eq!(near.id, stage.entities[1].id());
        assert!((near.distance - 2.0).abs() < 1e-3, "实际 {}", near.distance);
        assert!(near.on_right, "它在右边");
        // 反过来看是对称的(身位取两只的均值)
        let back = stage.perceive(1).nearest.expect("旁边有一只");
        assert!((back.distance - 2.0).abs() < 1e-3);
        assert!(!back.on_right);

        // 把两只都缩小一半,像素距离不变 → 身位翻倍
        for entity in &mut stage.entities {
            if let Actor::Pet(pet) = &mut entity.actor {
                pet.body_px = TEST_BODY_PX / 2.0;
            }
        }
        let near = stage.perceive(0).nearest.expect("旁边有一只");
        assert!((near.distance - 4.0).abs() < 1e-3, "实际 {}", near.distance);
    }

    #[test]
    fn nearest_is_by_foot_point_and_ignores_self() {
        let mut stage = two_pets();
        stage.spawn(Actor::Sprite(Sprite::test_pattern(64)));
        stage.entities[0].pos = (0.0, 400.0);
        stage.entities[1].pos = (600.0, 400.0);
        // 精灵摆在 0 号右边一点:它比 1 号近,感知不该只认宠物
        stage.entities[2].pos = (100.0, 400.0);
        let near = stage.perceive(0).nearest.expect("旁边有东西");
        assert_eq!(near.id, stage.entities[2].id(), "该取最近的那一个");
        // 台上只剩自己时没有邻居
        let id = stage.entities[0].id();
        let others: Vec<EntityId> = stage
            .entities()
            .iter()
            .map(|e| e.id())
            .filter(|other| *other != id)
            .collect();
        for other in others {
            stage.despawn(other);
        }
        assert_eq!(stage.perceive(0).nearest, None, "自己不算自己的邻居");
    }

    #[test]
    fn neighbours_greet_once_then_go_on_cooldown() {
        let mut stage = two_pets();
        // 挨着站(1.5 身位,在 NOTICE_DISTANCE 之内)
        stage.entities[0].pos = (0.0, 400.0);
        stage.entities[1].pos = (TEST_BODY_PX * 1.5, 400.0);
        let (a, b) = (stage.entities[0].id(), stage.entities[1].id());

        // 待机结束时会发注意意图并当场落地。**判据是冷却表**:只有真走完
        // 「发意图 → dispatch → apply_notice」这条链才会有记录
        for _ in 0..200 {
            stage.tick(0.05);
            // 别让它们走开:这条测的是打招呼与冷却,不是走位
            stage.entities[0].pos.0 = 0.0;
            stage.entities[1].pos.0 = TEST_BODY_PX * 1.5;
        }
        for entity in stage.entities() {
            if let Actor::Pet(pet) = entity.actor() {
                let other = if entity.id() == a { b } else { a };
                assert!(!pet.notice_ready(other), "挨着站该互相打过招呼并进冷却");
            }
        }

        // 打招呼看得见的那一半:转过去 + 播一段动作
        stage.entities[0].pos = (0.0, 400.0);
        if let Actor::Pet(pet) = &mut stage.entities[0].actor {
            pet.notices.clear();
            pet.target_yaw = 0.0;
            pet.activity = Activity::Idle { remaining: 5.0 };
        }
        assert!(stage.apply_notice(Intent {
            from: a,
            kind: IntentKind::Notice,
            target: Some(b),
        }));
        match stage.entities[0].actor() {
            Actor::Pet(pet) => {
                assert!(matches!(pet.activity, Activity::React { .. }), "该播一段");
                assert_eq!(pet.target_yaw, camera_yaw(true), "该转向右边那只");
            }
            _ => panic!("不是宠物"),
        }
        // 对象已经不在台上:当没发生过,不能恐慌
        stage.despawn(b);
        assert!(!stage.apply_notice(Intent {
            from: a,
            kind: IntentKind::Notice,
            target: Some(b),
        }));
    }

    #[test]
    fn a_lone_pet_never_greets() {
        // 台上只有一只时,待机结束该照常去走动/做表情,而不是卡在「等意图落地」
        let mut stage = two_pets();
        let extra = stage.entities[1].id();
        stage.despawn(extra);
        for _ in 0..400 {
            stage.tick(0.05);
        }
        assert!(stage.intents.is_empty(), "没有邻居就不该发注意意图");
        if let Actor::Pet(pet) = stage.entities[0].actor() {
            assert!(pet.notices.is_empty(), "没打过招呼,冷却表该是空的");
        }
    }
}

/// 演出脚本(design.md §9 Phase 5 第 6 步)。
#[cfg(test)]
mod act_tests {
    use super::*;

    fn script() -> &'static Script {
        &act::SCRIPTS[0]
    }

    /// 两位正主,挨着站(1 身位),都闲着。
    fn cast_on_stage() -> Stage {
        let model = Arc::new(Model::for_test(&[
            "Idle", "Walk", "Run", "CallOut", "Alert", "Show", "Happy", "Shock",
        ]));
        let mut stage = Stage::new((2000, 600));
        for form_id in script().cast {
            stage.spawn(Actor::Pet(PetActor::new(PetBuild {
                form_id,
                ..test_build(Arc::clone(&model), form_id as u64)
            })));
        }
        stage.entities[0].pos = (400.0, 400.0);
        stage.entities[1].pos = (400.0 + TEST_BODY_PX, 400.0);
        stage
    }

    fn acting(stage: &Stage, index: usize) -> bool {
        match stage.entities[index].actor() {
            Actor::Pet(pet) => pet.acting,
            _ => false,
        }
    }

    /// 推进到开演,返回用掉的秒数。
    fn run_until_start(stage: &mut Stage) -> f32 {
        let mut t = 0.0;
        for _ in 0..400 {
            stage.tick(0.05);
            t += 0.05;
            if stage.performance.is_some() {
                return t;
            }
        }
        panic!("一直没开演");
    }

    #[test]
    fn casting_needs_both_and_close_enough() {
        let mut stage = cast_on_stage();
        run_until_start(&mut stage);
        assert!(acting(&stage, 0) && acting(&stage, 1), "两位都该被占住");

        // 隔太远(> max_distance 身位)就不开演
        let mut far = cast_on_stage();
        far.entities[1].pos.0 = 400.0 + TEST_BODY_PX * (script().max_distance + 2.0);
        for _ in 0..400 {
            far.tick(0.05);
        }
        assert!(far.performance.is_none(), "隔太远不该开演");

        // 少一位也不开演
        let mut alone = cast_on_stage();
        let second = alone.entities[1].id();
        alone.despawn(second);
        for _ in 0..400 {
            alone.tick(0.05);
        }
        assert!(alone.performance.is_none(), "少一位不该开演");
    }

    #[test]
    fn same_form_twice_does_not_cast_itself() {
        // 台上两只**同一形态**时,不能拿同一只凑两个角色
        let model = Arc::new(Model::for_test(&["Idle", "Walk", "CallOut"]));
        let mut stage = Stage::new((2000, 600));
        for seed in 0..2 {
            stage.spawn(Actor::Pet(PetActor::new(PetBuild {
                form_id: script().cast[0],
                ..test_build(Arc::clone(&model), seed)
            })));
        }
        assert_eq!(stage.cast_for(script()), None);
    }

    #[test]
    fn a_poke_ends_the_show() {
        let mut stage = cast_on_stage();
        run_until_start(&mut stage);
        // 戳一下正在演的那只:受惊走 `react`,acting 被清掉,演出该收场
        let poked = stage.entities[1].id();
        if let Some(Actor::Pet(pet)) = stage.entity_mut(poked).map(|e| &mut e.actor) {
            pet.react(PetReaction::Startled, 0.5);
        }
        stage.tick(0.05);
        assert!(stage.performance.is_none(), "被打断该收场");
        assert!(!acting(&stage, 0) && !acting(&stage, 1), "两位都该放开");
        // 打断的也记冷却:松手之后不该立刻又演一遍
        for _ in 0..400 {
            stage.tick(0.05);
        }
        assert!(stage.performance.is_none(), "冷却里不该再开演");
    }

    #[test]
    fn a_removed_actor_ends_the_show() {
        let mut stage = cast_on_stage();
        run_until_start(&mut stage);
        let gone = stage.entities[1].id();
        stage.despawn(gone);
        stage.tick(0.05);
        assert!(stage.performance.is_none(), "演员被撤下该收场");
        assert!(!acting(&stage, 0), "剩下那位该放开");
    }

    #[test]
    fn the_whole_show_runs_and_then_releases() {
        let mut stage = cast_on_stage();
        run_until_start(&mut stage);
        let length = script().length;
        // 演完之前不许有人自己溜达走(`acting` 期间待机不触发 choose_next)
        let mut ticks = 0.0;
        while ticks < length - 0.2 {
            stage.tick(0.05);
            ticks += 0.05;
            assert!(stage.performance.is_some(), "{ticks:.1}s 时不该提前收场");
        }
        // 到点收场,两位都放开
        for _ in 0..20 {
            stage.tick(0.05);
        }
        assert!(stage.performance.is_none(), "到点该收场");
        assert!(!acting(&stage, 0) && !acting(&stage, 1));
        // 所有拍子都放过了
        assert!(
            stage
                .script_cooldown
                .iter()
                .any(|(id, _)| *id == script().id)
        );
    }

    #[test]
    fn a_missing_clip_only_skips_that_beat() {
        // 全库动作覆盖不齐:缺 CallOut 的形态也得能把整场演完
        let model = Arc::new(Model::for_test(&["Idle", "Walk", "Run", "Show"]));
        let mut stage = Stage::new((2000, 600));
        for form_id in script().cast {
            stage.spawn(Actor::Pet(PetActor::new(PetBuild {
                form_id,
                ..test_build(Arc::clone(&model), form_id as u64)
            })));
        }
        stage.entities[0].pos = (400.0, 400.0);
        stage.entities[1].pos = (400.0 + TEST_BODY_PX, 400.0);
        run_until_start(&mut stage);
        for _ in 0..(script().length / 0.05) as usize + 20 {
            stage.tick(0.05);
        }
        assert!(stage.performance.is_none(), "缺动作也该演到收场");
    }

    #[test]
    fn the_walker_ends_up_near_its_partner() {
        // 「跑过去」那一拍真的把它带到对方旁边:开演时隔 2 身位,`Approach{gap:1.3}` 之后
        // 该落在 1.3 身位附近
        let mut stage = cast_on_stage();
        stage.entities[1].pos.0 = 400.0 + TEST_BODY_PX * 2.0;
        run_until_start(&mut stage);
        // Approach 在第 2.0 秒,跑完给到 3.5 秒
        for _ in 0..70 {
            stage.tick(0.05);
        }
        let a = stage.entities[0].foot_point().0;
        let b = stage.entities[1].foot_point().0;
        let gap = (a - b).abs() / TEST_BODY_PX;
        assert!((gap - 1.3).abs() < 0.4, "该停在 1.3 身位左右,实际 {gap:.2}");
    }
}

/// 叫声(design.md §7 与 §9 Phase 6)。
#[cfg(test)]
mod voice_tests {
    use super::*;

    /// 一层假声音:字节内容无所谓,测试不解码。键是动作逻辑名。
    fn layer(keys: &[&str]) -> std::collections::HashMap<String, Arc<crate::audio::Pcm>> {
        keys.iter()
            .map(|k| (k.to_string(), Arc::new(crate::audio::Pcm::for_test())))
            .collect()
    }

    /// 一套只有叫声、没有动作音效的假库(音效层单独测)。
    /// 曲线取实测最常见的 ±300 音分。
    fn bank() -> Arc<VoiceBank> {
        Arc::new(VoiceBank {
            clips: layer(&["Happy", "Shock", "CallOut", "Relax"]),
            sfx: std::collections::HashMap::new(),
            cents_low: -300.0,
            cents_high: 300.0,
        })
    }

    fn pet_with_voice(voice_value: f32) -> Stage {
        let model = Arc::new(Model::for_test(&["Idle", "Walk", "Shock", "Happy"]));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            voice: Some(bank()),
            ..test_build(model, 7)
        })));
        if let Actor::Pet(pet) = &mut stage.entities[0].actor {
            pet.voice_value = voice_value;
        }
        stage
    }

    #[test]
    fn pitch_follows_the_voice_attribute() {
        // 游戏里 voice ∈ −100..100 经 RTPC 曲线变调,而 Wwise 的 pitch 就是重采样,
        // 所以这里是「按 2^(音分/1200) 调播放速率」。0 = 原声、+1 = 婉转、−1 = 粗嗓门
        for (value, cents) in [(0.0, 0.0), (1.0, 300.0), (-1.0, -300.0), (0.5, 150.0)] {
            let mut stage = pet_with_voice(value);
            let id = stage.entities[0].id();
            stage.speak(id, "Happy");
            let cues = stage.take_sounds();
            assert_eq!(cues.len(), 1, "voice={value} 该出一声");
            assert!(
                (cues[0].speed - speed_for_cents(cents)).abs() < 1e-6,
                "voice={value} 该按 {cents} 音分放,实际速率 {}",
                cues[0].speed
            );
        }
    }

    #[test]
    fn cues_are_drained_once() {
        let mut stage = pet_with_voice(0.0);
        let id = stage.entities[0].id();
        stage.speak(id, "CallOut");
        assert_eq!(stage.take_sounds().len(), 1);
        assert!(stage.take_sounds().is_empty(), "收过就没了,不会一直重放");
    }

    /// 两层一起响,而**只有叫声那层变调** —— 动作音效来自 `Pet_Action_*` 库,
    /// 650 个里只有 1 个挂了 `Pet_Vo_Pitch` 曲线,那条 RTPC 是给嗓子的。
    #[test]
    fn both_layers_play_and_only_the_voice_is_pitched() {
        let model = Arc::new(Model::for_test(&["Idle", "Happy"]));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            voice: Some(Arc::new(VoiceBank {
                clips: layer(&["Happy"]),
                sfx: layer(&["Happy"]),
                cents_low: -300.0,
                cents_high: 300.0,
            })),
            voice_value: Some(1.0),
            ..test_build(model, 11)
        })));
        let id = stage.entities[0].id();
        stage.speak(id, "Happy");
        let speeds: Vec<f32> = stage.take_sounds().iter().map(|c| c.speed).collect();
        assert_eq!(speeds.len(), 2, "叫声与音效该一起响");
        let want = speed_for_cents(300.0);
        assert!((speeds[0] - want).abs() < 1e-6, "叫声该变调: {speeds:?}");
        assert!((speeds[1] - 1.0).abs() < 1e-6, "音效不该变调: {speeds:?}");
    }

    /// 声音跟着**动作那张降级表**退。没有 `Shock` 只有 `Alert` 的形态,动作退到 Alert,
    /// 声音也得退到 Alert —— 两边各退各的就会出现「做着警觉的动作、叫着受惊的声」。
    #[test]
    fn sound_falls_back_along_the_same_table_as_the_clip() {
        let model = Arc::new(Model::for_test(&["Idle", "Alert"]));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            voice: Some(Arc::new(VoiceBank {
                clips: layer(&["Alert"]),
                sfx: std::collections::HashMap::new(),
                cents_low: -300.0,
                cents_high: 300.0,
            })),
            ..test_build(model, 13)
        })));
        let id = stage.entities[0].id();
        assert_eq!(fallbacks("Shock"), ["Alert"], "这条测试依赖降级表的这一行");
        stage.speak(id, "Shock");
        assert_eq!(stage.take_sounds().len(), 1, "该退到 Alert 那段,而不是哑掉");
    }

    /// 自发的声音有冷却,**人点出来的没有**。
    ///
    /// 待机表情大约每 20~40 秒一个,做一次响一次的话桌上那只每半分钟叫你一嗓子;
    /// 而受惊/点动作是人要它出声的,连点就该连响。
    #[test]
    fn spontaneous_sounds_are_rationed_but_asked_for_ones_are_not() {
        let model = Arc::new(Model::for_test(&["Idle", "Happy"]));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            voice: Some(bank()),
            ..test_build(model, 19)
        })));
        // 这只没有 `Sad`(bank() 只有四段):**没出声就不该占掉这一分钟**
        let Actor::Pet(pet) = &mut stage.entities[0].actor else {
            unreachable!()
        };
        pet.speak_self("Sad");
        assert!(stage.take_sounds().is_empty(), "没这段声音,本来就没得响");

        let Actor::Pet(pet) = &mut stage.entities[0].actor else {
            unreachable!()
        };
        pet.speak_self("Happy");
        // 上一下没出声,冷却不该已经起来
        assert_eq!(stage.take_sounds().len(), 1, "该响");

        let Actor::Pet(pet) = &mut stage.entities[0].actor else {
            unreachable!()
        };
        pet.speak_self("Happy");
        assert!(stage.take_sounds().is_empty(), "冷却里不该再自己叫");

        // 人点的不受冷却管
        let id = stage.entities[0].id();
        stage.speak(id, "Happy");
        assert_eq!(stage.take_sounds().len(), 1, "点出来的该响");
    }

    /// 换键那天,已经下载过的旧包不该整只哑掉。旧包的键是小写的四个触发点名。
    #[test]
    fn packs_exported_before_the_key_change_still_make_a_sound() {
        let model = Arc::new(Model::for_test(&["Idle", "Happy"]));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            voice: Some(Arc::new(VoiceBank {
                clips: layer(&["happy", "shock", "callout", "relax"]),
                sfx: std::collections::HashMap::new(),
                cents_low: -300.0,
                cents_high: 300.0,
            })),
            ..test_build(model, 23)
        })));
        let id = stage.entities[0].id();
        for name in ["Happy", "Shock", "CallOut", "Relax"] {
            stage.speak(id, name);
            assert_eq!(stage.take_sounds().len(), 1, "旧包的 {name} 该还认得出来");
        }
    }

    /// 配置窗口那张动作表点一下,**动作与声音一起来**。
    /// 原来点出来的动作是全程哑的:`play_clip` 压根不出声。
    #[test]
    fn the_action_table_makes_a_sound_too() {
        let model = Arc::new(Model::for_test(&["Idle", "Happy"]));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            voice: Some(bank()),
            ..test_build(model, 17)
        })));
        let id = stage.entities[0].id();
        assert!(stage.play_clip(id, "Happy"));
        assert_eq!(stage.take_sounds().len(), 1, "点动作该出声");
    }

    #[test]
    fn a_poke_cries_and_a_pickup_does_not() {
        // 受惊要出声;**被拎起来不出声** —— 拖动时指针一动就可能重入,
        // 叫起来会连成一串
        let mut stage = pet_with_voice(0.0);
        let (x, y) = (500.0f64, (600.0 - GROUND_MARGIN - 90.0) as f64);
        stage.handle(StageEvent::PointerPressed { x, y });
        stage.handle(StageEvent::PointerReleased);
        assert_eq!(stage.take_sounds().len(), 1, "点一下该受惊出声");

        stage.handle(StageEvent::PointerPressed { x, y });
        stage.handle(StageEvent::PointerMoved { x: x + 60.0, y });
        assert!(stage.take_sounds().is_empty(), "拎起来不该出声");
    }

    #[test]
    fn a_form_without_that_clip_stays_silent() {
        // 全库叫声覆盖不齐:缺哪一段就不出声,不能恐慌也不能放错的那段
        let model = Arc::new(Model::for_test(&["Idle"]));
        let clips = layer(&["Happy"]);
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            voice: Some(Arc::new(VoiceBank {
                clips,
                sfx: std::collections::HashMap::new(),
                cents_low: -300.0,
                cents_high: 300.0,
            })),
            ..test_build(model, 3)
        })));
        let id = stage.entities[0].id();
        stage.speak(id, "Sad");
        assert!(stage.take_sounds().is_empty(), "缺 Sad 就该没声");
        stage.speak(id, "Happy");
        assert_eq!(stage.take_sounds().len(), 1, "有 Happy 就该有声");
    }

    #[test]
    fn no_voice_bank_is_silent() {
        // 没导出叫声的形态(或者用户把音量设成 0)照样得能跑
        let model = Arc::new(Model::for_test(&["Idle", "Shock"]));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(test_build(model, 5))));
        let id = stage.entities[0].id();
        stage.speak(id, "Shock");
        assert!(stage.take_sounds().is_empty());
    }

    /// 不设嗓音就是**原调**,四只同物种听着就该一样 —— 以前这里是上台随机掷,
    /// 于是同一只每次启动都换个嗓子;现在要变声得自己去配置窗口里重掷。
    #[test]
    fn pets_sound_the_same_until_someone_rerolls() {
        let model = Arc::new(Model::for_test(&["Idle"]));
        let mut stage = Stage::new((1000, 600));
        for seed in 1..=3u64 {
            stage.spawn(Actor::Pet(PetActor::new(PetBuild {
                voice: Some(bank()),
                ..test_build(Arc::clone(&model), seed * 7919)
            })));
        }
        // 第四只手动设过嗓音(配置窗口里重掷出来的那种)
        stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            voice: Some(bank()),
            voice_value: Some(-0.6),
            ..test_build(Arc::clone(&model), 42)
        })));
        let ids: Vec<EntityId> = stage.entities().iter().map(|e| e.id()).collect();
        for id in ids {
            stage.speak(id, "Happy");
        }
        let speeds: Vec<f32> = stage.take_sounds().iter().map(|c| c.speed).collect();
        assert_eq!(speeds.len(), 4);
        assert!(
            speeds[..3].windows(2).all(|w| (w[0] - w[1]).abs() < 1e-6),
            "没设过嗓音的该是同一个音调: {speeds:?}"
        );
        assert!(
            (speeds[3] - speeds[0]).abs() > 1e-4,
            "重掷过的那只该听得出来不一样: {speeds:?}"
        );
    }
}

#[cfg(test)]
mod rate_tests {
    use super::*;

    /// 每一档都要真的落到推进间隔上,**空台也算** ——
    /// 台上没有宠物时也得有个合法的间隔,否则定时器排不出下一次。
    #[test]
    fn every_step_reaches_the_interval() {
        let mut stage = Stage::new((800, 600));
        for (fps, _) in crate::control::FPS_STEPS {
            stage.set_fps(*fps as f32);
            assert_eq!(
                stage.tick_interval(),
                Duration::from_secs_f32(1.0 / *fps as f32),
                "{fps} 帧没落到间隔上"
            );
        }
    }

    /// 默认值必须是配置里那个,否则「没配过」的台和「配了默认值」的台跑得不一样。
    #[test]
    fn a_fresh_stage_runs_at_the_configured_default() {
        let stage = Stage::new((800, 600));
        assert_eq!(
            stage.tick_interval(),
            Duration::from_secs_f32(1.0 / crate::config::DEFAULT_FPS as f32)
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
            "Run",
            "Shock",
            "SleepStart",
            "SleepLoop",
            "SleepEnd",
        ]);
        let actor = Actor::Pet(PetActor::new(test_build(Arc::new(model), 99)));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(actor);
        stage
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

    /// 睡着也照样按目标帧率推进。这里曾经降到 10Hz —— 那条优化取消了。
    #[test]
    fn sleeping_does_not_change_the_frame_rate() {
        let mut s = pet_stage();
        s.set_fps(60.0);
        match s.actor_mut_for_test() {
            Actor::Pet(pet) => pet.needs.sleepiness = 0.99,
            _ => unreachable!(),
        }
        run(&mut s, 30.0, |s| {
            matches!(pet(s).activity, Activity::Sleeping(SleepPhase::Asleep))
        });
        assert_eq!(s.tick_interval(), Duration::from_secs_f32(1.0 / 60.0));
    }
}
