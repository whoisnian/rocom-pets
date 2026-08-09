//! 读宠物包的 manifest.toml。
//!
//! manifest 是导出器与运行时之间唯一的契约(schema 见 docs/design.md §4.3),
//! 运行时只认里面的**逻辑动作名**与形态元数据,不关心资产原名。
//! 缺字段就按默认值降级——包是本地生成物,宁可少个动作也不该整只加载不出来。
//!
//! 包可以是**解开的目录**,也可以是一个 `.rkpet`(zip)。这一层不关心是哪种:
//! 位置一律叫 `path`,内容一律走 [`crate::assets`] 读(见那个模块的「虚拟路径」说明)。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// 运行时能读的 manifest 版本;更高的版本直接拒。
const SUPPORTED_SCHEMA: u32 = 1;

#[derive(Deserialize)]
struct RawManifest {
    schema: u32,
    /// 导出时的 pak 指纹;只用于日志/排查,不参与逻辑。
    #[serde(default)]
    source_version: Option<String>,
    species: RawSpecies,
    #[serde(default)]
    forms: Vec<RawForm>,
}

#[derive(Deserialize)]
struct RawSpecies {
    id: i64,
    name: String,
}

#[derive(Deserialize)]
struct RawForm {
    id: i64,
    name: String,
    #[serde(default)]
    stage: i64,
    asset: String,
    model: String,
    #[serde(default = "one")]
    scale: f32,
    #[serde(default)]
    height_cm: f32,
    #[serde(default)]
    locomotion: String,
    #[serde(default)]
    clips: HashMap<String, RawClip>,
    #[serde(default)]
    materials: HashMap<String, RawMaterial>,
    #[serde(default)]
    voice: Option<RawVoice>,
    /// `[forms.sfx]`:动作音效层,键与 `[forms.voice]`、`[forms.clips]` 同一套。
    #[serde(default)]
    sfx: HashMap<String, RawVoiceClip>,
}

/// `[forms.voice]`:叫声。`cents_low/high` 是游戏里 `voice` 属性拉到 ±100 时的音分
/// (「粗嗓门」「婉转声」),运行时按 `2^(音分/1200)` 调播放速率复刻。
#[derive(Deserialize)]
struct RawVoice {
    #[serde(default)]
    cents_low: f32,
    #[serde(default)]
    cents_high: f32,
    /// 其余键都是「动作逻辑名 → 音频文件」。
    #[serde(flatten)]
    clips: HashMap<String, RawVoiceClip>,
}

#[derive(Deserialize)]
struct RawVoiceClip {
    path: String,
    #[serde(default)]
    ms: u32,
}

/// `[forms.materials]` 一条:导出器从游戏材质实例里解出来的「这个槽该画什么」。
#[derive(Deserialize)]
struct RawMaterial {
    /// 基色贴图的包内相对路径。**缺失 = 纯特效层**(火焰/水壳/光晕:材质里没有
    /// BaseTex/EyeTex,固有色是 shader 算的),运行时整片跳过;
    /// 将来做特效通道时改成按 blend 走半透/加色,见 design.md 横向待办。
    #[serde(default)]
    base_color: Option<String>,
    /// 贴图 alpha 是不是真遮罩。眼/嘴的表情图集是(不剔就是一块方糊),
    /// 本体贴图不是(它的 alpha 是美术塞的遮罩通道,拿来剔会把身体啃掉)。
    #[serde(default)]
    mask_alpha: bool,
    /// 材质的父链(游戏里的材质实例继承)。**眼/嘴那两个槽认它**:
    /// 表情是贴在 `M_P_Eyes` 这一族上的图集(见 `Material::face`)。
    #[serde(default)]
    parents: Vec<String>,
    /// 以下都只对特效层有意义(`base_color` 缺失时)。
    #[serde(default)]
    tint: Option<[f32; 4]>,
    #[serde(default = "one")]
    opacity: f32,
    #[serde(default = "one")]
    glow: f32,
    #[serde(default)]
    flow: Option<[f32; 4]>,
    #[serde(default)]
    mask_tex: Option<String>,
    #[serde(default)]
    noise_tex: Option<String>,
    #[serde(default)]
    mask_matcap: bool,
    /// 以下对所有材质都可能有(有基色的也一样)。
    #[serde(default)]
    translucent: bool,
    /// 同名 `_Ol` 描边材质在不在(导出器逐材质写)。**旧包没有这个字段** ⇒ `None`,
    /// 那时按老规矩来:不透明的画描边、半透的不画。
    #[serde(default)]
    outline: Option<bool>,
    /// 描边宽度(米),由 `_Ol` 材质的 `OutlineWidthPC × MaxWidthScale` 算出。
    /// **旧包没有这个字段** ⇒ `None`,退回全库模态值(见 `MaterialSpec::outline_width`)。
    #[serde(default)]
    outline_width: Option<f32>,
    /// 按画家序画:不写深度,后画的盖住先画的。见 `MaterialSpec::paint_order`。
    #[serde(default)]
    paint_order: bool,
    #[serde(default)]
    star_tex: Option<String>,
    /// 星点层来自「假半透」族(`NoiseTex` + `Color02`),着色走 `star_color` 而不是四段渐变
    #[serde(default)]
    star_fake_trans: bool,
    #[serde(default)]
    star_tiling: Option<[f32; 2]>,
    #[serde(default)]
    star_color: Option<[f32; 3]>,
    #[serde(default = "one")]
    stick_intensity: f32,
    #[serde(default)]
    matcap_tex: Option<String>,
    #[serde(default)]
    matcap_color: Option<[f32; 3]>,
    #[serde(default)]
    rim_color: Option<[f32; 3]>,
    #[serde(default)]
    rim_intensity: f32,
    #[serde(default)]
    emissive: Option<[f32; 3]>,
    #[serde(default)]
    emissive_intensity: f32,
    #[serde(default = "default_rim_power")]
    rim_power: f32,
    #[serde(default = "default_rim_soft_edge")]
    rim_soft_edge: f32,
    /// `M_P_Object_Trans` 的高光/alpha 覆盖参数。旧包缺字段时退回根材质默认值。
    #[serde(default)]
    highlight_offset: Option<[f32; 3]>,
    #[serde(default)]
    highlight_color: Option<[f32; 3]>,
    #[serde(default = "default_highlight_power")]
    highlight_power: f32,
    #[serde(default = "one")]
    highlight_intensity: f32,
    #[serde(default)]
    force_default_opacity: f32,
    /// `M_P_Object_Trans` 场景深度淡化距离(UE 厘米)与开启强度。
    #[serde(default)]
    opacity_depth_distance: f32,
    #[serde(default)]
    open_depth_distance: f32,
    /// 精确父材质 `MI_P_Object_Trans` 在目标 ES3.1/Low 排列中的局部着色链。
    #[serde(default)]
    object_trans_low: bool,
    #[serde(default)]
    light_mask_tex: Option<String>,
    #[serde(default)]
    ramp_tex: Option<String>,
    #[serde(default = "default_object_trans_soft_edge")]
    object_trans_soft_edge: f32,
    #[serde(default)]
    main_color: Option<[f32; 3]>,
    #[serde(default = "one")]
    main_bright: f32,
    #[serde(default)]
    noise_uv: Option<[f32; 4]>,
    /// 基色 alpha 是不透明度(而不是纹路遮罩)——静态开关 `Opacity or OpacityMask` 开着的那批
    #[serde(default)]
    alpha_opacity: bool,
    #[serde(default)]
    flow_tex: Option<String>,
    #[serde(default = "one")]
    flow_power: f32,
    #[serde(default)]
    mask_id_tex: Option<String>,
    #[serde(default)]
    mask_id_range: Option<[f32; 2]>,
    #[serde(default)]
    flicker: Option<[f32; 2]>,
    #[serde(default)]
    interior_tex: Option<String>,
    #[serde(default)]
    interior_color: Option<[f32; 3]>,
    #[serde(default = "one")]
    refraction: f32,
    #[serde(default)]
    refract_depth: f32,
    /// `M_ShuiMu_ByIn` 的独立材质分支。参数来自 shader 71636 对应的根材质默认/实例覆盖。
    #[serde(default)]
    glassy_inner: bool,
    #[serde(default)]
    glassy_flow1: Option<[f32; 4]>,
    #[serde(default)]
    glassy_flow2: Option<[f32; 4]>,
    #[serde(default)]
    glassy_fresnel: Option<[f32; 4]>,
    /// [GlassyNoiseSpeed, UVScale, Refract, Depth]
    #[serde(default)]
    glassy_noise: Option<[f32; 4]>,
    /// [FresnelMaskPow, Offset, Smooth, TriPlannarBlendInt]
    #[serde(default)]
    glassy_mask: Option<[f32; 4]>,
    /// `MI_P_Object_XiaoYou` 的目标 Low 专用分支。
    #[serde(default)]
    xiaoyou: bool,
    #[serde(default)]
    xiaoyou_base1: Option<[f32; 4]>,
    #[serde(default)]
    xiaoyou_base2: Option<[f32; 4]>,
    #[serde(default)]
    xiaoyou_flow1: Option<[f32; 4]>,
    #[serde(default)]
    xiaoyou_flow2: Option<[f32; 4]>,
    #[serde(default)]
    xiaoyou_star_color: Option<[f32; 4]>,
    #[serde(default)]
    xiaoyou_noise_flow: Option<[f32; 4]>,
    #[serde(default)]
    xiaoyou_shape: Option<[f32; 4]>,
    #[serde(default)]
    xiaoyou_star_uv: Option<[f32; 4]>,
    /// `M_Gra_Yutu_Ear_Lighting` 的目标 Low 专用分支。
    #[serde(default)]
    yutu_ear: bool,
    #[serde(default)]
    yutu_bubble_tex: Option<String>,
    #[serde(default)]
    yutu_distort_tex: Option<String>,
    #[serde(default)]
    yutu_flow_tex: Option<String>,
    #[serde(default)]
    yutu_bubble_color: Option<[f32; 4]>,
    #[serde(default)]
    yutu_flow_color: Option<[f32; 4]>,
    #[serde(default)]
    yutu_fresnel_color: Option<[f32; 4]>,
    #[serde(default)]
    yutu_inner_color: Option<[f32; 4]>,
    #[serde(default)]
    yutu_overall_color: Option<[f32; 4]>,
    #[serde(default)]
    yutu_ramp_color: Option<[f32; 4]>,
    #[serde(default)]
    yutu_top_color: Option<[f32; 4]>,
    #[serde(default)]
    yutu_bubble_shape: Option<[f32; 4]>,
    #[serde(default)]
    yutu_flow_shape: Option<[f32; 4]>,
    #[serde(default)]
    yutu_light_shape: Option<[f32; 4]>,
    #[serde(default)]
    yutu_top_shape: Option<[f32; 4]>,
    /// `M_P_FakeFulid`（资产原拼写）的液面/玻璃分支。
    #[serde(default)]
    fake_fluid: bool,
    #[serde(default)]
    fluid_edge_color: Option<[f32; 4]>,
    #[serde(default)]
    fluid_fresnel_color: Option<[f32; 4]>,
    #[serde(default)]
    fluid_plane_color: Option<[f32; 4]>,
    #[serde(default)]
    fluid_gradient1: Option<[f32; 4]>,
    #[serde(default)]
    fluid_gradient2: Option<[f32; 4]>,
    #[serde(default)]
    fluid_height_tiling: Option<[f32; 4]>,
    #[serde(default)]
    fluid_plane_axis: Option<[f32; 4]>,
    #[serde(default)]
    fluid_plane_center: Option<[f32; 4]>,
    #[serde(default)]
    fluid_body_shape: Option<[f32; 4]>,
    #[serde(default)]
    fluid_gradient_shape: Option<[f32; 4]>,
    #[serde(default)]
    fluid_top_shape: Option<[f32; 4]>,
    /// `M_P_MatCap_Masked` 的目标 Low PS 19654 分支。
    #[serde(default)]
    matcap_masked: bool,
    #[serde(default)]
    matcap_masked_base: Option<[f32; 4]>,
    #[serde(default)]
    matcap_masked_light_ramp: Option<[f32; 4]>,
    #[serde(default)]
    matcap_masked_flat: Option<[f32; 4]>,
    #[serde(default)]
    matcap_masked_main: Option<[f32; 4]>,
    #[serde(default)]
    matcap_masked_selection: Option<[f32; 4]>,
    #[serde(default)]
    matcap_masked_rim: Option<[f32; 4]>,
    #[serde(default)]
    matcap_masked_surface: Option<[f32; 4]>,
}

#[derive(Deserialize)]
struct RawClip {
    #[serde(default)]
    ms: u32,
    /// 走跑类动作:动画自带的位移换算出的速度(cm/s);0 表示原地循环。
    #[serde(default)]
    speed_cm_s: f32,
}

fn one() -> f32 {
    1.0
}

/// `M_P_Object_Trans` 根材质的原始默认值。
fn default_rim_power() -> f32 {
    0.4
}

fn default_rim_soft_edge() -> f32 {
    0.3
}

fn default_highlight_power() -> f32 {
    10.0
}

fn default_object_trans_soft_edge() -> f32 {
    0.5
}

// manifest 是契约的一部分:这些字段现在还没人读(形态切换/行为要用),但照着 schema
// 解出来放着,比等到要用时再补解析更省事
#[allow(dead_code)]
#[derive(Clone)]
pub struct Clip {
    pub seconds: f32,
    pub speed_cm_s: f32,
}

/// 一个材质槽该怎么画。由导出器解析游戏材质实例得出,取代原来按贴图命名约定的猜法。
#[derive(Clone)]
pub struct Material {
    /// 基色贴图的绝对路径;None = 纯特效层,走 `effect` 那套画法。
    pub base_color: Option<PathBuf>,
    pub mask_alpha: bool,
    /// 这是脸(眼睛/嘴)吗 —— 父链里有 `M_P_Eyes` 就是。
    ///
    /// **表情就画在这两个槽上**:贴图是 2×4 的表情图集,网格 UV 落在左上那一格,
    /// 换表情 = 给 UV 加一个整格的偏移(见 persona.rs 的 `Expression`)。
    pub face: bool,
    /// 脸的另一种做法:父链里是 `M_P_Eyes_Mesh`(全库 859 片脸网格里有 21 片)。
    ///
    /// 这一族**不偏 UV**:八种表情各是一份独立几何,叠在同一处,UV 各自钉死在图集的一格上,
    /// 顶点色 G 通道写着自己是第几张(`floor(G × 10)` ∈ 1..8)。游戏靠材质参数只画其中一张
    /// (根材质 `M_P_Eyes_Mesh` 上那个默认值为 1 的 `Number`)。
    /// **我们原来把它当普通图集脸画,于是八张一起画** —— 乖乖鹄一家的
    /// 「眉毛、眼睛、腮红搅在一起」就是这么来的。选哪张见 persona.rs 的 `Expression::card`。
    pub face_cards: bool,
    /// 只在 `base_color` 为 None 时有效。
    pub effect: Effect,
    /// 半透。**有基色的材质也可能是半透**:暮星辰的裙子与那两个球都是,
    /// 当不透明画就是死板的实心块。
    pub translucent: bool,
    /// 这个材质画不画描边 —— 游戏是**逐材质**开的(`Mat/` 里有没有配套的 `_Ol` 资产)。
    /// `None` = 旧包没这个字段,退回「不透明画、半透不画」。
    pub outline: Option<bool>,
    /// 沿法线外扩多少**米**。来自 `_Ol` 材质:`0.01 × OutlineWidthPC × MaxWidthScale` 厘米,
    /// 全库 847/854 是 `0.13 × 300` ⇒ 0.0039 米。推导见 `Materials.cs` 的 `OutlineWidthOf`。
    /// `None` = 旧包没这个字段,退回那个模态值(比旧包自己的「对角线 × 0.004」更接近实机)。
    pub outline_width: Option<f32>,
    /// **按画家序画:进不写深度的那一遍,后画的三角盖住先画的。**
    /// 幽火那一族(`M_Gho_XiaoYou_GhostFire`)每团是「外壳套内壳」两层闭合几何,
    /// 索引缓冲里就是「外壳 → 内壳」的顺序;走不透明通道的话外壳会把内壳整个挡住
    /// (实机是两层都看得见,而且背景一点不透 ⇒ 不是 alpha 混合,是不写深度)。
    /// 判据与三条证据见 `Materials.cs` 的 `IsPaintOrder`。
    pub paint_order: bool,
    pub opacity: f32,
    /// 身上那些细碎星光。
    pub star: Option<PathBuf>,
    /// 星点层的 uv 平铺。来自材质的**标量** `StarStickTiling`(汇编里那一乘是单个标量,
    /// u/v 同一个数);这个名字在材质图里同名还有一个向量参数,别读错(见 Materials.cs)。
    pub star_tiling: [f32; 2],
    /// 星点层来自「假半透」族:着色用 `star_color`(= `Color02`),不是四段渐变
    pub star_fake_trans: bool,
    pub star_color: [f32; 3],
    /// 星点层的强度(根材质 `Stick_Intensity` = 1.5)。
    pub stick_intensity: f32,
    /// 球面反射查找表:玻璃/金属高光。
    pub matcap: Option<PathBuf>,
    pub matcap_color: [f32; 3],
    pub rim_color: [f32; 3],
    pub rim_intensity: f32,
    /// 加在光照**之后**的那层色,遮罩是基色 alpha 的重映射(见 pet.wgsl 的 `detail_mask`)。
    ///
    /// 来源是材质的 `Emitter Color × Emitter Intensity`(Low PS 68952 第 62~65 行)。
    /// **水体族也走这一条**:水灵/波波拉的水体层只写了强度、颜色留在根默认(白),
    /// 身上那几道竖向浅色条纹就是「白 × 0.5 × 基色 alpha」。一度改成拿水体预设的
    /// `Color1`(蓝)当颜色,量下来色相与强度都不对(线上提升 (21,11,10),
    /// 白 × 0.5 是 (43,15.6,9.0),实机 (96,23,9)),已撤回。
    pub emissive: [f32; 3],
    pub emissive_intensity: f32,
    /// 边缘光的衰减次数。**小于 1 = 整片泛色**(幽星光的球 0.35),不是一圈细边。
    pub rim_power: f32,
    pub rim_soft_edge: f32,
    /// 高光方向偏移(xyz,已由 UE Z-up 换成 glTF Y-up)、颜色、次数与强度。
    pub highlight_offset: [f32; 3],
    pub highlight_color: [f32; 3],
    pub highlight_power: f32,
    pub highlight_intensity: f32,
    /// `ForceUseDefOpacity`:1 时强制只用基色 alpha,0 时保留高光覆盖。
    pub force_default_opacity: f32,
    /// 场景深度淡化距离(UE 厘米)与开启强度；原材质参数原样保留。
    pub opacity_depth_distance: f32,
    pub open_depth_distance: f32,
    /// 目标实机 Low `MI_P_Object_Trans` 的 BaseTex/MaskTex/RampTex 局部链。
    pub object_trans_low: bool,
    pub light_mask: Option<PathBuf>,
    pub ramp: Option<PathBuf>,
    pub object_trans_soft_edge: f32,
    pub main_color: [f32; 3],
    pub main_bright: f32,
    /// 假半透族星点层:[速度X, 速度Y, 强度, 是否用 UV0]。见 pet.wgsl 的 stick_layer。
    pub noise_uv: [f32; 4],
    /// **基色贴图的 alpha 是不透明度**(不是纹路遮罩)。判据是静态开关 `Opacity or OpacityMask`,
    /// 开着的 11 个材质:蜜蜂/小甲虫的翅膀、果冻、暮星辰的裙子……
    pub alpha_opacity: bool,
    /// 卷动色带:一张渐变图沿 UV 滚过表面,叠在固有色上(暮星辰环带的青↔粉渐变)。
    pub flow: Option<PathBuf>,
    /// [u 速度, v 速度, u 平铺, v 平铺] + 混入强度。
    pub flow_uv: [f32; 4],
    pub flow_power: f32,
    /// 色带的 **ID 遮罩**:只在 `mask_id_tex` 的 alpha 落在 `mask_id_range` 时才卷动。
    /// 实测暮星辰那张 By_M 的 alpha 是离散 ID 台阶,环带是 0.72、额头与身体中央的黄装饰是 0.50,
    /// 阈值 0.6~0.8 正好只选中环带。不门控的话黄装饰会跟着在黄绿之间来回变。
    pub mask_id: Option<PathBuf>,
    pub mask_id_range: [f32; 2],
    /// **玻璃内部那颗星**:四角星场贴图(`StarTex` = `T_EMeng003`),沿折射光线在物体空间
    /// march、三向投影采样、按时间卷动。读 shader 汇编得来,见 docs/design.md §1。
    pub interior: Option<PathBuf>,
    pub interior_color: [f32; 3],
    /// 折射率(材质里的 `GlobalRefraction` = 1.3)。
    pub refraction: f32,
    /// march 深度(`GlobalDepth` = 100)。**量纲是从汇编定出来的**:
    /// `marchDist = |半包围盒| × 0.01 × GlobalDepth` —— 代 100 进去正好等于 `|半包围盒|`。
    /// 以前这里故意不读、在 gpu.rs 里写死 0.4「对着截图挑的」,现在按汇编算。
    pub refract_depth: f32,
    /// 球内那颗星的闪烁:[速度, 次数](`FlickerSpeed`/`FlickerPower`)。
    pub flicker: [f32; 2],
    /// `M_ShuiMu_ByIn` 的原始流动内胆；`None` 表示走普通纯特效/基色路径。
    pub glassy_inner: Option<GlassyInner>,
    /// `MI_P_Object_XiaoYou` 的不透明 MainTex/NoiseTex/StarTex 合成链。
    pub xiaoyou: Option<XiaoYou>,
    /// 莫比乌乌内层的原生不透明液体材质。
    pub yutu_ear: Option<YutuEar>,
    /// 克莱因龙的原生 FakeFulid 玻璃/液面材质。
    pub fake_fluid: Option<FakeFluid>,
    /// `M_P_MatCap_Masked` 的不透明 MatCap 外壳。
    pub matcap_masked: Option<MatcapMasked>,
}

/// `M_ShuiMu_ByIn` 的材质局部链。字段顺序对应 71636 的原始参数；`noise.z` 是
/// `GlassyNoiseRefract`，shader 再按 preshader 原式求 `1 / (1 + noise.z)`。
#[derive(Clone)]
pub struct GlassyInner {
    pub flow1: [f32; 4],
    pub flow2: [f32; 4],
    pub fresnel: [f32; 4],
    pub noise: [f32; 4],
    pub mask: [f32; 4],
}

/// 小灵面家族目标 ES3.1/Low PS 32511 的材质参数。贴图分别沿用 Material 的
/// `base_color` / Effect.noise 对应的第二槽 / `star`，这里不重复存路径。
#[derive(Clone)]
pub struct XiaoYou {
    pub base1: [f32; 4],
    pub base2: [f32; 4],
    pub flow1: [f32; 4],
    pub flow2: [f32; 4],
    pub star_color: [f32; 4],
    pub noise_flow: [f32; 4],
    pub shape: [f32; 4],
    pub star_uv: [f32; 4],
}

#[derive(Clone)]
pub struct YutuEar {
    pub bubble: Option<PathBuf>,
    pub distort: Option<PathBuf>,
    pub flow: Option<PathBuf>,
    pub bubble_color: [f32; 4],
    pub flow_color: [f32; 4],
    pub fresnel_color: [f32; 4],
    pub inner_color: [f32; 4],
    pub overall_color: [f32; 4],
    pub ramp_color: [f32; 4],
    pub top_color: [f32; 4],
    pub bubble_shape: [f32; 4],
    pub flow_shape: [f32; 4],
    pub light_shape: [f32; 4],
    pub top_shape: [f32; 4],
}

#[derive(Clone)]
pub struct FakeFluid {
    pub edge_color: [f32; 4],
    pub fresnel_color: [f32; 4],
    pub plane_color: [f32; 4],
    pub gradient1: [f32; 4],
    pub gradient2: [f32; 4],
    pub height_tiling: [f32; 4],
    pub plane_axis: [f32; 4],
    pub plane_center: [f32; 4],
    pub body_shape: [f32; 4],
    pub gradient_shape: [f32; 4],
    pub top_shape: [f32; 4],
}

#[derive(Clone)]
pub struct MatcapMasked {
    /// MatCapTex；路径复用 Effect.mask，避免 manifest 重复记录同一张贴图。
    pub matcap: Option<PathBuf>,
    pub base_color: [f32; 4],
    pub light_ramp: [f32; 4],
    pub flat_emissive: [f32; 4],
    pub main_color: [f32; 4],
    pub selection_color: [f32; 4],
    /// [Rim Power, Rim Soft Edge, Rim Intensity, FresnelPow]
    pub rim_shape: [f32; 4],
    /// [Flat intensity, Flat ratio, MainBright, max(Xray,Common_Xray)]
    pub surface_shape: [f32; 4],
}

/// 特效层(火焰/水壳/光晕)的画法参数,全部来自游戏材质。
#[derive(Clone)]
pub struct Effect {
    /// 主色,**可能是 HDR**:火花的 `Color01` = (6, 0.8, 0)。任一通道 >1 就当加色发光。
    pub tint: [f32; 4],
    pub opacity: f32,
    pub glow: f32,
    /// [u 速度, v 速度, u 平铺, v 平铺]
    pub flow: [f32; 4],
    pub mask: Option<PathBuf>,
    pub noise: Option<PathBuf>,
    /// 遮罩是 MatCap:要按**视空间法线**采样(球面反射查找表),不是网格 UV。
    pub mask_matcap: bool,
}

impl Effect {
    /// 主色任一通道 >1 说明美术是当**加色发光**用的(黑=加零),此时不该按半透混合。
    pub fn additive(&self) -> bool {
        self.tint[0] > 1.0 || self.tint[1] > 1.0 || self.tint[2] > 1.0
    }
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct Form {
    pub id: i64,
    pub name: String,
    pub stage: i64,
    pub asset: String,
    /// glb 的绝对路径。
    pub model: PathBuf,
    pub scale: f32,
    pub height_cm: f32,
    pub locomotion: String,
    pub clips: HashMap<String, Clip>,
    /// 声音;None = 这个形态没导出(两族 Wwise 库都没有,或者导出时缺 vgmstream/ffmpeg)。
    pub voice: Option<Voice>,
    /// glb 里的材质名 → 该画什么。**载入模型必需**,空的话 `Model::load` 直接报错
    /// (旧版导出的包没有这一节,重导即可)。
    pub materials: HashMap<String, Material>,
}

/// 一个形态的声音。**两层**:嗓子发出来的叫声,和身体动静的音效 ——
/// 游戏里同一段情绪就是这两条叠着放的,分别来自 `Pet_Vo_*` 与 `Pet_Action_*` 两族库。
#[derive(Clone)]
pub struct Voice {
    /// `voice` 属性拉到 ±100 时的音分(粗嗓门 / 婉转声),运行时按
    /// `2^(音分/1200)` 调播放速率 —— Wwise 的 pitch 本来就是重采样。
    /// **只管叫声那层**:动作音效不跟着变调(`Pet_Action_*` 库压根没挂这条曲线)。
    pub cents_low: f32,
    pub cents_high: f32,
    /// 动作逻辑名(与 `clips` 同一把键)→ 叫声文件。
    pub clips: HashMap<String, VoiceClip>,
    /// 同一把键 → 动作音效文件。
    pub sfx: HashMap<String, VoiceClip>,
}

#[derive(Clone)]
pub struct VoiceClip {
    pub path: PathBuf,
    #[allow(dead_code)] // 时长目前只用于排查;播放不需要预先知道长度
    pub seconds: f32,
}

/// manifest 里那一节音频表 → 包内绝对路径。两层各调一次。
fn sound_files(root: &Path, raw: HashMap<String, RawVoiceClip>) -> HashMap<String, VoiceClip> {
    raw.into_iter()
        .map(|(key, clip)| {
            (
                key,
                VoiceClip {
                    path: root.join(clip.path),
                    seconds: clip.ms as f32 / 1000.0,
                },
            )
        })
        .collect()
}

impl Form {
    pub fn clip(&self, logical: &str) -> Option<&Clip> {
        self.clips.get(logical)
    }

    /// 只带一张动作表的形态,给动作覆盖率的单测用。
    #[cfg(test)]
    pub fn for_test(clips: HashMap<String, Clip>) -> Self {
        Self {
            id: 0,
            name: "测试".into(),
            stage: 1,
            asset: "Test_001".into(),
            model: PathBuf::from("<test>"),
            scale: 1.0,
            height_cm: 80.0,
            locomotion: "ground".into(),
            clips,
            voice: None,
            materials: HashMap::new(),
        }
    }
}

/// 包目录里的一项:够列一行表格,但**不含动作表与材质表**(见 [`Pack::list_entries`])。
pub struct PackEntry {
    /// 物种名(链首的名字)。
    pub name: String,
    /// 整条进化链的形态名。列表里直接写成「喵喵 → 喵呜 → 魔力猫」——
    /// 比只写链名好搜:想找魔力猫的人不一定记得它的链首叫喵喵。
    pub forms: Vec<String>,
    /// 包的位置:目录,或者 `.rkpet` 文件。
    pub path: PathBuf,
    /// 占多少字节。
    pub size: u64,
}

impl PackEntry {
    /// 「喵喵 → 喵呜 → 魔力猫」。单形态的包就只有一个名字。
    pub fn chain(&self) -> String {
        if self.forms.is_empty() {
            return self.name.clone();
        }
        self.forms.join(" → ")
    }

    /// 是 `.rkpet` 归档还是解开的目录。列表里要标出来(对应 `--list` 的 `[rkpet]`)。
    pub fn archived(&self) -> bool {
        self.path.is_file()
    }
}

pub struct Pack {
    pub species_id: i64,
    pub species_name: String,
    pub forms: Vec<Form>,
    /// 包的位置(目录或 `.rkpet` 文件)。列表显示与包内相对路径都要用。
    pub path: PathBuf,
}

/// 放大件数据的目录(不含 `rocom-pets` 那一层)。
#[cfg(not(target_os = "windows"))]
fn data_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
}

#[cfg(target_os = "windows")]
fn data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

impl Pack {
    /// 默认包目录:Linux 是 `$XDG_DATA_HOME/rocom-pets/packs`,
    /// Windows 是 `%LOCALAPPDATA%\rocom-pets\packs`(包有几 GB,不该跟着漫游配置走)。
    /// 见 config.rs 的 `config_dir`:Windows 上没有 `HOME`/`XDG_*`。
    pub fn default_dir() -> Option<PathBuf> {
        Some(data_dir()?.join("rocom-pets").join("packs"))
    }

    /// 包目录里所有**看着像包**的位置(目录含 manifest,或 `.rkpet` 文件),按路径排序。
    ///
    /// 以下几个「扫包目录」的方法**只有桌面版有** —— 浏览器里的包是 JS 喂进来的字节,
    /// 没有目录可扫(见 assets.rs 的 `memory`)。
    #[cfg(not(target_arch = "wasm32"))]
    fn candidates(dir: &Path) -> Vec<PathBuf> {
        let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
            Ok(read) => read
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| crate::assets::is_pack(p))
                .collect(),
            Err(e) => {
                log::debug!("包目录 {dir:?} 读不了: {e}");
                return Vec::new();
            }
        };
        entries.sort();
        entries
    }

    /// 列出包目录下所有能读的包(按名字排序)。读不动的只警告,不让一个坏包挡住其他的。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn list(dir: &Path) -> Vec<Pack> {
        Self::candidates(dir)
            .iter()
            .filter_map(|path| match Pack::load(path) {
                Ok(pack) => Some(pack),
                Err(e) => {
                    log::warn!("跳过 {path:?}: {e:#}");
                    None
                }
            })
            .collect()
    }

    /// 只列**名字**,不读形态。托盘的「加一只」菜单要把包目录整个列出来
    /// (手上就有 539 个),而 [`Pack::list`] 会把每个包的动作表与材质表全解析出来 ——
    /// 菜单只需要一行字,真选中了再 [`Pack::load`]。
    ///
    /// 解析不了的包退用目录名:它多半仍能加载(名字这一节坏了不代表形态坏了),
    /// 真加载失败时再报错也不迟。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn list_entries(dir: &Path) -> Vec<PackEntry> {
        let mut packs: Vec<PackEntry> = Self::candidates(dir)
            .into_iter()
            .map(|path| {
                let (name, forms) = Self::peek(&path);
                let size = crate::assets::size(&path);
                PackEntry {
                    name,
                    forms,
                    path,
                    size,
                }
            })
            .collect();
        packs.sort_by(|a, b| a.name.cmp(&b.name));
        packs
    }

    /// 只把 manifest 里的物种名与形态名抠出来。读不动就退用文件名(去掉 `.rkpet` 后缀)。
    ///
    /// **这一趟已经把 manifest 读进内存了**,顺手多解一层 `[[forms]].name` 是白捡的 ——
    /// 比起单独为「列表要显示进化链」再读一遍全库五百多个 manifest 划算得多。
    /// 动作表与材质表仍然不解:那才是 `Pack::load` 慢的地方。
    pub fn peek(path: &Path) -> (String, Vec<String>) {
        #[derive(Deserialize)]
        struct NamesOnly {
            species: RawSpecies,
            #[serde(default)]
            forms: Vec<FormName>,
        }
        #[derive(Deserialize)]
        struct FormName {
            name: String,
        }

        let parsed = crate::assets::read_manifest(path)
            .ok()
            .and_then(|text| toml::from_str::<NamesOnly>(&text).ok());
        match parsed {
            Some(raw) => (
                raw.species.name,
                raw.forms.into_iter().map(|f| f.name).collect(),
            ),
            None => {
                let stem = if path.is_file() {
                    path.file_stem()
                } else {
                    path.file_name()
                };
                let name = stem
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "?".to_string());
                (name, Vec::new())
            }
        }
    }

    /// 只要名字那一半。
    pub fn peek_name(path: &Path) -> String {
        Self::peek(path).0
    }

    /// 按「路径」或「包名」定位一个包:优先当路径用,否则在包目录里按物种名/文件名找。
    ///
    /// 文件名那一条要**连去掉后缀的也认**:阵容存的是 `喵喵.rkpet`,而用户在配置里
    /// 多半只写 `喵喵` —— 同一个包换成目录形态之后名字还得对得上。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn resolve(value: &str, packs_dir: Option<&Path>) -> Result<Pack> {
        let scanned;
        let entries = match packs_dir {
            Some(dir) => {
                scanned = Pack::list_entries(dir);
                scanned.as_slice()
            }
            None => &[],
        };
        // 包目录写进上下文里,但**别用 Debug 格式** —— `{:?}` 出来是 `Some("/home/…")`,
        // 这句话是要摆到报错窗口里给人看的
        Pack::resolve_in(value, entries).with_context(|| match packs_dir {
            Some(dir) => format!("包目录是 {}", dir.display()),
            None => "而且定不出包目录".to_string(),
        })
    }

    /// 同上,但**目录已经扫好了**。
    ///
    /// 一次要认好几个名字时必须走这条:`resolve` 自己会把整个包目录的 manifest 读一遍,
    /// 每只宠物调一次就是把整库读上几遍 —— 实测六只在场时,`reload` 有 242ms 花在这儿
    /// (库里 201 个包),而且随在场只数线性涨。用户看到的「加一只宠物就像把所有包
    /// 重新加载一遍,卡一两秒」就是它。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn resolve_in(value: &str, entries: &[PackEntry]) -> Result<Pack> {
        // 存档里也可能直接写着路径(`--pack /some/where` 存下来的那种)
        let as_path = crate::config::Config::expand_path(value);
        if crate::assets::is_pack(&as_path) {
            return Pack::load(&as_path);
        }
        for entry in entries {
            let file_name = entry.path.file_name().map(|n| n.to_string_lossy());
            let stem = entry.path.file_stem().map(|n| n.to_string_lossy());
            let hit = entry.name == value
                || file_name.as_deref() == Some(value)
                || stem.as_deref() == Some(value);
            if hit {
                return Pack::load(&entry.path);
            }
        }
        bail!("找不到宠物包 {value}(既不是包目录/`.rkpet`,也不在包目录里)")
    }

    /// `root` 是包目录(含 manifest.toml)或 `.rkpet` 文件。
    pub fn load(root: &Path) -> Result<Self> {
        let path = crate::assets::manifest_path(root);
        let text = crate::assets::read_manifest(root)
            .with_context(|| format!("读不到 {path:?}(不是宠物包?)"))?;
        Self::parse(&text, root)
    }

    /// manifest 正文 → `Pack`。**从读盘那一步拆出来**是为了能拿一段内联 manifest 做单测:
    /// 包是本地生成物、不入仓库,不拆的话「这个字段解没解出来」只能靠跑全库肉眼看。
    fn parse(text: &str, root: &Path) -> Result<Self> {
        let path = crate::assets::manifest_path(root);
        let raw: RawManifest =
            toml::from_str(text).with_context(|| format!("{path:?} 解析失败"))?;
        if raw.schema > SUPPORTED_SCHEMA {
            bail!(
                "{path:?} 的 schema 是 {},本运行时只支持到 {SUPPORTED_SCHEMA}",
                raw.schema
            );
        }

        if let Some(version) = &raw.source_version {
            log::debug!("{path:?} 由源 {version} 导出");
        }
        let species_id = raw.species.id;
        let species_name = raw.species.name;
        let forms = raw
            .forms
            .into_iter()
            .map(|form| Form {
                id: form.id,
                name: form.name,
                stage: form.stage,
                asset: form.asset,
                model: root.join(form.model),
                scale: form.scale,
                // 没给高度就按一只猫的量级兜底,免得算出 0 像素
                height_cm: if form.height_cm > 1.0 {
                    form.height_cm
                } else {
                    80.0
                },
                locomotion: form.locomotion,
                // 两族库各自可能缺席(叫声 621 个 bnk、音效 650 个),**任一有就算有声音**
                voice: (form.voice.is_some() || !form.sfx.is_empty()).then(|| {
                    let (cents_low, cents_high, clips) = match form.voice {
                        Some(v) => (v.cents_low, v.cents_high, v.clips),
                        None => (0.0, 0.0, HashMap::new()),
                    };
                    Voice {
                        cents_low,
                        cents_high,
                        clips: sound_files(root, clips),
                        sfx: sound_files(root, form.sfx),
                    }
                }),
                clips: form
                    .clips
                    .into_iter()
                    .map(|(name, clip)| {
                        (
                            name,
                            Clip {
                                seconds: clip.ms as f32 / 1000.0,
                                speed_cm_s: clip.speed_cm_s,
                            },
                        )
                    })
                    .collect(),
                materials: form
                    .materials
                    .into_iter()
                    .map(|(name, mat)| {
                        (
                            // 键统一小写:材质名在「资产文件名」与「对象名」之间大小写会漂
                            // (喵呜是 MiaoMiao/Miaomiao、魔力猫反过来),查表必须不区分大小写
                            name.to_ascii_lowercase(),
                            Material {
                                base_color: mat.base_color.map(|rel| root.join(rel)),
                                mask_alpha: mat.mask_alpha,
                                face: mat.parents.iter().any(|p| p.contains("P_Eyes")),
                                face_cards: mat.parents.iter().any(|p| p.contains("P_Eyes_Mesh")),
                                effect: Effect {
                                    // 没给主色就用白,至少形体在
                                    tint: mat.tint.unwrap_or([1.0; 4]),
                                    opacity: mat.opacity,
                                    glow: mat.glow,
                                    flow: mat.flow.unwrap_or([0.0, 0.0, 1.0, 1.0]),
                                    mask: mat.mask_tex.clone().map(|rel| root.join(rel)),
                                    noise: mat.noise_tex.map(|rel| root.join(rel)),
                                    mask_matcap: mat.mask_matcap,
                                },
                                translucent: mat.translucent,
                                outline: mat.outline,
                                outline_width: mat.outline_width,
                                paint_order: mat.paint_order,
                                opacity: mat.opacity,
                                star: mat.star_tex.map(|rel| root.join(rel)),
                                star_fake_trans: mat.star_fake_trans,
                                star_tiling: mat.star_tiling.unwrap_or([1.0, 1.0]),
                                star_color: mat.star_color.unwrap_or([1.0; 3]),
                                stick_intensity: mat.stick_intensity,
                                matcap: mat.matcap_tex.map(|rel| root.join(rel)),
                                matcap_color: mat.matcap_color.unwrap_or([1.0; 3]),
                                rim_color: mat.rim_color.unwrap_or([1.0; 3]),
                                rim_intensity: mat.rim_intensity,
                                emissive: mat.emissive.unwrap_or([0.0; 3]),
                                emissive_intensity: mat.emissive_intensity,
                                rim_power: mat.rim_power,
                                rim_soft_edge: mat.rim_soft_edge,
                                highlight_offset: mat.highlight_offset.unwrap_or([0.0; 3]),
                                highlight_color: mat.highlight_color.unwrap_or([1.0; 3]),
                                highlight_power: mat.highlight_power,
                                highlight_intensity: mat.highlight_intensity,
                                force_default_opacity: mat.force_default_opacity,
                                opacity_depth_distance: mat.opacity_depth_distance,
                                open_depth_distance: mat.open_depth_distance,
                                object_trans_low: mat.object_trans_low,
                                light_mask: mat.light_mask_tex.map(|rel| root.join(rel)),
                                ramp: mat.ramp_tex.map(|rel| root.join(rel)),
                                object_trans_soft_edge: mat.object_trans_soft_edge,
                                main_color: mat.main_color.unwrap_or([1.0; 3]),
                                main_bright: mat.main_bright,
                                noise_uv: mat.noise_uv.unwrap_or([0.0, 0.0, 1.0, 1.0]),
                                alpha_opacity: mat.alpha_opacity,
                                flow: mat.flow_tex.map(|rel| root.join(rel)),
                                flow_uv: mat.flow.unwrap_or([0.0, 0.0, 1.0, 1.0]),
                                flow_power: mat.flow_power,
                                mask_id: mat.mask_id_tex.map(|rel| root.join(rel)),
                                mask_id_range: mat.mask_id_range.unwrap_or([0.0, 1.0]),
                                interior: mat.interior_tex.map(|rel| root.join(rel)),
                                interior_color: mat.interior_color.unwrap_or([1.0; 3]),
                                refraction: mat.refraction,
                                refract_depth: mat.refract_depth,
                                flicker: mat.flicker.unwrap_or([0.3, 5.0]),
                                glassy_inner: mat.glassy_inner.then(|| GlassyInner {
                                    flow1: mat.glassy_flow1.unwrap_or([1.0; 4]),
                                    flow2: mat.glassy_flow2.unwrap_or([1.0; 4]),
                                    fresnel: mat.glassy_fresnel.unwrap_or([1.0; 4]),
                                    // 旧包若只带开关而缺数组,退回游戏根材质的原始默认值。
                                    noise: mat.glassy_noise.unwrap_or([-0.1, 1.0, 0.2, 30.0]),
                                    mask: mat.glassy_mask.unwrap_or([1.0, 0.7, 0.1, 0.0]),
                                }),
                                xiaoyou: mat.xiaoyou.then(|| XiaoYou {
                                    base1: mat.xiaoyou_base1.unwrap_or([0.0, 0.0, 0.0, 1.0]),
                                    base2: mat.xiaoyou_base2.unwrap_or([0.0, 0.0, 0.0, 1.0]),
                                    flow1: mat.xiaoyou_flow1.unwrap_or([0.0, 0.0, 0.0, 1.0]),
                                    flow2: mat.xiaoyou_flow2.unwrap_or([0.0, 0.0, 0.0, 1.0]),
                                    star_color: mat.xiaoyou_star_color.unwrap_or([0.0; 4]),
                                    noise_flow: mat.xiaoyou_noise_flow.unwrap_or([0.0; 4]),
                                    shape: mat.xiaoyou_shape.unwrap_or([1.0, 1.0, 1.0, 0.0]),
                                    star_uv: mat.xiaoyou_star_uv.unwrap_or([1.0, 0.0, 1.0, 0.0]),
                                }),
                                yutu_ear: mat.yutu_ear.then(|| YutuEar {
                                    bubble: mat.yutu_bubble_tex.map(|rel| root.join(rel)),
                                    distort: mat.yutu_distort_tex.map(|rel| root.join(rel)),
                                    flow: mat.yutu_flow_tex.map(|rel| root.join(rel)),
                                    bubble_color: mat
                                        .yutu_bubble_color
                                        .unwrap_or([0.0, 0.508735, 1.0, 1.0]),
                                    flow_color: mat.yutu_flow_color.unwrap_or([1.0, 1.0, 1.0, 0.0]),
                                    fresnel_color: mat
                                        .yutu_fresnel_color
                                        .unwrap_or([1.0, 1.0, 1.0, 0.0]),
                                    inner_color: mat.yutu_inner_color.unwrap_or([1.0; 4]),
                                    overall_color: mat
                                        .yutu_overall_color
                                        .unwrap_or([1.0, 1.0, 1.0, 0.0]),
                                    ramp_color: mat.yutu_ramp_color.unwrap_or([1.0, 1.0, 1.0, 0.0]),
                                    top_color: mat.yutu_top_color.unwrap_or([0.0; 4]),
                                    bubble_shape: mat
                                        .yutu_bubble_shape
                                        .unwrap_or([0.05, 0.05, 5.0, 0.2]),
                                    flow_shape: mat
                                        .yutu_flow_shape
                                        .unwrap_or([0.1, -0.5, 1.0, 0.8]),
                                    light_shape: mat
                                        .yutu_light_shape
                                        .unwrap_or([0.3, 1.0, 1.0, 0.0]),
                                    top_shape: mat.yutu_top_shape.unwrap_or([0.0, 0.0, 1.0, 0.0]),
                                }),
                                fake_fluid: mat.fake_fluid.then(|| FakeFluid {
                                    edge_color: mat.fluid_edge_color.unwrap_or([1.0; 4]),
                                    fresnel_color: mat
                                        .fluid_fresnel_color
                                        .unwrap_or([1.0, 1.0, 1.0, 0.0]),
                                    plane_color: mat.fluid_plane_color.unwrap_or([1.0; 4]),
                                    gradient1: mat.fluid_gradient1.unwrap_or([1.0; 4]),
                                    gradient2: mat.fluid_gradient2.unwrap_or([1.0; 4]),
                                    height_tiling: mat
                                        .fluid_height_tiling
                                        .unwrap_or([1.0, 1.0, 0.0, 0.0]),
                                    plane_axis: mat
                                        .fluid_plane_axis
                                        .unwrap_or([0.0, 0.0, 1.0, 1.0]),
                                    plane_center: mat.fluid_plane_center.unwrap_or([0.0; 4]),
                                    body_shape: mat
                                        .fluid_body_shape
                                        .unwrap_or([5.0, 0.8, 0.1, 5.0]),
                                    gradient_shape: mat
                                        .fluid_gradient_shape
                                        .unwrap_or([0.5, 0.01, 0.3, 0.2]),
                                    top_shape: mat
                                        .fluid_top_shape
                                        .unwrap_or([0.3, 0.05, 1.0, 30.0]),
                                }),
                                matcap_masked: mat.matcap_masked.then(|| MatcapMasked {
                                    matcap: mat.mask_tex.map(|rel| root.join(rel)),
                                    base_color: mat
                                        .matcap_masked_base
                                        .unwrap_or([1.0, 1.0, 1.0, 0.0]),
                                    light_ramp: mat
                                        .matcap_masked_light_ramp
                                        .unwrap_or([1.0, 1.0, 1.0, 0.0]),
                                    flat_emissive: mat.matcap_masked_flat.unwrap_or([1.0; 4]),
                                    main_color: mat.matcap_masked_main.unwrap_or([1.0; 4]),
                                    selection_color: mat
                                        .matcap_masked_selection
                                        .unwrap_or([0.0; 4]),
                                    rim_shape: mat
                                        .matcap_masked_rim
                                        .unwrap_or([0.4, 0.3, 0.0, 3.0]),
                                    surface_shape: mat
                                        .matcap_masked_surface
                                        .unwrap_or([1.0, 0.0, 1.0, 0.0]),
                                }),
                            },
                        )
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        if forms.is_empty() {
            bail!("{path:?} 里没有任何形态");
        }
        Ok(Self {
            species_id,
            species_name,
            forms,
            path: root.to_path_buf(),
        })
    }

    /// 形态在 `forms` 里的下标(按资产名或中文名);给 None 就是 0。
    pub fn form_index(&self, asset: Option<&str>) -> Result<usize> {
        match asset {
            None => Ok(0),
            Some(want) => self
                .forms
                .iter()
                .position(|f| f.asset == want || f.name == want)
                .with_context(|| {
                    format!(
                        "包里没有形态 {want};有的是: {}",
                        self.forms
                            .iter()
                            .map(|f| format!("{}({})", f.asset, f.name))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// 一段真实 manifest 的节选(导出器的产物,只删了无关的表)。
    const MANIFEST: &str = r#"
schema = 1
[species]
id = 3001
name = "喵喵"

[[forms]]
id = 3001
name = "喵喵"
stage = 1
asset = "Gra_MiaoMiao1_001"
model = "forms/Gra_MiaoMiao1_001/model.glb"
height_cm = 55.6

  [forms.clips]
  Happy = { clip = "Happy", ms = 1500, frames = 46 }
  Alert = { clip = "Alert", ms = 4000, frames = 121 }

  [forms.voice]
  cents_low = -300
  cents_high = 300
  Happy = { path = "forms/Gra_MiaoMiao1_001/voice/Happy.ogg", ms = 2366 }
  Alert = { path = "forms/Gra_MiaoMiao1_001/voice/Alert.ogg", ms = 4499 }

  [forms.sfx]
  Happy = { path = "forms/Gra_MiaoMiao1_001/sfx/Happy.ogg", ms = 2354 }
"#;

    /// 声音是**两层**,而且两层与 `[forms.clips]` 同一把键 —— 运行时按动作名取声音就靠这个。
    #[test]
    fn both_sound_layers_are_read_and_keyed_by_clip_name() {
        let pack = Pack::parse(MANIFEST, Path::new("/packs/喵喵")).expect("该解得开");
        let form = &pack.forms[0];
        let voice = form.voice.as_ref().expect("该有声音");

        let mut keys: Vec<&str> = voice.clips.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["Alert", "Happy"], "叫声的键就是动作逻辑名");
        assert!(form.clip("Happy").is_some(), "同一把键在动作表里查得到");

        assert_eq!(voice.sfx.len(), 1, "音效层单独一节,少几段是常事");
        assert_eq!(
            voice.sfx["Happy"].path,
            Path::new("/packs/喵喵/forms/Gra_MiaoMiao1_001/sfx/Happy.ogg"),
            "路径要拼到包根上"
        );
        assert_eq!(voice.cents_low, -300.0);
    }

    /// 只有 `Pet_Action_*` 库、没有 `Pet_Vo_*` 的形态(全库 22 个)照样算有声音 ——
    /// 原来 `[forms.voice]` 一缺就整只哑掉。
    #[test]
    fn a_form_with_only_sfx_still_has_sound() {
        let text = MANIFEST
            .split("  [forms.voice]")
            .next()
            .expect("前半段")
            .to_string()
            + "  [forms.sfx]\n  Happy = { path = \"forms/x/sfx/Happy.ogg\", ms = 1 }\n";
        let pack = Pack::parse(&text, Path::new("/packs/x")).expect("该解得开");
        let voice = pack.forms[0].voice.as_ref().expect("只有音效也算有声音");
        assert!(voice.clips.is_empty());
        assert_eq!(voice.sfx.len(), 1);
        // 没有叫声那层就没有曲线,按原调走
        assert_eq!((voice.cents_low, voice.cents_high), (0.0, 0.0));
    }
}
