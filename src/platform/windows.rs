//! Windows 后端:每个显示器一个置顶的无边框窗口 + DirectComposition 合成。
//!
//! **开发机是 Linux**,这个后端靠交叉编译 + wine 冒烟 + 用户实机反馈来回磨
//! (已验:窗口/置顶/逐像素 alpha/真宠物渲染/命中与拖动/点击穿透/托盘;
//! 未验:加撤宠物的菜单、跑动时的边缘裁剪、多显示器)。验收清单见 docs/design.md §9 Phase 8。
//!
//! 关键选择(与 Wayland 后端的对照):
//! - **不用 `UpdateLayeredWindow`**:那条 GDI 路线要把每帧从 GPU 读回内存再交给系统,
//!   对逐帧渲染的桌宠不合适。走 `WS_EX_NOREDIRECTIONBITMAP` + DirectComposition,
//!   帧一直留在 GPU 上。
//! - **DComp 的 device/target/visual 交给 wgpu 建**:wgpu 30 的 dx12 后端有
//!   `Dx12SwapchainKind::DxgiFromVisual`,给个 HWND 它就自己建一棵最小合成树
//!   (design.md 原本写的是自己建 `IDCompositionDevice` 再把 visual 指针交给
//!   `SurfaceTargetUnsafe::CompositionVisual` —— 那条路仍然通,但没必要手写这段 COM)。
//! - **输入区在 Win32 叫「窗口区域」**(`SetWindowRgn`),和 Wayland 的
//!   `wl_surface::set_input_region` 是同一件事:交一组矩形过去,区域外的点击落到下面的
//!   程序上。**不能只靠 `WM_NCHITTEST` 返回 `HTTRANSPARENT`** —— 那个只在**同一线程**的
//!   窗口之间往下转发,穿不到别的进程去(实机第一次跑就栽在这:铺满工作区的窗口把整屏
//!   点击全吃了,除了任务栏哪儿都点不动)。区域的代价是**它同时裁剪渲染**,Wayland 那边
//!   不裁;好在矩形本来就是按掩码算的,裁掉的只是边缘那圈近乎全透明的像素。
//! - **全局穿透也只是「区域照旧 + 加 `WS_EX_TRANSPARENT`」**,别去清区域。Wayland 那边
//!   穿透是交个空输入区(不影响渲染),Win32 照搬不了 —— 区域清成整窗,上面那个「除了
//!   任务栏哪儿都点不动」就原样回来了(实机第二次栽在这);清成空区域倒是全屏都能穿,
//!   可宠物也一起被裁没了。所以形状始终按掩码来,`WS_EX_TRANSPARENT` 只负责宠物身上
//!   那几十个格子。
//! - **窗口铺满「工作区」**(`MONITORINFO::rcWork`,已经去掉任务栏),对应 Wayland 那边
//!   `exclusive_zone(0)` 拿到的区域:宠物踩在任务栏上沿,而不是藏到它后面。

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use glam::Vec3;
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, DeleteObject, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, RGN_OR, SetWindowRgn,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GWL_EXSTYLE, GetMessageW, HTCLIENT,
    HTTRANSPARENT, HWND_MESSAGE, HWND_TOPMOST, IDC_ARROW, KillTimer, LoadCursorW, MSG,
    PostQuitMessage, RegisterClassW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SetTimer, SetWindowLongPtrW, SetWindowPos, TranslateMessage, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCHITTEST, WM_TIMER, WNDCLASSW,
    WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE,
};
use windows::core::{HSTRING, PCWSTR};

use crate::audio::Audio;
use crate::control::{self, Control, TrayHandle};
use crate::pack::{Form, Pack, PackEntry};
use crate::pet::mask::MaskReadback;
use crate::pet::target::{PetTarget, view_proj};
use crate::pet::{Model, PetGpu};
use crate::render::{Gpu, Quad, QuadDraw, Target};
use crate::stage::{Actor, EntityId, Reaction, Stage, StageEvent, VoiceKind};

use super::Options;
use super::shared::{self, Assets, CANVAS_PADDING, Member, PetOptions};

/// 台上一只都没有时定时器的间隔(有台就按 `Stage::tick_interval`,
/// 那是配置里的目标帧率)。
const TICK_HZ: f32 = 30.0;
/// 窗口区域比掩码矩形往外放这么多**逻辑像素**。
///
/// 因为区域**同时裁剪渲染**:矩形跟着位置走是准的(`coverage` 是角色局部坐标,每帧按当前
/// 位置平移),但**姿势**是异步回读来的、滞后约 140ms —— 跑起来时甩动的四肢有可能已经
/// 越出上一帧算出的格子,那就会被裁掉一角。往外放两格(掩码格子是 8px)当保险。
/// 代价是宠物周围多一圈十几像素的地方也吃鼠标。
const REGION_MARGIN: f32 = 16.0;

const STAGE_CLASS: &str = "rocom-pets-stage";
/// 动画定时器 id(每个 stage 窗口一个,同一个 id 即可)。
const TIMER_TICK: usize = 1;

thread_local! {
    /// 窗口过程是 C 回调,拿不到 `&mut App`,只能靠这个线程局部指针绕回来。
    ///
    /// **为什么套一层 `RefCell`**:Win32 有一堆调用会**同步**把消息派回窗口过程 ——
    /// `CreateWindowExW` 发 `WM_CREATE`、`SetWindowPos` 发 `WM_WINDOWPOSCHANGED`、
    /// `TrackPopupMenu` 干脆自己跑一个模态消息循环。这时外层已经握着 `&mut App`,
    /// 裸指针再借一次就是别名 UB。借不到就把消息交回系统 —— 那几条重入进来的消息
    /// 本来也不需要我们处理。
    static APP: Cell<*const RefCell<App>> = const { Cell::new(std::ptr::null()) };
}

/// 特效层 UV 卷动的时间源(与 Wayland 后端同义)。
fn effect_time() -> f32 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

pub fn run(options: Options) -> Result<()> {
    // 必须在建任何窗口之前:否则系统会按「系统 DPI」给我们放大的假坐标,
    // 多显示器不同缩放时全乱。per-monitor v2 还负责 WM_DPICHANGED。
    // SAFETY: 进程级设置,只在启动时调一次。
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };

    let roster = shared::start_roster(options.pets);
    // 阵容为空就上调试精灵 —— 与 Wayland 后端同义,也是这一层的验收模式(S1):
    // 没有宠物包也能验「置顶 / 逐像素 alpha / 命中 / 穿透」。
    let sprite_mode = roster.is_empty();
    if sprite_mode {
        log::info!("阵容是空的,用调试精灵(平台层验收模式)");
    }

    // DX12 + 「从 HWND 自动建合成 visual」。**这两条缺一不可**:
    // Vulkan 后端在 Windows 上没有合成路径,而默认的 DxgiFromHwnd 不支持透明。
    let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
    instance_desc.backends = wgpu::Backends::DX12;
    instance_desc.backend_options.dx12.presentation_system =
        wgpu::Dx12SwapchainKind::DxgiFromVisual;
    let instance = wgpu::Instance::new(instance_desc);

    // 「加一只」菜单要列整个包目录。**只读名字**:五百多个包,把动作表与材质表全解析
    // 出来只为显示一行字,启动就得多花一秒
    let available = match options.packs_dir.as_deref() {
        Some(dir) => {
            let entries = Pack::list_entries(dir);
            log::info!("包目录 {} 里有 {} 个包可加", dir.display(), entries.len());
            entries
        }
        None => Vec::new(),
    };

    register_classes()?;
    // 隐藏的消息窗口:托盘回调、外部命令、动画定时器都落在它上面
    let control_hwnd = create_control_window()?;

    let (control_tx, control_rx) = channel();
    let app = App {
        instance,
        gpu: None,
        roster,
        available,
        packs_dir: options.packs_dir,
        roster_path: options.roster_path,
        config_path: options.config_path,
        sprite: crate::sprite::Sprite::test_pattern(192),
        sprite_mode,
        assets: Assets::default(),
        audio: if options.volume > 0.0 {
            Audio::open(options.volume)
        } else {
            None
        },
        px_per_cm: options.px_per_cm,
        fps: options.fps,
        stages: Vec::new(),
        control_hwnd,
        passthrough: options.passthrough,
        tray: None,
        control_rx,
        exit: false,
    };

    let app = RefCell::new(app);
    // 把 App 交给窗口过程。**必须在建 stage 窗口之前**:建窗口就会同步发消息回来。
    APP.set(&app as *const RefCell<App>);

    if options.tray {
        let mut guard = app.borrow_mut();
        let muted = guard.audio.as_ref().map(|a| a.muted());
        let pets = guard.tray_pets();
        let volume = guard.audio.as_ref().map(|a| a.volume()).unwrap_or(0.0);
        let common = control::Common {
            fps: guard.fps,
            px_per_cm: guard.px_per_cm,
            volume,
        };
        match control::spawn_tray(
            control_hwnd,
            control_tx.clone(),
            options.passthrough,
            pets,
            muted,
            common,
        ) {
            Ok(mut tray) => {
                tray.set_roster(guard.tray_pets());
                guard.tray = Some(tray);
            }
            Err(e) => log::warn!("托盘不可用({e:#});只能用 `rocom-pets --quit` 退出"),
        }
    }

    let result = (|| -> Result<()> {
        {
            let mut guard = app.borrow_mut();
            guard.create_stages()?;
            // **起不来就别进消息循环**:`ensure_target` 里 GPU 初始化失败只是把 exit 置位,
            // 而 `GetMessageW` 会一直阻塞 —— 结果是一个看不见的进程挂在那儿
            // (wine 里没有 DX12 适配器时实测如此)。
            anyhow::ensure!(!guard.exit, "GPU 起不来,退出");
            // 等窗口都建好再挂定时器,免得第一次 tick 时台还是空的
            guard.rearm_timer();
        }
        let mut msg = MSG::default();
        loop {
            // SAFETY: 标准消息循环;`GetMessageW` 返回 -1 表示出错。
            let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if got.0 == 0 {
                break; // WM_QUIT
            }
            anyhow::ensure!(got.0 != -1, "消息循环出错");
            // SAFETY: msg 刚由 GetMessageW 填好。
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if app.borrow().exit {
                break;
            }
        }
        Ok(())
    })();
    // SAFETY: 与 `rearm_timer` 里的 SetTimer 配对。
    let _ = unsafe { KillTimer(Some(control_hwnd), TIMER_TICK) };
    // 托盘图标要在指针失效前摘掉(它的 Drop 会用到 hwnd)
    app.borrow_mut().tray = None;
    APP.set(std::ptr::null());
    result
}

/// 一个显示器上的 stage 窗口。
struct StageWindow {
    hwnd: HWND,
    target: Option<Target>,
    pending_surface: Option<wgpu::Surface<'static>>,
    pets: Vec<PetSurfaces>,
    /// 精灵模式的合成四边形(整台一块,不走离屏画布)。
    sprite_quad: Option<Quad>,
    stage: Stage,
    /// 逻辑尺寸(= 物理 / scale)。
    logical: (u32, u32),
    /// 物理尺寸(窗口就是这么大;Win32 的窗口坐标本来就是物理像素)。
    physical: (u32, u32),
    scale: f32,
    last_tick: Option<Instant>,
    readback_cursor: usize,
    /// 指针是不是已经在这个窗口里(用来只注册一次 `WM_MOUSELEAVE`)。
    tracking: bool,
    /// 阵容插槽 → 这台上对应实体的标识。**下标与 `App::roster` 严格对齐**:
    /// 托盘发过来的是插槽号,而每台上的 `EntityId` 各自独立(各 stage 自己发号)。
    slots: Vec<EntityId>,
}

/// 一只宠物在某个 stage 上的渲染资源(与 Wayland 后端同构)。
struct PetSurfaces {
    id: EntityId,
    gpu: Arc<PetGpu>,
    canvas: PetTarget,
    quad: Quad,
    readback: MaskReadback,
}

struct App {
    instance: wgpu::Instance,
    gpu: Option<Gpu>,
    roster: Vec<Member>,
    /// 包目录里能加的包(只有名字与位置,选中了才 `Pack::load`)。
    available: Vec<PackEntry>,
    packs_dir: Option<PathBuf>,
    roster_path: Option<PathBuf>,
    /// 配置文件路径:托盘改音量/整体大小时写回它,`Reload` 时重读它。
    config_path: Option<PathBuf>,
    /// 阵容为空时的占位(平台层验收模式)。
    sprite: crate::sprite::Sprite,
    sprite_mode: bool,
    /// 按形态共享的模型/管线/叫声(见 platform/shared.rs)。
    assets: Assets,
    audio: Option<Audio>,
    px_per_cm: f32,
    /// 目标帧率,新建 stage 时交给它(见 `Stage::set_fps`)。
    fps: u32,
    stages: Vec<StageWindow>,
    /// 隐藏的消息窗口:托盘回调、外部命令、动画定时器都挂在它上面。
    control_hwnd: HWND,
    passthrough: bool,
    tray: Option<TrayHandle>,
    control_rx: Receiver<Control>,
    exit: bool,
}

impl App {
    fn tray_pets(&self) -> Vec<String> {
        shared::tray_pets(&self.roster)
    }

    fn refresh_tray(&mut self) {
        let pets = shared::tray_pets(&self.roster);
        let common = control::Common {
            fps: self.fps,
            px_per_cm: self.px_per_cm,
            volume: self.audio.as_ref().map(|a| a.volume()).unwrap_or(0.0),
        };
        if let Some(tray) = self.tray.as_mut() {
            tray.set_roster(pets);
            tray.set_common(common);
        }
    }

    /// 把阵容写回存档。先把运行时才知道的两项(嗓音、落脚点)收回来,见 shared.rs。
    fn save_roster(&mut self) {
        if let Some(first) = self.stages.first() {
            let slots = first.slots.clone();
            shared::sync_runtime_fields(&mut self.roster, &first.stage, &slots);
        }
        shared::save_roster(
            &self.roster,
            self.packs_dir.as_deref(),
            self.roster_path.as_deref(),
        );
    }

    /// 每个显示器建一个窗口。
    fn create_stages(&mut self) -> Result<()> {
        for rect in monitors()? {
            if let Err(e) = self.create_stage(rect) {
                log::error!("在 {rect:?} 上建窗口失败: {e:#}");
            }
        }
        anyhow::ensure!(!self.stages.is_empty(), "一个显示器窗口都没建起来");
        Ok(())
    }

    fn create_stage(&mut self, rect: RECT) -> Result<()> {
        let width = (rect.right - rect.left).max(1) as u32;
        let height = (rect.bottom - rect.top).max(1) as u32;
        let class = HSTRING::from(STAGE_CLASS);
        // SAFETY: 类已注册;参数都是刚算出来的合法值。
        let hwnd = unsafe {
            CreateWindowExW(
                stage_ex_style(self.passthrough),
                PCWSTR(class.as_ptr()),
                PCWSTR(HSTRING::from("rocom-pets").as_ptr()),
                WS_POPUP | WS_VISIBLE,
                rect.left,
                rect.top,
                width as i32,
                height as i32,
                None,
                None,
                Some(module_handle()?),
                None,
            )
        }
        .context("建窗口失败")?;

        // SAFETY: hwnd 刚建好。DPI 拿不到时退回 96(= 100%)。
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
        let logical = (
            ((width as f32 / scale).round() as u32).max(1),
            ((height as f32 / scale).round() as u32).max(1),
        );

        let surface = self.create_surface(hwnd)?;
        let mut stage = Stage::new(logical);
        stage.set_fps(self.fps as f32);
        // 第三项是落脚点:勾了「记住」的回它自己那儿,其余交给 Stage 错开摆
        let builds: Vec<(Form, PetOptions, Option<f32>)> = self
            .roster
            .iter()
            .map(|m| (m.form().clone(), m.options.clone(), shared::home_of(m)))
            .collect();
        let mut slots = Vec::with_capacity(builds.len());
        for (form, options, home) in &builds {
            match self.build_pet_actor(form, options) {
                Ok(actor) => slots.push(stage.spawn_at(actor, *home)),
                Err(e) => log::error!("加载宠物失败: {e:#}"),
            }
        }
        if self.sprite_mode {
            stage.spawn(Actor::Sprite(self.sprite.clone()));
        }
        if self.passthrough != stage.passthrough() {
            stage.handle(StageEvent::TogglePassthrough);
        }
        // Stage 建出来时尺寸就是真的,不像 Wayland 那样要等 configure —— 但仍然走一次
        // Resized,让「首次拿到真实尺寸就重摆」那条逻辑生效(见 stage.rs 的 `place`)
        stage.handle(StageEvent::Resized {
            width: logical.0,
            height: logical.1,
        });

        log::info!(
            "显示器 {}x{} @{:.2}x 上新建 stage(逻辑 {}x{})",
            width,
            height,
            scale,
            logical.0,
            logical.1
        );
        self.stages.push(StageWindow {
            hwnd,
            target: None,
            pending_surface: Some(surface),
            pets: Vec::new(),
            sprite_quad: None,
            stage,
            logical,
            physical: (width, height),
            scale,
            last_tick: None,
            readback_cursor: 0,
            tracking: false,
            slots,
        });
        let index = self.stages.len() - 1;
        self.ensure_target(index);
        // **必须马上设一次**:在此之前窗口是整块的,会吃掉整屏的点击
        self.update_window_region(index);
        // **也必须马上画一帧**。之后的帧都由 `WM_TIMER` 驱动,而 tick 只在
        // `Reaction::redraw` 时才出帧 —— 调试精灵**永远不产生 redraw**
        // (`tick_entity` 对非宠物直接返回 `Reaction::NONE`),于是窗口一直是空的。
        // 实机反馈就是这个:双击起来什么都没有,点了「召回」才冒出来(召回里有 render)。
        // Wayland 那边不会遇到,因为首次 configure 的处理里就渲了一帧。
        self.render(index);
        Ok(())
    }

    fn create_surface(&self, hwnd: HWND) -> Result<wgpu::Surface<'static>> {
        let handle = std::num::NonZeroIsize::new(hwnd.0 as isize).context("窗口句柄为空")?;
        // SAFETY: hwnd 由 StageWindow 持有,活到窗口销毁;Surface 也在那之前被丢掉。
        unsafe {
            self.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(
                        RawDisplayHandle::Windows(WindowsDisplayHandle::new()),
                    ),
                    raw_window_handle: RawWindowHandle::Win32(Win32WindowHandle::new(handle)),
                })
        }
        .context("create_surface_unsafe 失败")
    }

    /// 首次拿到表面:惰性建 GPU,再建渲染目标与每只宠物的画布。
    fn ensure_target(&mut self, index: usize) {
        let Some(surface) = self.stages[index].pending_surface.take() else {
            return;
        };
        if self.gpu.is_none() {
            match Gpu::new(&self.instance, &surface) {
                Ok(gpu) => {
                    // 画布尺寸按它钳(见 `Assets::build_actor`)。**建角色之前就得填上**
                    self.assets
                        .set_max_canvas(gpu.device.limits().max_texture_dimension_2d);
                    self.gpu = Some(gpu);
                }
                Err(e) => {
                    log::error!("初始化 GPU 失败: {e:#}");
                    self.exit = true;
                    return;
                }
            }
        }
        let gpu = self.gpu.as_ref().expect("上面刚建好");
        let physical = self.stages[index].physical;
        self.stages[index].target = Some(gpu.create_target(surface, physical));
        self.rebuild_pet_surfaces(index);
    }

    /// 造一只角色。资产缓存与换算都在 platform/shared.rs。
    fn build_pet_actor(&mut self, form: &Form, options: &PetOptions) -> Result<Actor> {
        let with_audio = self.audio.is_some();
        self.assets.build_actor(
            form,
            self.px_per_cm,
            options,
            with_audio,
            self.stages.len() as u64,
        )
    }

    fn pet_gpu(&mut self, model: &Arc<Model>) -> Result<Arc<PetGpu>> {
        let Some(gpu) = self.gpu.as_ref() else {
            anyhow::bail!("GPU 还没初始化");
        };
        self.assets.pet_gpu(gpu, model)
    }

    /// GPU 的最大 2D 纹理边长;还没起来时按保守值来(见 `shared::canvas_size`)。
    fn max_canvas(&self) -> u32 {
        self.gpu
            .as_ref()
            .map(|g| g.device.limits().max_texture_dimension_2d)
            .unwrap_or(8192)
    }

    fn rebuild_pet_surfaces(&mut self, index: usize) {
        if self.gpu.is_none() {
            return;
        }
        let scale = self.stages[index].scale;
        let max_canvas = self.max_canvas();
        // (标识, 模型, 逻辑尺寸);模型为 None = 调试精灵
        type Wanted = (EntityId, Option<Arc<Model>>, (u32, u32));
        let mut wanted: Vec<Wanted> = Vec::new();
        for entity in self.stages[index].stage.entities() {
            let size = entity.actor().size();
            let model = match entity.actor() {
                Actor::Pet(pet) => Some(Arc::clone(&pet.model)),
                Actor::Sprite(_) => None,
            };
            wanted.push((entity.id(), model, size));
        }
        let live: Vec<EntityId> = wanted.iter().map(|(id, _, _)| *id).collect();
        self.stages[index].pets.retain(|s| live.contains(&s.id));

        for (id, model, (aw, ah)) in wanted {
            let Some(model) = model else {
                // 调试精灵:整台一块合成四边形,不走离屏画布
                if let Some(gpu) = self.gpu.as_ref() {
                    let view = gpu.upload_sprite(&self.sprite);
                    self.stages[index].sprite_quad = Some(gpu.create_quad(&view));
                }
                continue;
            };
            if self.stages[index].pets.iter().any(|s| s.id == id) {
                continue;
            }
            let canvas_size = shared::canvas_size((aw, ah), scale, max_canvas);
            let pet_gpu = match self.pet_gpu(&model) {
                Ok(pet_gpu) => pet_gpu,
                Err(e) => {
                    log::error!("建宠物管线失败: {e:#}");
                    return;
                }
            };
            let gpu = self.gpu.as_ref().expect("上面判过");
            let canvas = PetTarget::new(&gpu.device, gpu.format(), canvas_size, &pet_gpu);
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

    fn render(&mut self, index: usize) {
        let Some(gpu) = self.gpu.as_ref() else { return };
        let stage = &mut self.stages[index];
        if stage.target.is_none() {
            return;
        }
        let scale = stage.scale;
        let order = stage.stage.draw_order();
        for id in &order {
            let Some(entity) = stage.stage.entity(*id) else {
                continue;
            };
            let Actor::Pet(pet) = entity.actor() else {
                continue;
            };
            let (aw, ah) = entity.actor().size();
            let canvas_size =
                shared::canvas_size((aw, ah), scale, gpu.device.limits().max_texture_dimension_2d);
            let view = view_proj(pet.model.motion_bounds, pet.yaw, CANVAS_PADDING);
            let matrices = pet.player.matrices.clone();
            // 表情:性格决定脸上那张图集用哪一格(见 persona.rs)
            let face_uv = pet.face_uv();
            let Some(surfaces) = stage.pets.iter_mut().find(|s| s.id == *id) else {
                continue;
            };
            if surfaces
                .canvas
                .resize(&gpu.device, canvas_size, &surfaces.gpu)
            {
                surfaces.quad = gpu.create_quad(surfaces.canvas.view());
            }
            surfaces.gpu.update(
                &gpu.queue,
                &crate::pet::FrameParams {
                    view_proj: view,
                    light_dir: Vec3::new(-0.4, 0.8, 0.6),
                    outline_scale: 1.0,
                    time: effect_time(),
                    // 复现目标实机的 MaterialQualityLevel=Low shader map。
                    high_material_quality: false,
                    face_uv,
                },
                &matrices,
            );
            surfaces
                .canvas
                .render(&gpu.device, &gpu.queue, &surfaces.gpu);
            surfaces
                .readback
                .resize(&gpu.device, surfaces.canvas.size());
        }

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

        let mut draws = Vec::with_capacity(order.len());
        for id in &order {
            let Some(entity) = stage.stage.entity(*id) else {
                continue;
            };
            let (px, py) = entity.pos();
            let (aw, ah) = entity.actor().size();
            let quad = match stage.pets.iter().find(|s| s.id == *id) {
                Some(surfaces) => &surfaces.quad,
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
        let target = stage.target.as_mut().expect("上面已判过");
        if let Err(e) = target.render(gpu, &draws) {
            log::error!("stage {index} 出帧失败: {e:#}");
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        for index in 0..self.stages.len() {
            let dt = match self.stages[index].last_tick {
                Some(prev) => (now - prev).as_secs_f32().min(0.25),
                None => 1.0 / TICK_HZ,
            };
            self.stages[index].last_tick = Some(now);
            let mut reaction = self.stages[index].stage.tick(dt);
            if let Some(gpu) = self.gpu.as_ref() {
                let mut ready: Vec<(EntityId, _)> = Vec::new();
                for surfaces in &mut self.stages[index].pets {
                    if let Some(mask) = surfaces.readback.poll(&gpu.device) {
                        ready.push((surfaces.id, mask));
                    }
                }
                for (id, mask) in ready {
                    let one = self.stages[index].stage.set_entity_mask(id, mask);
                    reaction.redraw |= one.redraw;
                    reaction.regions_dirty |= one.regions_dirty;
                }
            }
            self.apply(index, reaction);
        }
        self.flush_sounds();
        self.rearm_timer();
    }

    /// 按各 stage 里最急的那个间隔重挂定时器。
    ///
    /// **只挂一个**,而且挂在控制窗口上。每个 stage 窗口各挂一个的话,两块屏一个间隔里
    /// 就会 tick 两次(而 `tick` 自己已经遍历了所有 stage),动画直接快一倍。
    fn rearm_timer(&self) {
        let interval = self
            .stages
            .iter()
            .map(|s| s.stage.tick_interval())
            .min()
            .unwrap_or_else(|| Duration::from_secs_f32(1.0 / TICK_HZ));
        let ms = interval.as_millis().max(1) as u32;
        // SAFETY: 同一个 id 重复 SetTimer 就是改间隔,不会堆积。
        unsafe { SetTimer(Some(self.control_hwnd), TIMER_TICK, ms, None) };
    }

    fn flush_sounds(&mut self) {
        let Some(audio) = self.audio.as_ref() else {
            return;
        };
        for stage in &mut self.stages {
            for cue in stage.stage.take_sounds() {
                audio.play(&cue);
            }
        }
    }

    /// 重新贴合这块屏的工作区(缩放变了、分辨率变了、任务栏挪了都走这里)。
    fn relayout(&mut self, index: usize) {
        let hwnd = self.stages[index].hwnd;
        // SAFETY: hwnd 有效;两个调用都只读系统状态。
        let (rect, dpi) = unsafe {
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let rect = if GetMonitorInfoW(monitor, &mut info).as_bool() {
                info.rcWork
            } else {
                return;
            };
            (rect, GetDpiForWindow(hwnd))
        };
        let width = (rect.right - rect.left).max(1) as u32;
        let height = (rect.bottom - rect.top).max(1) as u32;
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
        let logical = (
            ((width as f32 / scale).round() as u32).max(1),
            ((height as f32 / scale).round() as u32).max(1),
        );
        let stage = &mut self.stages[index];
        if stage.physical == (width, height) && (stage.scale - scale).abs() < 1e-4 {
            return;
        }
        log::info!("stage {index}: 重新贴合 {width}x{height} @{scale:.2}x");
        // SAFETY: 把窗口挪到工作区并改大小;NOACTIVATE 保持不抢焦点。
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                rect.left,
                rect.top,
                width as i32,
                height as i32,
                SWP_NOACTIVATE,
            );
        }
        stage.physical = (width, height);
        stage.scale = scale;
        stage.logical = logical;
        stage.stage.handle(StageEvent::Resized {
            width: logical.0,
            height: logical.1,
        });
        if let (Some(gpu), Some(target)) = (self.gpu.as_ref(), self.stages[index].target.as_mut()) {
            target.resize(gpu, (width, height));
        }
        // 画布尺寸跟着缩放走(**逐只取**:阵容里各宠物尺寸不同)
        if let Some(gpu) = self.gpu.as_ref() {
            let max_canvas = gpu.device.limits().max_texture_dimension_2d;
            let sizes: Vec<(EntityId, (u32, u32))> = self.stages[index]
                .stage
                .entities()
                .iter()
                .map(|e| (e.id(), e.actor().size()))
                .collect();
            for (id, (aw, ah)) in sizes {
                let canvas = shared::canvas_size((aw, ah), scale, max_canvas);
                let Some(surfaces) = self.stages[index].pets.iter_mut().find(|s| s.id == id) else {
                    continue;
                };
                if surfaces.canvas.resize(&gpu.device, canvas, &surfaces.gpu) {
                    surfaces.quad = gpu.create_quad(surfaces.canvas.view());
                }
                surfaces.readback.resize(&gpu.device, canvas);
            }
        }
        // 窗口大小变了,区域是按旧尺寸算的
        self.update_window_region(index);
        self.render(index);
    }

    /// 把窗口的形状交给系统。Win32 的对应物是**窗口区域**,不是 `WM_NCHITTEST`。
    ///
    /// **`HTTRANSPARENT` 不够**:它只在**同一线程**的窗口之间往下转发命中,穿不到别的
    /// 进程去。一个铺满工作区的窗口只靠它的话,整屏的点击都会被我们吃掉 —— 实机第一次
    /// 跑就是这个症状:除了任务栏(不在 `rcWork` 里)哪儿都点不动。真正让点击落到别的
    /// 程序上的是窗口区域:区域之外的像素压根不属于这个窗口。
    ///
    /// 代价是**区域同时也裁剪渲染**(Wayland 的输入区只管输入)。好在这些矩形本来就是
    /// 按掩码生成的、覆盖了所有不透明像素,裁掉的只是边缘那圈低于阈值、近乎全透明的部分。
    ///
    /// **穿透时也照设不误**(用 `shape_regions`,它不看穿透开关)。这里曾经改成
    /// `SetWindowRgn(None)`「恢复整窗免得画面被裁」,只靠 `WS_EX_TRANSPARENT` 穿透 ——
    /// 实机上那个样式并没能把点击放过去,于是整窗又把整屏点击吃了个干净,还是只剩任务栏
    /// 能点(和上面那次一模一样的症状)。形状留着,最坏也就是宠物身上那几十个格子点不穿,
    /// 屏幕其余部分照常;`WS_EX_TRANSPARENT` 生效的话连宠物身上也穿。
    fn update_window_region(&mut self, index: usize) {
        let stage = &self.stages[index];
        let hwnd = stage.hwnd;
        let scale = stage.scale;
        let rects = stage.stage.shape_regions();
        // SAFETY: 下面是 GDI 区域的标准用法;临时区域用完即删,合并出来的那个交给
        // `SetWindowRgn` 之后**归系统所有**,不能再删。
        unsafe {
            let combined = CreateRectRgn(0, 0, 0, 0);
            let margin = REGION_MARGIN * scale;
            for r in &rects {
                let x = (r.x as f32 * scale - margin).floor() as i32;
                let y = (r.y as f32 * scale - margin).floor() as i32;
                let w = (r.w as f32 * scale + margin * 2.0).ceil() as i32;
                let h = (r.h as f32 * scale + margin * 2.0).ceil() as i32;
                let piece = CreateRectRgn(x, y, x + w, y + h);
                CombineRgn(Some(combined), Some(combined), Some(piece), RGN_OR);
                let _ = DeleteObject(piece.into());
            }
            SetWindowRgn(hwnd, Some(combined), false);
        }
        log::trace!("stage {index}: 窗口区域 {} 个矩形", rects.len());
    }

    /// 按 stage 逻辑的反馈更新窗口区域并出帧(与 Wayland 后端的 `apply` 同义)。
    fn apply(&mut self, index: usize, reaction: Reaction) {
        if reaction.regions_dirty {
            self.update_window_region(index);
        }
        if reaction.redraw {
            self.render(index);
        }
    }

    fn stage_index(&self, hwnd: HWND) -> Option<usize> {
        self.stages.iter().position(|s| s.hwnd == hwnd)
    }

    fn handle_control(&mut self, control: Control) {
        match control {
            Control::TogglePassthrough => self.toggle_passthrough(),
            Control::ToggleMute => self.toggle_mute(),
            Control::Recall => self.recall(),
            // 退出是「这套东西都收了」——配置窗口是另一个进程,得单独叫一声
            Control::Quit => {
                control::close_settings();
                self.exit = true;
                // SAFETY: 让消息循环退出。
                unsafe { PostQuitMessage(0) };
            }
            Control::Play(slot, clip) => self.play_clip(slot, clip),
            Control::SetFps(value) => self.set_fps(value),
            Control::SetPxPerCm(value) => self.set_px_per_cm(value),
            Control::SetVolume(value) => self.set_volume(value),
            Control::Reload => self.reload(false),
            Control::ReloadPacks => self.reload(true),
            Control::OpenSettings(page) => control::open_settings(page),
        }
    }

    /// 配置窗口的动作表点了一下:让第 `slot` 只播那段动作。
    fn play_clip(&mut self, slot: u32, clip: u32) {
        let Some((name, label)) = crate::stage::RUNTIME_CLIPS.get(clip as usize) else {
            return;
        };
        let mut played = false;
        for stage in &mut self.stages {
            if let Some(id) = stage.slots.get(slot as usize) {
                played |= stage.stage.play_clip(*id, name);
            }
        }
        log::info!(
            "手动播 {label}{}",
            if played { "" } else { "(这只没有这段)" }
        );
        for index in 0..self.stages.len() {
            self.apply(index, Reaction::BOTH);
        }
    }

    /// 改目标帧率。定时器每次触发都重新按 `tick_interval` 排下一次,
    /// 所以这里只要把新上限交给每一台就行(见 `rearm_timer`)。
    fn set_fps(&mut self, value: u32) {
        let value = value.clamp(
            *crate::config::FPS_RANGE.start(),
            *crate::config::FPS_RANGE.end(),
        );
        if self.fps == value {
            return;
        }
        log::info!("帧率: {value} 帧/秒");
        self.fps = value;
        self.write_config(&[("fps", crate::config::Setting::Int(value))]);
        self.apply_fps();
        self.refresh_tray();
    }

    /// 把目标帧率交给每一台。新建 stage 的那条路在 `create_stage` 里。
    fn apply_fps(&mut self) {
        for stage in &mut self.stages {
            stage.stage.set_fps(self.fps as f32);
        }
    }

    /// 改全局的每厘米像素数:所有宠物一起重建(画布尺寸与走速都是按它算的)。
    fn set_px_per_cm(&mut self, value: f32) {
        let value = value.clamp(0.5, 8.0);
        if (self.px_per_cm - value).abs() < 1e-4 {
            return;
        }
        log::info!("整体大小: {:.1} px/cm", value);
        self.px_per_cm = value;
        self.write_config(&[("px_per_cm", crate::config::Setting::Num(value))]);
        for slot in 0..self.roster.len() {
            self.rebuild_slot(slot);
        }
        self.assets.prune();
    }

    /// 改叫声音量。0 也只是不出声,**不关设备**(关了再想开就得重新初始化)。
    fn set_volume(&mut self, value: f32) {
        let value = value.clamp(0.0, 1.0);
        if let Some(audio) = self.audio.as_mut() {
            audio.set_volume(value);
            log::info!("叫声音量 {:.0}%", value * 100.0);
        }
        self.write_config(&[("volume", crate::config::Setting::Num(value))]);
        self.refresh_tray();
    }

    /// 写回 config.toml。失败只警告:值在内存里已经生效了。
    fn write_config(&self, updates: &[(&str, crate::config::Setting)]) {
        let Some(path) = self.config_path.as_deref() else {
            return;
        };
        if let Err(e) = crate::config::Config::write_back(path, updates) {
            log::warn!("配置没写回去({e:#});这次改动重启后会丢");
        }
    }

    /// 重新读配置与阵容存档,把台上的一切对齐过去(配置窗口存完盘就发这个)。
    /// **整个阵容重来**,不做差量 —— 理由见 Wayland 后端同名函数。
    fn reload(&mut self, rescan: bool) {
        // 重载耗时值得常驻一行:「加一只就卡一下」这类反馈全靠它定位是慢在哪一步
        let started = Instant::now();
        if let Some(path) = self.config_path.as_deref() {
            match crate::config::Config::load_or_create(path) {
                Ok(config) => {
                    self.px_per_cm = config.px_per_cm;
                    self.fps = config.fps;
                    self.apply_fps();
                    if let Some(audio) = self.audio.as_mut() {
                        audio.set_volume(config.volume);
                    }
                }
                Err(e) => log::warn!("重读配置失败({e:#}),沿用当前设置"),
            }
        }
        let slots = self
            .roster_path
            .as_deref()
            .and_then(crate::roster::Roster::load)
            .map(|saved| saved.pets)
            .unwrap_or_default();
        // 「新来的那几只要打个招呼」——加宠物现在走配置窗口 + Reload,
        // 不再有专门的 add_pet 命令,这一声就得在这儿认出来
        let before: Vec<String> = self
            .roster
            .iter()
            .map(|m| m.pack.species_name.clone())
            .collect();
        // **包目录只在 `rescan` 时重扫**(启动 / `--reload` / 导入删除包)。
        // 阵容解析共用这一份缓存,不再各扫各的
        if rescan {
            self.refresh_packs();
        }
        let entries = std::mem::take(&mut self.available);
        self.roster = shared::load_roster(&slots, &entries);
        self.available = entries;
        let greeting = shared::newcomers(&before, &self.roster);
        if !self.roster.is_empty() {
            self.sprite_mode = false;
        }
        self.respawn_all();
        // **计时要含重建**:重建才是剩下的大头(模型/GPU/叫声解码),
        // 打在它前面的话这行数字好看得没有意义
        log::info!(
            "已重载:{} 只在台上,用时 {:?}{}",
            self.roster.len(),
            started.elapsed(),
            if rescan { "(含重扫包目录)" } else { "" }
        );
        // 「启用召唤」那一声;开机恢复阵容时不叫
        for stage in &mut self.stages {
            for slot in &greeting {
                if let Some(id) = stage.slots.get(*slot) {
                    stage.stage.speak(*id, VoiceKind::CallOut);
                }
            }
        }
        self.flush_sounds();
        self.assets.prune();
        self.refresh_tray();
    }

    /// 把每台上的角色全部推倒重建(reload 用)。
    /// 重扫包目录。**只在该扫的时候扫**:启动、`--reload`、以及配置窗口那边
    /// 导入/删除了包。切形态、调大小那些走 `Reload` 的改动一律不扫 ——
    /// 把 201 个包的 manifest 全读一遍是热缓存 40ms、冷缓存 400ms,
    /// 而它们跟包目录八竿子打不着(「切形态有明显延迟」就是这么来的)。
    fn refresh_packs(&mut self) {
        if let Some(dir) = self.packs_dir.as_deref() {
            self.available = Pack::list_entries(dir);
        }
    }

    fn respawn_all(&mut self) {
        // 第三项是落脚点:勾了「记住」的回它自己那儿,其余交给 Stage 错开摆
        let builds: Vec<(Form, PetOptions, Option<f32>)> = self
            .roster
            .iter()
            .map(|m| (m.form().clone(), m.options.clone(), shared::home_of(m)))
            .collect();
        for index in 0..self.stages.len() {
            let old: Vec<EntityId> = self.stages[index]
                .stage
                .entities()
                .iter()
                .map(|e| e.id())
                .collect();
            for id in old {
                self.stages[index].stage.despawn(id);
            }
            self.stages[index].slots.clear();
            self.stages[index].pets.clear();
            self.stages[index].sprite_quad = None;
            for (form, options, home) in &builds {
                match self.build_pet_actor(form, options) {
                    Ok(actor) => {
                        let id = self.stages[index].stage.spawn_at(actor, *home);
                        self.stages[index].slots.push(id);
                    }
                    Err(e) => log::error!("重载 {} 失败: {e:#}", form.name),
                }
            }
            if self.sprite_mode {
                let sprite = self.sprite.clone();
                self.stages[index].stage.spawn(Actor::Sprite(sprite));
            }
            self.rebuild_pet_surfaces(index);
            self.apply(index, Reaction::BOTH);
        }
    }

    fn toggle_passthrough(&mut self) {
        self.passthrough = !self.passthrough;
        for index in 0..self.stages.len() {
            self.stages[index]
                .stage
                .handle(StageEvent::TogglePassthrough);
            let hwnd = self.stages[index].hwnd;
            // `WS_EX_TRANSPARENT` 管的只是**宠物身上**那几十个格子穿不穿得过去;屏幕
            // 其余部分靠窗口区域(见 `update_window_region`)。别指望这个样式包办全屏 ——
            // 实机上它没把点击放过去。
            // SAFETY: hwnd 有效;改的是自己窗口的扩展样式。
            unsafe {
                SetWindowLongPtrW(
                    hwnd,
                    GWL_EXSTYLE,
                    stage_ex_style(self.passthrough).0 as isize,
                );
                // `SWP_FRAMECHANGED`:样式是拿 `SetWindowLongPtr` 直接改的,得让系统
                // 重算一遍缓存才作数(文档明写了这条)
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }
            // 形状不随穿透变(`shape_regions` 不看开关),但 TogglePassthrough 清了拖动
            // 状态,顺手重设一次
            self.update_window_region(index);
            self.render(index);
        }
        if let Some(tray) = self.tray.as_mut() {
            tray.set_passthrough(self.passthrough);
        }
        log::info!("全局穿透: {}", if self.passthrough { "开" } else { "关" });
    }

    fn toggle_mute(&mut self) {
        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        let muted = !audio.muted();
        audio.set_muted(muted);
        if let Some(tray) = self.tray.as_mut() {
            tray.set_muted(muted);
        }
        log::info!("叫声: {}", if muted { "关" } else { "开" });
    }

    /// 按阵容里现在的形态与选项,把这一只在每台上的角色重建一遍。
    fn rebuild_slot(&mut self, slot: usize) {
        let Some(member) = self.roster.get(slot) else {
            return;
        };
        let form = member.form().clone();
        let options = member.options.clone();
        // 同 `add_pet`:全建出来才提交,免得一半的台换了一半没换
        let mut actors = Vec::with_capacity(self.stages.len());
        for _ in 0..self.stages.len() {
            match self.build_pet_actor(&form, &options) {
                Ok(actor) => actors.push(actor),
                Err(e) => {
                    log::error!("重建 {} 失败: {e:#}", form.name);
                    return;
                }
            }
        }
        for (stage_index, actor) in actors.into_iter().enumerate() {
            let Some(id) = self.stages[stage_index].slots.get(slot).copied() else {
                continue;
            };
            self.stages[stage_index].stage.replace_actor(id, actor);
            // 网格/贴图/画布/掩码缓冲全都跟形态绑,**只重建这一只的**
            self.stages[stage_index].pets.retain(|s| s.id != id);
            self.rebuild_pet_surfaces(stage_index);
            self.apply(stage_index, Reaction::BOTH);
        }
        self.assets.prune();
        self.save_roster();
        self.refresh_tray();
    }

    fn recall(&mut self) {
        for index in 0..self.stages.len() {
            self.stages[index].stage.reset_position();
            // 位置变了,窗口区域也得跟着挪 —— 只 render 的话画面会被停在原处的旧区域裁掉
            self.apply(index, Reaction::BOTH);
        }
        log::info!("宠物已召回");
    }

    /// 把攒在通道里的外部命令处理掉(托盘回调与别的进程都往那里发)。
    fn drain_control(&mut self) {
        while let Ok(control) = self.control_rx.try_recv() {
            self.handle_control(control);
        }
    }

    /// 指针事件。坐标进来是**物理像素**,stage 要的是逻辑像素。
    fn pointer(&mut self, index: usize, event: StageEvent) {
        let reaction = self.stages[index].stage.handle(event);
        self.apply(index, reaction);
        self.flush_sounds();
    }

    fn logical_point(&self, index: usize, x: i32, y: i32) -> (f64, f64) {
        let scale = self.stages[index].scale as f64;
        (x as f64 / scale, y as f64 / scale)
    }
}

/// stage 窗口的扩展样式。
///
/// - `NOREDIRECTIONBITMAP`:不要系统给的重定向位图 —— 这是走 DirectComposition 的前提;
/// - `TOPMOST`:置顶;`TOOLWINDOW`:不进 Alt-Tab 与任务栏;
/// - `NOACTIVATE`:点它不抢焦点(桌宠不该把你正在打字的窗口顶掉);
/// - `TRANSPARENT`:穿透时才加,让系统跳过命中判定。
fn stage_ex_style(passthrough: bool) -> windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE {
    let base = WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    if passthrough {
        base | WS_EX_TRANSPARENT
    } else {
        base
    }
}

fn module_handle() -> Result<windows::Win32::Foundation::HINSTANCE> {
    // SAFETY: 传 None 拿的是本进程的模块句柄。
    let module = unsafe { GetModuleHandleW(None) }.context("拿不到模块句柄")?;
    Ok(module.into())
}

fn register_classes() -> Result<()> {
    let hinstance = module_handle()?;
    // SAFETY: 光标是系统内置资源。
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.context("加载光标失败")?;
    let stage_class = HSTRING::from(STAGE_CLASS);
    let control_class = HSTRING::from(control::CONTROL_CLASS);
    for (name, proc) in [
        (
            &stage_class,
            stage_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
        ),
        (&control_class, control_proc),
    ] {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: hinstance,
            lpszClassName: PCWSTR(name.as_ptr()),
            hCursor: cursor,
            ..Default::default()
        };
        // SAFETY: wc 填好了必需字段;返回 0 表示注册失败。
        let atom = unsafe { RegisterClassW(&wc) };
        anyhow::ensure!(atom != 0, "注册窗口类 {name} 失败");
    }
    Ok(())
}

/// 隐藏的消息窗口:只收托盘回调与别的进程发来的命令,不显示任何东西。
fn create_control_window() -> Result<HWND> {
    let class = HSTRING::from(control::CONTROL_CLASS);
    // SAFETY: 父窗口给 HWND_MESSAGE 就是「消息专用窗口」,永远不显示。
    unsafe {
        CreateWindowExW(
            Default::default(),
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            Default::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(module_handle()?),
            None,
        )
    }
    .context("建控制窗口失败")
}

/// 列出各显示器的**工作区**(已去掉任务栏)。
fn monitors() -> Result<Vec<RECT>> {
    let mut rects: Vec<RECT> = Vec::new();
    // SAFETY: 回调只往 `rects` 里推;lparam 就是它的地址,枚举期间一直有效。
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(monitor_proc),
            LPARAM(&mut rects as *mut Vec<RECT> as isize),
        );
    }
    anyhow::ensure!(!rects.is_empty(), "一个显示器都没枚举到");
    Ok(rects)
}

unsafe extern "system" fn monitor_proc(
    monitor: HMONITOR,
    _dc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> windows::core::BOOL {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: info 填好了 cbSize;lparam 是 `monitors()` 里那个 Vec 的地址。
    unsafe {
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let rects = &mut *(lparam.0 as *mut Vec<RECT>);
            rects.push(info.rcWork);
        }
    }
    true.into()
}

/// 取线程局部的 App。**借不到就返回 None**(重入,见 `APP` 的说明)。
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    let ptr = APP.get();
    if ptr.is_null() {
        return None;
    }
    // SAFETY: 指针指向 `run` 栈上那个 `RefCell<App>`,消息循环期间一直有效;
    // 全部在主线程,`RefCell` 的借用检查足以挡住重入。
    let cell = unsafe { &*ptr };
    let mut app = cell.try_borrow_mut().ok()?;
    Some(f(&mut app))
}

unsafe extern "system" fn stage_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // 鼠标消息的坐标在 lparam 里打包成两个 i16(**必须按有符号取**:窗口左边的负坐标
    // 按无符号读会变成 65000 多)
    let point = || {
        (
            (lparam.0 & 0xFFFF) as u16 as i16 as i32,
            ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32,
        )
    };
    let handled = with_app(|app| {
        let index = app.stage_index(hwnd)?;
        match message {
            WM_NCHITTEST => {
                // 「点击能不能穿到别的程序」由窗口区域决定(见 `update_window_region`),
                // 这里只回答**落在区域里的那些点算不算点在宠物身上** —— 区域是 8px 粒度的
                // 格子,这一步按掩码逐点细化
                let (sx, sy) = point();
                let mut p = POINT { x: sx, y: sy };
                // SAFETY: 只把屏幕坐标换算到窗口客户区。
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut p);
                }
                let (lx, ly) = app.logical_point(index, p.x, p.y);
                let hit = !app.passthrough && app.stages[index].stage.hit_test(lx, ly);
                Some(LRESULT(if hit {
                    HTCLIENT as isize
                } else {
                    HTTRANSPARENT as isize
                }))
            }
            WM_MOUSEMOVE => {
                if !app.stages[index].tracking {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    // SAFETY: 结构填好了 cbSize;失败只是收不到 WM_MOUSELEAVE。
                    let _ = unsafe { TrackMouseEvent(&mut tme) };
                    app.stages[index].tracking = true;
                }
                let (x, y) = point();
                let (lx, ly) = app.logical_point(index, x, y);
                app.pointer(index, StageEvent::PointerMoved { x: lx, y: ly });
                Some(control::HANDLED)
            }
            WM_MOUSELEAVE => {
                app.stages[index].tracking = false;
                app.pointer(index, StageEvent::PointerLeft);
                Some(control::HANDLED)
            }
            WM_LBUTTONDOWN => {
                // 捕获鼠标:不然拖到宠物轮廓之外就收不到移动与松手了
                // SAFETY: hwnd 有效。
                unsafe { SetCapture(hwnd) };
                let (x, y) = point();
                let (lx, ly) = app.logical_point(index, x, y);
                app.pointer(index, StageEvent::PointerPressed { x: lx, y: ly });
                Some(control::HANDLED)
            }
            WM_LBUTTONUP => {
                // SAFETY: 与上面的 SetCapture 配对。
                let _ = unsafe { ReleaseCapture() };
                app.pointer(index, StageEvent::PointerReleased);
                Some(control::HANDLED)
            }
            WM_DPICHANGED | WM_DISPLAYCHANGE => {
                // 缩放变了、或者分辨率/任务栏变了:重新贴合这块屏的工作区。
                // **显示器插拔(多出来/少一块)还没处理** —— 那要增删窗口,留到实机验过再做
                app.relayout(index);
                Some(control::HANDLED)
            }
            _ => None,
        }
    })
    .flatten();
    match handled {
        Some(result) => result,
        // SAFETY: 没处理的消息一律交回系统。
        None => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

unsafe extern "system" fn control_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if control::is_tray_message(message) {
        // 托盘回调:哪个鼠标消息在 lparam 里。
        //
        // **菜单必须在 `with_app` 之外弹**:`TrackPopupMenu` 会一直不返回,自己在里面跑一条
        // 模态消息循环,`WM_TIMER` 照样派发进来。要是这会儿 `App` 还借着,那些 tick 全都
        // `try_borrow_mut` 失败被丢掉 —— 菜单开着的整段时间桌面上的宠物定在那儿不动
        // (Windows 上左右键都会走到这儿,所以两个键点下去都是这个样子)。
        let menu = with_app(|app| {
            let menu = app.tray.as_ref().and_then(|t| t.menu_for(lparam.0 as u32));
            app.drain_control();
            menu
        })
        .flatten();
        if let Some(menu) = menu {
            if let Err(e) = menu.popup() {
                log::warn!("弹托盘菜单失败: {e}");
            }
            // 选中的那一项是 `TrackPopupMenu` 返回之后才送进通道的(`TPM_RETURNCMD`
            // 不发 `WM_COMMAND`),所以这儿要再收一次
            with_app(|app| app.drain_control());
        }
        return control::HANDLED;
    }
    if message == WM_TIMER {
        with_app(|app| {
            app.drain_control();
            if !app.exit {
                app.tick();
            }
        });
        return control::HANDLED;
    }
    if message == control::WM_CONTROL {
        if let Some(command) = control::control_from_message(wparam, lparam) {
            with_app(|app| app.handle_control(command));
        }
        return control::HANDLED;
    }
    // SAFETY: 其余交回系统。
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
