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

use std::ptr::NonNull;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use glam::Vec3;
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_dispatch2, delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::calloop::{
        EventLoop, channel,
        timer::{TimeoutAction, Timer},
    },
    reexports::calloop_wayland_source::WaylandSource,
    reexports::client::{
        Connection, Proxy, QueueHandle,
        globals::registry_queue_init,
        protocol::{wl_output, wl_pointer, wl_seat, wl_surface},
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

use crate::pack::{Form, Pack};
use crate::pet::mask::MaskReadback;
use crate::pet::target::{PetTarget, view_proj};
use crate::pet::{Model, PetGpu};
use crate::render::{Gpu, Quad, QuadDraw, Target};
use crate::sprite::Sprite;
use crate::stage::{Actor, PetActor, Reaction, Stage, StageEvent};

use super::Options;

/// 定时器的起始间隔;之后每次由 `Stage::tick_interval` 按状态决定(待机降频)。
const TICK_HZ: f32 = 30.0;

/// 离屏画布相对宠物身高留的余量:跳跃/伸展类动作会超出绑定姿势包围盒。
const CANVAS_PADDING: f32 = 1.35;

/// 鼠标左键(linux input event code)。
const BTN_LEFT: u32 = 0x110;

/// 外部信号转成的控制消息。
enum Control {
    TogglePassthrough,
    Quit,
}

pub fn run(options: &Options) -> Result<()> {
    let conn = Connection::connect_to_env().context("连不上 Wayland 合成器")?;
    let (globals, event_queue) = registry_queue_init(&conn).context("注册表初始化失败")?;
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).context("拿不到 wl_compositor")?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .context("合成器不支持 wlr-layer-shell(本项目只支持 KDE Plasma Wayland)")?;

    // 包在起窗口前就读掉:manifest 有问题要立刻报错,而不是等到画第一帧
    let form = match &options.pack {
        Some(dir) => {
            let pack = Pack::load(dir)?;
            let form = pack.form(options.form.as_deref())?.clone();
            log::info!(
                "宠物包 {}({}):形态 {}({}),高 {:.0}cm,{} 个动作",
                pack.species_name,
                pack.species_id,
                form.name,
                form.asset,
                form.height_cm,
                form.clips.len()
            );
            Some(form)
        }
        None => {
            log::info!("没给 --pack,用调试精灵(平台层验收模式)");
            None
        }
    };

    let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
    instance_desc.backends = wgpu::Backends::VULKAN;
    let instance = wgpu::Instance::new(instance_desc);

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        compositor,
        layer_shell,
        instance,
        gpu: None,
        sprite: Sprite::test_pattern(192),
        form,
        px_per_cm: options.px_per_cm,
        stages: Vec::new(),
        pointer: None,
        exit: false,
    };

    let mut event_loop: EventLoop<App> = EventLoop::try_new().context("建 calloop 事件循环失败")?;
    let handle = event_loop.handle();
    WaylandSource::new(conn.clone(), event_queue)
        .insert(handle.clone())
        .map_err(|e| anyhow::anyhow!("挂 Wayland 事件源失败: {e}"))?;
    install_signal_source(&handle)?;

    // 只有宠物需要推进时间;精灵模式保持「只在事件时出帧」,空闲 CPU 才能是 0
    if app.form.is_some() {
        let interval = Duration::from_secs_f32(1.0 / TICK_HZ);
        handle
            .insert_source(
                Timer::from_duration(interval),
                move |_, _, app: &mut App| {
                    app.tick();
                    // 间隔随状态变:待机降到 12fps,交互/行走回到 30fps
                    TimeoutAction::ToDuration(app.tick_interval())
                },
            )
            .map_err(|e| anyhow::anyhow!("挂动画定时器失败: {e}"))?;
    }

    log::info!("pid {} — kill -USR1 切换全局穿透", std::process::id());
    while !app.exit {
        event_loop
            .dispatch(None, &mut app)
            .context("事件循环出错")?;
    }
    Ok(())
}

/// 把 SIGUSR1(切换穿透)与 SIGINT/SIGTERM(退出)接进 calloop。
fn install_signal_source(
    handle: &smithay_client_toolkit::reexports::calloop::LoopHandle<'static, App>,
) -> Result<()> {
    use signal_hook::consts::{SIGINT, SIGTERM, SIGUSR1};

    let (tx, rx) = channel::channel();
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
    handle
        .insert_source(rx, |event, _, app| {
            if let channel::Event::Msg(msg) = event {
                match msg {
                    Control::TogglePassthrough => app.toggle_passthrough(),
                    Control::Quit => app.exit = true,
                }
            }
        })
        .map_err(|e| anyhow::anyhow!("挂信号事件源失败: {e}"))?;
    Ok(())
}

/// 一个 output 上的 stage 表面。
struct StageWindow {
    output: wl_output::WlOutput,
    layer: LayerSurface,
    /// 建好但还没配置尺寸时先存着,首次 configure 后移进 `target`。
    pending_surface: Option<wgpu::Surface<'static>>,
    target: Option<Target>,
    /// 宠物的离屏画布 + 它在 stage 上的合成四边形;精灵模式下是 None。
    pet: Option<PetSurfaces>,
    /// 精灵模式的合成四边形。
    sprite_quad: Option<Quad>,
    stage: Stage,
    /// 逻辑尺寸(合成器给的),物理尺寸 = 逻辑 × scale。
    logical: (u32, u32),
    scale: u32,
    configured: bool,
    /// 上次推进动画的时刻,用来算 dt。
    last_tick: Option<Instant>,
}

/// 宠物在某个 stage 上的一套 GPU 资源。
/// 网格/贴图目前每个 stage 各一份:多显示器时略浪费,等真有多屏需求再做共享。
struct PetSurfaces {
    gpu: PetGpu,
    canvas: PetTarget,
    quad: Quad,
    /// 轮廓掩码的异步回读:每帧提交一次,好了就换上(滞后一两帧无所谓)。
    readback: MaskReadback,
}

impl StageWindow {
    fn physical(&self) -> (u32, u32) {
        (self.logical.0 * self.scale, self.logical.1 * self.scale)
    }
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    instance: wgpu::Instance,
    gpu: Option<Gpu>,
    sprite: Sprite,
    /// 要显示的形态(来自宠物包);None = 调试精灵模式。
    form: Option<Form>,
    px_per_cm: f32,
    stages: Vec<StageWindow>,
    pointer: Option<wl_pointer::WlPointer>,
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
        layer.wl_surface().set_buffer_scale(scale as i32);
        // 首次提交必须不带 buffer,等合成器回 configure
        layer.commit();

        let surface = match self.create_wgpu_surface(conn, layer.wl_surface()) {
            Ok(surface) => surface,
            Err(e) => {
                log::error!("output {name}: 建 wgpu 表面失败: {e:#}");
                return;
            }
        };

        // 宠物模式下每个 stage 各加载一份模型:Model 不便共享,而加载只有几十毫秒
        let actor = match &self.form {
            Some(form) => match self.build_pet_actor(form) {
                Ok(actor) => actor,
                Err(e) => {
                    log::error!("output {name}: 加载宠物失败: {e:#}");
                    self.exit = true;
                    return;
                }
            },
            None => Actor::Sprite(self.sprite.clone()),
        };

        log::info!("output {name} 上新建 stage(scale {scale})");
        self.stages.push(StageWindow {
            output,
            layer,
            pending_surface: Some(surface),
            target: None,
            pet: None,
            sprite_quad: None,
            stage: Stage::new(actor, (1, 1)),
            logical: (1, 1),
            scale,
            configured: false,
            last_tick: None,
        });
    }

    /// 把 manifest 里的厘米单位换成屏幕像素,算出画布尺寸与脚底位置。
    fn build_pet_actor(&self, form: &Form) -> Result<Actor> {
        let model = Model::load(&form.model)?;
        let extent = model.bounds.1 - model.bounds.0;
        let height_px = form.height_cm * form.scale * self.px_per_cm;
        // 画布是方的(取景按包围盒最长边),边长 = 身高像素 × 余量 × (最长边/身高)
        let longest = extent.x.max(extent.y).max(extent.z).max(1e-4);
        let side = (height_px * CANVAS_PADDING * longest / extent.y.max(1e-4))
            .round()
            .max(16.0);
        // 脚底在画布里的位置:正交框半径 = 最长边/2 × 余量,包围盒下沿的 NDC 是 -(高/2)/半径
        let radius = longest * 0.5 * CANVAS_PADDING;
        let ndc_bottom = -(extent.y * 0.5) / radius;
        let foot_offset = (1.0 - ndc_bottom) * 0.5 * side;

        // 走路速度优先用动画自带位移反推的值(见 spike-s3.md),没有就给个常速
        let walk_speed_cm = form
            .clip("Walk")
            .map(|c| c.speed_cm_s)
            .filter(|v| *v > 1.0)
            .unwrap_or(40.0);
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x5eed)
            ^ (self.stages.len() as u64 * 0x9E3779B97F4A7C15);

        log::info!(
            "  {} 屏幕高 {:.0}px(画布 {}px,脚底 {:.0}px),走速 {:.0}cm/s",
            form.name,
            height_px,
            side as u32,
            foot_offset,
            walk_speed_cm
        );
        Ok(Actor::Pet(PetActor::new(
            model,
            (side as u32, side as u32),
            foot_offset,
            walk_speed_cm * self.px_per_cm,
            seed,
        )))
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

            // 看看上一帧要的轮廓回来了没
            if let (Some(gpu), Some(surfaces)) =
                (self.gpu.as_ref(), self.stages[index].pet.as_mut())
            {
                if let Some(mask) = surfaces.readback.poll(&gpu.device) {
                    let mask_reaction = self.stages[index].stage.set_pet_mask(mask);
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

    fn toggle_passthrough(&mut self) {
        let mut state = None;
        for i in 0..self.stages.len() {
            let reaction = self.stages[i].stage.handle(StageEvent::TogglePassthrough);
            state = Some(self.stages[i].stage.passthrough());
            self.apply(i, reaction);
        }
        if let Some(on) = state {
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
        let (px, py) = stage.stage.actor_pos();
        let (aw, ah) = stage.stage.actor().size();
        let highlight = stage.stage.is_dragging();

        // 宠物:先画进离屏画布,再把画布作为一张纹理合成到 stage 上
        if let (Actor::Pet(pet), Some(surfaces)) = (stage.stage.actor(), stage.pet.as_mut()) {
            let canvas_size = ((aw as f32 * scale) as u32, (ah as f32 * scale) as u32);
            if surfaces.canvas.resize(&gpu.device, canvas_size) {
                // 画布重建了,合成用的四边形绑的是旧纹理,要重绑
                surfaces.quad = gpu.create_quad(surfaces.canvas.view());
            }
            let bounds = pet.model.bounds;
            let extent = bounds.1 - bounds.0;
            let outline = extent.length() * 0.004;
            surfaces.gpu.update(
                &gpu.queue,
                view_proj(bounds, pet.yaw, CANVAS_PADDING),
                Vec3::new(-0.4, 0.8, 0.6),
                outline,
                &pet.player.matrices,
            );
            surfaces
                .canvas
                .render(&gpu.device, &gpu.queue, &surfaces.gpu);
            // 顺手要一份轮廓:拷贝很小(几十 KB)且是异步的,不阻塞出帧;
            // 回读结果在后续 tick 里 poll(见 App::tick)
            surfaces
                .readback
                .resize(&gpu.device, surfaces.canvas.size());
            surfaces
                .readback
                .request(&gpu.device, &gpu.queue, surfaces.canvas.texture());
        }

        let stage = &mut self.stages[index];
        let quad = match (stage.pet.as_ref(), stage.sprite_quad.as_ref()) {
            (Some(surfaces), _) => &surfaces.quad,
            (None, Some(quad)) => quad,
            (None, None) => return,
        };
        let target = stage.target.as_mut().expect("上面已判过");
        let draws = [QuadDraw {
            quad,
            pos: (px * scale, py * scale),
            size: (aw as f32 * scale, ah as f32 * scale),
            highlight,
        }];
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

        let scale = self.stages[index].scale as f32;
        let (aw, ah) = self.stages[index].stage.actor().size();
        match self.stages[index].stage.actor() {
            Actor::Pet(pet) => {
                let canvas_size = ((aw as f32 * scale) as u32, (ah as f32 * scale) as u32);
                match PetGpu::new(&gpu.device, &gpu.queue, &pet.model, gpu.format()) {
                    Ok(pet_gpu) => {
                        let canvas = PetTarget::new(&gpu.device, gpu.format(), canvas_size);
                        let quad = gpu.create_quad(canvas.view());
                        let readback = MaskReadback::new(&gpu.device, canvas_size);
                        self.stages[index].pet = Some(PetSurfaces {
                            gpu: pet_gpu,
                            canvas,
                            quad,
                            readback,
                        });
                    }
                    Err(e) => {
                        log::error!("建宠物管线失败: {e:#}");
                        self.exit = true;
                    }
                }
            }
            Actor::Sprite(sprite) => {
                let view = gpu.upload_sprite(sprite);
                self.stages[index].sprite_quad = Some(gpu.create_quad(&view));
            }
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
        let scale = new_factor.max(1) as u32;
        if self.stages[index].scale == scale {
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
