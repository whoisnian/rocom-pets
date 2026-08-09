//! 下载站上的宠物预览:把桌宠那套渲染搬进浏览器。
//!
//! **这里只有胶水**。模型加载、蒙皮、toon 着色、动作降级、表情图集,全是
//! `pet` / `pack` / `stage` / `persona` 里桌面版正在跑的那份代码 —— 网页和桌面
//! 看到的是同一只宠物,不是照着做的第二套。差别只有三处:
//!
//! 1. **没有文件系统**:资产由 JS 逐个喂进来(`put`),存进 `assets::memory`;
//! 2. **不能阻塞**:`request_adapter`/`request_device` 在浏览器里是异步的,
//!    桌面那边包在 `pollster::block_on` 里的两句在这儿得 `await`;
//! 3. **相机能拖**:桌宠只绕 Y 转、画布恒为正方,预览要俯仰也要宽高比,走
//!    [`crate::pet::orbit_view`]。
//!
//! 只支持 WebGPU。骨骼矩阵是只读 storage buffer,WebGL2 没有这东西 ——
//! 检测不到 `navigator.gpu` 时前端不该加载这个模块(见 web/src/lib/preview.ts)。

use std::sync::Arc;

use glam::Vec3;
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

use crate::pack::Pack;
use crate::persona::{EXPRESSIONS, Expression};
use crate::pet::{
    FrameParams, Model, PetGpu, Player, framing_radius, gpu::DEPTH_FORMAT, orbit_rotation,
    orbit_view,
};
use crate::stage::{RUNTIME_CLIPS, find_clip};

/// 包在内存里的假根。虚拟路径由它拼出来,`Pack::load` 那条链一个字不用改。
const ROOT: &str = "/rkpet";

/// 取景余量。与离屏渲染一致(包围盒已含各动作的伸展)。
const PADDING: f32 = 1.15;

/// 拖过**一个画布高**转多少弧度 —— 一整圈。
///
/// 两个方向共用这一个尺度,而且都按高度算。以前横向除宽、纵向除高,同样的像素位移
/// 竖直方向转得快一倍(那块画布是 724×352),斜着拖时画面不跟手。`OrbitControls`
/// 两轴都除 `clientHeight`,就是为了避开这个。
const DRAG_TURN: f32 = std::f32::consts::TAU;

/// 平移能把轨道中心推出多远,单位是取景半径。超过一个半径宠物就出画了,留一点余量到 1.5,
/// 再多就只剩「找不回来」——「复位」虽然能救,但让人先迷路再按按钮不算好设计。
const PAN_LIMIT: f32 = 1.5;

/// 缩放范围(相对默认取景)。下限退到还看得出这是只什么,上限顶到脸上 ——
/// 再放大也没有更多细节,模型本身就那么些三角形。
const ZOOM_MIN: f32 = 0.5;
const ZOOM_MAX: f32 = 5.0;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

/// 一段可播的动作:逻辑名 + 界面上显示的中文名。
#[wasm_bindgen(getter_with_clone)]
pub struct ClipInfo {
    pub name: String,
    pub label: String,
}

/// 包里的一个形态。
#[wasm_bindgen(getter_with_clone)]
pub struct FormInfo {
    pub asset: String,
    pub name: String,
}

/// 表情。`name` 就是界面上那个中文名,回头原样传给 `set_face`。
#[wasm_bindgen]
pub fn expressions() -> Vec<String> {
    EXPRESSIONS.iter().map(|e| e.name.to_string()).collect()
}

struct Gpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::TextureView,
    depth_bind: Option<wgpu::BindGroup>,
}

/// 当前这只:模型 + 它的 GPU 资源 + 播放游标。
struct Pet {
    model: Arc<Model>,
    gpu: PetGpu,
    player: Player,
}

/// 一块画布上的预览。JS 侧 `new Preview()` 拿到它,之后所有操作都走它。
#[wasm_bindgen]
pub struct Preview {
    gpu: Option<Gpu>,
    pack: Option<Pack>,
    pet: Option<Pet>,
    yaw: f32,
    pitch: f32,
    /// 取景倍率。1 = `PADDING` 那档默认余量,越大越近。
    zoom: f32,
    /// 轨道中心的偏移,**世界坐标**。见 [`orbit_view`] 里那段:存世界坐标,平移完再转视角时
    /// 宠物待在原地,而不是跟着镜头甩。
    target: Vec3,
    face: Expression,
    /// 喂给着色器的「秒」:火焰流动、星点闪烁靠它推进。
    time: f32,
    /// 清屏色。见 `attach` 里那段:网页画布只能是不透明的。
    background: wgpu::Color,
}

impl Default for Preview {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl Preview {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            gpu: None,
            pack: None,
            pet: None,
            yaw: 0.0,
            pitch: 0.0,
            zoom: 1.0,
            target: Vec3::ZERO,
            face: crate::persona::DEFAULT_FACE,
            time: 0.0,
            // 中性灰:前端还没告诉我们主题色之前先用它,总比纯黑洞好
            background: wgpu::Color {
                r: 0.12,
                g: 0.12,
                b: 0.14,
                a: 1.0,
            },
        }
    }

    /// 喂一份包内文件。`path` 就是 `.rkpet` 里的条目名(`manifest.toml`、
    /// `forms/<资产>/model.glb`、`forms/<资产>/tex/*.png`)。
    ///
    /// **换包之前先 `reset`**:不清的话上一只的贴图会一直占着内存。
    pub fn put(&mut self, path: &str, bytes: &[u8]) {
        crate::assets::memory::insert(std::path::Path::new(ROOT).join(path), bytes.to_vec());
    }

    /// 清掉喂进来的资产与当前这只。GPU 留着(建一次就够)。
    pub fn reset(&mut self) {
        crate::assets::memory::clear();
        self.pack = None;
        self.pet = None;
        if let Some(gpu) = &mut self.gpu {
            gpu.depth_bind = None;
        }
    }

    /// 接管这块 canvas 并起 GPU。**失败就是这台机器没有 WebGPU**,
    /// 前端据此退回静态头像。
    pub async fn attach(&mut self, canvas: HtmlCanvasElement) -> Result<(), JsValue> {
        let width = canvas.width().max(1);
        let height = canvas.height().max(1);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .map_err(|e| JsValue::from_str(&format!("拿不到画布表面: {e}")))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|e| JsValue::from_str(&format!("没有可用的 GPU 适配器: {e}")))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("rocom-pets-preview"),
                ..Default::default()
            })
            .await
            .map_err(|e| JsValue::from_str(&format!("创建 GPU 设备失败: {e}")))?;

        let caps = surface.get_capabilities(&adapter);
        // 与桌面版同一条规矩:纹理字节已是最终颜色,过一道 sRGB 编码只会偏色
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        // **画布一定是不透明的**:wgpu 的 WebGPU 后端只报 `[Opaque]`(实测 Chromium 151,
        // 尽管 WebGPU 规范里有 `premultiplied`)。所以背景色得自己清 —— 由前端把弹窗那块
        // 底色传进来(`set_background`),深浅色主题下都能和卡片融在一起。
        let alpha_mode = caps.alpha_modes[0];
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            view_formats: vec![],
            alpha_mode,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            desired_maximum_frame_latency: 2,
            // 浏览器只给这一种:出帧节奏由 requestAnimationFrame 定
            present_mode: wgpu::PresentMode::Fifo,
        };
        surface.configure(&device, &config);
        let depth = make_depth(&device, width, height);
        self.gpu = Some(Gpu {
            surface,
            device,
            queue,
            config,
            depth,
            depth_bind: None,
        });
        Ok(())
    }

    /// 读 manifest,返回包里的形态清单(链首排在最前,和桌面版一个顺序)。
    pub fn load_pack(&mut self) -> Result<Vec<FormInfo>, JsValue> {
        let pack = Pack::load(std::path::Path::new(ROOT))
            .map_err(|e| JsValue::from_str(&format!("{e:#}")))?;
        let forms = pack
            .forms
            .iter()
            .map(|f| FormInfo {
                asset: f.asset.clone(),
                name: f.name.clone(),
            })
            .collect();
        self.pack = Some(pack);
        Ok(forms)
    }

    /// 装一个形态,**默认站着待机**。返回它做得了的动作(界面据此出按钮)。
    ///
    /// 「做得了」用的是桌面版那张降级表:没有 `Shock` 而有 `Alert` 的形态,
    /// 点「受惊」照样有反应 —— 两边同一套判断,不会出现「网页上能点、装上却没有」。
    pub fn load_form(&mut self, asset: &str) -> Result<Vec<ClipInfo>, JsValue> {
        let gpu = self
            .gpu
            .as_mut()
            .ok_or_else(|| JsValue::from_str("还没接上画布"))?;
        let pack = self
            .pack
            .as_ref()
            .ok_or_else(|| JsValue::from_str("还没读 manifest"))?;
        let index = pack
            .form_index(Some(asset))
            .map_err(|e| JsValue::from_str(&format!("{e:#}")))?;
        let form = &pack.forms[index];

        let model = Arc::new(
            Model::load(&form.model, &form.materials)
                .map_err(|e| JsValue::from_str(&format!("{e:#}")))?,
        );
        let pet = PetGpu::new(&gpu.device, &gpu.queue, &model, gpu.config.format)
            .map_err(|e| JsValue::from_str(&format!("{e:#}")))?;
        gpu.depth_bind = Some(pet.bind_scene_depth(&gpu.device, &gpu.depth));

        let clips = RUNTIME_CLIPS
            .iter()
            .filter(|(name, _)| find_clip(&model, name).is_some())
            .map(|(name, label)| ClipInfo {
                name: (*name).to_string(),
                label: (*label).to_string(),
            })
            .collect();

        // 默认待机:和桌宠上台时一样。缺 Idle 的形态就退到第 0 段,总得播点什么
        let idle = find_clip(&model, "Idle").unwrap_or(0);
        let player = Player::new(&model, idle);
        self.pet = Some(Pet {
            model,
            gpu: pet,
            player,
        });
        self.face = crate::persona::DEFAULT_FACE;
        // 换形态就把平移归零:偏移是按上一只的取景半径算的,新的一只可能小得多,
        // 不清的话切过去第一眼人就在画面外(缩放留着,那是「想看多近」,跟哪只无关)
        self.target = Vec3::ZERO;
        Ok(clips)
    }

    /// 播一段动作。**表情跟着换** —— 和桌宠一样,正在播的那段说了算
    /// (`persona::face_for_clip`);那段没意见就保持人选的那张脸。
    pub fn play(&mut self, name: &str) -> bool {
        let Some(pet) = &mut self.pet else {
            return false;
        };
        let Some(clip) = find_clip(&pet.model, name) else {
            return false;
        };
        pet.player.play(clip);
        true
    }

    /// 人手动挑的表情。传 [`expressions`] 里的名字;认不出来就当默认那张。
    pub fn set_face(&mut self, name: &str) {
        self.face = EXPRESSIONS
            .iter()
            .find(|e| e.name == name)
            .copied()
            .unwrap_or(crate::persona::DEFAULT_FACE);
    }

    /// 转视角。`dx`/`dy` 是位移**占画布高度的比例**(由 JS 折算,见 web/src/lib/preview.ts)。
    ///
    /// **别在这儿拿 `config.width/height` 去除**:那是设备像素,而指针事件给的是 CSS 像素,
    /// 2 倍屏上除下来只有一半,同一份代码在不同显示器上手感不一样(踩过)。
    pub fn drag(&mut self, dx: f32, dy: f32) {
        if !dx.is_finite() || !dy.is_finite() {
            return;
        }
        self.yaw -= dx * DRAG_TURN;
        self.pitch = (self.pitch - dy * DRAG_TURN)
            .clamp(-crate::pet::gpu::MAX_PITCH, crate::pet::gpu::MAX_PITCH);
    }

    /// 平移轨道中心。单位同 [`drag`](Self::drag):位移占画布高度的比例。
    ///
    /// **正交投影下画面高度正好是 `2 * radius`**,所以「一个画布高」就是 `2 * radius` 的
    /// 世界距离,与相机远近无关 —— 换算成这个比例后物体精确跟手,拉近了也不会突然变快。
    /// 屏幕的右/上方向由当前朝向给出,累加进 `target`;推得太远会找不回来,夹在
    /// [`PAN_LIMIT`] 个半径内。
    pub fn pan(&mut self, dx: f32, dy: f32) {
        if !dx.is_finite() || !dy.is_finite() {
            return;
        }
        let Some(pet) = &self.pet else { return };
        let radius = framing_radius(pet.model.motion_bounds, PADDING / self.zoom);
        let rotation = orbit_rotation(self.yaw, self.pitch);
        // 抓着模型走:往右拖,中心就得往左挪。屏幕 y 向下为正,所以 dy 直接配 +up
        let step = rotation * Vec3::new(-dx, dy, 0.0) * (2.0 * radius);
        self.target = (self.target + step).clamp_length_max(radius * PAN_LIMIT);
    }

    /// 缩放。`factor` 是**乘上去**的:滚轮一格约 1.1,双指捏合传两次触点距离的比值。
    ///
    /// 投影是正交的(见 [`orbit_view`]),所以「拉近」就是把取景余量按比例收紧,
    /// 相机不用动 —— `frame` 里传的是 `PADDING / zoom`。
    pub fn zoom_by(&mut self, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        self.zoom = (self.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
    }

    /// 画布底色(0~1)。网页上的画布不能透明,所以这块底得自己画 ——
    /// 前端把弹窗那块的 CSS 背景色算出来传进来,深浅主题都对得上。
    pub fn set_background(&mut self, r: f32, g: f32, b: f32) {
        self.background = wgpu::Color {
            r: f64::from(r),
            g: f64::from(g),
            b: f64::from(b),
            a: 1.0,
        };
    }

    /// 转回正面,**缩放与平移一并复位** —— 这个按钮是「我弄乱了,回到刚打开的样子」,
    /// 只把角度归零会留下一个放大到看不出转没转、宠物还被推在角落的画面。
    pub fn recenter(&mut self) {
        self.yaw = 0.0;
        self.pitch = 0.0;
        self.zoom = 1.0;
        self.target = Vec3::ZERO;
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let Some(gpu) = &mut self.gpu else { return };
        let (width, height) = (width.max(1), height.max(1));
        if gpu.config.width == width && gpu.config.height == height {
            return;
        }
        gpu.config.width = width;
        gpu.config.height = height;
        gpu.surface.configure(&gpu.device, &gpu.config);
        gpu.depth = make_depth(&gpu.device, width, height);
        // 深度绑定指向刚被换掉的那张纹理,必须跟着重建
        if let Some(pet) = &self.pet {
            gpu.depth_bind = Some(pet.gpu.bind_scene_depth(&gpu.device, &gpu.depth));
        }
    }

    /// 推进 `dt` 秒并画一帧。没有模型时什么都不做(前端照旧调,省一个状态判断)。
    pub fn frame(&mut self, dt: f32) {
        let (Some(gpu), Some(pet)) = (self.gpu.as_mut(), self.pet.as_mut()) else {
            return;
        };
        let Some(depth_bind) = gpu.depth_bind.as_ref() else {
            return;
        };
        let dt = dt.clamp(0.0, 0.1); // 切走再回来时 rAF 会攒出一个巨大的 dt
        self.time += dt;
        pet.player.advance(&pet.model, dt);
        pet.player.update(&pet.model);

        // 正在播的那段动作说了算,它没意见才用人选的那张脸 —— 与 `PetActor::face` 同一条规矩
        let face = crate::persona::face_for_clip(&pet.model.clips[pet.player.current()].name)
            .unwrap_or(self.face);
        let aspect = gpu.config.width as f32 / gpu.config.height.max(1) as f32;
        pet.gpu.update(
            &gpu.queue,
            &FrameParams {
                view_proj: orbit_view(
                    pet.model.motion_bounds,
                    self.yaw,
                    self.pitch,
                    PADDING / self.zoom,
                    aspect,
                    self.target,
                ),
                light_dir: Vec3::new(-0.4, 0.8, 0.6),
                outline_scale: 1.0,
                time: self.time,
                high_material_quality: false,
                face_uv: face.uv_offset(),
                face_card: face.card(),
            },
            &pet.player.matrices,
        );

        use wgpu::CurrentSurfaceTexture as Acquired;
        let frame = match gpu.surface.get_current_texture() {
            Acquired::Success(t) | Acquired::Suboptimal(t) => t,
            // 画布被隐藏、尺寸归零、或者刚 resize 过:这一帧跳过,下一帧再说
            _ => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("preview"),
            });
        // 两遍:先画写深度的,再拿那份场景深度画半透明外壳(与桌面/离屏同一套)
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("preview-opaque"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.background),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &gpu.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pet.gpu.draw_opaque(&mut pass, true);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("preview-translucent"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                // depth_ops = None ⇒ 只读:同一张深度既当附件又被采样,
                // WebGPU 只在只读时允许
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &gpu.depth,
                    depth_ops: None,
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pet.gpu.draw_translucent(&mut pass, depth_bind);
        }
        gpu.queue.submit(Some(encoder.finish()));
        gpu.queue.present(frame);
    }
}

fn make_depth(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("preview-depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            // 半透明那一遍要采它,所以除了当附件还得能绑
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}
