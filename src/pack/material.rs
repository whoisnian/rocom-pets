//! manifest 里 `[forms.materials]` 那半:**导出器与运行时之间的材质契约**。
//!
//! 从 `pack.rs` 拆出来的 —— 加一个材质族要动的就是这份(外加 `pet/gpu/mod.rs` 的
//! 「④ 逐材质」那段和 `pet/shader/`)。三块内容:
//!
//! 1. [`RawMaterial`]:manifest 的**字段表**。每个字段都 `#[serde(default)]` ——
//!    旧包缺字段要能按默认值降级,不该整只加载不出来。
//! 2. [`Material`] 与七个原生族的参数结构:运行时真正拿去打包 uniform 的形状。
//! 3. [`material_table`]:把前者翻成后者,顺带把相对路径解成绝对路径、判出各族。
//!
//! **字段的含义一律写在这里**(某个数是从哪条汇编、哪份 resource 读来的),
//! 着色器那边只写「怎么用」。改之前先看 docs/findings.md 对应那节。

use super::*;

/// `[forms.materials]` 一条:导出器从游戏材质实例里解出来的「这个槽该画什么」。
#[derive(Deserialize)]
pub(super) struct RawMaterial {
    /// 基色贴图的包内相对路径。**缺失 = 纯特效层**(火焰/水壳/光晕:材质里没有
    /// BaseTex/EyeTex,固有色是 shader 算的),运行时整片跳过;
    /// 将来做特效通道时改成按 blend 走半透/加色,见 design.md 横向待办。
    #[serde(default)]
    base_color: Option<String>,
    /// 贴图 alpha 是不是真遮罩。眼/嘴的眼神图集是(不剔就是一块方糊),
    /// 本体贴图不是(它的 alpha 是美术塞的遮罩通道,拿来剔会把身体啃掉)。
    #[serde(default)]
    mask_alpha: bool,
    /// 材质的父链(游戏里的材质实例继承)。**脸那几个槽认它**:
    /// 眼神是贴在 `M_P_Eyes` 这一族上的图集(见 `Material::face`)。
    #[serde(default)]
    parents: Vec<String>,
    /// 这个脸槽跟哪条眼神曲线走(`eye` / `eye_1` / `mouth` / `dynamic1`…,
    /// 键与 `[forms.face]` 那几个同一套)。**旧包没有这个字段** ⇒ 按材质名后缀猜
    /// (见 `face_slot_of`),那时数不出「同后缀里的第几个」,多槽的形态只有第一个能动。
    #[serde(default)]
    face_track: Option<String>,
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
    /// 描边的**五档颜色**(线性 RGB)与挑档用的遮罩。见 `MaterialSpec::outline_colors`。
    #[serde(default)]
    outline_colors: Option<[[f32; 3]; 5]>,
    #[serde(default)]
    outline_id_tex: Option<String>,
    /// 逐 `MatID` 的高光。见 `MaterialSpec::spec_slots`。
    #[serde(default)]
    spec_slots: Option<[[f32; 3]; 4]>,
    #[serde(default)]
    spec_color: Option<[f32; 3]>,
    /// `MaskTex`:RG 是切线空间法线、A 是 `MatID`。见 `MaterialSpec::mat_id_mask`。
    #[serde(default)]
    mat_id_tex: Option<String>,
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
    /// 见 `MaterialSpec::glassy_star_tiling`。
    #[serde(default)]
    glassy_star_tiling: Option<f32>,
    /// 见 `MaterialSpec::glassy_params`。
    #[serde(default)]
    glassy_params: Option<[f32; 11]>,
    /// 见 `MaterialSpec::glassy_rim`。
    #[serde(default)]
    glassy_rim: Option<[f32; 4]>,
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
    /// `M_P_Object` 公共链上的加性流动层。
    #[serde(default)]
    uv_flow_tex: Option<String>,
    #[serde(default)]
    uv_flow_color: Option<[f32; 4]>,
    #[serde(default)]
    uv_flow_shape: Option<[f32; 4]>,
    #[serde(default)]
    uv_flow_radial: Option<[f32; 4]>,
    /// 火系族多的那两层。见 `Material::fire`。
    #[serde(default)]
    fire1: Option<[f32; 4]>,
    #[serde(default)]
    fire2: Option<[f32; 4]>,
    #[serde(default)]
    fire3: Option<[f32; 4]>,
    #[serde(default)]
    fire4: Option<[f32; 4]>,
    #[serde(default)]
    fire_shape: Option<[f32; 4]>,
    /// 同一条链上那圈菲涅尔发光。
    #[serde(default)]
    fresnel: Option<[f32; 4]>,
    #[serde(default)]
    fresnel_shape: Option<[f32; 4]>,
    #[serde(default)]
    fresnel_hard: Option<[f32; 4]>,
    /// 炫彩的**区域门**。见 `Material::glassy_id_mask`。
    #[serde(default)]
    glassy_id_tex: Option<String>,
    /// 赛季传说精灵的专属基色贴图。见 `Material::season_base_color`。
    #[serde(default)]
    season_base_color: Option<String>,
    #[serde(default)]
    season_flow_noise: Option<String>,
    #[serde(default)]
    season_mix_mask: Option<String>,
    #[serde(default)]
    season_matcap: Option<String>,
    #[serde(default)]
    season_red: Option<[f32; 4]>,
    #[serde(default)]
    season_green: Option<[f32; 4]>,
    #[serde(default)]
    season_blue: Option<[f32; 4]>,
    #[serde(default)]
    season_metal: Option<[f32; 4]>,
    #[serde(default)]
    season_metal2: Option<[f32; 4]>,
    #[serde(default)]
    season_flow: Option<[f32; 4]>,
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
    /// 第二层星点(`Star_BA_*`)。见 `XiaoYou::star_uv2` / `star2`。
    #[serde(default)]
    xiaoyou_star_uv2: Option<[f32; 4]>,
    #[serde(default)]
    xiaoyou_star2: Option<[f32; 4]>,
    /// `MI_P_Object_Water_NoMetal` 的水体预设。见 `MaterialSpec::water`。
    #[serde(default)]
    water_color1: Option<[f32; 4]>,
    #[serde(default)]
    water_color2: Option<[f32; 4]>,
    #[serde(default)]
    water_main: Option<[f32; 4]>,
    #[serde(default)]
    water_caustics: Option<[f32; 4]>,
    #[serde(default)]
    water_flow: Option<[f32; 4]>,
    #[serde(default)]
    water_shape: Option<[f32; 4]>,
    /// `MI_P_Object_Trans_XingGuang_Fresnel`(暮星辰那两颗球)。见 `MaterialSpec::xing_fresnel`。
    #[serde(default)]
    xing_fresnel: Option<[f32; 4]>,
    #[serde(default)]
    xing_fresnel2: Option<[f32; 4]>,
    #[serde(default)]
    xing_fresnel_shape: Option<[f32; 4]>,
    #[serde(default)]
    xing_fresnel_alpha: Option<[f32; 4]>,
    /// `M_P_BackRenderEmissive`:只画一侧的不透明背板(unlit)。见 `MaterialSpec::back_render`。
    #[serde(default)]
    back_render: bool,
    #[serde(default)]
    back_render_flow_tex: Option<String>,
    #[serde(default)]
    back_render_level: Option<[f32; 4]>,
    #[serde(default)]
    back_render_saturation: Option<[f32; 4]>,
    #[serde(default)]
    back_render_flow_color: Option<[f32; 4]>,
    #[serde(default)]
    back_render_flow: Option<[f32; 4]>,
    #[serde(default)]
    back_render_radial: Option<[f32; 4]>,
    #[serde(default)]
    back_render_main: Option<[f32; 4]>,
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
    /// `M_FairyBall_BallFront` 的目标 PS 52626 分支(沙漏/水晶球的玻璃壳)。
    #[serde(default)]
    fairy_ball: bool,
    #[serde(default)]
    fairy_matcap_tex: Option<PathBuf>,
    #[serde(default)]
    fairy_base: Option<[f32; 4]>,
    #[serde(default)]
    fairy_matcap_color: Option<[f32; 4]>,
    #[serde(default)]
    fairy_rim_dark: Option<[f32; 4]>,
    #[serde(default)]
    fairy_rim_light: Option<[f32; 4]>,
    #[serde(default)]
    fairy_main: Option<[f32; 4]>,
    #[serde(default)]
    fairy_shape: Option<[f32; 4]>,
}


pub(super) fn one() -> f32 {
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

/// 炫彩刷在哪些材质上 —— 见 `Material::glassy_target`。
///
/// 客户端那边给的是**材质槽后缀**清单 `{"by", "by0", …, "by9"}`,而我们手上是材质名
/// (`MI_Gra_Miaomiao1_001_By`),两者的对应关系就是「名字以 `_<后缀>` 结尾」。
/// 大小写不能较真:同一只宠物的材质名在资产文件名与对象名之间会漂(`MiaoMiao`/`Miaomiao`)。
fn is_glassy_target(name: &str, parents: &[String]) -> bool {
    let lower = name.to_ascii_lowercase();
    let suffix_ok = lower.strip_suffix("_by").is_some()
        || lower
            .rsplit_once("_by")
            .is_some_and(|(_, tail)| tail.len() == 1 && tail.as_bytes()[0].is_ascii_digit());
    // `M_P_Object` 会同时匹配 `M_P_Object_Trans` 等派生族,这正是想要的:
    // 那一族(不透明与半透)都带 `GlassySwitch`。
    suffix_ok && parents.iter().any(|p| p.contains("M_P_Object"))
}


/// 这个脸槽跟哪条曲线走(`pack::face_slot` 的编号)。
///
/// **优先用导出器写的 `face_track`** —— 「同后缀里的第几个」是按网格的槽序数的,
/// 而这张表是按名字查的哈希表,自己数不出来。旧包没有那个字段时按名字后缀退一档:
/// `_Es*` → 眼、`_Mh*` → 嘴、`_Dynamic<n>` → 对应那条,序号一律当 0
/// (多槽的形态因此只有第一个会动 —— 全库 8 个形态如此,重导即可)。
///
/// 大小写不较真,理由同 `is_glassy_target`:材质名在资产文件名与对象名之间会漂。
fn face_slot_of(name: &str, track: Option<&str>) -> usize {
    if let Some(slot) = track.and_then(crate::pack::face_slot) {
        return slot;
    }
    let lower = name.to_ascii_lowercase();
    let suffix = lower.rsplit('_').next().unwrap_or_default();
    let digits_only = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    if let Some(tail) = suffix.strip_prefix("mh")
        && digits_only(tail)
    {
        return crate::pack::face_slot("mouth").unwrap_or(0);
    }
    if let Some(tail) = suffix.strip_prefix("dynamic")
        && digits_only(tail)
    {
        let n = if tail.is_empty() { "1" } else { tail };
        return crate::pack::face_slot(&format!("dynamic{n}")).unwrap_or(0);
    }
    0
}

/// 一个材质槽该怎么画。由导出器解析游戏材质实例得出,取代原来按贴图命名约定的猜法。
#[derive(Clone)]
pub struct Material {
    /// 基色贴图的绝对路径;None = 纯特效层,走 `effect` 那套画法。
    pub base_color: Option<PathBuf>,
    pub mask_alpha: bool,
    /// 这是脸(眼睛/嘴)吗 —— 父链里有 `M_P_Eyes` 就是。
    ///
    /// **眼神就画在这两个槽上**:贴图是 2×4 的眼神图集,网格 UV 落在左上那一格,
    /// 换眼神 = 给 UV 加一个整格的偏移(见 persona.rs 的 `Expression`)。
    pub face: bool,
    /// 这个脸槽跟哪条眼神曲线走(`pack::face_slot` 的编号:0 眼、2 嘴、4..6 Dynamic1..3)。
    /// 只在 `face` 为真时有意义。
    ///
    /// 一个形态可以有好几个脸槽,各有各的图集与曲线:全库 1084 个挂 `P_Eyes` 的材质里
    /// `_Es` 762 个、`_Mh` 290 个、`_Dynamic1..3` 40 个。同一段动作里它们可以指向不同的格
    /// (幽影树的 `Relax` 是眼第 6/7 格、两条藤第 4 格),所以运行时逐槽偏。
    ///
    /// 判据是导出器写的 `face_track`(它按网格槽序数了「同后缀里的第几个」),
    /// 旧包退回按名字后缀猜,见 `face_slot_of`。
    pub face_slot: u8,
    /// 脸的另一种做法:父链里是 `M_P_Eyes_Mesh`(全库 859 片脸网格里有 21 片)。
    ///
    /// 这一族**不偏 UV**:八种眼神各是一份独立几何,叠在同一处,UV 各自钉死在图集的一格上,
    /// 顶点色 G 通道写着自己是第几张(`floor(G × 10)` ∈ 1..8)。游戏靠材质参数只画其中一张
    /// (根材质 `M_P_Eyes_Mesh` 上那个默认值为 1 的 `Number`)。
    /// **我们原来把它当普通图集脸画,于是八张一起画** —— 乖乖鹄一家的
    /// 「眉毛、眼睛、腮红搅在一起」就是这么来的。选哪张见 persona.rs 的 `Expression::card`。
    pub face_cards: bool,
    /// 炫彩要刷在这个槽上吗。**判据照抄客户端**:`PetMutationUtils.SetGlassyDiffMutation`
    /// 只取后缀 `by` / `by0..by9` 的材质,再加一道父链闸 —— `GlassySwitch` 这个动态开关
    /// 只存在于 `M_P_Object` 那一族(全库 898 个材质),眼睛走的 `M_P_Eyes` 一族没有它,
    /// 刷上去只会把脸糊掉。
    pub glassy_target: bool,
    /// 只在 `base_color` 为 None 时有效。
    pub effect: Effect,
    /// 半透。**有基色的材质也可能是半透**:暮星辰的裙子与那两个球都是,
    /// 当不透明画就是死板的实心块。
    pub translucent: bool,
    /// 这个材质画不画描边 —— 游戏是**逐材质**开的(`Mat/` 里有没有配套的 `_Ol` 资产)。
    /// `None` = 旧包没这个字段,退回「不透明画、半透不画」。
    pub outline: Option<bool>,
    /// 沿法线外扩多少**米**,导出器逐材质算好。**它是随宠物大小走的**:全库 851/854 的描边
    /// 在实机里是**屏幕空间常数**(见 `Materials.cs` 的 `OutlineOf`),换算到我们的正交取景
    /// 就是「占这个形态包围盒高度 0.255% × OutlineWidthPC/0.13」;剩下 3 份(火源)才是
    /// 世界空间常数。所以同一个数字在不同形态上并不相同,别再当成全库一份的常数用。
    /// `None` = 旧包没这个字段,退回 [`DEFAULT_OUTLINE_WIDTH`](../pet/gpu.rs) 那个模态值。
    pub outline_width: Option<f32>,
    /// **描边的五档颜色**(线性 RGB,已乘过 `Outline Intensity`),按 `outline_id_mask` 的
    /// alpha 挑:`挡位 = floor(min((1 − MatID) × 5 + 1, 5))`。推导见 `Materials.cs` 的 `OutlineOf`。
    /// `None` = 旧包没这个字段(或那份 `_Ol` 挂在别的根材质上)⇒ 退回「固有色 × 0.80」。
    pub outline_colors: Option<[[f32; 3]; 5]>,
    /// 挑档用的 `MatID` 遮罩(读 alpha)。**不能拿 `glassy_id_mask` 顶替** ——
    /// 854 份 `_Ol` 里 38 份指着另一张图、81 份本体压根没有 `MaskTex`。
    pub outline_id_mask: Option<PathBuf>,
    /// **逐 `MatID` 的高光**:四档 `(SpecPow, SpecIntensity, SpecRadius)`(挡位 2~5;
    /// 第 1 档在汇编里是硬写的立即数,见 pet/shader/90-outline.wgsl 的 `matid_specular`)。
    /// `None` = 这个材质四档强度全 0(全库 2539 份里 2326 份如此),或者旧包没这个字段。
    pub spec_slots: Option<[[f32; 3]; 4]>,
    /// 上面那层的染色 `SpecColor`(线性 RGB,根默认白)。
    pub spec_color: Option<[f32; 3]>,
    /// **`MaskTex`** —— 一张图装三样:**RG 是切线空间法线**、B 是明暗覆写(全库恒 0.298,
    /// 是死的)、**A 是 `MatID`**(炫彩区域门、描边五档、高光五档都读它)。
    /// 只在有基色贴图的材质上导 —— 纯特效层与几个专用族的这个槽装的不是法线。
    /// 和 `glassy_id_mask` 是**同一张图**,但那一份只给炫彩槽导,所以单开一条。
    pub mat_id_mask: Option<PathBuf>,
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
    /// **炫彩星贴层的 uv 平铺**,也就是这个材质自己的标量 `StarStickTiling`。
    ///
    /// 和上面的 `star_tiling` 是同一个参数,但**不能共用**:`star_tiling` 会被导出器
    /// 跨材质统一成「这只宠物的那一份」(见 Program.cs 里 `starLayer` 那段),而且只在
    /// 真开了星点层的材质上才写;炫彩这条要的是**每个材质自己那一份**,而且哪怕这个
    /// 材质平时不画星点也要有。
    ///
    /// `None` = 旧包没这个字段 ⇒ 退回根材质 `M_P_Object` 的默认 **4**。
    /// 炫彩只刷在那一族的 `_by*` 槽上,所以这个兜底在能上炫彩的材质上总是对的口径;
    /// 逐材质的微调(鸭吉吉 4.11)要重导一次包才拿得到。
    pub glassy_star_tiling: Option<f32>,
    /// **炫彩玻璃层的那几个标量,也是逐材质的**:
    /// `[GlobalRefraction, GlobalDepth, MainTexTiling, FlowSpeedX, FlowSpeedY,
    ///   NormalEffectAmount, BaseColorDetail, FlowColorIntensity,
    ///   StarTiling, StarDensity, StarIntensity]` —— 后三个是闪点层的
    /// (格子大小 / 密度 / 亮度,见 pet/shader/70-glassy.wgsl 的 `glassy_sparkle`)。
    ///
    /// lua 给常规炫彩只覆盖两个 Channel 色与贴图,这几条一个都不碰 ⇒ 实机用的就是
    /// 材质实例自己那份。差别不小:加油海葵 `MainTexTiling = 0.2`(根默认 1.5)、
    /// 查过的宠物 `GlobalRefraction` 一律 1.3 / `GlobalDepth` 100(根默认 2.0 / 30)。
    ///
    /// `None` = 旧包没这个字段 ⇒ 退回 `glassy::ROOT_PARAMS`(根材质默认)。
    pub glassy_params: Option<[f32; 11]>,
    /// **炫彩那圈边缘光**:`[RimColor.rgb, RimIntensity]`。
    ///
    /// 和上面的 `rim_color`/`rim_intensity`(那是带空格的 `Rim LightColor`/`Rim Intensity`)
    /// **不是同一组参数**;玻璃层读的是不带空格的这两个。`None` = 旧包 ⇒ 退回根默认
    /// `(0.844, 0.961, 1) × 1.5`,见 `glassy::ROOT_RIM`。
    pub glassy_rim: Option<[f32; 4]>,
    /// 星点层的强度(根材质 `Stick_Intensity` = 1.5)。
    pub stick_intensity: f32,
    /// 球面反射查找表:玻璃/金属高光。
    pub matcap: Option<PathBuf>,
    pub matcap_color: [f32; 3],
    pub rim_color: [f32; 3],
    pub rim_intensity: f32,
    /// 加在光照**之后**的那层色,遮罩是基色 alpha 的重映射(见 pet/shader/*.wgsl 的 `detail_mask`)。
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
    /// 假半透族星点层:[速度X, 速度Y, 强度, 是否用 UV0]。见 pet/shader/40-layers.wgsl 的 stick_layer。
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
    /// **`M_P_Object` 公共链上的加性流动层**(读自波波拉 `_By` 的 quality=Num 排列 PS 49966
    /// 第 110~123 行;火系 41058 第 160~177 行逐指令相同)。`[FlowColor.rgb, FlowInt]`,
    /// `.w = 0` 表示这一层不画。贴图与 `flow` 共用 `noise` 那个槽。
    /// 流动贴图。**运行时和噪声/色带共用同一个绑定**(见 `gpu.rs` 里挑 `second` 那段)。
    pub uv_flow: Option<PathBuf>,
    pub uv_flow_color: [f32; 4],
    /// `[FlowPower, InverVertexColor, Inv Or Not, OpenRadialUV]`。
    pub uv_flow_shape: [f32; 4],
    /// `OpenRadialUV` 打开时的极坐标中心 `[x, y, -, -]`。
    pub uv_flow_radial: [f32; 4],
    /// **火系族**(`MI_P_Object_Fire*`)在同一个发光累加器上多的两层
    /// (读自火神 `_By` 的 quality=Num 排列 PS 41058 第 68~122 行):
    /// `[Color1.rgb, FresnelPower]` / `[Color2.rgb, FresnelInt]` / `[Color.rgb, Int]` /
    /// `[Color02.rgb, UseVertexColorG]` / `[Range, Soft, Use Opacity as Mask, 这一族(0/1)]`。
    pub fire1: [f32; 4],
    pub fire2: [f32; 4],
    pub fire3: [f32; 4],
    pub fire4: [f32; 4],
    pub fire_shape: [f32; 4],
    /// 同一条链上那圈菲涅尔发光:`[FresnelColor.rgb, FresnelIntensity]`,`.w = 0` 表示不画。
    pub fresnel: [f32; 4],
    /// `[FresnelExponent, FresnelBoost, FresnelBaseMin, FresnelSoftTohard]`。
    pub fresnel_shape: [f32; 4],
    /// `[HardLineCol.rgb, HardLineColMul]`。
    pub fresnel_hard: [f32; 4],
    /// **炫彩的区域门**:同一张 `_M` 贴图,但读的是另一道门 —— 玻璃层只刷在
    /// `alpha >= `[`GLASSY_MIN_ID`] 的地方,别处原样输出。
    ///
    /// 这就是「游戏只给部位上色」的机制:鸭吉吉那张 `By_M` 的 alpha 身体 1.0、**喙与脚 0**;
    /// 白金独角兽鬃毛/尾/腿毛 0.5、**身体 0**。不接这道门,整只连喙带脚都会被刷上玻璃色。
    /// 只有 `glassy_target` 的槽导得到;旧包没有这个字段 ⇒ `None` ⇒ 整片都刷(老行为)。
    pub glassy_id_mask: Option<PathBuf>,
    /// **赛季传说精灵的专属基色贴图**(材质里的 `BaseTexSketch`)。
    ///
    /// 游戏对 `HIDDEN_GLASS_CONF.season_pet` 里那几只走一条单独的路:开动态开关
    /// `MutationSwitch`,而那个开关在编译产物里做的就是**把基色贴图这个绑定槽从
    /// `BaseTex` 换成 `BaseTexSketch`**(两者 `index` 相同)—— 整套「赛季特殊效果」
    /// 就是换一张图。**它不开 `GlassySwitch`**,所以这几只上自家赛季炫彩时没有玻璃层。
    pub season_base_color: Option<PathBuf>,
    /// **`MI_P_Object_SeasonMutation*` 那族的赛季外观。** 和「换基色贴图」那条是两种做法:
    /// 铅字幻梦(加灵一家)换图,暗夜拾光的龙息帕尔与狂欢怪谈的机幕方舟走这一族。
    /// 骨架就是玻璃层,只是换了输入 —— 见 `pet::glassy::SeasonMutation`。
    pub season_mutation: Option<SeasonMutation>,
    /// **玻璃内部那颗星**:四角星场贴图(`StarTex` = `T_EMeng003`),沿折射光线在物体空间
    /// march、三向投影采样、按时间卷动。读 shader 汇编得来,见 docs/findings.md §1。
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
    /// `MI_P_Object_Water_NoMetal` 的水体预设(caustics + 两色菲涅尔),`None` = 这个材质没有。
    /// 判据是导出器写没写 `water_color1`(那一项只有这一族有)。
    pub water: Option<Water>,
    /// 幻星族那两颗球的菲涅尔换色层;判据是导出器写没写 `xing_fresnel`。
    pub xing_fresnel: Option<XingFresnel>,
    /// `M_P_BackRenderEmissive` 的不透明背板。**只画一侧**,哪一侧看 `level[3]`
    /// (`UseBackFace`:0 = 只画正面,1 = 只画背面)。判据与汇编见导出器的
    /// `MaterialInfo.IsBackRender`。
    pub back_render: Option<BackRender>,
    /// 莫比乌乌内层的原生不透明液体材质。
    pub yutu_ear: Option<YutuEar>,
    /// 克莱因龙的原生 FakeFulid 玻璃/液面材质。
    pub fake_fluid: Option<FakeFluid>,
    /// `M_P_MatCap_Masked` 的不透明 MatCap 外壳。
    pub matcap_masked: Option<MatcapMasked>,
    /// `M_FairyBall_BallFront` 的半透明玻璃壳。
    pub fairy_ball: Option<FairyBall>,
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
    /// `Star_RG_UV_Control` = (平铺U, 速度U, 平铺V, 速度V);两个速度在 shader 里除以 100。
    pub star_uv: [f32; 4],
    /// **星点是两层**:RG 那层用 `StarTex` 的 R(相位)/ G(遮罩),BA 那层用 B / A。
    /// 这两格是 BA 那层的:`star_uv2` = `Star_BA_UV_Control`,
    /// `star2` = [`Star_RG_DarkTime`, `Star_BA_DarkTime`, `Star_BA_Int`, `Star_BA_TwinkleSpeed`]。
    pub star_uv2: [f32; 4],
    pub star2: [f32; 4],
}

/// 水体预设的材质局部链。逐字段含义见 pet/shader/40-layers.wgsl 的 `water_layer`;
/// 全部来自 PS 16335(水灵 `_Fx` 的 `Num/lod=0/dsid=0`,resource `AC743E86…`)第 62~118 行。
#[derive(Clone, Copy)]
pub struct Water {
    /// [Color1.rgb, Emitter Intensity]
    pub color1: [f32; 4],
    /// [Color2.rgb, -]
    pub color2: [f32; 4],
    /// [Main Color.rgb, -]
    pub main: [f32; 4],
    /// caustics 那一路的 [u 平铺, v 平铺, u 速度, v 速度]
    pub caustics: [f32; 4],
    /// 流动扰动那一路的 [u 平铺, v 平铺, u 速度, v 速度]
    pub flow: [f32; 4],
    /// [CausticsInt, FlowDistort, FresnelInt, FresnelPower]
    pub shape: [f32; 4],
}

/// 幻星族那两颗球的菲涅尔换色层。字段含义见导出器的 `MaterialInfo.IsXingGuangFresnel`,
/// 公式见 pet/shader/*.wgsl 的 `xing_fresnel_layer`;来自 PS 53466(暮星辰 `_Fx2` 的
/// `Num/lod=0/dsid=0`,resource `6CCB83FD…`)第 151~209 行。
#[derive(Clone, Copy)]
pub struct XingFresnel {
    /// [Color.rgb, Int]
    pub color: [f32; 4],
    /// [Color02.rgb, OpenEmissiveBlend]
    pub color2: [f32; 4],
    /// [Range, Soft, UseVertexColorG, BottomLayer/TopLayer Opacity]
    pub shape: [f32; 4],
    /// [OpenOpacityAdd, UseOpacityMask, InversionMask, ForceUseDefOpacity]
    pub alpha: [f32; 4],
}

/// `M_P_BackRenderEmissive` 的材质局部链。字段含义见导出器的 `MaterialInfo.IsBackRender`;
/// 基色贴图沿用 `Material::base_color`,流动贴图在运行时和别的族共用 `noise_tex` 那个绑定。
#[derive(Clone)]
pub struct BackRender {
    pub flow: Option<PathBuf>,
    /// [RGB强度(Dark), RGB强度(Light), saturate(UVNumber), UseBackFace]
    pub level: [f32; 4],
    /// [饱和度变化.rgb, FlowPower]
    pub saturation: [f32; 4],
    /// [UVFlowColor.rgb, FlowInt]
    pub flow_color: [f32; 4],
    /// [U_Speed, V_Speed, U_Tiling, V_Tiling]
    pub flow_uv: [f32; 4],
    /// [RadialCenterX, RadialCenterY, OpenRadialUV, 流动贴图是不是 sRGB]
    pub radial: [f32; 4],
    /// [MainColor.rgb × MainBright, 基色贴图是不是 sRGB]
    pub main: [f32; 4],
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

/// 沙漏 / 水晶球外面那层玻璃壳(`M_FairyBall_BallFront`,全库 5 个材质)。
/// 目标 PS 52626 只采一张贴图,就是 MatCap;它不出固有色、也不吃光照。
#[derive(Clone)]
pub struct FairyBall {
    pub matcap: Option<PathBuf>,
    /// `BaseColor`:rgb 是加在 MatCap 上的底色,a 与 `Opacity` 相加是覆盖率的底。
    pub base_color: [f32; 4],
    /// `MatCapColor`:rgb 乘 MatCap,a 是 MatCap 亮度换算成覆盖率的增益。
    pub matcap_color: [f32; 4],
    pub rim_dark: [f32; 4],
    pub rim_light: [f32; 4],
    /// [MainColor.rgb, MainBright]
    pub main_color: [f32; 4],
    /// [RimArea, RimSmoothness, Opacity, 1](末位是这一族的开关)
    pub shape: [f32; 4],
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


/// `[forms.materials]` / `[forms.shiny_materials]` → 运行时的材质表。
///
/// **两节共用一套字段**:异色是换整套材质,不是另一种着色,所以每一条的形状与默认那套
/// 逐个相同,只是值来自 `<资产>/Yise/Mat/`。见导出器的 `Shiny`。
pub(super) fn material_table(root: &Path, raw: HashMap<String, RawMaterial>) -> HashMap<String, Material> {
    raw.into_iter()
        .map(|(name, mat)| {
            (
                // 键统一小写:材质名在「资产文件名」与「对象名」之间大小写会漂
                // (喵呜是 MiaoMiao/Miaomiao、魔力猫反过来),查表必须不区分大小写
                name.to_ascii_lowercase(),
                Material {
                    base_color: mat.base_color.map(|rel| root.join(rel)),
                    mask_alpha: mat.mask_alpha,
                    face: mat.parents.iter().any(|p| p.contains("P_Eyes")),
                    face_slot: face_slot_of(&name, mat.face_track.as_deref()) as u8,
                    face_cards: mat.parents.iter().any(|p| p.contains("P_Eyes_Mesh")),
                    glassy_target: is_glassy_target(&name, &mat.parents),
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
                    outline_colors: mat.outline_colors,
                    outline_id_mask: mat.outline_id_tex.map(|rel| root.join(rel)),
                    spec_slots: mat.spec_slots,
                    spec_color: mat.spec_color,
                    mat_id_mask: mat.mat_id_tex.map(|rel| root.join(rel)),
                    paint_order: mat.paint_order,
                    opacity: mat.opacity,
                    star: mat.star_tex.map(|rel| root.join(rel)),
                    star_fake_trans: mat.star_fake_trans,
                    star_tiling: mat.star_tiling.unwrap_or([1.0, 1.0]),
                    glassy_star_tiling: mat.glassy_star_tiling,
                    glassy_params: mat.glassy_params,
                    glassy_rim: mat.glassy_rim,
                    star_color: mat.star_color.unwrap_or([1.0; 3]),
                    stick_intensity: mat.stick_intensity,
                    matcap: mat.matcap_tex.map(|rel| root.join(rel)),
                    matcap_color: mat.matcap_color.unwrap_or([1.0; 3]),
                    // **默认黑,不是白。** `rim_intensity` 现在一律导(它还喂着覆盖率),
                    // 而 `rim_color` 仍只在「强度 > 1」时导 —— 两者不再同进同出。
                    // 默认白会让那 943 个「只有强度」的材质凭空多一圈白边;黑则是
                    // 「只顶覆盖率、不加颜色」,正是这条门想要的。
                    rim_color: mat.rim_color.unwrap_or([0.0; 3]),
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
                    uv_flow: mat.uv_flow_tex.map(|rel| root.join(rel)),
                    uv_flow_color: mat.uv_flow_color.unwrap_or([0.0; 4]),
                    uv_flow_shape: mat.uv_flow_shape.unwrap_or([1.0, 0.0, 0.0, 0.0]),
                    uv_flow_radial: mat.uv_flow_radial.unwrap_or([0.5, 0.5, 0.0, 0.0]),
                    fire1: mat.fire1.unwrap_or([1.0, 1.0, 1.0, 1.0]),
                    fire2: mat.fire2.unwrap_or([1.0, 1.0, 1.0, 0.0]),
                    fire3: mat.fire3.unwrap_or([1.0, 1.0, 1.0, 0.0]),
                    fire4: mat.fire4.unwrap_or([1.0, 1.0, 1.0, 0.0]),
                    fire_shape: mat.fire_shape.unwrap_or([0.0, 0.5, 0.0, 0.0]),
                    fresnel: mat.fresnel.unwrap_or([0.0; 4]),
                    fresnel_shape: mat.fresnel_shape.unwrap_or([8.0, 20.0, 0.4, 1.0]),
                    fresnel_hard: mat.fresnel_hard.unwrap_or([1.0; 4]),
                    glassy_id_mask: mat.glassy_id_tex.map(|rel| root.join(rel)),
                    season_base_color: mat.season_base_color.map(|rel| root.join(rel)),
                    // 判据用 `MixMask`:花纹图可以缺(`_By1` 就缺),遮罩不能缺 ——
                    // 缺了就没有「哪儿变」这回事。
                    season_mutation: mat.season_mix_mask.map(|mask| SeasonMutation {
                        flow_noise: mat.season_flow_noise.map(|rel| root.join(rel)),
                        mix_mask: Some(root.join(mask)),
                        matcap: mat.season_matcap.map(|rel| root.join(rel)),
                        red: mat.season_red.unwrap_or([1.0, 1.0, 1.0, 2.0]),
                        green: mat.season_green.unwrap_or([1.0, 1.0, 1.0, 30.0]),
                        blue: mat.season_blue.unwrap_or([1.0, 1.0, 1.0, 1.0]),
                        metal: mat.season_metal.unwrap_or([1.0, 1.0, 1.0, 1.0]),
                        metal2: mat.season_metal2.unwrap_or([1.0, 1.0, 1.0, 0.0]),
                        flow: mat.season_flow.unwrap_or([0.0, 0.0, 1.5, 0.1]),
                    }),
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
                        // 旧包没这两格 ⇒ 强度 0 ⇒ 第二层不出场,退回原来的单层近似
                        star_uv2: mat.xiaoyou_star_uv2.unwrap_or([1.0, 0.0, 1.0, 0.0]),
                        star2: mat.xiaoyou_star2.unwrap_or([0.0; 4]),
                    }),
                    water: mat.water_color1.map(|c1| Water {
                        color1: c1,
                        color2: mat.water_color2.unwrap_or([0.0; 4]),
                        main: mat.water_main.unwrap_or([0.0; 4]),
                        caustics: mat.water_caustics.unwrap_or([1.0, 0.8, 0.1, -0.5]),
                        flow: mat.water_flow.unwrap_or([1.0, 0.8, 0.1, -0.5]),
                        shape: mat.water_shape.unwrap_or([1.0, 0.2, 1.0, 1.771117]),
                    }),
                    xing_fresnel: mat.xing_fresnel.map(|c| XingFresnel {
                        color: c,
                        color2: mat.xing_fresnel2.unwrap_or([1.0; 4]),
                        // 兜底 = 根材质默认:Range 15 / Soft 0.5 ⇒ 一条极窄的边;
                        // `UseVertexColorG = 0` ⇒ 两颗球同色。
                        shape: mat.xing_fresnel_shape.unwrap_or([15.0, 0.5, 0.0, 1.0]),
                        alpha: mat.xing_fresnel_alpha.unwrap_or([0.0; 4]),
                    }),
                    back_render: mat.back_render.then(|| BackRender {
                        flow: mat.back_render_flow_tex.map(|rel| root.join(rel)),
                        // 兜底 = 根材质默认值:`lerp(0, 1, tex)` 是恒等、不去饱和、不流动。
                        level: mat.back_render_level.unwrap_or([0.0, 1.0, 0.0, 0.0]),
                        saturation: mat.back_render_saturation.unwrap_or([0.0, 0.0, 0.0, 1.0]),
                        flow_color: mat.back_render_flow_color.unwrap_or([1.0; 4]),
                        flow_uv: mat.back_render_flow.unwrap_or([0.0, 0.0, 1.0, 1.0]),
                        radial: mat.back_render_radial.unwrap_or([0.5, 0.5, 0.0, 0.0]),
                        main: mat.back_render_main.unwrap_or([1.0, 1.0, 1.0, 1.0]),
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
                    fairy_ball: mat.fairy_ball.then(|| FairyBall {
                        matcap: mat.fairy_matcap_tex.map(|rel| root.join(rel)),
                        base_color: mat.fairy_base.unwrap_or([1.0, 1.0, 1.0, 0.0]),
                        matcap_color: mat
                            .fairy_matcap_color
                            .unwrap_or([1.0, 1.0, 1.0, 0.1]),
                        rim_dark: mat.fairy_rim_dark.unwrap_or([1.0; 4]),
                        rim_light: mat.fairy_rim_light.unwrap_or([1.0; 4]),
                        main_color: mat.fairy_main.unwrap_or([1.0; 4]),
                        shape: mat.fairy_shape.unwrap_or([2.0, 0.05, 0.0, 1.0]),
                    }),
                },
            )
        })
        .collect()
}
