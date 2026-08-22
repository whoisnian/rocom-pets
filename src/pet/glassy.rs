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
//! **异色不在这个模块里** —— 它不是着色,是**整套材质替换**:美术为那只宠物另做了一份材质
//! (资产目录下的 `Yise/Mat/MI_…_101_*`),客户端把网格每个槽位的材质换成那一份。所以异色走
//! 的是导出器 + `Model` 那条既有路,包里多一套材质而已,见 `PackForm::shiny`。
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
//! 配套的描边材质只吃 `GlassySwitch` + 两个 Channel 色。
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
//! 6. **星点层**。`StarStickTex` 按 `uv0 × StarStickTiling` 采样,
//!    `k = 1.1 × lerp(|sin θ|, |cos θ|, tex.g)`(θ = `frac(time × 0.25) × 2π`)——
//!    **与本仓库既有的 `stick_layer` 是同一条公式**;覆盖率
//!    `t = saturate((tex.b × (k − tex.r) − 0.01) × 25)`,再过 `3t² − 2t³` 的平滑多项式,
//!    然后 `glass = lerp(glass, StarIntensity × 星色, 覆盖率)`。
//! 7. **边缘光**。`MutationRimColor` × 菲涅尔项,加进 glass。
//! 8. **合成**。`out = lerp(原着色, lerp(基色, glass, 星点覆盖率补项), BlendWeight)`。
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

/// 常规炫彩的粒子。四选一,决定 `StarStickTex` 与 `StarStickTiling`。
pub struct GlassyParticle {
    pub id: u32,
    /// 四角星 / 爱心 / 方块 / 镂空五角星。
    pub name: &'static str,
    /// 共享贴图名(不带目录与扩展名)。
    pub tex: &'static str,
    pub star_stick_tiling: f32,
}

/// 隐藏款要覆盖的标量。字段顺序与 `scripts/gen_glassy.py` 的 `SCALARS` 一致。
pub struct GlassyParams {
    pub star_intensity: f32,
    /// `refract()` 的 eta。根默认 2.0;铅字幻梦压到 0.0001 ≈ 不折射。
    pub global_refraction: f32,
    /// 沿折射线推进多远(厘米,游戏单位)。根默认 30,隐藏款一律 100。
    pub global_depth: f32,
    pub main_tex_flow_x: f32,
    pub main_tex_flow_y: f32,
    pub main_tex_tiling: f32,
    pub normal_effect_amount: f32,
    /// 底色亮度的指数。越大 ⇒ 暗处越暗、玻璃色越只出现在亮处。
    pub base_color_detail: f32,
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
    /// `StickRandomColor01..04`。**只有隐藏款覆盖这四个**,常规炫彩一个都不动。
    pub stick_colors: [[f32; 4]; 4],
    pub params: GlassyParams,
    /// 有专属贴图的宠物 `petbase_id`。
    pub season_pets: &'static [u32],
}

/// 材质自带的 `MainTex` —— 全库共享的红/绿双通道斑点图。常规炫彩就靠它出花纹。
pub const DEFAULT_MAIN_TEX: &str = "Tex_PetGlassy_007_D";

/// `MutationRimColor`,lua 里写死的。
pub const MUTATION_RIM_COLOR: [f32; 3] = [0.6, 0.6, 0.6];

/// 星点覆盖率的偏置(汇编 `cb6[62].x`)。材质根默认是 0,也就是覆盖率就是那条多项式本身。
///
/// **槽位没对上名字**,按语义接的:它同时出现在「星点混合系数」与「与基色的回混系数」
/// 两处,取 0 时后者退化成 1(= 玻璃层整片替换基色),与实机观感一致。
pub const STICK_COVER_BIAS: f32 = 0.0;

/// 整层混合系数(汇编 `cb6[62].y`)。材质里叫 `BlendWeight`,值 1.0 = 不衰减。
pub const GLASS_BLEND_WEIGHT: f32 = 1.0;

/// 玩家在配置窗口里选的外观。`None` = 原样。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mutation {
    /// 异色:换整套 `Yise/Mat/` 材质。只有包里带异色材质的形态给得出。
    Shiny,
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
    /// 四段渐变色。常规炫彩用不上(留白),隐藏款按 `StickRandomColor01..04` 填。
    pub stick_colors: [[f32; 4]; 4],
    pub use_stick_colors: bool,
    pub params: GlassyParams,
    pub star_stick_tiling: f32,
    /// 共享贴图名,运行时按名字到炫彩素材目录里取。
    pub main_tex: &'static str,
    pub star_tex: &'static str,
}

/// 根材质 `M_P_Object` 的标量默认值。常规炫彩只覆盖 `StarIntensity`,其余全用这一份。
///
/// 数值来自 `exporter/RootDefaults.cs` 那条路(根图的 `CachedExpressionData`),
/// 并用 `MI_Ill_XingGuang1_001_By` 的冻结块逐条复核过 —— 那只宠物一个都没覆盖,
/// 所以块里读到的就是根默认。
pub const ROOT_PARAMS: GlassyParams = GlassyParams {
    star_intensity: 1.0,
    global_refraction: 2.0,
    global_depth: 30.0,
    main_tex_flow_x: 0.0,
    main_tex_flow_y: 0.1,
    main_tex_tiling: 1.5,
    normal_effect_amount: 0.1,
    base_color_detail: 0.35,
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

impl Mutation {
    /// 游戏协议里的 `glass_value` 打包法:`(粒子id << 20) | 配色id`。
    /// 只对常规炫彩有意义 —— 隐藏款的 `glass_value` 直接就是 `HIDDEN_GLASS_CONF.id`,
    /// **两套编号不能混查**。
    pub fn glass_value(&self) -> Option<u32> {
        match self {
            Mutation::Common { color, particle } => Some((particle << 20) | color),
            Mutation::Hidden { id } => Some(*id),
            Mutation::Shiny => None,
        }
    }

    /// 从游戏的 `(glass_type, glass_value)` 还原。`glass_type`:1 = 常规、2 = 隐藏。
    pub fn from_glass_info(glass_type: u32, glass_value: u32) -> Option<Self> {
        match glass_type {
            1 => Some(Mutation::Common {
                color: glass_value & 0xf_ffff,
                particle: glass_value >> 20,
            }),
            2 => Some(Mutation::Hidden { id: glass_value }),
            _ => None,
        }
    }

    /// 给人看的名字,如「四角星 · 亮X暗 - 浅紫橙」。
    pub fn label(&self) -> String {
        match self {
            Mutation::Shiny => "异色".to_string(),
            Mutation::Common { color, particle } => {
                let c = self::color(*color).map_or("?", |c| c.name);
                let p = self::particle(*particle).map_or("?", |p| p.name);
                format!("{p} · {c}")
            }
            Mutation::Hidden { id } => {
                hidden_by_id(*id).map_or_else(|| format!("隐藏炫彩 {id}"), |h| h.name.to_string())
            }
        }
    }

    /// 存进 `roster.toml` 的写法。三种形态:
    ///
    /// ```toml
    /// mutation = "异色"
    /// mutation = "炫彩:3/33"   # 粒子 3(方块)· 配色 33(亮X暗 - 浅紫橙),编号与游戏一致
    /// mutation = "炫彩:黑白"
    /// ```
    ///
    /// 常规炫彩存**编号**而不是名字:名字是配置表里的展示文本,换版本可能改;
    /// 编号是协议里的东西,而且和游戏 UI 上看到的一致,对得上账。
    /// 隐藏款反过来存名字 —— 那四条的 id(1/2/3/**1000**)没有规律,写名字才看得懂。
    pub fn to_config(&self) -> String {
        match self {
            Mutation::Shiny => "异色".to_string(),
            Mutation::Common { color, particle } => format!("炫彩:{particle}/{color}"),
            Mutation::Hidden { id } => format!(
                "炫彩:{}",
                hidden_by_id(*id).map_or_else(|| id.to_string(), |h| h.name.to_string())
            ),
        }
    }

    /// `to_config` 的逆。认不出来返回 `None` —— 调用方该**报错**而不是默默按原样画:
    /// 配置里拼错了要让人看见,这和 config.rs 对未知键的态度一致。
    pub fn from_config(s: &str) -> Option<Self> {
        let s = s.trim();
        if s == "异色" {
            return Some(Mutation::Shiny);
        }
        let rest = s.strip_prefix("炫彩:")?.trim();
        if let Some((particle, color)) = rest.split_once('/') {
            return Some(Mutation::Common {
                color: color.trim().parse().ok()?,
                particle: particle.trim().parse().ok()?,
            });
        }
        // 隐藏款:先按名字找,再容一手直接写 id 的。
        HIDDEN
            .iter()
            .find(|h| h.name == rest)
            .map(|h| Mutation::Hidden { id: h.id })
            .or_else(|| {
                rest.parse()
                    .ok()
                    .filter(|id| hidden_by_id(*id).is_some())
                    .map(|id| Mutation::Hidden { id })
            })
    }

    /// 解析成 shader 输入。异色返回 `None`(它不走这条路)。
    pub fn render(&self) -> Option<GlassyRender> {
        match self {
            Mutation::Shiny => None,
            Mutation::Common { color, particle } => {
                let c = self::color(*color)?;
                let p = self::particle(*particle)?;
                Some(GlassyRender {
                    red_channel: c.red_channel,
                    green_channel: c.green_channel,
                    stick_colors: [[1.0; 4]; 4],
                    use_stick_colors: false,
                    params: GlassyParams {
                        star_intensity: c.shine_strength,
                        ..ROOT_PARAMS
                    },
                    star_stick_tiling: p.star_stick_tiling,
                    main_tex: DEFAULT_MAIN_TEX,
                    star_tex: p.tex,
                })
            }
            Mutation::Hidden { id } => {
                let h = hidden_by_id(*id)?;
                Some(GlassyRender {
                    red_channel: [h.red_channel[0], h.red_channel[1], h.red_channel[2]],
                    green_channel: [h.green_channel[0], h.green_channel[1], h.green_channel[2]],
                    stick_colors: h.stick_colors,
                    use_stick_colors: true,
                    params: GlassyParams { ..h.params },
                    // 隐藏款不给 `StarStickTiling`,沿用根默认 4.0。
                    star_stick_tiling: 4.0,
                    main_tex: h.main_tex,
                    star_tex: h.star_tex,
                })
            }
        }
    }
}

/// 炫彩素材目录:包目录**旁边**的 `glassy/`(`…/rocom-pets/glassy`)。
///
/// 为什么不放进宠物包:这几张图是**全库共用**的(常规炫彩的 `MainTex` 就一张
/// `Tex_PetGlassy_007_D`,粒子图四张),塞进 201 个包里要多背 120MB;而做成一份共享目录,
/// 已经导好的包不用重导也能用上炫彩。由 `rocom-pets-export --glassy` 导出。
///
/// 也不打进二进制:隐藏款那几张噪声图加起来 3MB 出头,会把 18MB 的产物顶到 21MB,
/// 而且**素材不该进代码仓库**(本仓库只有代码与导出器)。
#[cfg(not(target_arch = "wasm32"))]
pub fn assets_dir(packs_dir: &std::path::Path) -> std::path::PathBuf {
    packs_dir
        .parent()
        .map_or_else(|| std::path::PathBuf::from("glassy"), |p| p.join("glassy"))
}

/// 炫彩素材目录,`packs_dir` 没给时退回默认包目录旁边。
///
/// 两处都拿不到(比如 Windows 上连 `%LOCALAPPDATA%` 都没有)就返回 `None` ——
/// 那时炫彩画不出来,`Model::apply_glassy` 会在日志里说清楚。
#[cfg(not(target_arch = "wasm32"))]
pub fn default_assets_dir(packs_dir: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
    match packs_dir {
        Some(dir) => Some(assets_dir(dir)),
        None => crate::pack::Pack::default_dir().map(|d| assets_dir(&d)),
    }
}

/// 素材齐不齐。缺了就该把炫彩那几档在界面上禁掉并说清楚要跑什么命令 ——
/// 让用户看着一个点不出效果的选项,比直接说「素材没导」更糟。
#[cfg(not(target_arch = "wasm32"))]
pub fn assets_ready(dir: &std::path::Path) -> bool {
    let mut needed: Vec<&str> = vec![DEFAULT_MAIN_TEX];
    needed.extend(PARTICLES.iter().map(|p| p.tex));
    needed
        .iter()
        .all(|name| dir.join(format!("{name}.png")).exists())
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
            .field("star_intensity", &self.star_intensity)
            .field("global_refraction", &self.global_refraction)
            .field("global_depth", &self.global_depth)
            .field("main_tex_tiling", &self.main_tex_tiling)
            .finish_non_exhaustive()
    }
}

impl PartialEq for GlassyParams {
    fn eq(&self, other: &Self) -> bool {
        self.star_intensity == other.star_intensity
            && self.global_refraction == other.global_refraction
            && self.global_depth == other.global_depth
            && self.main_tex_flow_x == other.main_tex_flow_x
            && self.main_tex_flow_y == other.main_tex_flow_y
            && self.main_tex_tiling == other.main_tex_tiling
            && self.normal_effect_amount == other.normal_effect_amount
            && self.base_color_detail == other.base_color_detail
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

    /// 打包/解包要和客户端 `PetUtils.GetShineDataValue` 逐位一致 ——
    /// docs 里那个例子:1048609 = 四角星(1) · 亮X暗 - 浅紫橙(33)。
    #[test]
    fn glass_value_roundtrip() {
        let m = Mutation::Common {
            color: 33,
            particle: 1,
        };
        assert_eq!(m.glass_value(), Some(1_048_609));
        assert_eq!(Mutation::from_glass_info(1, 1_048_609), Some(m));
        assert_eq!(m.label(), "四角星 · 亮X暗 - 浅紫橙");
    }

    #[test]
    fn common_uses_shared_main_tex_and_no_stick_colors() {
        let r = Mutation::Common {
            color: 1,
            particle: 3,
        }
        .render()
        .expect("1 号配色 3 号粒子都在表里");
        assert_eq!(r.main_tex, DEFAULT_MAIN_TEX);
        assert_eq!(r.star_tex, "Tex_PetGlassyStar_001");
        assert_eq!(r.star_stick_tiling, 2.2);
        // 常规炫彩不覆盖四段渐变色 —— 覆盖了就等于把隐藏款的着色套到常规上。
        assert!(!r.use_stick_colors);
        assert_eq!(r.params.star_intensity, 10.0);
        // 其余标量必须原样落在根默认上。
        assert_eq!(r.params.global_refraction, ROOT_PARAMS.global_refraction);
        assert_eq!(r.params.global_depth, ROOT_PARAMS.global_depth);
    }

    #[test]
    fn hidden_overrides_its_own_textures_and_scalars() {
        let r = Mutation::Hidden { id: 3 }.render().expect("铅字幻梦");
        assert_eq!(r.main_tex, "T_PetGlassyNoiseS3_001");
        assert!(r.use_stick_colors);
        // 铅字幻梦把折射压到近乎为零、深度提到 100、贴图放大三倍。
        assert_eq!(r.params.global_refraction, 0.0001);
        assert_eq!(r.params.global_depth, 100.0);
        assert_eq!(r.params.main_tex_tiling, 3.0);
    }

    #[test]
    fn config_strings_round_trip() {
        for m in [
            Mutation::Shiny,
            Mutation::Common {
                color: 33,
                particle: 3,
            },
            Mutation::Hidden { id: 1000 },
            Mutation::Hidden { id: 3 },
        ] {
            let text = m.to_config();
            assert_eq!(Mutation::from_config(&text), Some(m), "{text}");
        }
        // 写法要看得懂 —— 这两条是文档里给用户看的样子。
        assert_eq!(
            Mutation::Common {
                color: 33,
                particle: 3
            }
            .to_config(),
            "炫彩:3/33"
        );
        assert_eq!(Mutation::Hidden { id: 1000 }.to_config(), "炫彩:黑白");
        // 认不出来的要说不认识,不能悄悄退成「没有变异」。
        assert_eq!(Mutation::from_config("炫彩:不存在的款"), None);
        assert_eq!(Mutation::from_config("闪光"), None);
    }

    #[test]
    fn shiny_is_not_a_glassy_layer() {
        assert!(Mutation::Shiny.render().is_none());
        assert!(Mutation::Shiny.glass_value().is_none());
    }
}
