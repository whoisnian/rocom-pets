//! 炫彩(游戏里的 `MDT_GLASS`)的配色数据与渲染参数。
//!
//! # 这一层是什么
//!
//! 游戏的宠物变异是一组位标志(`Enum.MutationDiffType`),其中两位改的是外观:
//!
//! | 位 | 枚举 | 游戏文案 | 客户端入口 |
//! | --- | --- | --- | --- |
//! | 1 | `MDT_SHINING` | **异色** | `PetMutationUtils.SetColorDiffMutation` |
//! | 8 | `MDT_GLASS` | **炫彩** | `PetMutationUtils.SetGlassyDiffMutation` |
//!
//! **这两位是独立的**,不是三选一:游戏里既有异色炫彩,也有原色炫彩。所以 `Mutation`
//! 是一个带两个字段的结构体,而不是枚举。
//!
//! **异色的渲染不在这个模块里** —— 它不是着色,是**整套材质替换**:美术为那只宠物另做了
//! 一份材质(资产目录下的 `Yise/Mat/MI_…_101_*`),客户端把网格每个槽位的材质换成那一份。
//! 所以异色走的是导出器 + `Model` 那条既有路,包里多一套材质而已,见 `Form::has_shiny`。
//!
//! 炫彩才是这里的事:它**不换材质**,而是在原材质上打开一个动态开关 `GlassySwitch`,
//! 再覆盖几个参数。开关一开,shader 走另一条分支 —— 那条分支就是下面复刻的东西。
//!
//! # 客户端怎么打开它
//!
//! `PetMutationUtils.SetGlassyDiffMutation` 对后缀 `by`/`by0..by9` 的材质做:
//!
//! ```lua
//! mat:SetSwitchParameterValue("GlassySwitch", true, mesh, false)
//! mat:SetVectorParameterValue("RedChannel",   colorA)    -- COLOR_RANDOM_CONF.mat_color_1
//! mat:SetVectorParameterValue("GreenChannel", colorB)    -- COLOR_RANDOM_CONF.mat_color_2
//! mat:SetScalarParameterValue("StarIntensity", strength) -- COLOR_RANDOM_CONF.shine_strength
//! mat:SetTextureParameterValue("StarStickTex", particle) -- PARTICLE_RANDOM_CONF.particle_res
//! mat:SetVectorParameterValue("MutationRimColor",       FLinearColor(0.6, 0.6, 0.6, 1))
//! mat:SetVectorParameterValue("MutationSpecularParams", FLinearColor(0.8, 0.3, 200, 0.2))
//! ```
//!
//! 配套的描边材质(本体材质的 `AdditionalMaterials`,就是同目录那份 `_Ol`)只吃
//! `GlassySwitch` + 两个 Channel 色 —— `processAdditionalMaterial` 里就这三行,
//! 贴图与平铺一概不动。**所以描边也会跟着变色**,见下面「描边那一支」。
//!
//! 隐藏/赛季款(`GT_HIDDEN`)在此之上再按 `HIDDEN_GLASS_CONF` 覆盖一整张清单:`MainTex`、
//! `StarStickTex`、`StickRandomColor01..04`、`GlobalRefraction`/`GlobalDepth`/
//! `MainTexFlowSpeedX,Y`/`MainTexTiling`/`NormalEffectAmount`/`BaseColorDetail`。
//!
//! # 那条 shader 分支
//!
//! `GlassySwitch` 是**动态开关**(本作自改引擎的 `DynamicSwitchParameters`),cook 时按
//! `DynamicSwitchId` 另存一份 resource,所以「开着」的那份是**编译好在包里的**,不用猜。
//! 定位办法见 docs/design.md「炫彩那条 shader 分支是怎么找到的」:
//! `MI_Ill_XingGuang1_001_By` 有 6 个动态开关排列,3 个开关(`GlassySwitch` /
//! `OpenPetFX` / `开启黑魔法效果`)按位组合,**id=1 就是只开炫彩那份**。
//!
//! 那份 PS 比默认那份多约 175 行、`cb6` 从 60 槽涨到 65 槽、多两张 2D 贴图(`MainTex` 与
//! `StarStickTex`)。主干逐条读下来是:
//!
//! 1. **折射**。按世界法线与视线做 `refract(V, N, GlobalRefraction)`,全反射(判别式 < 0)
//!    时整支置零;沿折射线推进 `GlobalDepth`,把落点变换到裁剪空间。
//! 2. **屏幕空间 UV**。落点的屏幕坐标**减去物体包围盒中心的屏幕坐标**、除以最大轴缩放,
//!    `× 0.25`,再 `× MainTexTiling + 0.5`,最后加 `frac(time × MainTexFlowSpeedXY)` 卷动。
//!    ——「相对物体中心」这一步是关键:它让花纹跟着宠物走而不是钉在屏幕上。
//! 3. **法线扰动**。UV 再按切线基下重构的法线偏移 `NormalEffectAmount`。
//! 4. **着色**(核心两行):
//!    ```text
//!    glass = RedChannel × MainTex.r + GreenChannel × MainTex.g
//!    glass = glass × StarIntensity                      // cb6[61].x
//!    ```
//! 5. **按底色亮度调制**。`m = pow(mean(基色链结果), BaseColorDetail)`,`mean ≤ 0` 时取 0,
//!    上限 1;`glass ×= m`。这是炫彩仍能看出原宠物明暗结构的原因。
//! 6. **星点层**。`StarStickTex` 按 `uv0 × StarStickTiling` 采样(**平铺是材质自己的那份**,
//!    不是 `PARTICLE_RANDOM_CONF` 里的 —— lua 只给随机蛋写那个标量,见
//!    [`GlassyParticle::star_stick_tiling`]),
//!    `k = 1.1 × lerp(|sin θ|, |cos θ|, tex.g)`(θ = `frac(time × 0.25) × 2π`)——
//!    **与本仓库既有的 `stick_layer` 是同一条公式**;覆盖率
//!    `t = saturate((tex.b × (k − tex.r) − 0.01) × 25)`,再过 `3t² − 2t³` 的平滑多项式,
//!    然后 `glass = lerp(glass, StarIntensity × 星色, 覆盖率)`。
//! 7. **边缘光**。`MutationRimColor` × 菲涅尔项,加进 glass。
//! 8. **合成**。`out = lerp(原着色, lerp(基色, glass, 星点覆盖率补项), BlendWeight)`。
//!
//! # 描边那一支
//!
//! `_Ol` 也有自己的 `GlassySwitch` 排列(鸭吉吉那份 6 条 resource = 2 个动态开关 ×
//! 2 档 LODUsed;`DSId=1` 就是只开炫彩那份,PS **52499**,比默认那份 58499 多 17 行、
//! 多一张 2D 贴图)。读下来它比本体那条简单得多:
//!
//! ```text
//! uv    = UV0 × GlassyUV.xy + frac(time × GlassyUV.zw)
//! glass = lerp(lerp(RedChannel × t.r, GreenChannel, t.g), BlueChannel, t.b)
//! out   = (MatID.a >= MinID) ? glass : OutLineOtherColor[挡位] × Outline Intensity
//! ```
//!
//! 没有折射、没有屏幕空间 UV、没有星贴层、也不按固有色亮度调制;区域门与本体那条
//! **同一张图同一个阈值**。四个输入全部写死在共享父材质 `MI_P_Outline` 上
//! (`GlassyUV = (1, 1, 0.08, 0.08)`、`BlueChannel` = 白、`MinID` = 0.4、
//! `MainTex` = `Tex_PetGlassy_007_D`),每份 `_Ol` 都继承 —— 鸭吉吉那份只覆盖了
//! 两个 `OutLineOtherColor` 和 `MatID` 贴图。
//!
//! **这解释了实机「炫彩宠物看不见原色描边」**:描边环在区域门里整片换成了玻璃色,
//! 和身体融在一起;门外(鸭吉吉的喙与脚)才是那条近黑的 `OutLineOtherColor`。
//!
//! **描边用的花纹图始终是共享的那张。** lua 对隐藏款只把 `tex_param` 写给**本体**材质
//! (`materialFunc`),描边那支只有两个 Channel 色 —— 所以隐藏款/赛季款的描边也用
//! `Tex_PetGlassy_007_D`,不是本体那张。运行时因此单开一条 binding。
//!
//! # 哪些数是读出来的、哪些还没定名
//!
//! 逐条对得上名字的:`RedChannel`(cb6[45])、`GreenChannel`(cb6[47])、
//! `GlobalRefraction`(cb6[58].z,refract 的 eta)、`GlobalDepth`(cb6[58].w,推进距离)、
//! `MainTexTiling`(cb6[59].y)、`MainTexFlowSpeedX/Y`(cb6[44].xy)、
//! `NormalEffectAmount`(cb6[60].x)、`StarStickTiling`(cb6[61].z)、
//! `StarIntensity`(cb6[61].w)、`MutationRimColor`(cb6[41])。
//! 三条独立证据同时指着同一组:汇编里的结构、lua 覆盖的参数名单、以及
//! `HIDDEN_GLASS_CONF` 每条参数自带的中文 `num_param_comment`(反射强度/纹理缩放深度/
//! X轴流动速度/法线/贴图缩放/颜色细节)。
//!
//! **还没定名的**:`cb6[49]`(星点色)、`cb6[62].x`(星点覆盖的偏置)、`cb6[62].y`
//! (整层混合系数,材质里 `BlendWeight` = 1.0)。这个材质实例的冻结块凑不出 65 槽的那份
//! (它只有 35 槽与 48 槽两种),所以槽位→名字这一步在这三个上是**按语义接的**,不是查出来的。
//! 常量注释里逐个标了。

use super::glassy_table::{COLORS, HIDDEN, PARTICLES};

/// 常规炫彩的一组配色。`red_channel`/`green_channel` 直接就是 shader 里那两个同名参数。
///
/// **不是 sRGB 颜色**:取值到 1.6,是线性空间里的 HDR 系数,`ui_color_*` 才是给人看的。
pub struct GlassyColor {
    pub id: u32,
    /// 游戏里的配色名,如「亮X暗 - 浅紫橙」。
    pub name: &'static str,
    pub red_channel: [f32; 3],
    pub green_channel: [f32; 3],
    /// 图鉴/UI 上代表这一组的两个色块(0xRRGGBB)。选色界面用它,别拿 `red_channel` 去画 ——
    /// 那是 HDR 系数,画出来会过曝。
    pub ui_color_1: u32,
    pub ui_color_2: u32,
    /// `StarIntensity`。全表恒为 10。
    pub shine_strength: f32,
}

/// 常规炫彩的粒子。四选一,决定 `StarStickTex`。
pub struct GlassyParticle {
    pub id: u32,
    /// 四角星 / 爱心 / 方块 / 镂空五角星。
    pub name: &'static str,
    /// 共享贴图名(不带目录与扩展名)。
    pub tex: &'static str,
    /// `PARTICLE_RANDOM_CONF.StarStickTiling`(方块/爱心/四角星 2.2、镂空五角星 1.0)。
    ///
    /// **宠物身上用不到这一条,原样转录配置表而已。** lua 里那句
    /// `starStickTiling = particleConf.StarStickTiling` 包在
    /// `if particleConf and PetMutationUtils.IsGlassyRandomEgg(petData)` 里 ——
    /// **只有随机蛋**(材质层参数那条路)才会写这个标量;宠物走的
    /// `processMaterial` 那一支连 `nil ~= starStickTiling` 都过不去,
    /// 于是材质自己的 `StarStickTiling` 原封不动地留着。
    /// 平铺因此逐材质取(鸭吉吉 `MI_Com_YaJiJi1_001_By` = 4.11、根默认 4),
    /// 见 `pack::MaterialSpec::glassy_star_tiling`。
    ///
    /// **接错这条的代价是看得见的**:2.2 对 4.11 差 1.87 倍,粒子因此大了近一倍、
    /// 密度只剩四分之一 —— 用户报的「粒子分布位置和大小密度与实机有出入」就是它。
    pub star_stick_tiling: f32,
}

/// 隐藏款要覆盖的标量。字段顺序与 `scripts/gen_glassy.py` 的 `SCALARS` 一致。
pub struct GlassyParams {
    /// `GlobalRefraction`。**注意不是 eta 本身** —— 汇编里那个槽的 preshader 是
    /// `1 / GlobalRefraction`(`const 1.0, param, Div`),见 [`GlassyParams::refraction_eta`]。
    /// 根默认 2.0,但**四只查过的宠物全都覆盖成 1.3**;铅字幻梦 0.0001 ⇒ eta 10000
    /// ⇒ 判别式恒负 ⇒ 整支折射被置零。
    pub global_refraction: f32,
    /// 沿折射线推进多远(厘米,**模型局部单位**)。根默认 30,查过的宠物一律 100。
    pub global_depth: f32,
    pub main_tex_flow_x: f32,
    pub main_tex_flow_y: f32,
    /// 花纹图的平铺。**逐材质差得很远**:加油海葵 `_By` 是 **0.2**,而鸭吉吉/火神/
    /// 白金独角兽都是根默认 1.5 —— 7.5 倍。小宠物的花纹之所以看着是整只一个颜色在扫,
    /// 就是这个数被美术调小了,不是相机或取景的事。
    pub main_tex_tiling: f32,
    pub normal_effect_amount: f32,
    /// 底色亮度的指数。越大 ⇒ 暗处越暗、玻璃色越只出现在亮处。
    pub base_color_detail: f32,
    /// 玻璃色总增益里的那个系数。增益 = `(base_color_detail + 1) × flow_color_intensity`,
    /// 见 [`GlassyParams::glass_gain`] 与 [`FLOW_COLOR_INTENSITY`]。
    pub flow_color_intensity: f32,
    /// 闪点层的格子大小(`StarTiling`,根默认 0.4)。见 pet.wgsl 的 `glassy_sparkle`。
    pub star_tiling: f32,
    /// 闪点层的密度(`StarDensity`,根默认 8)。格子边长 = `1 / (20 × StarTiling × StarDensity)`
    /// 的 UV 单位 —— 根默认下是 UV × 64,比星贴层(× 4)密得多,所以是「极小的亮点」。
    pub star_density: f32,
    /// 闪点层的亮度(`StarIntensity`)。**材质根默认是 1,但常规炫彩被 lua 覆盖成 10** ——
    /// 见 [`GlassyOverrides::star_intensity`]。
    pub star_intensity: f32,
}

impl GlassyParams {
    /// 从 manifest 的 `glassy_params` 那 8 个数还原;顺序与 `Materials.GlassyScalars` 一致。
    pub const fn from_pack(v: [f32; 11]) -> Self {
        Self {
            global_refraction: v[0],
            global_depth: v[1],
            main_tex_tiling: v[2],
            main_tex_flow_x: v[3],
            main_tex_flow_y: v[4],
            normal_effect_amount: v[5],
            base_color_detail: v[6],
            flow_color_intensity: v[7],
            star_tiling: v[8],
            star_density: v[9],
            star_intensity: v[10],
        }
    }

    /// 玻璃色的总增益(汇编里那个槽的 preshader = `(BaseColorDetail + 1) × FlowColorIntensity`)。
    pub fn glass_gain(&self) -> f32 {
        (self.base_color_detail + 1.0) * self.flow_color_intensity
    }

    /// `refract()` 的 eta(preshader = `1 / GlobalRefraction`)。
    /// 分母是配置里的值,可能小到 1e-4,除之前先兜一下底。
    pub fn refraction_eta(&self) -> f32 {
        1.0 / self.global_refraction.max(1e-6)
    }

    /// 套上隐藏款的覆盖。**没列到的那几条沿用材质自己的值,不是回根默认** ——
    /// lua 的 `num_param` 只写清单里那几个名字(狂欢怪谈就没列 `MainTexTiling`),
    /// 别的参数在材质上原封不动。
    pub fn with(&self, over: &GlassyOverrides) -> Self {
        Self {
            global_refraction: over.global_refraction.unwrap_or(self.global_refraction),
            global_depth: over.global_depth.unwrap_or(self.global_depth),
            main_tex_flow_x: over.main_tex_flow_x.unwrap_or(self.main_tex_flow_x),
            main_tex_flow_y: over.main_tex_flow_y.unwrap_or(self.main_tex_flow_y),
            main_tex_tiling: over.main_tex_tiling.unwrap_or(self.main_tex_tiling),
            normal_effect_amount: over
                .normal_effect_amount
                .unwrap_or(self.normal_effect_amount),
            base_color_detail: over.base_color_detail.unwrap_or(self.base_color_detail),
            flow_color_intensity: self.flow_color_intensity,
            star_tiling: self.star_tiling,
            star_density: self.star_density,
            star_intensity: over.star_intensity.unwrap_or(self.star_intensity),
        }
    }
}

/// 隐藏款 `num_param` 覆盖的那几条。**`None` = 这一款没列这个名字**,沿用材质自己的值。
///
/// 字段顺序与 `scripts/gen_glassy.py` 的 `SCALARS` 一致。四款各列了哪些见配置表:
/// 暗夜拾光 6 条、狂欢怪谈/黑白 5 条、铅字幻梦 6 条 —— 谁都没列全,
/// 所以「没列的回根默认」是错的,那会把加油海葵那种 `MainTexTiling = 0.2` 冲掉。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GlassyOverrides {
    /// **闪点层的亮度** —— 全表 39 组配色都写 10,隐藏款各自另写
    /// (暗夜拾光 1、狂欢怪谈与黑白 **0**、铅字幻梦没列 ⇒ 用材质自己的 1)。
    ///
    /// **这条曾被记成「实机空转」**:那是因为当时只读得到 quality=Low 那条排列,
    /// 里面确实没有 `StarIntensity`。高质量那条(鸭吉吉 `_By` 的 `[15]`,PS 5710)里它
    /// 是那层 Voronoi 闪点的亮度,见 pet.wgsl 的 `glassy_sparkle`。
    /// 实机对照直接印证:常规炫彩(10)身上一片极小的白亮点,而狂欢怪谈/黑白(0)
    /// 一个都没有 —— 奔波鼠与卡波两张截图同放大倍数一比就看得出。
    pub star_intensity: Option<f32>,
    pub global_refraction: Option<f32>,
    pub global_depth: Option<f32>,
    pub main_tex_flow_x: Option<f32>,
    pub main_tex_flow_y: Option<f32>,
    pub main_tex_tiling: Option<f32>,
    pub normal_effect_amount: Option<f32>,
    pub base_color_detail: Option<f32>,
}

/// 隐藏炫彩(常驻款与赛季款)。
pub struct HiddenGlass {
    pub id: u32,
    pub name: &'static str,
    /// `true` = 赛季款(`HIDDEN_GLASS_CONF.type == 2`)。赛季款只有 `season_pets` 里那几只
    /// 有专属贴图,别的宠物照样能上,只是走与常驻款相同的通用覆盖。
    pub season: bool,
    pub red_channel: [f32; 4],
    pub green_channel: [f32; 4],
    /// 覆盖 `MainTex`(共享贴图名)。常规炫彩不覆盖它,用材质自己的 `Tex_PetGlassy_007_D`。
    pub main_tex: &'static str,
    pub star_tex: &'static str,
    /// `StickRandomColor01..04` —— **星贴层四段渐变的四个色标**,不是四个离散色。
    /// 配置里没列出来的退回根默认(见 [`ROOT_STICK_RAMP`]),不是白。
    pub stick_colors: [[f32; 4]; 4],
    /// 这一款 `num_param` 覆盖的标量;没列的沿用**材质自己**那份。
    pub params: GlassyOverrides,
    /// 有专属贴图的宠物 `petbase_id`。
    pub season_pets: &'static [u32],
}

/// 材质自带的 `MainTex` —— 全库共享的红/绿双通道斑点图。常规炫彩就靠它出花纹。
pub const DEFAULT_MAIN_TEX: &str = "Tex_PetGlassy_007_D";

/// **区域门的阈值**(根材质的 `MinID`)。玻璃层整段包在 `if (MaskTex.a >= 0.4)` 里,
/// 门外原样输出原着色 —— 汇编 `ge r3.x, r3.w, l(4.0e-01)` / `if_nz` / `else mov r0.xyz, r7.xyzx`。
///
/// 这就是「游戏只给部位上色」的机制,见 `pack::Material::glassy_id_mask`。
pub const MIN_ID: f32 = 0.4;

/// 玻璃色的总增益里那个系数(根材质的 `FlowColorIntensity`)。
///
/// 汇编第 ④ 步乘的是 `cb6[61].x`,它的 preshader 是
/// **`(BaseColorDetail + 1) × FlowColorIntensity`**(`03<BaseColorDetail> 02<1.0> Add
/// 03<FlowColorIntensity> Mul`,在鸭吉吉与幽星光两份 resource 上逐字一致)。
/// 常规炫彩下就是 `(0.35 + 1) × 1.2 = 1.62`。
///
/// **这里原来接的是 `StarIntensity`(=10)**,于是整只被推到过曝白 —— 实机是柔和的两色渐变。
/// 见 [`GlassyRender::glass_gain`]。
pub const FLOW_COLOR_INTENSITY: f32 = 1.2;

/// 根材质 `M_P_Object` 的 `StarStickTiling` 默认值。
///
/// 炫彩的星贴层按 `uv0 × StarStickTiling` 采样,而这个标量**逐材质**:
/// 美术显式覆盖过就用那份(鸭吉吉 `MI_Com_YaJiJi1_001_By` = 4.11),没覆盖就是这个 4。
/// `PARTICLE_RANDOM_CONF` 里那个 2.2 **不是它** —— 那条只写给随机蛋,
/// 见 [`GlassyParticle::star_stick_tiling`]。
///
/// 旧包(没导 `glassy_star_tiling` 字段)退回这里。
pub const ROOT_STAR_STICK_TILING: f32 = 4.0;

/// 根材质的 `RimColor` 与 `RimIntensity` —— 玻璃层那圈边缘光。旧包没导这两个时兜底。
///
/// **和 lua 写的 `MutationRimColor` 不是一回事**:那个参数在这条排列的向量参数表里
/// 根本不存在(逐条查过),设了也没人读 —— 和当年的 `StarIntensity` 一个处境。
/// 实机读的是材质自己的 `RimColor`(鸭吉吉没覆盖 ⇒ 根默认 (0.844, 0.961, 1))
/// 与 `RimIntensity`(根默认 **1.5**)。
pub const ROOT_RIM: [f32; 4] = [0.84375, 0.961117, 1.0, 1.5];

/// 星点层的强度(根材质的 `Stick_Intensity`,汇编 `cb6[61].w`)。
/// 和既有 `stick_layer` 那条路读到的是同一个参数、同一个值。
pub const STICK_INTENSITY: f32 = 1.5;

/// 根材质 `M_P_Object` 的 `StickRandomColor01..04` —— 星贴层**四段渐变的色标**。
///
/// 和 pet.wgsl 里既有的 `STICK_RAMP_0..3` 是同一组数:那边是 `StarStickTex` 族的星贴层,
/// 炫彩这条排列采的也是 `StarStickTex`,**同一族同一条公式**。
///
/// 常规炫彩没人覆盖它们,所以就是这四个。实机验证:鸭吉吉那张截图里量到的方块颜色
/// 黄 (255,252,51) / 蓝 (116,148,240) / 紫 (201,155,255),对应 `ks` = 1.00 / 0.67 / **0.50**
/// —— 那个紫正好落在品红与蓝**之间的过渡段上**,离散地四选一取不出这个颜色,
/// 只有渐变取得出。这是「是渐变不是四选一」最硬的一条证据。
pub const ROOT_STICK_RAMP: [[f32; 4]; 4] = [
    [0.9462, 0.0636, 0.0214, 1.0],
    [0.9601, 0.1603, 0.9074, 1.0],
    [0.0489, 0.1545, 0.9774, 1.0],
    [0.9253, 0.7416, 0.0273, 1.0],
];

/// `MutationRimColor`,lua 里写死的。
pub const MUTATION_RIM_COLOR: [f32; 3] = [0.6, 0.6, 0.6];

/// 星点覆盖率的偏置(汇编 `cb6[62].x`)。材质根默认是 0,也就是覆盖率就是那条多项式本身。
///
/// **槽位没对上名字**,按语义接的:它同时出现在「星点混合系数」与「与基色的回混系数」
/// 两处,取 0 时后者退化成 1(= 玻璃层整片替换基色),与实机观感一致。
pub const STICK_COVER_BIAS: f32 = 0.0;

/// 整层混合系数(汇编 `cb6[62].y`)。材质里叫 `BlendWeight`,值 1.0 = 不衰减。
pub const GLASS_BLEND_WEIGHT: f32 = 1.0;

/// 玩家在配置窗口里选的外观。**异色与炫彩是两件独立的事,不是三选一。**
///
/// 游戏里这两位本来就能同时立(`MDT_SHINING | MDT_GLASS`):有异色炫彩,也有原色炫彩。
/// 两条路互不干涉 —— 异色换掉整套材质(包里已经导好了),炫彩往**当前这套**材质上刷一层。
/// 叠起来就是「给异色那套刷炫彩」,而刷在哪几个槽上的判据(`_by*` 后缀 + `M_P_Object` 父链)
/// 对两套材质同样成立,所以组合不必另写一条路。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mutation {
    /// 异色:换整套 `Yise/Mat/` 材质。只有包里带异色材质的形态给得出,见 `Form::has_shiny`。
    pub shiny: bool,
    /// 炫彩。`None` = 不上炫彩。
    pub glassy: Option<Glassy>,
}

/// 哪一种炫彩。常规款要自己挑配色与粒子,隐藏/赛季款是配好的一整套。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Glassy {
    /// 常规炫彩:39 组配色 × 4 种粒子。存的是**配色 id 与粒子 id**,和游戏编号一致。
    Common { color: u32, particle: u32 },
    /// 隐藏/赛季炫彩:`HIDDEN_GLASS_CONF.id`。
    Hidden { id: u32 },
}

/// 一次炫彩渲染要往 shader 里送的全部东西。异色不经过这里。
#[derive(Debug, Clone, PartialEq)]
pub struct GlassyRender {
    pub red_channel: [f32; 3],
    pub green_channel: [f32; 3],
    /// 这一款要盖在**材质自己那份标量**上的覆盖项(常规炫彩一条都不盖)。
    /// 实际送进 shader 的是 `material.glassy_params.with(&overrides)`。
    pub overrides: GlassyOverrides,
    /// **星贴层四段渐变的四个色标**(`StickRandomColor01..04`),每段 ⅓ 宽,
    /// 按每颗粒子自己的 `k` 取色 —— 所以粒子一边涨缩一边换色。
    /// 常规炫彩用根材质默认那四个,隐藏款按配置覆盖(没覆盖的仍退回根默认)。
    pub stick_ramp: [[f32; 4]; 4],
    /// 共享贴图名,运行时按名字到炫彩素材目录里取。
    pub main_tex: &'static str,
    pub star_tex: &'static str,
}

/// 根材质 `M_P_Object` 的标量默认值。**只在旧包(没导 `glassy_params`)时兜底** ——
/// 实机用的是材质实例自己那份,逐只都可能不同(见 [`GlassyParams::main_tex_tiling`])。
///
/// 数值来自 `exporter/RootDefaults.cs` 那条路(根图的 `CachedExpressionData`),
/// 并用 `MI_Ill_XingGuang1_001_By` 的冻结块逐条复核过 —— 那只宠物一个都没覆盖,
/// 所以块里读到的就是根默认。
pub const ROOT_PARAMS: GlassyParams = GlassyParams {
    global_refraction: 2.0,
    global_depth: 30.0,
    main_tex_flow_x: 0.0,
    main_tex_flow_y: 0.1,
    main_tex_tiling: 1.5,
    normal_effect_amount: 0.1,
    base_color_detail: 0.35,
    flow_color_intensity: FLOW_COLOR_INTENSITY,
    star_tiling: 0.4,
    star_density: 8.0,
    star_intensity: 1.0,
};

pub fn colors() -> &'static [GlassyColor] {
    &COLORS
}

pub fn particles() -> &'static [GlassyParticle] {
    &PARTICLES
}

pub fn hidden() -> &'static [HiddenGlass] {
    &HIDDEN
}

pub fn color(id: u32) -> Option<&'static GlassyColor> {
    COLORS.iter().find(|c| c.id == id)
}

pub fn particle(id: u32) -> Option<&'static GlassyParticle> {
    PARTICLES.iter().find(|p| p.id == id)
}

pub fn hidden_by_id(id: u32) -> Option<&'static HiddenGlass> {
    HIDDEN.iter().find(|h| h.id == id)
}

impl Glassy {
    /// 游戏协议里的 `glass_value` 打包法:`(粒子id << 20) | 配色id`。
    /// 隐藏款的 `glass_value` 直接就是 `HIDDEN_GLASS_CONF.id`,**两套编号不能混查**。
    pub fn glass_value(&self) -> u32 {
        match self {
            Glassy::Common { color, particle } => (particle << 20) | color,
            Glassy::Hidden { id } => *id,
        }
    }

    /// 从游戏的 `(glass_type, glass_value)` 还原。`glass_type`:1 = 常规、2 = 隐藏。
    pub fn from_glass_info(glass_type: u32, glass_value: u32) -> Option<Self> {
        match glass_type {
            1 => Some(Glassy::Common {
                color: glass_value & 0xf_ffff,
                particle: glass_value >> 20,
            }),
            2 => Some(Glassy::Hidden { id: glass_value }),
            _ => None,
        }
    }

    /// 给人看的名字,如「四角星 · 亮X暗 - 浅紫橙」。
    pub fn label(&self) -> String {
        match self {
            Glassy::Common { color, particle } => {
                let c = self::color(*color).map_or("?", |c| c.name);
                let p = self::particle(*particle).map_or("?", |p| p.name);
                format!("{p} · {c}")
            }
            Glassy::Hidden { id } => {
                hidden_by_id(*id).map_or_else(|| format!("隐藏炫彩 {id}"), |h| h.name.to_string())
            }
        }
    }

    /// `炫彩:` 后面那一段。常规炫彩写**编号**而不是名字:名字是配置表里的展示文本,
    /// 换版本可能改;编号是协议里的东西,而且和游戏 UI 上看到的一致,对得上账。
    /// 隐藏款反过来写名字 —— 那四条的 id(1/2/3/**1000**)没有规律,写名字才看得懂。
    fn config_part(&self) -> String {
        match self {
            Glassy::Common { color, particle } => format!("{particle}/{color}"),
            Glassy::Hidden { id } => {
                hidden_by_id(*id).map_or_else(|| id.to_string(), |h| h.name.to_string())
            }
        }
    }

    fn parse_part(rest: &str) -> Option<Self> {
        if let Some((particle, color)) = rest.split_once('/') {
            return Some(Glassy::Common {
                color: color.trim().parse().ok()?,
                particle: particle.trim().parse().ok()?,
            });
        }
        // 隐藏款:先按名字找,再容一手直接写 id 的。
        HIDDEN
            .iter()
            .find(|h| h.name == rest)
            .map(|h| Glassy::Hidden { id: h.id })
            .or_else(|| {
                rest.parse()
                    .ok()
                    .filter(|id| hidden_by_id(*id).is_some())
                    .map(|id| Glassy::Hidden { id })
            })
    }

    /// 这只宠物穿这一款炫彩时,该不该换成**赛季专属贴图**。
    ///
    /// 只有赛季款、而且这只在它的 `season_pet` 名单里才算数 —— 名单外的宠物走的是
    /// 与常驻款相同的通用覆盖(客户端那个 `bSeasonButNotCustomPet` 分支)。
    pub fn uses_season_art(&self, petbase_id: i64) -> bool {
        let Glassy::Hidden { id } = self else {
            return false;
        };
        hidden_by_id(*id).is_some_and(|h| {
            h.season && h.season_pets.iter().any(|p| i64::from(*p) == petbase_id)
        })
    }

    /// 解析成 shader 输入。
    pub fn render(&self) -> Option<GlassyRender> {
        match self {
            Glassy::Common { color, particle } => {
                let c = self::color(*color)?;
                let p = self::particle(*particle)?;
                Some(GlassyRender {
                    red_channel: c.red_channel,
                    green_channel: c.green_channel,
                    // 常规炫彩一条标量都不覆盖 —— lua 只写两个 Channel 色、
                    // `StarIntensity`(实机空转)与粒子贴图,别的全用材质自己那份。
                    overrides: GlassyOverrides {
                        star_intensity: Some(c.shine_strength),
                        ..GlassyOverrides::default()
                    },
                    stick_ramp: ROOT_STICK_RAMP,
                    main_tex: DEFAULT_MAIN_TEX,
                    star_tex: p.tex,
                })
            }
            Glassy::Hidden { id } => {
                let h = hidden_by_id(*id)?;
                Some(GlassyRender {
                    red_channel: [h.red_channel[0], h.red_channel[1], h.red_channel[2]],
                    green_channel: [h.green_channel[0], h.green_channel[1], h.green_channel[2]],
                    overrides: h.params.clone(),
                    stick_ramp: h.stick_colors,
                    main_tex: h.main_tex,
                    star_tex: h.star_tex,
                })
            }
        }
    }
}

impl Mutation {
    /// 什么都没选 —— 按包里原样画。
    pub fn is_plain(&self) -> bool {
        !self.shiny && self.glassy.is_none()
    }

    /// 画成这样要用到哪几张**共享贴图**(不带目录与扩展名)。
    ///
    /// **异色不在其中**:那是包里另一套材质,贴图跟着形态一起下,不走共享素材这条路。
    ///
    /// 浏览器版按这份名单去 `fetch`(见 [`shared`]);桌面版用不着 ——
    /// 烘进来的是全部 13 张,要么都在要么都不在。
    pub fn shared_assets(&self) -> Vec<&'static str> {
        let Some(glassy) = self.glassy else {
            return Vec::new();
        };
        let Some(render) = glassy.render() else {
            return Vec::new();
        };
        let mut out = vec![render.main_tex, render.star_tex];
        // **共享的那张 `MainTex` 无论选哪一款都要**:描边那一支用的始终是它
        // (`MI_P_Outline` 自己的 `MainTex`,lua 不覆盖),隐藏款的本体贴图顶不了它。
        // 赛季款还多一条理由:`SeasonMutation` 那一族里 `_By1`(手臂/肩甲/尖塔)
        // 不自带花纹图,也退回这张(见 `Model::load` 里 `flow_noise` 的 `or_else`)。
        if !out.contains(&DEFAULT_MAIN_TEX) {
            out.push(DEFAULT_MAIN_TEX);
        }
        out.dedup();
        out
    }

    /// 这几张里还缺哪些。空 = 现在就画得出来。
    pub fn missing_assets(&self) -> Vec<&'static str> {
        self.shared_assets()
            .into_iter()
            .filter(|name| !has_shared(name))
            .collect()
    }

    /// 给人看的名字,如「异色 · 四角星 · 亮X暗 - 浅紫橙」。
    pub fn label(&self) -> String {
        match (self.shiny, self.glassy) {
            (false, None) => "原样".to_string(),
            (true, None) => "异色".to_string(),
            (false, Some(g)) => g.label(),
            (true, Some(g)) => format!("异色 · {}", g.label()),
        }
    }

    /// 存进 `roster.toml` 的写法。两个轴各写一段,同时带就用 `+` 接起来:
    ///
    /// ```toml
    /// mutation = "异色"
    /// mutation = "炫彩:3/33"        # 粒子 3(方块)· 配色 33(亮X暗 - 浅紫橙)
    /// mutation = "炫彩:黑白"
    /// mutation = "异色+炫彩:黑白"   # 异色炫彩;不写异色就是原色炫彩
    /// ```
    ///
    /// 原样返回 `None` —— 默认值一律不写进存档,见 `Slot` 的说明。
    pub fn to_config(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.shiny {
            parts.push("异色".to_string());
        }
        if let Some(g) = self.glassy {
            parts.push(format!("炫彩:{}", g.config_part()));
        }
        (!parts.is_empty()).then(|| parts.join("+"))
    }

    /// `to_config` 的逆。认不出来返回 `None` —— 调用方该**报错或警告**而不是默默按原样画:
    /// 配置里拼错了要让人看见。同一个轴写两遍也算认不出来:那多半是手抖,
    /// 猜「以后面那个为准」不如直接说这行有问题。
    pub fn from_config(s: &str) -> Option<Self> {
        let mut out = Self::default();
        for part in s.split('+') {
            let part = part.trim();
            if part == "异色" {
                if out.shiny {
                    return None;
                }
                out.shiny = true;
            } else {
                let rest = part.strip_prefix("炫彩:")?;
                if out.glassy.is_some() {
                    return None;
                }
                out.glassy = Some(Glassy::parse_part(rest.trim())?);
            }
        }
        (!out.is_plain()).then_some(out)
    }
}

/// 构建期烘进来的炫彩贴图(`build.rs` 生成)—— **运行时唯一的素材来源**。
///
/// 炫彩要覆盖的两张贴图是全库共用的,既不进宠物包(塞进 201 个包要多背 120MB),
/// 也不在运行时找目录:找目录意味着「装好了还得再摆一份素材」,而这一层的东西只有 3.5MB,
/// 烘进来就没这一步了。导出器在正常导包时把它们写到 `<out>/glassy`,`build.rs` 构建时读那儿。
///
/// 素材不在仓库里 —— 烘的是**构建那台机器上自己导出来的那一份**,没有就是空表
/// (那时炫彩那几档在界面上是灰的)。见 build.rs 的模块头。
mod embed {
    include!(concat!(env!("OUT_DIR"), "/glassy_embed.rs"));
}

/// 烘进来了几张。0 = 这个二进制没带素材,只能读目录。
pub fn embedded_count() -> usize {
    embed::EMBEDDED.len()
}

/// 按名字取一张烘进来的贴图(不带目录与扩展名,如 `Tex_PetGlassyStar_003`)。
pub fn embedded(name: &str) -> Option<&'static [u8]> {
    embed::EMBEDDED
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, bytes)| *bytes)
}

/// 浏览器里那份共享素材:**运行时喂进来**,不烘进 wasm。
///
/// 网页预览是点开才下的一个 chunk,把 13 张图(3.6MB)烘进去等于让每个点开预览的人
/// 都先付这 3.6MB —— 而其中最大的一张(铅字幻梦的流动噪声)自己就有 2MB,多数人一次
/// 也用不上。改成**按需取**:前端问 [`Mutation::missing_assets`] 要名单、`fetch` 回来喂 `put_shared`,
/// 挑一次常规炫彩只多下两张(约 250KB)。
///
/// 和包一样是「谁部署谁提供」:导出器把这些图写在 `<out>/glassy`,部署时和 `packs/`
/// 一起传上去(见 web/README.md)。没传就取不到,前端把炫彩那几档禁掉。
#[cfg(target_arch = "wasm32")]
mod runtime_store {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::OnceLock;

    fn table() -> &'static Mutex<HashMap<String, Vec<u8>>> {
        static TABLE: OnceLock<Mutex<HashMap<String, Vec<u8>>>> = OnceLock::new();
        TABLE.get_or_init(Default::default)
    }

    pub fn put(name: &str, bytes: Vec<u8>) {
        if let Ok(mut map) = table().lock() {
            map.insert(name.to_string(), bytes);
        }
    }

    pub fn get(name: &str) -> Option<Vec<u8>> {
        table().lock().ok()?.get(name).cloned()
    }

    pub fn has(name: &str) -> bool {
        table().lock().is_ok_and(|map| map.contains_key(name))
    }
}

/// 喂一张共享贴图进来(浏览器专用,见 [`runtime_store`])。
#[cfg(target_arch = "wasm32")]
pub fn put_shared(name: &str, bytes: Vec<u8>) {
    runtime_store::put(name, bytes);
}

/// 按名字取一张共享贴图,从构建期烘进来的表里借。
///
/// **桌面版和浏览器版唯一的分岔就是这一对函数** —— 上面那两条加载路径都只认这个入口。
#[cfg(not(target_arch = "wasm32"))]
pub fn shared(name: &str) -> Option<std::borrow::Cow<'static, [u8]>> {
    embedded(name).map(std::borrow::Cow::Borrowed)
}

/// 同上,浏览器版:从运行时喂进来的表里拷一份。
#[cfg(target_arch = "wasm32")]
pub fn shared(name: &str) -> Option<std::borrow::Cow<'static, [u8]>> {
    runtime_store::get(name).map(std::borrow::Cow::Owned)
}

/// 这张在不在手上。**不要用 `shared(..).is_some()` 代替** —— 浏览器那边它会把
/// 整张图拷一份出来,只为回答一个 bool。
#[cfg(not(target_arch = "wasm32"))]
pub fn has_shared(name: &str) -> bool {
    embedded(name).is_some()
}

/// 同上,浏览器版。
#[cfg(target_arch = "wasm32")]
pub fn has_shared(name: &str) -> bool {
    runtime_store::has(name)
}

/// 常规炫彩要用到的贴图名(花纹 + 四种粒子)。隐藏款各带一对,单独查。
fn common_assets() -> impl Iterator<Item = &'static str> {
    std::iter::once(DEFAULT_MAIN_TEX).chain(PARTICLES.iter().map(|p| p.tex))
}

/// 素材齐不齐。桌面版看烘进来的那份(运行时不再找任何目录,见 `embed` 的说明),
/// 浏览器版看已经喂进来的那份。
///
/// 不齐就该把炫彩那几档在界面上禁掉并说清楚为什么:让用户看着一个点不出效果的选项,
/// 比直接说「这个二进制没带炫彩素材」更糟。
pub fn assets_ready() -> bool {
    common_assets().all(has_shared)
}

impl Clone for GlassyParams {
    fn clone(&self) -> Self {
        *self
    }
}

impl Copy for GlassyParams {}

impl std::fmt::Debug for GlassyParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlassyParams")
            .field("global_refraction", &self.global_refraction)
            .field("global_depth", &self.global_depth)
            .field("main_tex_tiling", &self.main_tex_tiling)
            .finish_non_exhaustive()
    }
}

impl PartialEq for GlassyParams {
    fn eq(&self, other: &Self) -> bool {
        self.global_refraction == other.global_refraction
            && self.global_depth == other.global_depth
            && self.main_tex_flow_x == other.main_tex_flow_x
            && self.main_tex_flow_y == other.main_tex_flow_y
            && self.main_tex_tiling == other.main_tex_tiling
            && self.normal_effect_amount == other.normal_effect_amount
            && self.base_color_detail == other.base_color_detail
            && self.flow_color_intensity == other.flow_color_intensity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_complete() {
        assert_eq!(COLORS.len(), 39, "COLOR_RANDOM_CONF 是 39 条");
        assert_eq!(PARTICLES.len(), 4);
        assert_eq!(HIDDEN.len(), 4);
        // 常驻款只有「黑白」一条,其余三条是赛季款。
        assert_eq!(HIDDEN.iter().filter(|h| !h.season).count(), 1);
    }

    /// 网页预览按这份名单去取共享贴图(桌面版烘的是全部 13 张,用不着这个)。
    ///
    /// 三条各有各的道理:**异色一张都不要**(那是包里另一套材质,跟着形态一起下);
    /// 常规炫彩只要花纹 + 挑中那种粒子**两张**(整份 3.6MB 里的 250KB);
    /// 赛季款要**三张** —— 多的那张是 `SeasonMutation` 那一族里 `_By1`(手臂/肩甲/尖塔)
    /// 不自带花纹图时退回的共享那张,漏了它机幕方舟的胳膊上会缺一块。
    #[test]
    fn only_the_textures_this_look_needs() {
        let shiny = Mutation {
            shiny: true,
            glassy: None,
        };
        assert!(shiny.shared_assets().is_empty(), "异色不走共享素材");

        let common = Mutation {
            shiny: false,
            glassy: Some(Glassy::Common {
                color: 1,
                particle: 1,
            }),
        };
        assert_eq!(
            common.shared_assets(),
            vec![DEFAULT_MAIN_TEX, particle(1).expect("有 1 号粒子").tex]
        );

        let season = HIDDEN.iter().find(|h| h.season).expect("有赛季款");
        let dressed = Mutation {
            shiny: true,
            glassy: Some(Glassy::Hidden { id: season.id }),
        };
        assert_eq!(
            dressed.shared_assets(),
            vec![season.main_tex, season.star_tex, DEFAULT_MAIN_TEX],
            "隐藏款也要带共享花纹图:描边那一支用的是它"
        );

        let plain = HIDDEN.iter().find(|h| !h.season).expect("有常驻款");
        assert_eq!(
            Mutation {
                shiny: false,
                glassy: Some(Glassy::Hidden { id: plain.id }),
            }
            .shared_assets()
            .len(),
            3,
            "常驻款自带一整套,但描边那一支仍要共享的那张花纹图"
        );
    }

    /// 打包/解包要和客户端 `PetUtils.GetShineDataValue` 逐位一致 ——
    /// docs 里那个例子:1048609 = 四角星(1) · 亮X暗 - 浅紫橙(33)。
    #[test]
    fn glass_value_roundtrip() {
        let g = Glassy::Common {
            color: 33,
            particle: 1,
        };
        assert_eq!(g.glass_value(), 1_048_609);
        assert_eq!(Glassy::from_glass_info(1, 1_048_609), Some(g));
        assert_eq!(g.label(), "四角星 · 亮X暗 - 浅紫橙");
    }

    #[test]
    fn common_uses_shared_main_tex_and_no_stick_colors() {
        let r = Glassy::Common {
            color: 1,
            particle: 3,
        }
        .render()
        .expect("1 号配色 3 号粒子都在表里");
        assert_eq!(r.main_tex, DEFAULT_MAIN_TEX);
        assert_eq!(r.star_tex, "Tex_PetGlassyStar_001");
        assert_eq!(r.overrides.star_intensity, Some(10.0));
        // 常规炫彩**一条标量都不覆盖** —— 那几个数要用材质自己那份。
        assert_eq!(r.overrides.global_refraction, None);
        assert_eq!(r.overrides.global_depth, None);
        assert_eq!(r.overrides.main_tex_tiling, None);
    }

    #[test]
    fn hidden_overrides_its_own_textures_and_scalars() {
        let r = Glassy::Hidden { id: 3 }.render().expect("铅字幻梦");
        assert_eq!(r.main_tex, "T_PetGlassyNoiseS3_001");
        // 铅字幻梦把折射压到近乎为零、深度提到 100、贴图放大三倍。
        assert_eq!(r.overrides.global_refraction, Some(0.0001));
        assert_eq!(r.overrides.global_depth, Some(100.0));
        assert_eq!(r.overrides.main_tex_tiling, Some(3.0));
    }

    #[test]
    fn config_strings_round_trip() {
        let common = Glassy::Common {
            color: 33,
            particle: 3,
        };
        for m in [
            Mutation {
                shiny: true,
                glassy: None,
            },
            Mutation {
                shiny: false,
                glassy: Some(common),
            },
            Mutation {
                shiny: true,
                glassy: Some(common),
            },
            Mutation {
                shiny: true,
                glassy: Some(Glassy::Hidden { id: 1000 }),
            },
            Mutation {
                shiny: false,
                glassy: Some(Glassy::Hidden { id: 3 }),
            },
        ] {
            let text = m.to_config().expect("非原样一定写得出来");
            assert_eq!(Mutation::from_config(&text), Some(m), "{text}");
        }
        // 写法要看得懂 —— 这几条是文档里给用户看的样子。
        assert_eq!(
            Mutation {
                shiny: false,
                glassy: Some(common)
            }
            .to_config()
            .as_deref(),
            Some("炫彩:3/33")
        );
        assert_eq!(
            Mutation {
                shiny: true,
                glassy: Some(Glassy::Hidden { id: 1000 })
            }
            .to_config()
            .as_deref(),
            Some("异色+炫彩:黑白")
        );
        // 原样不落盘。
        assert_eq!(Mutation::default().to_config(), None);
        // 认不出来的要说不认识,不能悄悄退成「没有变异」。
        assert_eq!(Mutation::from_config("炫彩:不存在的款"), None);
        assert_eq!(Mutation::from_config("闪光"), None);
        assert_eq!(Mutation::from_config(""), None);
        // 同一个轴写两遍 = 这行有问题,不猜哪个算数。
        assert_eq!(Mutation::from_config("异色+异色"), None);
        assert_eq!(Mutation::from_config("炫彩:黑白+炫彩:3/33"), None);
    }

    /// 第 ④ 步的乘数与 refract 的 eta 都是 **preshader 算出来的派生量**,不是参数本身。
    /// 这两条各自对应一次实测读错,数值锁在这里免得再滑回去:
    /// `cb6[61].x = (BaseColorDetail + 1) × FlowColorIntensity`、`cb6[58].z = 1 / GlobalRefraction`。
    #[test]
    fn derived_scalars_match_the_preshaders() {
        let common = Glassy::Common {
            color: 1,
            particle: 1,
        }
        .render()
        .expect("1 号配色 1 号粒子都在表里");
        // 常规炫彩一条都不覆盖 ⇒ 拿材质自己那份算;这里用根默认代表「没覆盖过的材质」。
        let p = ROOT_PARAMS.with(&common.overrides);
        assert_eq!(p.base_color_detail, 0.35);
        assert!((p.glass_gain() - 1.62).abs() < 1e-6, "{}", p.glass_gain());
        assert!((p.refraction_eta() - 0.5).abs() < 1e-6);
        // **不是 StarIntensity**:那个在这条排列里根本不存在,接上去是 10 倍,整只过曝白。
        assert_eq!(common.overrides.star_intensity, Some(10.0));
        assert!(p.glass_gain() < common.overrides.star_intensity.expect("表里写了 10"));

        // 加油海葵那种把 `MainTexTiling` 调到 0.2 的材质,常规炫彩下要原样保留。
        let haikui = GlassyParams {
            main_tex_tiling: 0.2,
            global_refraction: 1.3,
            global_depth: 100.0,
            ..ROOT_PARAMS
        };
        let p = haikui.with(&common.overrides);
        assert_eq!(p.main_tex_tiling, 0.2);
        assert_eq!(p.global_depth, 100.0);

        // 铅字幻梦把 GlobalRefraction 压到 1e-4 ⇒ eta 一万 ⇒ 判别式恒负 ⇒ 折射整支置零。
        let qz = Glassy::Hidden { id: 3 }.render().expect("铅字幻梦");
        let q = haikui.with(&qz.overrides);
        assert!(q.refraction_eta() > 1000.0, "{}", q.refraction_eta());
        // 它的 BaseColorDetail 是 0.3,增益跟着走。
        assert!((q.glass_gain() - 1.3 * FLOW_COLOR_INTENSITY).abs() < 1e-6);
        // **没列的那几条沿用材质自己的**:狂欢怪谈没写 `MainTexTiling`,不能回根默认 1.5。
        let kh = Glassy::Hidden { id: 2 }.render().expect("狂欢怪谈");
        assert_eq!(kh.overrides.main_tex_tiling, None);
        assert_eq!(haikui.with(&kh.overrides).main_tex_tiling, 0.2);
    }

    /// 赛季传说精灵认人:只有赛季款、而且这只在名单里才换专属贴图。
    #[test]
    fn season_art_only_for_the_listed_pets() {
        let qz = Glassy::Hidden { id: 3 }; // 铅字幻梦,名单 3230..3233
        assert!(qz.uses_season_art(3232), "加尔在名单里");
        assert!(!qz.uses_season_art(3438), "梦游不在名单里,走通用覆盖");
        // 常驻款(黑白)没有赛季专属这回事。
        assert!(!Glassy::Hidden { id: 1000 }.uses_season_art(3232));
        // 常规炫彩更不会。
        assert!(!Glassy::Common {
            color: 1,
            particle: 1
        }
        .uses_season_art(3232));
    }

    /// 星点色是**四段渐变**,不是四选一 —— 每颗粒子按自己的 `k` 取色,所以一边涨缩
    /// 一边换色。这条钉住「常规炫彩用根默认那四个色标、隐藏款用自己的」。
    #[test]
    fn stick_ramp_is_a_four_stop_gradient() {
        let common = Glassy::Common {
            color: 21,
            particle: 3,
        }
        .render()
        .expect("21 号配色 3 号粒子都在表里");
        assert_eq!(common.stick_ramp, ROOT_STICK_RAMP);

        // 铅字幻梦覆盖 02/03/04,**01 没列出来 ⇒ 退回根默认**(不是白)。
        let qz = Glassy::Hidden { id: 3 }.render().expect("铅字幻梦");
        assert_eq!(qz.stick_ramp[0], ROOT_STICK_RAMP[0]);
        assert_eq!(qz.stick_ramp[1], [0.67, 1.0, 0.49, 1.0]);
        assert_ne!(qz.stick_ramp[2], ROOT_STICK_RAMP[2]);

        // 暗夜拾光四个全覆盖,一个都不该落回根默认。
        let ay = Glassy::Hidden { id: 1 }.render().expect("暗夜拾光");
        assert_eq!(ay.stick_ramp[0], [1.0, 0.0, 0.7, 1.0]);
    }

    /// 异色与炫彩互不影响:异色自己不产生玻璃层,而且两个都开时玻璃层照样出。
    #[test]
    fn shiny_and_glassy_are_independent() {
        let shiny_only = Mutation {
            shiny: true,
            glassy: None,
        };
        assert!(shiny_only.glassy.is_none());
        assert!(!shiny_only.is_plain());

        let both = Mutation {
            shiny: true,
            glassy: Some(Glassy::Hidden { id: 3 }),
        };
        assert!(both.glassy.and_then(|g| g.render()).is_some());
        assert_eq!(both.label(), "异色 · 铅字幻梦");
    }
}
