//! KDE Plasma Wayland 后端:每个 output 一个 wlr-layer-shell 表面。
//!
//! 关键选择(理由见 docs/design.md §3.2):
//! - `Layer::Top` 而不是 `Overlay`:Overlay 会盖住菜单与通知;
//! - 四边 anchor + `set_size(0, 0)`:表面铺满整个 output,宠物是表面内的实体;
//! - `exclusive_zone(-1)`:不参与布局,不挤压其他窗口;
//! - `KeyboardInteractivity::None`:永不抢键盘焦点;
//! - `set_input_region` 给出宠物轮廓的矩形近似,区域外的点击直接落到下层窗口。
//!
//! 全局穿透开关在 S1 阶段用 `SIGUSR1` 切换(KDE Wayland 下没有全局按键抓取,
//! 正式实现要走 KGlobalAccel 的 D-Bus 注册或 XDG GlobalShortcuts portal)。

use std::ptr::NonNull;

use anyhow::{Context, Result};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_dispatch2, delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::calloop::{EventLoop, channel},
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

use crate::render::{Gpu, Target};
use crate::sprite::Sprite;
use crate::stage::{Reaction, Stage, StageEvent};

/// 鼠标左键(linux input event code)。
const BTN_LEFT: u32 = 0x110;

/// 外部信号转成的控制消息。
enum Control {
    TogglePassthrough,
    Quit,
}

pub fn run() -> Result<()> {
    let conn = Connection::connect_to_env().context("连不上 Wayland 合成器")?;
    let (globals, event_queue) = registry_queue_init(&conn).context("注册表初始化失败")?;
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).context("拿不到 wl_compositor")?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .context("合成器不支持 wlr-layer-shell(本项目只支持 KDE Plasma Wayland)")?;

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
    stage: Stage,
    /// 逻辑尺寸(合成器给的),物理尺寸 = 逻辑 × scale。
    logical: (u32, u32),
    scale: u32,
    configured: bool,
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
        // 铺满整个 output,但不参与布局
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, 0);
        layer.set_exclusive_zone(-1);
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

        log::info!("output {name} 上新建 stage(scale {scale})");
        self.stages.push(StageWindow {
            output,
            layer,
            pending_surface: Some(surface),
            target: None,
            stage: Stage::new(self.sprite.clone(), (1, 1)),
            logical: (1, 1),
            scale,
            configured: false,
        });
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
        let Some(target) = stage.target.as_mut() else {
            return;
        };
        let scale = stage.scale as f32;
        let (px, py) = stage.stage.sprite_pos();
        let sprite = stage.stage.sprite();
        let size = (
            (sprite.width as f32 * scale) as u32,
            (sprite.height as f32 * scale) as u32,
        );
        if let Err(e) = target.render(
            gpu,
            (px * scale, py * scale),
            size,
            stage.stage.is_dragging(),
        ) {
            log::error!("stage {index} 出帧失败: {e:#}");
        }
    }

    /// 首次 configure:惰性初始化 GPU(要有表面才能挑适配器),建渲染目标。
    fn ensure_target(&mut self, index: usize) {
        let Some(surface) = self.stages[index].pending_surface.take() else {
            return;
        };
        if self.gpu.is_none() {
            match Gpu::new(&self.instance, &surface, &self.sprite) {
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
            self.stages[index].stage.center();
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
