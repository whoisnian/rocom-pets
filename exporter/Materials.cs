// 解析宠物材质实例:拿到**每个材质槽真正用哪张贴图**、以及混合模式/遮罩阈值。
//
// 为什么需要:原来贴图是按命名约定硬接的(材质名后缀 `_By/_Es/_Mh` ↔ `T_<Asset>_<槽>_D`),
// 于是两类东西接不上——
//   ① 指向**共享贴图**的槽(眼睛用 CommonTexture 里的图集),按约定找不到,只能退用本体贴图;
//   ② 半透/加色材质(水蓝蓝的水体、幽星光的发光壳),压根没法判该怎么混合。
// 材质实例里这些信息是全的:`TextureParameterValues` 给「参数名 → 贴图」,
// `BasePropertyOverrides.BlendMode` 给混合模式。
//
// **`UMaterialInstance.Deserialize` 抛 OverflowException 这条旧结论是错的**(见 git 历史里
// docs/design.md §1 的旧表述):实测本作的材质实例能正常强类型加载,参数一条不少。
// 之所以一直以为不行,是 CUE4Parse 会为**别的**资产刷 OverflowException 日志,当时误当成材质的。
//
// 参数是**继承**的:材质实例只存自己覆盖的部分,其余要顺 `Parent` 链往上找,
// 一直找到根材质。所以这里逐级合并,子覆盖父。

using CUE4Parse.FileProvider;
using CUE4Parse.FileProvider.Vfs;
using CUE4Parse.UE4.Assets.Exports.Material;
using CUE4Parse.UE4.Assets.Exports.SkeletalMesh;
using CUE4Parse.UE4.Assets.Exports.Texture;
using CUE4Parse.UE4.Assets.Objects;
using CUE4Parse.UE4.Objects.Core.Misc;
using CUE4Parse.UE4.Objects.UObject;

namespace RocomPets.Export;

/// 一个材质槽解析出来的结果。
public record MaterialInfo(
    string Name,
    /// 参数名 → 贴图对象路径(已顺父链合并,子覆盖父)。
    Dictionary<string, string> Textures,
    /// 参数名 → 线性色(RGBA)。特效层的颜色就在这儿:火焰的 Color、光晕的 EmissColor 之类。
    Dictionary<string, float[]> Vectors,
    /// 参数名 → 标量。强度/流速/菲涅尔次数一类。
    Dictionary<string, float> Scalars,
    /// 参数名 → 该参数的 `ExpressionGUID`。**这是通往根材质默认值的桥**:根材质
    /// (`UMaterial`)的 `CachedExpressionData` 里参数**名字被剥了、只剩哈希**,但有一份
    /// 与值数组同序的 `ExpressionGuids`;而实例这边每条参数都同时带名字和 GUID。
    /// 两边按 GUID 一对,就能给根材质那 149 个标量 / 43 个向量 / 13 张贴图的默认值配上名字。
    ///
    /// 为什么非要读根默认值:顺父链只能合并到根**之前**(根不是 `UMaterialInstance`),
    /// 所以只在根上给了默认、实例没覆盖的参数,平时完全看不见 —— 而那两颗球的固有色
    /// 恰恰就在那儿(根默认里有 F94728 红橙、FFC635 琥珀、64358B 紫、FF1BE7 品红)。
    Dictionary<string, string> ParameterGuids,
    /// **静态开关**:参数名 → 开/关。这是「这个特性到底开没开」的**明写答案**,
    /// 名字多半是中文(`是否使用MatCap`、`开启黑魔法效果`、`使用顶点色`)。
    /// 在拿到它之前只能靠「美术有没有显式写某个参数」间接推断,那是猜。
    Dictionary<string, bool> Switches,
    EBlendMode BlendMode,
    float OpacityMaskClipValue,
    /// 父链上所有材质的名字,由近及远;排查用。
    List<string> ParentChain,
    /// **根材质的参数默认值**(参数名 → 值)。顺父链走不到根,所以只在根上给了默认、
    /// 没有任何实例覆盖过的参数,只能从这儿拿(见 RootDefaults.cs)。
    /// **刻意与上面几张表分开**:现有判据看的是「美术显式设了没有」,混进根默认会整片翻转。
    RootDefaults? RootDefaults = null,
    /// 材质资产是不是真的读到了。`false` = 网格引用的材质包在 pak 里根本不存在(悬空引用),
    /// 参数全空,导出器会退回按贴图命名约定给基色,见 Program.cs。
    bool Resolved = true)
{
    /// 承载基色的参数名。`BaseTex` = 本体一类,`EyeTex` = 眼/嘴那种贴脸的小面片。
    /// 没有这两个参数 = 这个材质**不画固有色**(纯 VFX:火焰、水壳、光晕),桌宠该整片跳过。
    public string? BaseColorParam =>
        Textures.Keys.FirstOrDefault(k => k.Equals("BaseTex", StringComparison.OrdinalIgnoreCase))
        ?? Textures.Keys.FirstOrDefault(k => k.Equals("EyeTex", StringComparison.OrdinalIgnoreCase));

    /// 基色贴图的对象路径;没有就是纯特效材质。
    public string? BaseColorTexture => BaseColorParam is { } p ? Textures[p] : null;

    /// 是不是贴脸的小面片(眼/嘴)。它的贴图是**带透明背景的表情图集**,alpha 是真遮罩,
    /// 渲的时候要按阈值剔;本体贴图的 alpha 是美术塞的遮罩通道,不能拿来剔(会把身体啃掉)。
    public bool IsFacePatch =>
        BaseColorParam?.Equals("EyeTex", StringComparison.OrdinalIgnoreCase) == true;

    /// 特效层的主色。这些材质没有基色贴图,固有色写在颜色参数里:
    /// 火焰是 `Color01`(火花实测 (6, 0.8, 0) —— R>1 的 HDR 橙,说明是加色发光),
    /// 水壳是 `MainColor`(水蓝蓝 (0.19, 0.65, 1)),其余族用 BaseColor 一类。
    /// **纯白的那个不算。** 这些名字里靠前的往往是父材质留下的中性默认值:果冻的内胆
    /// (`M_ShuiMu_ByIn`)同时有 `MainColor = (1,1,1,1)` 与 `BaseColor = (0.117,0.283,0.054)`,
    /// 按名字顺序取会拿到白色 —— 内胆于是渲成一颗白球(外壳不透明时看不见,一做成半透就露出来)。
    /// 所以先挑非白的,全白才退回白色。
    public float[]? Tint => FirstColor("Color01", "MainColor", "BaseColor", "BaseColor1",
        "Emitter Color", "FresnelColor", "PatternColor", "BackColor");

    /// 半透强度;没写就当全不透明。
    public float Opacity => Scalars.TryGetValue("Opacity", out var v) ? v : 1f;

    /// **基色贴图的 alpha 是不透明度还是纹路遮罩,由这个静态开关决定。**
    ///
    /// 本体贴图的 alpha 平时是美术塞的纹路遮罩(绝不能拿来剔像素);但 `Opacity or OpacityMask`
    /// 开着的那 11 个材质(蜜蜂/小甲虫的翅膀、果冻、暮星辰的裙子…)里,它就是不透明度。
    /// 两处独立测量对上了:暮星辰裙子那块 UV 的基色 alpha 中位 0.537,经汇编里那个重映射
    /// `saturate((a - 0.04) * 1.1111)` → 0.55;而拿实机截图的**水印对比度衰减**反推出来的
    /// 单层区 α ≈ 0.50。所以这个开关就是「区分两种 alpha」的判据,不必再找启发式。
    public bool AlphaIsOpacity => Switch("Opacity or OpacityMask");

    /// 遮罩/噪声贴图:特效的形状与流动来源。没有就当常量 1。
    public string? MaskTexture =>
        FirstTexture("Mask", "MaskTex", "BaseMap", "Base Color", "MatCap", "MatCapTex");

    /// 遮罩是不是 MatCap。**这决定采样方式**:matcap 要按视空间法线采(球面反射查找表),
    /// 拿网格 UV 采会变成一块块的斑,水灵的水膜就是这么糊掉的。
    public bool MaskIsMatcap =>
        !Textures.ContainsKey("Mask") && !Textures.ContainsKey("MaskTex")
        && !Textures.ContainsKey("BaseMap") && !Textures.ContainsKey("Base Color")
        && (Textures.ContainsKey("MatCap") || Textures.ContainsKey("MatCapTex"));

    public string? NoiseTexture => FirstTexture("Noise", "NoiseTex", "FlowTexture");

    /// UV 卷动:速度与平铺。火焰靠它动起来。
    public float[] Flow =>
    [
        Scalar("Flow_U_Speed"), Scalar("Flow_V_Speed"),
        Scalar("Flow_U_Tiling", 1f), Scalar("Flow_V_Tiling", 1f),
    ];

    /// 静态开关查询。**开关只在美术真的打开时才写进实例**(全量统计里这些开关是「N 个开 / 0 个关」),
    /// 所以「查不到这一条」= 用父材质的默认值,而这批开关的默认基本都是关。
    public bool Switch(string name) => Switches.TryGetValue(name, out var v) && v;

    /// **卷动色带**:一张渐变图沿 UV 滚过表面,给固有色叠上流动的颜色。
    /// 暮星辰的环带就是它——`MI_P_Object_XingGuang_UVFlow_Morph` 给 `FlowTexture`
    /// = `T_..._Fx_D`(青↔粉竖条纹渐变)+ `Flow_U_Speed` = 0.25,于是青粉渐变绕着环跑;
    /// 基色贴图里环带那一条是**纯粉的**,渐变完全来自这张图。
    ///
    /// **判据是「`UVFlow` 族(公式写死在父材质里)或静态开关 `是否需要BaseColor流动` 打开」。**
    /// 原来只看「美术给了流速」,那会多出 17 个火焰族材质(火花/迪莫/守夜烛):它们的
    /// `Flow_U_Speed` 是给**特效层自己的噪声卷动**用的,不是给固有色叠色带。
    public string? FlowTexture =>
        (ParentChain.Any(p => p.Contains("UVFlow", StringComparison.OrdinalIgnoreCase))
         || Switch("是否需要BaseColor流动"))
        && (Scalar("Flow_U_Speed") != 0f || Scalar("Flow_V_Speed") != 0f)
            ? FirstTexture("FlowTexture")
            : null;

    /// 色带的混入强度(暮星辰环带 0.8)—— 是**混色权重**,不是乘法强度。
    public float FlowPower => Scalar("FlowPower", 1f);

    /// **色带的 ID 遮罩**:只在 `MaskTex` 的 **alpha** 落在 [`MaskID Min`, `MaskID Max`] 的地方生效。
    ///
    /// 实测暮星辰(阈值 0.6~0.8):那张 By_M 的 alpha 是**离散 ID 台阶**(0.0 / 0.27 / 0.50 /
    /// 0.72 / 1.0),环带那片是 0.72(68.5% 落在区间内)、额头与身体中央的黄色装饰是 0.502
    /// (0% 落在区间内)。不按这个门控,色带会连黄装饰一起卷,装饰就在黄绿之间来回变 ——
    /// 而实机里那些装饰是固定黄色。
    public string? MaskIdTexture => FlowTexture is null ? null : FirstTexture("MaskTex", "Mask");

    public float[] MaskIdRange => [Scalar("MaskID Min", 0f), Scalar("MaskID Max", 1f)];

    /// **「假半透」族也是一层星点**:`..._FakeTrans*` 家族给 `NoiseTex`(黑底 + 粉白星点)
    /// + `NoiseTilingSpeed` + HDR 的 `Color02`,幽星光一族的身体看着半透、身上有星星靠它。
    ///
    /// 实机里这层**不流动**、也和别处的星点看着是同一份遮罩,所以运行时和 `StarStickTex`
    /// 走同一条路(按屏幕位置贴的一层遮罩),只是贴图与着色换成这一族自己的。
    /// 全量只有 3 个材质是这一族(幽星光的身体)。
    public bool IsFakeTrans =>
        ParentChain.Any(p => p.Contains("FakeTrans", StringComparison.OrdinalIgnoreCase));

    /// 发光强度(火焰族有);没有就 1。
    public float Glow => Scalar("Glow Intensity", 1f);

    /// 是不是半透材质。**有基色的材质也可能是半透**——暮星辰的裙子(`Fx1`)与那两个球(`Fx2`)
    /// 都是 `MI_P_Object_Trans_*` 家族、`BLEND_Translucent`,当成不透明画就是死板的实心块。
    public bool IsTranslucent =>
        BlendMode is EBlendMode.BLEND_Translucent or EBlendMode.BLEND_AlphaComposite;

    /// 星点贴图:游戏里身上那些细碎星光。共享图 `Tex_PetGlassyStar_004` 一类。
    ///
    /// **几乎每个宠物材质都挂着这张图,但绝大多数并没有真的启用它**——游戏靠静态开关
    /// 与遮罩通道决定要不要叠,那套我们复刻不了。判据取「美术是否显式设了 `StarStickTiling`」:
    /// 设了(暮星辰的裙子 = 4×4)才当启用。一开始无条件叠,结果整只宠物被星点冲白。
    public string? StarTexture =>
        Vectors.ContainsKey("StarStickTiling") ? FirstTexture("StarStickTex", "ShinyStarTex", "StarTex")
        : IsFakeTrans ? FirstTexture("NoiseTex", "Noise")
        : null;

    /// 星点平铺(前两位是 uv 平铺):`StarStickTiling` 是 vec4(暮星辰的裙子 = (4,4)),
    /// 「假半透」族则记在 `NoiseTilingSpeed` 里(幽星光的身体 = (2.5,2.5,…))。
    public float[] StarTiling =>
        Vectors.TryGetValue("StarStickTiling", out var v) && v[0] > 0 ? [v[0], v[1]]
        : IsFakeTrans && Vectors.TryGetValue("NoiseTilingSpeed", out var n) && n[0] > 0 ? [n[0], n[1]]
        : [1f, 1f];

    /// 平铺数是不是美术在 `StarStickTiling` 里明写的(而不是从「假半透」族的噪声参数推的)。
    public bool HasExplicitStarTiling => Vectors.ContainsKey("StarStickTiling");

    /// 星点层的强度。**根材质里叫 `Stick_Intensity`(默认 1.5)** —— 运行时原来写死 0.3,
    /// 那是手挑的。名字现在查实了(参数名哈希,见 RootDefaults.cs)。
    ///
    /// 顺带一条**查实后决定「不改」**的:那 4 个 `StickRandomColor01..04`
    /// (红橙/品红/蓝/金黄)不是给 shader 用来重新上色的 —— 共享星点图
    /// `Tex_PetGlassyStar_004` 里的颜色块本来就是红/橙/黄(R≈0.94、B≈0.06),
    /// 其中两种正好等于 `StickRandomColor01`(0.946,0.064,0.021)与
    /// `04`(0.925,0.742,0.027),而 `02`(品红)`03`(蓝)在贴图里根本不出现。
    /// 所以颜色是烘在贴图里的,运行时「用贴图 rgb」是对的,别去按那 4 个色重上。
    public float StickIntensity => RootScalar("Stick_Intensity", 1.5f);

    /// 星点着色。「假半透」族用 `Color02`,但那是**配着别处的衰减用的 HDR**
    /// (幽星光 = (15,15,15)、暮星辰 = (14.8,11,15)),直接乘会糊成一片白;
    /// 只取它的色相(按最大通道归一化),亮度交给运行时那一档固定系数。
    public float[]? StarColor
    {
        get
        {
            if (FirstVector("StarColor", "StarStickColor") is { } c) return c;
            if (!IsFakeTrans || FirstVector("Color02") is not { } hdr) return null;
            var peak = MathF.Max(hdr[0], MathF.Max(hdr[1], hdr[2]));
            return peak > 0 ? [hdr[0] / peak, hdr[1] / peak, hdr[2] / peak, 1f] : null;
        }
    }

    /// MatCap:球面反射查找表。暮星辰那两个球的玻璃感就是它 + `MatCapColor=(3,3,3)` 的 HDR 白。
    ///
    /// **判据直接用静态开关 `是否使用MatCap`**(全量 17 个材质开着)。很多材质的 MatCap 槽绑的
    /// 压根不是反射图(幽星光的 `By` 绑的是 `Fx_ID` 描边图),无条件当高光叠会把宠物冲成一片白。
    /// 原来拿「美术有没有显式设 `MatCapColor`」当判据,数目正好也是 17 个但对错各有两处
    /// (多算了果冻与翡翠水母、漏了莫比乌乌与风铃鲨三阶)—— 开关是明写的答案,不必再推断。
    public string? MatcapTexture =>
        Switch("是否使用MatCap") ? FirstTexture("MatCap", "MatCapTex") : null;

    public float[]? MatcapColor => FirstVector("MatCapColor");

    /// 边缘光颜色/强度。暮星辰的球有一圈紫边(`Rim LightColor` = (0.67, 0.11, 1))。
    public float[]? RimColor => FirstVector("Rim LightColor", "RimLightColor", "FresnelColor");

    public float RimIntensity => Scalar("Rim Intensity", Scalar("RimIntensity", 0f));

    /// **玻璃内部那颗星**:`StarTex`(根默认 = `T_EMeng003`,一张四角星场、alpha 是干净的
    /// 稀疏星形遮罩)沿**折射光线**在物体空间 march、按三向投影采样,坐标再叠时间卷动。
    /// 读 shader 汇编读出来的(见 docs/design.md §1):`refract()` 的教科书实现 + triplanar
    /// + `View` 的时间项 —— 这就是实机里「球内有颗星、自己在动、和球自转无关」的来源。
    ///
    /// 贴图取自根材质默认值:没有任何实例覆盖 `StarTex`,顺父链是看不见它的。
    ///
    /// **采样起点是 `(UV1.x, UV1.y, UV2.x)`,从 shader 里查出来的**:片元着色器里那句
    /// `r4.xy = v2.zw; r4.z = v3.x`,配 DXBC `ISGN` 签名段(`v2` = TEXCOORD0、`v3` = TEXCOORD1)
    /// 与 UE 的 UV 打包规则(TEXCOORD0 = UV0.xy + UV1.xy、TEXCOORD1 = UV2.xy + UV3.xy)。
    /// 顶点侧见 model.rs 的 `Vertex::interior_pos`。
    ///
    /// **判据是「直接父就是 `MI_P_Object_Trans_MatCap`」,不能用「父链里有」。**
    /// 暮星辰那两颗球的直接父是 `..._Trans_XingGuang_Fresnel`(它自己的父才是 Trans_MatCap),
    /// 而它的 shader 反汇编下来是**完全另一套**:223 行、4 张贴图、`N·L` + 遮罩 →
    /// 一维 `RampTex` 行查(采样 v 是常数 1/256),既没有折射也没有三向投影。
    /// 拿同一套画法套上去是错的 —— 按「父链里有」判会把它也算进来(踩过)。
    /// ~~曾因「区域白闪」关掉过~~ **已重新开启**:那个白闪的根因是上游把切线写进了 NORMAL
    /// (见 docs/design.md 法线那条),不是这一层的问题 —— 法线修好后它就稳了。
    public string? InteriorTexture =>
        ParentChain.FirstOrDefault() == "MI_P_Object_Trans_MatCap"
            ? RootDefaults?.Textures.GetValueOrDefault("StarTex")
            : null;

    /// 内部星光的着色(根默认 `StarColor` = (0.33, 0.67, 2) —— 偏蓝的 HDR)。
    public float[]? InteriorColor =>
        FirstVector("StarColor") ?? RootDefaults?.Vectors.GetValueOrDefault("StarColor");

    /// 折射率与 march 深度。这两个每个宠物材质都写着(1.3 / 100)。
    ///
    /// **`GlobalDepth` 的量纲是从汇编定出来的**:`marchDist = |半包围盒| × 0.01 × GlobalDepth`,
    /// 代 100 进去正好等于 `|半包围盒|` —— 那个 0.01 的配合是这个槽位就是 `GlobalDepth` 的强证据。
    public float Refraction => Scalar("GlobalRefraction", 1f);

    public float RefractDepth => Scalar("GlobalDepth", 0f);

    /// **球内那颗星的闪烁**:汇编里是 `pow(星场.B, q) - 1.2 × |sin(2π × frac(速度×时间 + 星场.G))|^p`,
    /// 再乘星场的 A(星形遮罩)。也就是说**星点不是在移动、是在一明一暗地闪**,
    /// 而每颗星的相位来自贴图的 G 通道(实测 T_EMeng003 的 G 均值 0.328、取值分散,正合此用)。
    ///
    /// 速度与次数取根材质里**语义对得上的命名默认值**:`FlickerSpeed` = 0.3、`FlickerPower` = 5。
    /// 这是语义匹配,不是靠 cb 槽位证实的(槽位↔参数名还没打通,见 docs/design.md)。
    public float FlickerSpeed => RootScalar("FlickerSpeed", 0.3f);

    public float FlickerPower => RootScalar("FlickerPower", 5f);

    private float RootScalar(string name, float fallback) =>
        Scalars.TryGetValue(name, out var v) ? v
        : RootDefaults?.Scalars.TryGetValue(name, out var r) == true ? r
        : fallback;

    /// 边缘光的衰减次数:`pow(1 - N·V, RimPower)`。**小于 1 就不是「一圈边」而是整片泛色**——
    /// 幽星光那两个球是 0.35,整颗都透着红,只写 RimColor 不写这个会画成一圈细红边。
    public float RimPower => Scalar("Rim Power", Scalar("RimPower", 3f));

    private float Scalar(string name, float fallback = 0f) =>
        Scalars.TryGetValue(name, out var v) ? v : fallback;

    private float[]? FirstVector(params string[] names) =>
        names.Select(n => Vectors.TryGetValue(n, out var v) ? v : null).FirstOrDefault(v => v is not null);

    /// 同 `FirstVector`,但**优先跳过纯白**(纯白是颜色参数的中性默认值,不带信息)。
    private float[]? FirstColor(params string[] names)
    {
        var present = names.Select(n => Vectors.TryGetValue(n, out var v) ? v : null)
            .Where(v => v is not null)
            .ToList();
        return present.FirstOrDefault(v => v![0] < 0.99f || v[1] < 0.99f || v[2] < 0.99f)
               ?? present.FirstOrDefault();
    }

    private string? FirstTexture(params string[] names) =>
        names.Select(n => Textures.TryGetValue(n, out var v) ? v : null).FirstOrDefault(v => v is not null);
}

public static class Materials
{
    /// UE 默认的遮罩阈值;材质没覆盖时用它。
    private const float DefaultMaskClip = 0.3333f;

    /// 解析**网格自己声明的**材质槽。键是材质对象名,与 glb 里的材质名一致。
    ///
    /// 为什么不去列 `<资产>/Mat/` 目录:那是个不成立的假设。小浣蛋(`Dem_XiaoHuanDan1_001`)
    /// 的 `Mat/` 里只有描边材质,本体材质根本不在那儿;还有些资产把材质放在 `Yise/Mat/`
    /// (异色变体)之类的子目录。网格的 `Materials` 数组是权威来源:它按槽序给出材质对象,
    /// 不管对象存在哪个包里。实测这一改把 13 个「材质表为空」的形态全救回来了。
    public static Dictionary<string, MaterialInfo> Load(
        USkeletalMesh mesh,
        List<string> warnings)
    {
        var result = new Dictionary<string, MaterialInfo>(StringComparer.OrdinalIgnoreCase);
        foreach (var slot in mesh.Materials)
        {
            if (slot is null) continue;
            try
            {
                if (slot.Load() is not UMaterialInstance material)
                {
                    // 悬空引用:网格声明了这个材质,但 pak 里没有对应资产(实测 13 个形态如此,
                    // 如小浣蛋的 `MI_Dem_XiaoHuanDan1_001_By`)。仍然登记一条空的,
                    // 让导出器能按名字去凑基色贴图,免得整只宠物画不出来。
                    warnings.Add($"材质 {slot.Name} 在 pak 里没有资产(悬空引用),退回按贴图名接基色");
                    result[slot.Name.Text] = new MaterialInfo(slot.Name.Text, [], [], [], [], [],
                        EBlendMode.BLEND_Opaque, DefaultMaskClip, [], Resolved: false);
                    continue;
                }
                // **键用对象名。** 本作的 pak 里对象名与资产文件名的大小写能对不上,方向还不一致:
                // 喵呜的文件是 `MI_Gra_MiaoMiao2_001_By`、对象名是 `…Miaomiao2…`,魔力猫正好反过来。
                // glb 里的材质名取的是对象名,键不一致运行时就查不到 → 整只宠物一片都画不出来。
                var key = material.Name;
                if (!string.IsNullOrEmpty(key))
                    result[key] = Resolve(key, material) with { RootDefaults = RootMaterial.Of(material) };
            }
            catch (Exception e)
            {
                warnings.Add($"材质槽 {slot.Name} 解析失败: {e.Message}");
            }
        }
        return result;
    }

    /// 全零 GUID 是「没记」(材质层参数就是这样),不收。
    private static void Remember(Dictionary<string, string> into, string? name, FGuid guid)
    {
        if (!string.IsNullOrEmpty(name) && guid != default) into.TryAdd(name, guid.ToString());
    }

    /// 顺父链合并参数。**从最远的祖先开始写**,近的覆盖远的,于是子实例的覆盖最终生效。
    private static MaterialInfo Resolve(string name, UMaterialInstance material)
    {
        var chain = new List<UMaterialInstance>();
        var parents = new List<string>();
        var current = material;
        // 防环:材质链正常只有两三层,超过 8 层就是数据坏了
        while (current is not null && chain.Count < 8)
        {
            chain.Add(current);
            var parent = current.Parent;
            if (parent is not null) parents.Add(parent.Name);
            current = parent as UMaterialInstance;
        }

        var textures = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        var vectors = new Dictionary<string, float[]>(StringComparer.OrdinalIgnoreCase);
        var scalars = new Dictionary<string, float>(StringComparer.OrdinalIgnoreCase);
        var switches = new Dictionary<string, bool>(StringComparer.OrdinalIgnoreCase);
        var guids = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        var blend = EBlendMode.BLEND_Opaque;
        var maskClip = DefaultMaskClip;
        // chain 是「自己 → 父 → 祖父」,倒着遍历 = 从祖先到自己
        for (var i = chain.Count - 1; i >= 0; i--)
        {
            var mi = chain[i];
            foreach (var param in mi.GetOrDefault<FTextureParameterValue[]>("TextureParameterValues", []))
            {
                var texture = param.ParameterValue.ResolvedObject?.Object?.Value as UTexture;
                var path = texture?.GetPathName();
                if (!string.IsNullOrEmpty(param.Name) && !string.IsNullOrEmpty(path))
                    textures[param.Name] = path;
                Remember(guids, param.Name, param.ExpressionGUID);
            }
            foreach (var param in mi.GetOrDefault<FVectorParameterValue[]>("VectorParameterValues", []))
            {
                var c = param.ParameterValue;
                if (!string.IsNullOrEmpty(param.Name) && c is not null)
                    vectors[param.Name] = [c.Value.R, c.Value.G, c.Value.B, c.Value.A];
                Remember(guids, param.Name, param.ExpressionGUID);
            }
            foreach (var param in mi.GetOrDefault<FScalarParameterValue[]>("ScalarParameterValues", []))
            {
                if (!string.IsNullOrEmpty(param.Name))
                    scalars[param.Name] = param.ParameterValue;
                Remember(guids, param.Name, param.ExpressionGUID);
            }
            // 静态开关。**`bOverride` 一律是 false 而 `Value` 却各不相同**(实测幽星光一族
            // 100 条里没有一条 bOverride=true,但 `是否使用MatCap` 是 true、`GlassySwitch` 是
            // false),说明本作存的是**合并后的有效值**而不是「我覆盖了什么」——
            // 和 BasePropertyOverrides 那边一个套路。所以照样「有值就用、近的覆盖远的」。
            var staticSet = mi.GetOrDefault<FStructFallback>("StaticParameters");
            foreach (var entry in staticSet?.GetOrDefault<FStructFallback[]>("StaticSwitchParameters", [])
                                 ?? [])
            {
                var pname = entry.GetOrDefault<FStructFallback>("ParameterInfo")
                    ?.GetOrDefault<FName>("Name").Text;
                if (!string.IsNullOrEmpty(pname)) switches[pname] = entry.GetOrDefault<bool>("Value");
            }
            // BasePropertyOverrides 只在「勾了 override」时才有意义,但本作的实例普遍不写
            // bOverride_* 标记,所以按「有值就用」处理:BLEND_Opaque 是 0,等于没覆盖。
            var overrides = mi.BasePropertyOverrides;
            if (overrides is not null)
            {
                if (overrides.BlendMode != EBlendMode.BLEND_Opaque) blend = overrides.BlendMode;
                if (overrides.OpacityMaskClipValue > 0) maskClip = overrides.OpacityMaskClipValue;
            }
        }
        return new MaterialInfo(name, textures, vectors, scalars, guids, switches, blend, maskClip, parents);
    }
}
