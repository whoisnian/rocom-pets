//! KDE Plasma Wayland 后端:每个 output 一个 wlr-layer-shell 表面。
//!
//! 关键选择(理由见 docs/design.md §3.2):
//! - `Layer::Top` 而不是 `Overlay`:Overlay 会盖住菜单与通知;
//! - 四边 anchor + `set_size(0, 0)`:表面铺满整个 output,宠物是表面内的实体;
//! - `exclusive_zone(0)`:不占地方但尊重别人的独占区,于是拿到的是去掉任务栏的工作区;
//! - `KeyboardInteractivity::None`:永不抢键盘焦点;
//! - `set_input_region` 给出宠物轮廓的矩形近似,区域外的点击直接落到下层窗口。
//!
//! 全局穿透开关在 S1 阶段用 `SIGUSR1` 切换(KDE Wayland 下没有全局按键抓取,
//! 正式实现要走 KGlobalAccel 的 D-Bus 注册或 XDG GlobalShortcuts portal)。

use std::collections::HashMap;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use glam::Vec3;
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_dispatch2, delegate_registry,
    dispatch2::Dispatch2,
    globals::GlobalData,
    output::{OutputHandler, OutputState},
    reexports::calloop::{
        EventLoop, LoopHandle, channel,
        timer::{TimeoutAction, Timer},
    },
    reexports::calloop_wayland_source::WaylandSource,
    reexports::client::{
        Connection, Proxy, QueueHandle,
        globals::registry_queue_init,
        protocol::{wl_output, wl_pointer, wl_seat, wl_surface},
    },
    reexports::protocols::wp::{
        fractional_scale::v1::client::{
            wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
            wp_fractional_scale_v1::{self, WpFractionalScaleV1},
        },
        viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
};

use crate::control::{self, Control, TrayHandle, TrayPet};
use crate::pack::{Form, Pack, PackEntry};
use crate::pet::mask::MaskReadback;
use crate::pet::target::{PetTarget, view_proj};
use crate::pet::{Model, PetGpu};
use crate::render::{Gpu, Quad, QuadDraw, Target};
use crate::roster::{Roster, Slot};
use crate::sprite::Sprite;
use crate::stage::{Actor, EntityId, PetActor, PetBuild, Reaction, Stage, StageEvent};

use super::Options;

/// 定时器的起始间隔;之后每次由 `Stage::tick_interval` 按状态决定(待机降频)。
const TICK_HZ: f32 = 30.0;

/// 离屏画布的取景余量。伸展类动作已经算进 `Model::motion_bounds` 了,
/// 这里只需给描边外扩与边缘光留一点边。
const CANVAS_PADDING: f32 = 1.15;

/// 鼠标左键(linux input event code)。
const BTN_LEFT: u32 = 0x110;

/// 特效层的 UV 卷动需要一个连续时间源。取进程启动至今的秒数就够——
/// 它只驱动噪声流动,不参与任何逻辑,不必和动画时钟对齐。
fn effect_time() -> f32 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

pub fn run(options: Options) -> Result<()> {
    let conn = Connection::connect_to_env().context("连不上 Wayland 合成器")?;
    let (globals, event_queue) = registry_queue_init(&conn).context("注册表初始化失败")?;
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).context("拿不到 wl_compositor")?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .context("合成器不支持 wlr-layer-shell(本项目只支持 KDE Plasma Wayland)")?;
    // 这两个是「有则更好」:缺了只是画面略软,不影响功能。
    // ROCOM_PETS_NO_FRACTIONAL=1 可强制退回整数缩放,用来对比效果 / 排查合成器差异。
    let force_integer = std::env::var_os("ROCOM_PETS_NO_FRACTIONAL").is_some();
    if force_integer {
        log::warn!("ROCOM_PETS_NO_FRACTIONAL 已设:强制走整数缩放");
    }
    let fractional_scale = (!force_integer)
        .then(|| {
            globals
                .bind::<WpFractionalScaleManagerV1, _, _>(&qh, 1..=1, GlobalData)
                .inspect_err(|e| {
                    log::warn!("拿不到 wp_fractional_scale_manager_v1({e}),退回整数缩放")
                })
                .ok()
        })
        .flatten();
    let viewporter = (!force_integer)
        .then(|| {
            globals
                .bind::<WpViewporter, _, _>(&qh, 1..=1, GlobalData)
                .inspect_err(|e| log::warn!("拿不到 wp_viewporter({e}),退回整数缩放"))
                .ok()
        })
        .flatten();

    // 包已经由 main 读好(读不动的在那边就报错/警告过了),这里只挑形态
    let mut roster: Vec<Member> = Vec::new();
    for pet in options.pets {
        let form = match pet.pack.form_index(pet.form.as_deref()) {
            Ok(index) => index,
            Err(e) => {
                log::warn!("{} 的形态选不出来({e:#}),退用链首", pet.pack.species_name);
                0
            }
        };
        let f = &pet.pack.forms[form];
        log::info!(
            "宠物包 {}({}):{} 个形态,当前 {}({}),高 {:.0}cm,{} 个动作",
            pet.pack.species_name,
            pet.pack.species_id,
            pet.pack.forms.len(),
            f.name,
            f.asset,
            f.height_cm,
            f.clips.len()
        );
        roster.push(Member {
            pack: pet.pack,
            form,
        });
    }
    // 调试精灵是「一只都没有」时的占位(S1 的平台层验收对象):托盘里加了真宠物就撤掉,
    // 之后撤空也不再回来 —— 用过托盘的人再看见测试图案只会以为是坏了
    let sprite_mode = roster.is_empty();
    if sprite_mode {
        log::info!("阵容是空的,先用调试精灵占位(托盘里「加一只」就换成真宠物)");
    }
    // 「加一只」菜单要列整个包目录。**只读名字**:全库 539 个包,把动作表与材质表
    // 全解析出来只为显示一行字,启动就得多花一秒
    let available = match options.packs_dir.as_deref() {
        Some(dir) => {
            let entries = Pack::list_entries(dir);
            log::info!("包目录 {} 里有 {} 个包可加", dir.display(), entries.len());
            entries
        }
        None => Vec::new(),
    };

    let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
    instance_desc.backends = wgpu::Backends::VULKAN;
    let instance = wgpu::Instance::new(instance_desc);

    // 事件循环要先建:App 得攥着它的句柄 —— 空着台起来的那种情况下动画定时器是
    // 加第一只宠物时才挂上去的(见 `ensure_timer`)
    let mut event_loop: EventLoop<'static, App> =
        EventLoop::try_new().context("建 calloop 事件循环失败")?;
    let handle = event_loop.handle();

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        compositor,
        layer_shell,
        fractional_scale,
        viewporter,
        instance,
        gpu: None,
        sprite: Sprite::test_pattern(192),
        roster,
        available,
        sprite_mode,
        packs_dir: options.packs_dir,
        roster_path: options.roster_path,
        models: HashMap::new(),
        pet_gpus: HashMap::new(),
        px_per_cm: options.px_per_cm,
        stages: Vec::new(),
        pointer: None,
        tray: None,
        passthrough: options.passthrough,
        loop_handle: handle.clone(),
        ticking: false,
        exit: false,
    };

    WaylandSource::new(conn.clone(), event_queue)
        .insert(handle.clone())
        .map_err(|e| anyhow::anyhow!("挂 Wayland 事件源失败: {e}"))?;
    // 托盘 / 热键 / 信号都往同一个通道发命令
    let (control_tx, control_rx) = channel::channel();
    install_signal_source(control_tx.clone())?;
    if options.tray {
        let pets = app.tray_pets();
        let available = app.available.iter().map(|p| p.name.clone()).collect();
        match control::spawn_tray(control_tx.clone(), options.passthrough, pets, available) {
            Ok(tray) => app.tray = Some(tray),
            Err(e) => log::warn!("托盘不可用({e:#});用 kill -USR1 或热键代替"),
        }
    }
    if let Some(trigger) = options.hotkey {
        control::spawn_hotkey(control_tx.clone(), trigger);
    }
    // 自己的 D-Bus 接口:给「KDE 自定义快捷键绑命令」与脚本用
    if let Err(e) = control::serve_dbus(control_tx.clone()) {
        log::warn!("D-Bus 控制接口不可用({e:#})");
    }
    handle
        .insert_source(control_rx, |event, _, app: &mut App| {
            if let channel::Event::Msg(control) = event {
                app.handle_control(control);
            }
        })
        .map_err(|e| anyhow::anyhow!("挂控制通道失败: {e}"))?;

    // 只有宠物需要推进时间;精灵模式保持「只在事件时出帧」,空闲 CPU 才能是 0
    app.ensure_timer();

    log::info!(
        "pid {} 就位:托盘菜单 / 全局热键 / `rocom-pets --toggle-passthrough` / kill -USR1 都能切穿透",
        std::process::id()
    );
    while !app.exit {
        event_loop
            .dispatch(None, &mut app)
            .context("事件循环出错")?;
    }
    Ok(())
}

/// 把 SIGUSR1(切换穿透)与 SIGINT/SIGTERM(退出)接到控制通道上。
fn install_signal_source(tx: channel::Sender<Control>) -> Result<()> {
    use signal_hook::consts::{SIGINT, SIGTERM, SIGUSR1};

    let mut signals = signal_hook::iterator::Signals::new([SIGUSR1, SIGINT, SIGTERM])
        .context("注册信号处理失败")?;
    std::thread::spawn(move || {
        for signal in signals.forever() {
            let msg = if signal == SIGUSR1 {
                Control::TogglePassthrough
            } else {
                Control::Quit
            };
            if tx.send(msg).is_err() {
                break;
            }
        }
    });
    Ok(())
}

/// 阵容里的一只:包(整条进化链都在,供切形态)+ 当前形态下标。
struct Member {
    pack: Pack,
    form: usize,
}

/// 一个 output 上的 stage 表面。
struct StageWindow {
    output: wl_output::WlOutput,
    layer: LayerSurface,
    /// 建好但还没配置尺寸时先存着,首次 configure 后移进 `target`。
    pending_surface: Option<wgpu::Surface<'static>>,
    target: Option<Target>,
    /// 每只宠物一份画布 + 合成四边形;精灵模式下为空。按实体标识对应。
    pets: Vec<PetSurfaces>,
    /// 精灵模式的合成四边形。
    sprite_quad: Option<Quad>,
    stage: Stage,
    /// 逻辑尺寸(合成器给的),物理尺寸 = 逻辑 × scale。
    logical: (u32, u32),
    /// 精确缩放系数:有 wp_fractional_scale 时是它给的 n/120(如 1.5),
    /// 否则退回 wl_output 的整数 scale。
    scale: f32,
    /// viewport 把物理像素的 buffer 映射到逻辑尺寸;没有它就只能用整数 buffer_scale。
    viewport: Option<WpViewport>,
    /// 分数缩放对象要持有着才会继续收事件。
    _fractional: Option<WpFractionalScaleV1>,
    configured: bool,
    /// 上次推进动画的时刻,用来算 dt。
    last_tick: Option<Instant>,
    /// 掩码回读的轮转游标:**一帧最多回读一只**。N 只同帧全回读会把 Phase 2 压下去的
    /// 开销乘回来 —— 取景改按动作包围盒之后画布面积还涨了 1.64 倍,回读量按面积走。
    readback_cursor: usize,
    /// 阵容插槽 → 这台上对应实体的标识。**下标与 `App::roster` 严格对齐**:
    /// 托盘发过来的是插槽号,而每台上的 `EntityId` 各自独立(各 stage 自己发号)。
    slots: Vec<EntityId>,
}

/// 一只宠物在某个 stage 上的渲染资源。
///
/// **管线/网格/贴图(`PetGpu`)是共享的**,按 (包, 形态) 缓存在 `App.pet_gpus` 上 ——
/// 多实体、多显示器都只有一份。**每实体独立的是画布**(各自渲各自)与它的掩码回读。
struct PetSurfaces {
    id: EntityId,
    gpu: Arc<PetGpu>,
    canvas: PetTarget,
    quad: Quad,
    /// 轮廓掩码的异步回读:每帧提交一次,好了就换上(滞后一两帧无所谓)。
    readback: MaskReadback,
}

impl StageWindow {
    /// 该渲多大的 buffer(物理像素)。
    fn physical(&self) -> (u32, u32) {
        (
            ((self.logical.0 as f32 * self.scale).round() as u32).max(1),
            ((self.logical.1 as f32 * self.scale).round() as u32).max(1),
        )
    }

    /// 有 viewport 时告诉合成器「这张 buffer 该显示成多大」(逻辑像素)。
    /// 不设的话合成器会按 buffer_scale=1 把物理像素当逻辑像素,画面就大一圈。
    fn apply_viewport(&self) {
        if let Some(viewport) = &self.viewport {
            viewport.set_destination(self.logical.0.max(1) as i32, self.logical.1.max(1) as i32);
        }
    }
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    /// 分数缩放:KDE 的 150% 缩放在 wl_output 上只能报整数 2,靠这个协议才能拿到精确的 1.5。
    /// 合成器不支持时为 None,退回整数 buffer_scale(画面会被降采样,略软)。
    fractional_scale: Option<WpFractionalScaleManagerV1>,
    /// 配合分数缩放:buffer 按物理像素做,viewport 把它映射到逻辑尺寸。
    viewporter: Option<WpViewporter>,
    instance: wgpu::Instance,
    gpu: Option<Gpu>,
    sprite: Sprite,
    /// 在场阵容。**下标即插槽号**,托盘菜单与各 stage 的 `slots` 都按它对齐。
    roster: Vec<Member>,
    /// 包目录里能加的包(只有名字与位置,选中了才 `Pack::load`)。
    available: Vec<PackEntry>,
    /// 现在台上是不是那只调试精灵(阵容空着起来才有,加了真宠物就永久撤掉)。
    sprite_mode: bool,
    packs_dir: Option<PathBuf>,
    /// 阵容存档路径;None = 定不出位置,这次的加/撤只在内存里。
    roster_path: Option<PathBuf>,
    /// 已加载的模型,按 glb 路径(= 包 + 形态)缓存。多实体/多屏共享同一份网格与贴图,
    /// 否则每多一只 RSS 就翻一档(单只已经 219MB)。见 design.md §9 Phase 5 第 2 步。
    models: HashMap<PathBuf, Arc<Model>>,
    /// 同上,GPU 侧:管线/顶点缓冲/贴图按 (包, 形态) 共享。每实体独立的只有画布。
    pet_gpus: HashMap<PathBuf, Arc<PetGpu>>,
    px_per_cm: f32,
    stages: Vec<StageWindow>,
    pointer: Option<wl_pointer::WlPointer>,
    /// 托盘句柄:拿着它才能把勾选状态同步回菜单;None = 没起托盘。
    tray: Option<TrayHandle>,
    /// 当前是否穿透(stage 各自也有,这里存一份用于新建 stage 与回显托盘)。
    passthrough: bool,
    /// 事件循环句柄:动画定时器是**按需**挂的(空着台起来时不挂),见 `ensure_timer`。
    loop_handle: LoopHandle<'static, App>,
    /// 定时器已经挂上了。挂上就不摘 —— 撤空之后再加回来还得用它,
    /// 而空台的 `tick` 本身就是一次空转。
    ticking: bool,
    exit: bool,
}

impl App {
    fn add_output(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let info = self.output_state.info(&output);
        let name = info
            .as_ref()
            .and_then(|i| i.name.clone())
            .unwrap_or_else(|| "?".into());
        let scale = info
            .as_ref()
            .map(|i| i.scale_factor.max(1) as u32)
            .unwrap_or(1);

        let wl_surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            wl_surface,
            Layer::Top,
            Some("rocom-pets"),
            Some(&output),
        );
        // 铺满可用区域,但不参与布局:
        // exclusive_zone = 0 表示「不占地方,但尊重别人占的地方」,于是合成器给的是
        // 去掉任务栏之后的工作区——宠物正好踩在任务栏上沿,而不是藏到面板后面。
        // (-1 是「连别人的独占区一起无视」,那样脚底会被面板挡住。)
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, 0);
        layer.set_exclusive_zone(0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        // 走 viewport 时 buffer_scale 必须留在 1:缩放交给 viewport 的 destination,
        // 两个机制叠加会把尺寸乘两次
        let viewport = self
            .viewporter
            .as_ref()
            .map(|v| v.get_viewport(layer.wl_surface(), qh, GlobalData));
        let fractional = self.fractional_scale.as_ref().map(|m| {
            m.get_fractional_scale(
                layer.wl_surface(),
                qh,
                FractionalScaleData(layer.wl_surface().clone()),
            )
        });
        if viewport.is_none() {
            layer.wl_surface().set_buffer_scale(scale as i32);
        }
        // 首次提交必须不带 buffer,等合成器回 configure
        layer.commit();

        let surface = match self.create_wgpu_surface(conn, layer.wl_surface()) {
            Ok(surface) => surface,
            Err(e) => {
                log::error!("output {name}: 建 wgpu 表面失败: {e:#}");
                return;
            }
        };

        // 模型按形态共享(见 `load_model`),这里 clone 的只是 manifest 里那份元数据
        let forms: Vec<Form> = self
            .roster
            .iter()
            .map(|m| m.pack.forms[m.form].clone())
            .collect();
        let mut stage = Stage::new((1, 1));
        let mut slots = Vec::with_capacity(forms.len());
        for form in &forms {
            match self.build_pet_actor(form) {
                Ok(actor) => slots.push(stage.spawn(actor)),
                Err(e) => {
                    log::error!("output {name}: 加载宠物失败: {e:#}");
                    self.exit = true;
                    return;
                }
            }
        }
        if self.sprite_mode {
            stage.spawn(Actor::Sprite(self.sprite.clone()));
        }
        if self.passthrough != stage.passthrough() {
            stage.handle(StageEvent::TogglePassthrough);
        }

        log::info!("output {name} 上新建 stage(scale {scale})");
        self.stages.push(StageWindow {
            output,
            layer,
            pending_surface: Some(surface),
            target: None,
            pets: Vec::new(),
            sprite_quad: None,
            stage,
            logical: (1, 1),
            // 先用 wl_output 的整数 scale 兜着,分数缩放的 preferred_scale 一到就覆盖
            scale: scale as f32,
            viewport,
            _fractional: fractional,
            configured: false,
            last_tick: None,
            readback_cursor: 0,
            slots,
        });
    }

    /// 取这个形态的模型:缓存里有就直接共享,没有才读盘。
    ///
    /// **顺带清掉没人用的**:切形态之后旧模型的 `Arc` 只剩缓存这一份,不清的话
    /// 每访问一个形态就永久多占几 MB。判据是 `strong_count == 1`(只有缓存持有)。
    fn load_model(&mut self, form: &Form) -> Result<Arc<Model>> {
        if let Some(model) = self.models.get(&form.model) {
            return Ok(Arc::clone(model));
        }
        let model = Arc::new(Model::load(&form.model, &form.materials)?);
        self.models.retain(|_, cached| Arc::strong_count(cached) > 1);
        self.models.insert(form.model.clone(), Arc::clone(&model));
        Ok(model)
    }

    /// 把 manifest 里的厘米单位换成屏幕像素,算出画布尺寸与脚底位置。
    fn build_pet_actor(&mut self, form: &Form) -> Result<Actor> {
        let model = self.load_model(form)?;
        // 两个包围盒各管一件事:**尺寸**按绑定姿势(站姿高度不能随动作变),
        // **取景**按动作包围盒(否则伸手/张翅/跳跃会被画布裁掉,见 model.rs 的 motion_bounds)
        let stand = model.bounds.1 - model.bounds.0;
        let (frame_min, frame_max) = model.motion_bounds;
        let frame_extent = frame_max - frame_min;
        let frame_center = (frame_min + frame_max) * 0.5;
        let height_px = form.height_cm * form.scale * self.px_per_cm;
        // 画布是方的,取景按动作包围盒最长边;正交框半径 = 最长边/2 × 余量
        let longest = frame_extent
            .x
            .max(frame_extent.y)
            .max(frame_extent.z)
            .max(1e-4);
        let radius = longest * 0.5 * CANVAS_PADDING;
        // 画布边长 = 正交框的 2×半径(米),按「站姿高 ↔ height_px」的比例换成像素
        let side = (height_px * 2.0 * radius / stand.y.max(1e-4))
            .round()
            .max(16.0);
        // 脚底 = 绑定姿势下沿在正交框里的 NDC 位置(框心是动作包围盒中心,不一定等于站姿中心)
        let ndc_bottom = (model.bounds.0.y - frame_center.y) / radius;
        let foot_offset = (1.0 - ndc_bottom) * 0.5 * side;

        // 走路速度优先用动画自带位移反推的值(见 spike-s3.md),没有就给个常速
        let walk_speed_cm = form
            .clip("Walk")
            .map(|c| c.speed_cm_s)
            .filter(|v| *v > 1.0)
            .unwrap_or(40.0);
        // 跑速同理,但**必须钳制**:全库反推值中位 417cm/s、p90 563、最高 1125
        // (魔力猫那只 7.5m/s),照搬会让宠物一瞬间横穿屏幕。按走速的倍数夹 ——
        // 保留「这只跑起来相对更快」的个性,又不至于离谱。代价是极端值那几只脚会打滑,
        // 比起「一眨眼跑没影」是划算的。
        let run_speed_cm = form
            .clip("Run")
            .map(|c| c.speed_cm_s)
            .filter(|v| *v > 1.0)
            .unwrap_or(walk_speed_cm * 2.0)
            .clamp(walk_speed_cm * 1.2, walk_speed_cm * 3.0);
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x5eed)
            ^ (self.stages.len() as u64 * 0x9E3779B97F4A7C15);

        log::info!(
            "  {} 屏幕高 {:.0}px(画布 {}px,脚底 {:.0}px),走速 {:.0}cm/s,跑速 {:.0}cm/s",
            form.name,
            height_px,
            side as u32,
            foot_offset,
            walk_speed_cm,
            run_speed_cm
        );
        Ok(Actor::Pet(PetActor::new(PetBuild {
            model,
            size: (side as u32, side as u32),
            foot_offset,
            // 本体高度(≠ 画布边长:画布带取景余量)。距离阈值按它换算成「身位」
            body_px: height_px,
            walk_speed: walk_speed_cm * self.px_per_cm,
            run_speed: run_speed_cm * self.px_per_cm,
            form_id: form.id,
            seed,
        })))
    }

    /// 合成器告知这个表面的精确缩放(单位 1/120)。
    fn fractional_scale_changed(&mut self, surface: &wl_surface::WlSurface, scale_120: u32) {
        let Some(index) = self.stage_index(surface) else {
            return;
        };
        let scale = (scale_120 as f32 / 120.0).max(0.1);
        if (self.stages[index].scale - scale).abs() < 1e-4 {
            return;
        }
        log::info!(
            "stage {index}: 精确缩放 {scale}(wl_output 的整数值只能报到 {})",
            scale.ceil()
        );
        self.stages[index].scale = scale;
        self.resize_surfaces(index);
        self.render(index);
    }

    /// 缩放或逻辑尺寸变了:重配表面、重建宠物画布与掩码缓冲。
    fn resize_surfaces(&mut self, index: usize) {
        let Some(gpu) = self.gpu.as_ref() else { return };
        let physical = self.stages[index].physical();
        self.stages[index].apply_viewport();
        if let Some(target) = self.stages[index].target.as_mut() {
            target.resize(gpu, physical);
        }
        // 宠物画布跟着缩放走:换算在 render 里做,这里只要保证 buffer 尺寸对上。
        // **画布尺寸得逐只取**:阵容里可以是 161px 的喵喵配 481px 的魔力猫,
        // 拿其中一只的尺寸套到全台上,另一只不是糊就是被裁
        let scale = self.stages[index].scale;
        let sizes: Vec<(EntityId, (u32, u32))> = self.stages[index]
            .stage
            .entities()
            .iter()
            .map(|e| (e.id(), e.actor().size()))
            .collect();
        for (id, (aw, ah)) in sizes {
            let canvas = (
                ((aw as f32 * scale) as u32).max(1),
                ((ah as f32 * scale) as u32).max(1),
            );
            let Some(surfaces) = self.stages[index].pets.iter_mut().find(|s| s.id == id) else {
                continue;
            };
            if surfaces.canvas.resize(&gpu.device, canvas) {
                surfaces.quad = gpu.create_quad(surfaces.canvas.view());
            }
            surfaces.readback.resize(&gpu.device, canvas);
        }
    }

    /// 定时器驱动:推进所有 stage 的行为与动画。
    fn tick(&mut self) {
        let now = Instant::now();
        for index in 0..self.stages.len() {
            let dt = match self.stages[index].last_tick {
                Some(prev) => (now - prev).as_secs_f32().min(0.25), // 卡顿后别一次跳太远
                None => 1.0 / TICK_HZ,
            };
            self.stages[index].last_tick = Some(now);
            if !self.stages[index].configured {
                continue;
            }
            let mut reaction = self.stages[index].stage.tick(dt);

            // 看看上一帧要的轮廓回来了没(每只各有一份回读)
            if let Some(gpu) = self.gpu.as_ref() {
                let mut ready: Vec<(EntityId, _)> = Vec::new();
                for surfaces in &mut self.stages[index].pets {
                    if let Some(mask) = surfaces.readback.poll(&gpu.device) {
                        ready.push((surfaces.id, mask));
                    }
                }
                for (id, mask) in ready {
                    let mask_reaction = self.stages[index].stage.set_entity_mask(id, mask);
                    reaction.redraw |= mask_reaction.redraw;
                    reaction.regions_dirty |= mask_reaction.regions_dirty;
                }
            }
            self.apply(index, reaction);
        }
    }

    /// 所有 stage 里最急的那个推进间隔(姿势几乎不动时会自动放慢)。
    fn tick_interval(&self) -> Duration {
        self.stages
            .iter()
            .map(|s| s.stage.tick_interval())
            .min()
            .unwrap_or_else(|| Duration::from_secs_f32(1.0 / TICK_HZ))
    }

    fn create_wgpu_surface(
        &self,
        conn: &Connection,
        surface: &wl_surface::WlSurface,
    ) -> Result<wgpu::Surface<'static>> {
        let display =
            NonNull::new(conn.backend().display_ptr() as *mut _).context("wl_display 指针为空")?;
        let window =
            NonNull::new(surface.id().as_ptr() as *mut _).context("wl_surface 指针为空")?;
        // SAFETY: display 与 surface 的生命周期都长于返回的 Surface——Connection 活到
        // 进程结束,wl_surface 由 StageWindow 持有,且 Surface 与它同时被 drop。
        unsafe {
            self.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                        display,
                    ))),
                    raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(window)),
                })
        }
        .context("create_surface_unsafe 失败")
    }

    fn stage_index(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.stages
            .iter()
            .position(|s| s.layer.wl_surface() == surface)
    }

    fn handle_control(&mut self, control: Control) {
        match control {
            Control::TogglePassthrough => self.toggle_passthrough(),
            Control::Recall => self.recall(),
            Control::SwitchForm { slot, form } => self.switch_form(slot, form),
            Control::AddPet(index) => self.add_pet(index),
            Control::RemovePet(slot) => self.remove_pet(slot),
            Control::Quit => self.exit = true,
        }
    }

    /// 动画定时器按需挂上:空着台起来时不挂,加第一只宠物时才挂 ——
    /// 精灵模式的「空闲 CPU 0」是 S1 验收项,不能为了将来可能加宠物就一直空转。
    fn ensure_timer(&mut self) {
        if self.ticking || self.roster.is_empty() {
            return;
        }
        let interval = Duration::from_secs_f32(1.0 / TICK_HZ);
        match self.loop_handle.insert_source(
            Timer::from_duration(interval),
            |_, _, app: &mut App| {
                app.tick();
                // 间隔随状态变:待机降到 12fps,交互/行走回到 30fps
                TimeoutAction::ToDuration(app.tick_interval())
            },
        ) {
            Ok(_) => self.ticking = true,
            Err(e) => log::error!("挂动画定时器失败({e});宠物不会动"),
        }
    }

    /// 托盘菜单要的那份阵容快照。
    fn tray_pets(&self) -> Vec<TrayPet> {
        self.roster
            .iter()
            .map(|m| TrayPet {
                name: m.pack.forms[m.form].name.clone(),
                forms: m.pack.forms.iter().map(|f| f.name.clone()).collect(),
                current_form: m.form,
            })
            .collect()
    }

    fn refresh_tray(&self) {
        if let Some(tray) = &self.tray {
            tray.set_roster(self.tray_pets());
        }
    }

    /// 把阵容写回存档。存**包名**而不是路径 —— 包目录整个搬走时阵容还认得出来;
    /// 只有包不在包目录里(`--pack /some/where`)才存绝对路径。
    fn save_roster(&self) {
        let Some(path) = self.roster_path.as_deref() else {
            return;
        };
        let roster = Roster {
            pets: self
                .roster
                .iter()
                .map(|m| {
                    let in_packs_dir = self
                        .packs_dir
                        .as_deref()
                        .is_some_and(|dir| m.pack.dir.parent() == Some(dir));
                    let pack = match (in_packs_dir, m.pack.dir.file_name()) {
                        (true, Some(name)) => name.to_string_lossy().into_owned(),
                        _ => m.pack.dir.to_string_lossy().into_owned(),
                    };
                    Slot {
                        pack,
                        form: Some(m.pack.forms[m.form].asset.clone()),
                    }
                })
                .collect(),
        };
        if let Err(e) = roster.save(path) {
            log::warn!("阵容没存上({e:#});下次启动会回到上一次的阵容");
        }
    }

    /// 没实体在用的模型与管线丢掉。撤一只/切形态之后不清的话,它的网格与贴图
    /// 会一直占着(单只就有一两百 MB)。
    fn prune_caches(&mut self) {
        self.models.retain(|_, cached| Arc::strong_count(cached) > 1);
        self.pet_gpus.retain(|_, cached| Arc::strong_count(cached) > 1);
    }

    /// 从包目录里加一只。
    fn add_pet(&mut self, index: usize) {
        let Some(entry) = self.available.get(index) else {
            log::warn!("要加的包不在列表里(下标 {index})");
            return;
        };
        let dir = entry.dir.clone();
        let pack = match Pack::load(&dir) {
            Ok(pack) => pack,
            Err(e) => {
                log::error!("加一只失败:{dir:?} 读不了: {e:#}");
                return;
            }
        };
        let form = pack.forms[0].clone();
        // **先把每台的角色都建出来再提交**:中途失败会让插槽与实体对不上号,
        // 而插槽错位比「加不上」难查得多
        let mut actors = Vec::with_capacity(self.stages.len());
        for _ in 0..self.stages.len() {
            match self.build_pet_actor(&form) {
                Ok(actor) => actors.push(actor),
                Err(e) => {
                    log::error!("加一只 {} 失败: {e:#}", pack.species_name);
                    return;
                }
            }
        }
        log::info!("加一只 {}({})", pack.species_name, form.name);
        for (stage_index, actor) in actors.into_iter().enumerate() {
            // 占位的调试精灵让位
            if self.sprite_mode {
                let sprites: Vec<EntityId> = self.stages[stage_index]
                    .stage
                    .entities()
                    .iter()
                    .filter(|e| matches!(e.actor(), Actor::Sprite(_)))
                    .map(|e| e.id())
                    .collect();
                for id in sprites {
                    self.stages[stage_index].stage.despawn(id);
                }
                self.stages[stage_index].sprite_quad = None;
            }
            let id = self.stages[stage_index].stage.spawn(actor);
            self.stages[stage_index].slots.push(id);
            self.rebuild_pet_surfaces(stage_index);
            self.apply(
                stage_index,
                Reaction {
                    redraw: true,
                    regions_dirty: true,
                },
            );
        }
        self.sprite_mode = false;
        self.roster.push(Member { pack, form: 0 });
        self.ensure_timer();
        self.save_roster();
        self.refresh_tray();
    }

    /// 撤下阵容里的第几只。撤空了就是空台 —— 调试精灵不会回来。
    fn remove_pet(&mut self, slot: usize) {
        if slot >= self.roster.len() {
            return;
        }
        let member = self.roster.remove(slot);
        log::info!(
            "撤下 {}(还剩 {} 只)",
            member.pack.species_name,
            self.roster.len()
        );
        drop(member);
        for stage_index in 0..self.stages.len() {
            if slot >= self.stages[stage_index].slots.len() {
                continue;
            }
            let id = self.stages[stage_index].slots.remove(slot);
            self.stages[stage_index].stage.despawn(id);
            self.stages[stage_index].pets.retain(|s| s.id != id);
            self.apply(
                stage_index,
                Reaction {
                    redraw: true,
                    regions_dirty: true,
                },
            );
        }
        self.prune_caches();
        self.save_roster();
        self.refresh_tray();
    }

    /// 把某一只切到进化链上的另一个形态:重建模型与那套 GPU 资源,位置重新落地。
    fn switch_form(&mut self, slot: usize, index: usize) {
        let Some(member) = self.roster.get(slot) else {
            return;
        };
        if index >= member.pack.forms.len() || index == member.form {
            return;
        }
        let form = member.pack.forms[index].clone();
        // 同 `add_pet`:全建出来才提交,免得一半的台换了一半没换
        let mut actors = Vec::with_capacity(self.stages.len());
        for _ in 0..self.stages.len() {
            match self.build_pet_actor(&form) {
                Ok(actor) => actors.push(actor),
                Err(e) => {
                    log::error!("切形态失败: {e:#}");
                    return;
                }
            }
        }
        log::info!("切换形态 → {}({})", form.name, form.asset);
        self.roster[slot].form = index;
        for (stage_index, actor) in actors.into_iter().enumerate() {
            let Some(id) = self.stages[stage_index].slots.get(slot).copied() else {
                continue;
            };
            self.stages[stage_index].stage.replace_actor(id, actor);
            // 网格/贴图/画布/掩码缓冲全都跟形态绑,**只重建这一只的**
            self.stages[stage_index].pets.retain(|s| s.id != id);
            self.rebuild_pet_surfaces(stage_index);
            self.apply(
                stage_index,
                Reaction {
                    redraw: true,
                    regions_dirty: true,
                },
            );
        }
        self.prune_caches();
        self.save_roster();
        self.refresh_tray();
    }

    /// 把宠物召回屏幕中间(跑到边角、或多屏切换后找不着了)。
    fn recall(&mut self) {
        for index in 0..self.stages.len() {
            self.stages[index].stage.reset_position();
            self.apply(
                index,
                Reaction {
                    redraw: true,
                    regions_dirty: true,
                },
            );
        }
        log::info!("宠物已召回");
    }

    fn toggle_passthrough(&mut self) {
        let mut state = None;
        for i in 0..self.stages.len() {
            let reaction = self.stages[i].stage.handle(StageEvent::TogglePassthrough);
            state = Some(self.stages[i].stage.passthrough());
            self.apply(i, reaction);
        }
        if let Some(on) = state {
            self.passthrough = on;
            // 菜单里的勾选要跟上(穿透也可能是热键/信号切的)
            if let Some(tray) = &self.tray {
                tray.set_passthrough(on);
            }
            log::info!(
                "全局穿透: {}",
                if on {
                    "开(点击全部落到下层)"
                } else {
                    "关(可交互)"
                }
            );
        }
    }

    /// 按 stage 逻辑的反馈更新输入区并出帧。
    fn apply(&mut self, index: usize, reaction: Reaction) {
        if reaction.regions_dirty {
            self.update_input_region(index);
        }
        if reaction.redraw {
            self.render(index);
        }
    }

    fn update_input_region(&mut self, index: usize) {
        let stage = &self.stages[index];
        let region = match Region::new(&self.compositor) {
            Ok(region) => region,
            Err(e) => {
                log::error!("建 wl_region 失败: {e}");
                return;
            }
        };
        let rects = stage.stage.input_regions();
        for r in &rects {
            region.add(r.x, r.y, r.w as i32, r.h as i32);
        }
        // 空区域 = 全穿透;非空 = 只有这些矩形吃鼠标
        stage
            .layer
            .wl_surface()
            .set_input_region(Some(region.wl_region()));
        log::debug!("stage {index}: 输入区 {} 个矩形", rects.len());
    }

    fn render(&mut self, index: usize) {
        let Some(gpu) = self.gpu.as_ref() else { return };
        let stage = &mut self.stages[index];
        if stage.target.is_none() {
            return;
        }
        let scale = stage.scale as f32;

        // 每只宠物先画进**自己的**离屏画布(管线是共享的,画布不是);
        // 逐只 update + render + submit,所以共享那份 camera/joints 缓冲不会串。
        let order = stage.stage.draw_order();
        for id in &order {
            let Some(entity) = stage.stage.entity(*id) else {
                continue;
            };
            let Actor::Pet(pet) = entity.actor() else {
                continue;
            };
            let (aw, ah) = entity.actor().size();
            let canvas_size = ((aw as f32 * scale) as u32, (ah as f32 * scale) as u32);
            // 取景用动作包围盒(与 build_pet_actor 的画布尺寸算法必须一致),
            // 描边宽度用绑定姿势的尺度:免得动作一伸展描边就跟着变粗
            let extent = pet.model.bounds.1 - pet.model.bounds.0;
            let outline = extent.length() * 0.004;
            let view = view_proj(pet.model.motion_bounds, pet.yaw, CANVAS_PADDING);
            let matrices = pet.player.matrices.clone();
            let Some(surfaces) = stage.pets.iter_mut().find(|s| s.id == *id) else {
                continue;
            };
            if surfaces.canvas.resize(&gpu.device, canvas_size) {
                // 画布重建了,合成用的四边形绑的是旧纹理,要重绑
                surfaces.quad = gpu.create_quad(surfaces.canvas.view());
            }
            surfaces.gpu.update(
                &gpu.queue,
                view,
                Vec3::new(-0.4, 0.8, 0.6),
                outline,
                effect_time(),
                &matrices,
            );
            surfaces
                .canvas
                .render(&gpu.device, &gpu.queue, &surfaces.gpu);
            surfaces
                .readback
                .resize(&gpu.device, surfaces.canvas.size());
        }

        // 轮廓回读:**一帧只要一只**,按游标轮转。拷贝本身很小(几十 KB)且异步,
        // 但每只的画布都不小,N 只同帧全要会把出帧开销乘回来。
        // 从游标处起找**第一个到点的**(还在 140ms 节流里的跳过),否则这一帧的名额白费。
        // 回读结果在后续 tick 里 poll(见 App::tick)。
        let count = stage.pets.len();
        if count > 0 {
            let start = stage.readback_cursor % count;
            let chosen = (0..count)
                .map(|step| (start + step) % count)
                .find(|i| stage.pets[*i].readback.is_due());
            stage.readback_cursor = (start + 1) % count;
            if let Some(i) = chosen {
                let surfaces = &mut stage.pets[i];
                surfaces
                    .readback
                    .request(&gpu.device, &gpu.queue, surfaces.canvas.texture());
            }
        }

        // 再把每只的画布按 z 序合成到 stage 上(靠后的画在上面)
        let stage = &mut self.stages[index];
        let mut draws = Vec::with_capacity(order.len().max(1));
        for id in &order {
            let Some(entity) = stage.stage.entity(*id) else {
                continue;
            };
            let (px, py) = entity.pos();
            let (aw, ah) = entity.actor().size();
            let quad = match stage.pets.iter().find(|s| s.id == *id) {
                Some(surfaces) => &surfaces.quad,
                // 精灵模式:整台只有一只,共用那块合成四边形
                None => match stage.sprite_quad.as_ref() {
                    Some(quad) => quad,
                    None => continue,
                },
            };
            draws.push(QuadDraw {
                quad,
                pos: (px * scale, py * scale),
                size: (aw as f32 * scale, ah as f32 * scale),
                highlight: entity.is_dragging(),
            });
        }
        // **空台也要出一帧**:撤掉最后一只之后不画的话,合成器留着的还是上一帧,
        // 宠物看着像是没撤掉。空 draws 就是清成透明。
        let target = stage.target.as_mut().expect("上面已判过");
        if let Err(e) = target.render(gpu, &draws) {
            log::error!("stage {index} 出帧失败: {e:#}");
        }
    }

    /// 首次 configure:惰性初始化 GPU(要有表面才能挑适配器),建渲染目标与合成资源。
    fn ensure_target(&mut self, index: usize) {
        let Some(surface) = self.stages[index].pending_surface.take() else {
            return;
        };
        if self.gpu.is_none() {
            match Gpu::new(&self.instance, &surface) {
                Ok(gpu) => self.gpu = Some(gpu),
                Err(e) => {
                    log::error!("初始化 GPU 失败: {e:#}");
                    self.exit = true;
                    return;
                }
            }
        }
        let gpu = self.gpu.as_ref().expect("上面刚建好");
        let physical = self.stages[index].physical();
        self.stages[index].target = Some(gpu.create_target(surface, physical));

        self.rebuild_pet_surfaces(index);
    }

    /// 取这个形态的 GPU 资源(管线/顶点缓冲/贴图):缓存里有就共享,没有才建。
    /// 键与模型缓存同一把 —— 模型的来源路径,即 (包, 形态)。
    fn pet_gpu(&mut self, model: &Arc<Model>) -> Result<Arc<PetGpu>> {
        let Some(gpu) = self.gpu.as_ref() else {
            anyhow::bail!("GPU 还没初始化");
        };
        if let Some(cached) = self.pet_gpus.get(&model.source) {
            return Ok(Arc::clone(cached));
        }
        let built = Arc::new(PetGpu::new(&gpu.device, &gpu.queue, model, gpu.format())?);
        // 和 `load_model` 同样的清理:没实体在用的就别占着显存
        self.pet_gpus.retain(|_, cached| Arc::strong_count(cached) > 1);
        self.pet_gpus
            .insert(model.source.clone(), Arc::clone(&built));
        Ok(built)
    }

    /// (重)建这个 stage 上每只宠物的渲染资源:共享管线 + 每只自己的画布/四边形/掩码回读。
    /// 首次 configure、切形态、以及上/下实体之后都走这里。
    fn rebuild_pet_surfaces(&mut self, index: usize) {
        if self.gpu.is_none() {
            return;
        }
        let scale = self.stages[index].scale;
        // 先收齐这一台上的实体(id, 模型/精灵),免得后面借用打架
        let mut wanted: Vec<(EntityId, Option<Arc<Model>>, (u32, u32))> = Vec::new();
        for entity in self.stages[index].stage.entities() {
            let size = entity.actor().size();
            let model = match entity.actor() {
                Actor::Pet(pet) => Some(Arc::clone(&pet.model)),
                Actor::Sprite(_) => None,
            };
            wanted.push((entity.id(), model, size));
        }
        // 已经不在场的那些连着画布一起丢掉
        let live: Vec<EntityId> = wanted.iter().map(|(id, _, _)| *id).collect();
        self.stages[index].pets.retain(|s| live.contains(&s.id));

        for (id, model, (aw, ah)) in wanted {
            let canvas_size = (
                ((aw as f32 * scale) as u32).max(1),
                ((ah as f32 * scale) as u32).max(1),
            );
            let Some(model) = model else {
                // 调试精灵:整台一块合成四边形,不走离屏画布
                if let (Some(gpu), Actor::Sprite(sprite)) = (
                    self.gpu.as_ref(),
                    self.stages[index]
                        .stage
                        .entity(id)
                        .map(|e| e.actor())
                        .expect("刚收集的实体"),
                ) {
                    let view = gpu.upload_sprite(sprite);
                    self.stages[index].sprite_quad = Some(gpu.create_quad(&view));
                }
                continue;
            };
            if self.stages[index].pets.iter().any(|s| s.id == id) {
                continue; // 已经有了(画布尺寸由 render/resize 负责跟)
            }
            let pet_gpu = match self.pet_gpu(&model) {
                Ok(pet_gpu) => pet_gpu,
                Err(e) => {
                    log::error!("建宠物管线失败: {e:#}");
                    self.exit = true;
                    return;
                }
            };
            let gpu = self.gpu.as_ref().expect("上面判过");
            let canvas = PetTarget::new(&gpu.device, gpu.format(), canvas_size);
            let quad = gpu.create_quad(canvas.view());
            let readback = MaskReadback::new(&gpu.device, canvas_size);
            self.stages[index].pets.push(PetSurfaces {
                id,
                gpu: pet_gpu,
                canvas,
                quad,
                readback,
            });
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        let Some(index) = self.stage_index(surface) else {
            return;
        };
        // 有 wp_fractional_scale 时精确值以它为准,整数事件忽略,否则两边来回改写
        if self.stages[index].viewport.is_some() {
            return;
        }
        let scale = new_factor.max(1) as f32;
        if (self.stages[index].scale - scale).abs() < 1e-4 {
            return;
        }
        self.stages[index].scale = scale;
        self.stages[index]
            .layer
            .wl_surface()
            .set_buffer_scale(new_factor);
        let physical = self.stages[index].physical();
        if let (Some(gpu), Some(target)) = (self.gpu.as_ref(), self.stages[index].target.as_mut()) {
            target.resize(gpu, physical);
        }
        log::info!("stage {index}: scale 变为 {scale}");
        self.render(index);
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    /// 只在有变化时主动出帧,不靠 frame 回调驱动(空闲时一帧都不提交)。
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.add_output(conn, qh, output);
    }

    fn update_output(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        // 分辨率/缩放变化由 layer surface 的 configure 与 scale_factor_changed 承接
    }

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        // 拔掉显示器:连带销毁该 output 上的 stage(Drop 会释放 layer surface 与 wgpu 表面)
        if let Some(index) = self.stages.iter().position(|s| s.output == output) {
            log::info!("output 移除,销毁 stage {index}");
            self.stages.remove(index);
        }
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if let Some(index) = self.stage_index(layer.wl_surface()) {
            log::info!("stage {index} 被合成器关闭");
            self.stages.remove(index);
        }
        if self.stages.is_empty() {
            self.exit = true;
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let Some(index) = self.stage_index(layer.wl_surface()) else {
            return;
        };
        let (w, h) = configure.new_size;
        if w == 0 || h == 0 {
            log::warn!("stage {index}: configure 给了 {w}x{h},忽略");
            return;
        }
        self.stages[index].logical = (w, h);
        self.stages[index].apply_viewport();
        let physical = self.stages[index].physical();

        let first = !self.stages[index].configured;
        if first {
            self.stages[index].configured = true;
            self.ensure_target(index);
            log::info!(
                "stage {index}: 首次 configure {w}x{h}(物理 {}x{})",
                physical.0,
                physical.1
            );
        } else if let (Some(gpu), Some(target)) =
            (self.gpu.as_ref(), self.stages[index].target.as_mut())
        {
            target.resize(gpu, physical);
        }

        let reaction = self.stages[index].stage.handle(StageEvent::Resized {
            width: w,
            height: h,
        });
        if first {
            self.stages[index].stage.reset_position();
        }
        self.apply(
            index,
            Reaction {
                redraw: true,
                regions_dirty: true,
            },
        );
        let _ = reaction;
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        // 只要指针:键盘交互被设成 None,永不抢焦点
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(pointer) => self.pointer = Some(pointer),
                Err(e) => log::error!("拿指针失败: {e}"),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let Some(index) = self.stage_index(&event.surface) else {
                continue;
            };
            let (x, y) = event.position;
            let stage_event = match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    StageEvent::PointerMoved { x, y }
                }
                PointerEventKind::Leave { .. } => StageEvent::PointerLeft,
                PointerEventKind::Press { button, .. } if button == BTN_LEFT => {
                    StageEvent::PointerPressed { x, y }
                }
                PointerEventKind::Release { button, .. } if button == BTN_LEFT => {
                    StageEvent::PointerReleased
                }
                _ => continue,
            };
            let reaction = self.stages[index].stage.handle(stage_event);
            self.apply(index, reaction);
        }
    }
}

delegate_registry!(App);
delegate_dispatch2!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

// ── 自己 dispatch 的两个协议 ────────────────────────────────────────
// sctk 没封装分数缩放与 viewporter,而 delegate_dispatch2! 是「所有接口」的 blanket 实现,
// 不能再手写 Dispatch,所以按 sctk 的路子给 UserData 实现 Dispatch2。

/// 分数缩放对象的 UserData:记着它属于哪个表面,事件来了才知道改哪个 stage。
struct FractionalScaleData(wl_surface::WlSurface);

impl Dispatch2<WpFractionalScaleV1, App> for FractionalScaleData {
    fn event(
        &self,
        app: &mut App,
        _obj: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _conn: &Connection,
        _qh: &QueueHandle<App>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            app.fractional_scale_changed(&self.0, scale);
        }
    }
}

// 这三个只发请求、不收事件
impl Dispatch2<WpFractionalScaleManagerV1, App> for GlobalData {
    fn event(
        &self,
        _: &mut App,
        _: &WpFractionalScaleManagerV1,
        _: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}

impl Dispatch2<WpViewporter, App> for GlobalData {
    fn event(
        &self,
        _: &mut App,
        _: &WpViewporter,
        _: <WpViewporter as Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}

impl Dispatch2<WpViewport, App> for GlobalData {
    fn event(
        &self,
        _: &mut App,
        _: &WpViewport,
        _: <WpViewport as Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}
