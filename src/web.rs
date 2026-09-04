//! 下载站上的宠物预览:把桌宠那套渲染搬进浏览器。
//!
//! **这里只有胶水**。模型加载、蒙皮、toon 着色、动作降级、眼神图集,全是
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

/// 表情包一帧的边长上下限(像素)。上限 512 是 Telegram 那档;微信表情是 240。
const MIN_STICKER: u32 = 64;
const MAX_STICKER: u32 = 512;
/// 一个循环最多抓这么多帧。**帧率是前端挑的**,这条只是不让它离谱。
///
/// 全库 11513 段运行时动作里最长的是 12.27 秒(放松),400 帧摊在它上面是 32fps,
/// 也就是前端那三档(20/25/50)里只有 50fps 撞得到这条,而撞到了也只是降帧率、
/// 不截断(采样点仍铺满一个周期)。原来这里是 60 —— 那 9% 的长动作会掉到 5~15fps,
/// 实机反馈的「掉帧严重」就是它。
///
/// 一次抓不下这么多没关系:回读缓冲按 [`MAX_STICKER_BYTES`] 分批,
/// 前端按 `first` 逐批要(见 [`Preview::capture`])。
const MAX_STICKER_FRAMES: u32 = 400;
/// 表情包那一路的超采样倍率。和桌宠画布同一条理由(见 `pet::target::SUPERSAMPLE`):
/// 管线一个采样点都没有,不超采样的话 240px 的贴纸边缘全是硬锯齿、描边还是一圈虚线。
/// 先渲 2 倍再由 GPU 缩回去 —— **缩完再回读**,回读量与贴纸尺寸一致而不是它的四倍。
const STICKER_SS: u32 = 2;

/// 把 2 倍的那张缩回贴纸尺寸。一个全屏三角形 + 线性采样:目标像素中心正好落在
/// 2×2 个源纹素正中,双线性 = 精确的盒式降采样(和 `platform::shared::quad_rect` 同一条)。
/// 不开混合,预乘 alpha 原样写出去。
const DOWNSAMPLE_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) idx: u32) -> VsOut {
    // 覆盖整个视口的大三角形
    let uv = vec2<f32>(f32((idx << 1u) & 2u), f32(idx & 2u));
    var out: VsOut;
    out.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(src, samp, in.uv);
}
"#;
/// **一批**回读缓冲的上限。512² 一帧就是 1MB,200 帧要 210MB —— 浏览器里一口气要这么大
/// 不合适,所以分批:这条定的是一批多少字节([`Preview::capture`] 返回这一批抓了几帧)。
const MAX_STICKER_BYTES: u64 = 48 << 20;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

/// 一段可播的动作:逻辑名 + 界面上显示的中文名。
#[wasm_bindgen(getter_with_clone)]
pub struct ClipInfo {
    pub name: String,
    pub label: String,
    /// 这段动作一个循环多少秒。前端按它算表情包要抓多少帧。
    pub seconds: f32,
}

/// 包里的一个形态。
#[wasm_bindgen(getter_with_clone)]
pub struct FormInfo {
    pub asset: String,
    pub name: String,
    /// 这个形态导了异色材质吗。**多数没有** —— 游戏里也是,得美术另做一套。
    pub shiny: bool,
}

/// 一款隐藏/赛季炫彩。`name` 原样回传给 `set_mutation`(写法见 `Mutation::to_config`)。
#[wasm_bindgen(getter_with_clone)]
pub struct HiddenGlassInfo {
    pub name: String,
    /// 赛季款。专属贴图只给游戏指定的那几只,别的宠物走通用外观。
    pub season: bool,
}

/// 常规炫彩的一组配色。`color1`/`color2` 是给人看的两块色(0xRRGGBB),
/// **不是** shader 里那两个 HDR 系数 —— 后者取到 1.6,直接当颜色画会一片过曝。
#[wasm_bindgen(getter_with_clone)]
pub struct GlassyColorInfo {
    pub id: u32,
    pub name: String,
    pub color1: u32,
    pub color2: u32,
}

/// 常规炫彩的一种粒子。
#[wasm_bindgen(getter_with_clone)]
pub struct GlassyParticleInfo {
    pub id: u32,
    pub name: String,
}

/// 隐藏/赛季炫彩那几款,顺序照游戏自己的分法(常驻在前、赛季在后)。
#[wasm_bindgen]
pub fn glassy_hidden() -> Vec<HiddenGlassInfo> {
    let mut out: Vec<HiddenGlassInfo> = crate::pet::glassy::hidden()
        .iter()
        .map(|h| HiddenGlassInfo {
            name: h.name.to_string(),
            season: h.season,
        })
        .collect();
    out.sort_by_key(|h| h.season);
    out
}

/// 常规炫彩的 39 组配色。
#[wasm_bindgen]
pub fn glassy_colors() -> Vec<GlassyColorInfo> {
    crate::pet::glassy::colors()
        .iter()
        .map(|c| GlassyColorInfo {
            id: c.id,
            name: c.name.to_string(),
            color1: c.ui_color_1,
            color2: c.ui_color_2,
        })
        .collect()
}

/// 常规炫彩的 4 种粒子。
#[wasm_bindgen]
pub fn glassy_particles() -> Vec<GlassyParticleInfo> {
    crate::pet::glassy::particles()
        .iter()
        .map(|p| GlassyParticleInfo {
            id: p.id,
            name: p.name.to_string(),
        })
        .collect()
}

/// 画成 `mutation` 这样还缺哪几张**共享贴图**(不带目录与扩展名)。
///
/// 网页版不把这 13 张烘进 wasm(3.6MB,而挑一次常规炫彩只用得上两张),
/// 由前端照这份名单去取、再喂 [`Preview::put_glassy`]。见 `glassy::shared`。
#[wasm_bindgen]
pub fn glassy_missing(mutation: &str) -> Result<Vec<String>, JsValue> {
    Ok(parse_mutation(mutation)?
        .missing_assets()
        .into_iter()
        .map(str::to_string)
        .collect())
}

/// 装一个形态的模型。**两个轴在这里分头落地**,和桌面版 `Assets::model` 同一条路:
/// 异色换的是整套材质(包里已经是换好的那一份,挑一张表就够),炫彩往挑中的那套上刷一层。
fn build_model(form: &crate::pack::Form, mutation: crate::pet::Mutation) -> Result<Arc<Model>, JsValue> {
    let mut model = Model::load(&form.model, form.materials_for(mutation.shiny), &form.clips)
        .map_err(|e| JsValue::from_str(&format!("{e:#}")))?;
    model.apply_mutation(mutation, form.id);
    Ok(Arc::new(model))
}

/// 认不得就报错,别默默按原样画 —— 那样「选了没反应」查不出是哪一步错了。
fn parse_mutation(text: &str) -> Result<crate::pet::Mutation, JsValue> {
    if text.is_empty() {
        return Ok(crate::pet::Mutation::default());
    }
    crate::pet::Mutation::from_config(text)
        .ok_or_else(|| JsValue::from_str(&format!("认不得的外观「{text}」")))
}

/// 眼神(脸那张图集里的一格)。`name` 就是界面上那个中文名,回头原样传给 `set_face`。
///
/// **不是**「表情」那一套 —— 那是 Happy/Sad 那几段动作,见 `stage::EMOTES`。
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
    /// 当前这个形态的资产名。换外观要照它重建,所以得记着。
    asset: String,
    /// 当前这只穿的外观(异色 / 炫彩)。写法同 `roster.toml` 的 `mutation`。
    mutation: crate::pet::Mutation,
    /// 喂给着色器的「秒」:火焰流动、星点闪烁靠它推进。
    time: f32,
    /// 清屏色。见 `attach` 里那段:网页画布只能是不透明的。
    background: wgpu::Color,
    /// 正在飞的那次表情包抓帧。见 [`Preview::capture`]。
    capture: Option<Capture>,
}

/// 一次抓帧:命令已经提交、缓冲正在映射。**回读是异步的**,和桌面版那份
/// `pet::mask::MaskReadback` 同一套路数 —— 前端提交后隔帧来问 `capture_take`。
struct Capture {
    buffer: wgpu::Buffer,
    /// 每帧的边长(像素)与帧数。
    size: u32,
    frames: u32,
    /// 缓冲里每行多少字节(256 对齐后的)。
    padded_row: u32,
    /// 纹理格式是 BGRA 还是 RGBA —— 浏览器给的表面格式两种都见过,
    /// 回读出来要按它决定换不换 R/B。
    bgra: bool,
    /// 映射完成了没有:`None` = 还在飞,`Some(ok)` = 好了(或者失败了)。
    done: std::rc::Rc<std::cell::Cell<Option<bool>>>,
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
            asset: String::new(),
            mutation: crate::pet::Mutation::default(),
            time: 0.0,
            // 中性灰:前端还没告诉我们主题色之前先用它,总比纯黑洞好
            background: wgpu::Color {
                r: 0.12,
                g: 0.12,
                b: 0.14,
                a: 1.0,
            },
            capture: None,
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
        self.asset = String::new();
        self.mutation = crate::pet::Mutation::default();
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
                shiny: f.has_shiny(),
            })
            .collect();
        self.pack = Some(pack);
        Ok(forms)
    }

    /// 装一个形态,**默认站着待机**。返回它做得了的动作(界面据此出按钮)。
    ///
    /// 「做得了」用的是桌面版那张降级表:没有 `Shock` 而有 `Alert` 的形态,
    /// 点「震惊」照样有反应 —— 两边同一套判断,不会出现「网页上能点、装上却没有」。
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

        let model = build_model(form, self.mutation)?;
        let pet = PetGpu::new(&gpu.device, &gpu.queue, &model, gpu.config.format)
            .map_err(|e| JsValue::from_str(&format!("{e:#}")))?;
        gpu.depth_bind = Some(pet.bind_scene_depth(&gpu.device, &gpu.depth));

        let clips = RUNTIME_CLIPS
            .iter()
            .filter_map(|(name, label)| {
                let index = find_clip(&model, name)?;
                Some(ClipInfo {
                    name: (*name).to_string(),
                    label: (*label).to_string(),
                    seconds: model.clips[index].duration,
                })
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
        self.asset = asset.to_string();
        self.face = crate::persona::DEFAULT_FACE;
        // 换形态就把平移归零:偏移是按上一只的取景半径算的,新的一只可能小得多,
        // 不清的话切过去第一眼人就在画面外(缩放留着,那是「想看多近」,跟哪只无关)
        self.target = Vec3::ZERO;
        Ok(clips)
    }

    /// 喂一张炫彩共享贴图。名字不带目录与扩展名(`Tex_PetGlassy_007_D`),
    /// 该喂哪几张问 [`glassy_missing`]。**要在 `set_mutation` 之前喂**。
    pub fn put_glassy(&mut self, name: &str, bytes: &[u8]) {
        crate::pet::glassy::put_shared(name, bytes.to_vec());
    }

    /// 换外观。写法同 `roster.toml` 的 `mutation`:`异色` / `炫彩:3/33` /
    /// `炫彩:黑白`,两个轴可以用 `+` 同时带;空串 = 按包里原样画。
    ///
    /// **视角、缩放、正在播的那段动作都留着** —— 换外观是「这只穿另一身」,
    /// 不是换了一只:镜头跳回去、动作从头再来,恰恰看不成前后对比。
    ///
    /// 素材不齐时**不静默退回原样**:那样人点了没反应,查不出是缺素材还是没做。
    pub fn set_mutation(&mut self, text: &str) -> Result<(), JsValue> {
        let mutation = parse_mutation(text)?;
        if mutation == self.mutation {
            return Ok(());
        }
        let missing = mutation.missing_assets();
        if !missing.is_empty() {
            return Err(JsValue::from_str(&format!(
                "还差炫彩素材:{} —— 先 put_glassy 喂进来",
                missing.join("、")
            )));
        }
        let previous = std::mem::replace(&mut self.mutation, mutation);
        if let Err(e) = self.rebuild() {
            // 建不起来就退回上一身,别把预览停在一只画不出来的宠物上
            self.mutation = previous;
            let _ = self.rebuild();
            return Err(e);
        }
        Ok(())
    }

    /// 按当前的形态与外观重建这只。**接着上一身的动作与时刻播** ——
    /// 网格与骨架没变,变的只有材质,所以段号是通用的。
    fn rebuild(&mut self) -> Result<(), JsValue> {
        let (Some(gpu), Some(pack)) = (self.gpu.as_mut(), self.pack.as_ref()) else {
            return Ok(()); // 还没装宠物,记下来就行,下一次 load_form 自然带上
        };
        let Some(pet) = self.pet.as_ref() else {
            return Ok(());
        };
        let index = pack
            .form_index(Some(&self.asset))
            .map_err(|e| JsValue::from_str(&format!("{e:#}")))?;
        let (clip, at) = (pet.player.current(), pet.player.time());

        let model = build_model(&pack.forms[index], self.mutation)?;
        let built = PetGpu::new(&gpu.device, &gpu.queue, &model, gpu.config.format)
            .map_err(|e| JsValue::from_str(&format!("{e:#}")))?;
        gpu.depth_bind = Some(built.bind_scene_depth(&gpu.device, &gpu.depth));
        let mut player = Player::new(&model, clip.min(model.clips.len().saturating_sub(1)));
        player.seek(at);
        self.pet = Some(Pet {
            model,
            gpu: built,
            player,
        });
        Ok(())
    }

    /// 播一段动作。**眼神跟着换** —— 和桌宠一样,正在播的那段说了算
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

    /// 人手动挑的眼神。传 [`expressions`] 里的名字(**不带「眼」字**:桌面版那句
    /// 「急躁『生气眼』」的后缀是那一行自己加的);认不出来就当默认那张。
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

        let faces = faces_of(pet, self.face);
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
                face_uv: faces.map(|f| f.uv_offset()),
                face_card: faces[0].card(),
                morph_weights: pet.player.morph_weights,
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
        draw_pet(
            &mut encoder,
            &pet.gpu,
            &view,
            &gpu.depth,
            depth_bind,
            self.background,
        );
        gpu.queue.submit(Some(encoder.finish()));
        gpu.queue.present(frame);
    }

    /// 抓一整个循环的帧,给「存成表情包」用。用的是**当前的一切**:形态、眼神、
    /// 异色/炫彩、朝向与缩放、正在播的那段动作。
    ///
    /// **不是把画布录下来**,三条理由:
    ///
    /// 1. 画布只能不透明(见 `attach`),录下来抠不出干净的透明背景;
    /// 2. rAF 的采样点和动作时长没关系,录出来首尾接不上;
    /// 3. 画布是长方的,而表情包要方的。
    ///
    /// 所以另开一张方的 RGBA 离屏纹理,时间轴按 `t = i × 时长 / total` 均匀取
    /// (首尾不重复 ⇒ 循环接得上),清成全透明 ⇒ 拿到的是**带真 alpha** 的帧。
    ///
    /// **取的正是预览里那个方框**(前端画的虚线框):画布的世界高度是 `2 × 取景半径`,
    /// 而贴纸是方的 ⇒ 取「画布正中、边长 = 画布短边」的那块。横屏时就是画布高度那么大的
    /// 中央方块;竖屏(手机上弹窗会变窄)时按宽度来,不然会把画布外的东西也抓进去。
    ///
    /// **着色器那个 `time` 是冻住的**(取当前值)。火焰流动、星点闪烁的周期和动作时长
    /// 没有公倍数,跟着推进的话每绕一圈就跳一下 —— 表情包是要无限循环的,宁可让那几层
    /// 停在一个好看的相位上。
    ///
    /// 回读是异步的,和桌面版 `pet::mask::MaskReadback` 同一套路数:这里只提交,
    /// 前端隔帧问 [`Preview::capture_take`]。同时只许一次在飞。
    ///
    /// `transparent = false` 时清成 `(r, g, b)` 那个不透明纯色。
    ///
    /// ## 分批
    ///
    /// 一整个循环最多 [`MAX_STICKER_FRAMES`] 帧,而 512² 一帧就是 1MB —— 全塞一个回读缓冲
    /// 要 210MB。所以**按 [`MAX_STICKER_BYTES`] 分批**:调用方给这一批从哪一帧 `first` 起,
    /// 拿回**这一批抓了几帧**(0 = 没抓成:没有模型、上一次还在飞、或者 `first` 已经到头)。
    /// `total` 只影响时间轴怎么切,所以分不分批、怎么分,出来的帧都是同一批。
    // 参数是多,但这是给 JS 的接口:包成结构体那边就得先造个对象再传,
    // 而 `wasm_bindgen` 对结构体入参要额外的胶水。八个标量直接传更省事。
    #[allow(clippy::too_many_arguments)]
    pub fn capture(
        &mut self,
        size: u32,
        total: u32,
        first: u32,
        transparent: bool,
        r: f32,
        g: f32,
        b: f32,
    ) -> u32 {
        if self.capture.is_some() {
            return 0;
        }
        let (Some(gpu), Some(pet)) = (self.gpu.as_mut(), self.pet.as_mut()) else {
            return 0;
        };
        let size = size.clamp(MIN_STICKER, MAX_STICKER);
        let total = total.clamp(1, MAX_STICKER_FRAMES);
        let Some(left) = total.checked_sub(first).filter(|n| *n > 0) else {
            return 0;
        };
        let padded_row = (size * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let per_frame = padded_row as u64 * size as u64;
        let budget = (MAX_STICKER_BYTES / per_frame.max(1)).max(1) as u32;
        let frames = left.min(budget);
        let bytes = per_frame * frames as u64;

        // 渲 2 倍那张,再由 GPU 缩回贴纸尺寸(见 `STICKER_SS`)
        let hi = size * STICKER_SS;
        let extent = wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        };
        // 管线是按表面格式建的(`load_form` 里那句 `PetGpu::new`),这两张也得跟它一样,
        // 否则整套管线都不认。浏览器给的常是 BGRA,回读之后再换 R/B。
        let format = gpu.config.format;
        let hi_color = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sticker-hi"),
            size: wgpu::Extent3d {
                width: hi,
                height: hi,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let hi_view = hi_color.create_view(&wgpu::TextureViewDescriptor::default());
        let color = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sticker-color"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = make_depth(&gpu.device, hi, hi);
        let depth_bind = pet.gpu.bind_scene_depth(&gpu.device, &depth_view);
        let (shrink, shrink_bind) = downsampler(&gpu.device, format, &hi_view);
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sticker-readback"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let clear = if transparent {
            wgpu::Color::TRANSPARENT
        } else {
            wgpu::Color {
                r: r as f64,
                g: g as f64,
                b: b as f64,
                a: 1.0,
            }
        };
        let duration = pet.model.clips[pet.player.current()].duration.max(1e-4);
        let resume = pet.player.time();
        // 「画布正中、边长 = 画布短边」那块。画布的世界**高度**是 `2 × 取景半径`
        // (`orbit_view` 只按 aspect 放宽横向),而取景半径正比于 padding ——
        // 所以把 padding 乘上「短边 ÷ 高」就正好是那个方框。前端画的虚线框同此。
        let (cw, ch) = (gpu.config.width.max(1), gpu.config.height.max(1));
        let fit = cw.min(ch) as f32 / ch as f32;
        let view_proj = orbit_view(
            pet.model.motion_bounds,
            self.yaw,
            self.pitch,
            PADDING / self.zoom * fit,
            // 方的:表情包是方的,而画布不是
            1.0,
            self.target,
        );
        for i in 0..frames {
            pet.player
                .seek(duration * (first + i) as f32 / total as f32);
            pet.player.update(&pet.model);
            let faces = faces_of(pet, self.face);
            pet.gpu.update(
                &gpu.queue,
                &FrameParams {
                    view_proj,
                    light_dir: Vec3::new(-0.4, 0.8, 0.6),
                    outline_scale: 1.0,
                    time: self.time,
                    high_material_quality: false,
                    face_uv: faces.map(|f| f.uv_offset()),
                    face_card: faces[0].card(),
                    morph_weights: pet.player.morph_weights,
                },
                &pet.player.matrices,
            );
            // **一帧一次 submit**:`update` 是 `queue.write_buffer`,它相对 submit 有序 ——
            // 攒着一次提交的话每一遍读到的都是最后那次写进去的姿势。
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("sticker"),
                });
            draw_pet(
                &mut encoder,
                &pet.gpu,
                &hi_view,
                &depth_view,
                &depth_bind,
                clear,
            );
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("sticker-shrink"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            // 整张都会被那个全屏三角形盖住,清不清都一样
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&shrink);
                pass.set_bind_group(0, &shrink_bind, &[]);
                pass.draw(0..3, 0..1);
            }
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &color,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: padded_row as u64 * size as u64 * i as u64,
                        bytes_per_row: Some(padded_row),
                        rows_per_image: Some(size),
                    },
                },
                extent,
            );
            gpu.queue.submit(Some(encoder.finish()));
        }
        // 抓完把播放游标放回去,预览那边不该因为存了张图就跳一下
        pet.player.seek(resume);
        pet.player.update(&pet.model);

        let done = std::rc::Rc::new(std::cell::Cell::new(None));
        let flag = done.clone();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |res| flag.set(Some(res.is_ok())));
        self.capture = Some(Capture {
            buffer,
            size,
            frames,
            padded_row,
            bgra: matches!(
                gpu.config.format,
                wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
            ),
            done,
        });
        frames
    }

    /// 取走 [`Preview::capture`] 的结果。**非阻塞**:还没好就返回 `undefined`,
    /// 前端隔帧再问。好了就给一整块紧排的 RGBA(`frames × size × size × 4` 字节,
    /// **预乘 alpha** —— 和管线里那条约定一致,前端合成/去预乘见 `sticker.ts`)。
    pub fn capture_take(&mut self) -> Result<Option<Vec<u8>>, JsValue> {
        let Some(cap) = self.capture.as_ref() else {
            return Ok(None);
        };
        let Some(ok) = cap.done.get() else {
            return Ok(None);
        };
        let cap = self.capture.take().expect("上一句刚看过");
        if !ok {
            return Err(JsValue::from_str("抓帧回读失败"));
        }
        let row = (cap.size * 4) as usize;
        let mut out = vec![0u8; row * cap.size as usize * cap.frames as usize];
        {
            let view = cap
                .buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|e| JsValue::from_str(&format!("抓帧映射失败: {e}")))?;
            for i in 0..(cap.frames * cap.size) as usize {
                let src = i * cap.padded_row as usize;
                out[i * row..(i + 1) * row].copy_from_slice(&view[src..src + row]);
            }
        }
        cap.buffer.unmap();
        if cap.bgra {
            for px in out.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
        Ok(Some(out))
    }
}

/// 正在播的那段动作说了算,它没意见才用人选的那张脸 —— 与 `PetActor::faces` 同一条规矩。
fn faces_of(
    pet: &Pet,
    fallback: Expression,
) -> [crate::persona::Expression; crate::pack::MAX_FACE_SLOTS] {
    let clip = &pet.model.clips[pet.player.current()];
    if clip.faces.iter().all(|t| t.is_empty()) {
        let face = crate::persona::face_for_clip(&clip.name).unwrap_or(fallback);
        return [face; crate::pack::MAX_FACE_SLOTS];
    }
    let time = pet.player.time();
    std::array::from_fn(|slot| {
        crate::pack::face_at(&clip.faces[slot], time)
            .and_then(crate::persona::Expression::from_card)
            .unwrap_or(fallback)
    })
}

/// 两遍:先画写深度的,再拿那份场景深度画半透明外壳(与桌面/离屏同一套)。
///
/// `clear` 的 alpha 是有意义的:画布那条路只能不透明(见 `attach`),
/// 而表情包那条路清成全透明,拿到的就是**带真 alpha** 的一帧。
fn draw_pet(
    encoder: &mut wgpu::CommandEncoder,
    pet: &PetGpu,
    color: &wgpu::TextureView,
    depth: &wgpu::TextureView,
    depth_bind: &wgpu::BindGroup,
    clear: wgpu::Color,
) {
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("preview-opaque"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
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
        pet.draw_opaque(&mut pass, true);
    }
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("preview-translucent"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: color,
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
            view: depth,
            depth_ops: None,
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pet.draw_translucent(&mut pass, depth_bind);
}

/// 「把 `src` 缩到目标附件那么大」的管线与绑定。见 [`DOWNSAMPLE_WGSL`]。
fn downsampler(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    src: &wgpu::TextureView,
) -> (wgpu::RenderPipeline, wgpu::BindGroup) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sticker-shrink"),
        source: wgpu::ShaderSource::Wgsl(DOWNSAMPLE_WGSL.into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sticker-shrink"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("sticker-shrink"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sticker-shrink"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(src),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sticker-shrink"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sticker-shrink"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            // **不开混合**:预乘 alpha 原样写出去
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    (pipeline, bind)
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
