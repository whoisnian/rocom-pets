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
    /// `M_P_Object_XiaoYou` 是一套独立的不透明材质，固有色入口明确叫 `MainTex`；
    /// 过去只认前两项会把它误分成纯特效层，正是小灵面身体缺失的直接原因。
    /// 没有这两个参数 = 这个材质**不画固有色**(纯 VFX:火焰、水壳、光晕),桌宠该整片跳过。
    public string? BaseColorParam =>
        (IsXiaoYou
            ? Textures.Keys.FirstOrDefault(k => k.Equals("MainTex", StringComparison.OrdinalIgnoreCase))
            : null)
        ?? Textures.Keys.FirstOrDefault(k => k.Equals("BaseTex", StringComparison.OrdinalIgnoreCase))
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
    ///
    /// **`OverAllColor`/`InColor` 是「一只一份」的定制材质留的口子。** 春兔耳朵里那泡粉色
    /// 液体走的是 `M_Gra_Yutu_Ear_Lighting` —— 一个只给这只宠物写的材质,整套参数名
    /// (`Bubble Color` / `InColor` / `OverAllColor` / `TopColor` / 「小球大小」…)都不在
    /// 上面那批通用名里,于是 `Tint` 拿到 null、耳朵渲成一泡白的(实机报的第二条)。
    /// 这两个名字**语义明确**(「整体颜色」/「内部颜色」,实测都是 (1, 0.343, 0.733) 粉),
    /// 补进来就够把颜色接对;泡泡、液面高度、折射那些还没做,见 docs/design.md §1.1。
    ///
    /// `M_ShuiMu_ByIn` 不读这里挑出的通用主色；它由下方 `Glassy*` 字段把原 shader 的
    /// 两个 flow 端点、噪声与 Fresnel 完整传给专用管线。这里不再用两色均值冒充流动结果。
    public float[]? Tint => FirstColor("Color01", "MainColor", "OverAllColor", "InColor",
        "BaseColor", "BaseColor1", "Emitter Color", "FresnelColor", "PatternColor",
        "BackColor");

    /// 半透强度;没写就当全不透明。
    public float Opacity => Scalars.TryGetValue("Opacity", out var v) ? v : 1f;

    /// **基色贴图的 alpha 是不透明度还是纹路遮罩,由这个静态开关决定。**
    ///
    /// 本体贴图的 alpha 平时是美术塞的纹路遮罩(绝不能拿来剔像素);但 `Opacity or OpacityMask`
    /// 开着的那 11 个材质(蜜蜂/小甲虫的翅膀、果冻、暮星辰的裙子…)里,它就是不透明度。
    /// 两处独立测量对上了:暮星辰裙子那块 UV 的基色 alpha 中位 0.537,经汇编里那个重映射
    /// `saturate((a - 0.04) * 1.1111)` → 0.55;而拿实机截图的**水印对比度衰减**反推出来的
    /// 单层区 α ≈ 0.50。
    ///
    /// **但只看那个开关是不够的 —— `M_P_Object_Trans` 族无条件就这么干**(2026-08-04,
    /// 实机报「春花兔的耳朵也是半透明」)。春花兔的 `_Fx` 没设这个开关,于是被我们当不透明画,
    /// 耳朵渲成一坨实心白;实机是透的。汇编说了话:
    ///
    /// - `M_P_Object_Trans` 的三个排列(51670 / 8752 / 21938)**每一条**都有
    ///   `add r1.z, r8.w, l(-0.04)` + `mul_sat r1.z, r1.z, l(1.1111)` 接在基色采样之后 ——
    ///   **没有「alpha 当纹路遮罩」的那条分支**;
    /// - `MI_P_Object_Trans_MatCap`(幽星光的玻璃球)那三条(37998 / 20284 / 70710)也都有;
    /// - 春兔 `_Fx`(开关**开**)与春花兔 `_Fx`(开关**没设**)**命中的是同一批 24 个
    ///   shader map、同一批 shader**(51670 打头)—— 静态排列一模一样,也就是说这个开关
    ///   的根默认本来就是开的,那 11 个只是把它又写了一遍。
    ///
    /// 数据也对得上:春花兔 `_Fx` 那块 UV 的基色 alpha 中位 **0.378**(它的 `_By` 是 1.000),
    /// 是张画出来的不透明度图。所以判据改成「开关开着 **或** 父链走 `M_P_Object_Trans`」。
    ///
    /// `Trans_MatCap` 也必须包含在内：它的目标 PS 最终是
    /// `lerp(max(重映射 alpha, 高光, 菲涅尔), 重映射 alpha, ForceUseDefOpacity)`。
    /// 先前为了让两颗星光球在不完整的 MatCap 近似下保持实心而排除了这一支，副作用是
    /// 莫比乌乌的整个玻璃外壳 alpha 恒为 1，原生不透明内层无论怎么画都会被挡住。
    /// 现在高光/菲涅尔覆盖已进入运行时，按 cooked shader 恢复整族语义。
    public bool AlphaIsOpacity =>
        Switch("Opacity or OpacityMask")
        || ParentChain.Any(p => p.Contains("Object_Trans", StringComparison.OrdinalIgnoreCase));

    /// 遮罩/噪声贴图:特效的形状与流动来源。没有就当常量 1。
    public string? MaskTexture =>
        (IsYutuEar ? YutuBubbleTexture : null)
        ?? FirstTexture("FuildMask", "Mask", "MaskTex", "BaseMap", "Base Color", "MatCap", "MatCapTex");

    /// 遮罩是不是 MatCap。**这决定采样方式**:matcap 要按视空间法线采(球面反射查找表),
    /// 拿网格 UV 采会变成一块块的斑,水灵的水膜就是这么糊掉的。
    public bool MaskIsMatcap =>
        !Textures.ContainsKey("FuildMask")
        && !Textures.ContainsKey("Mask") && !Textures.ContainsKey("MaskTex")
        && !Textures.ContainsKey("BaseMap") && !Textures.ContainsKey("Base Color")
        && (Textures.ContainsKey("MatCap") || Textures.ContainsKey("MatCapTex"));

    /// 果冻内胆使用的独立材质图。它不是通用“纯特效层”:原 shader 71636 输出 alpha=1,
    /// 用物体空间折射坐标三向采 `GlassyNoiseTex`,再做 flow 两色与 Fresnel 色的两次 lerp。
    public bool IsGlassyInner =>
        ParentChain.Any(p => p.Equals("M_ShuiMu_ByIn", StringComparison.OrdinalIgnoreCase));

    /// 小灵面家族专用的 `MI_P_Object_XiaoYou`。目标 Low PS 32511 输出 alpha=1，
    /// 并用 MainTex/NoiseTex/StarTex 与 COLOR_0 合成，不是通用半透或 VFX。
    public bool IsXiaoYou =>
        ParentChain.Any(p => p.Equals("MI_P_Object_XiaoYou", StringComparison.OrdinalIgnoreCase));

    /// 莫比乌乌内层使用的独立不透明材质。目标 Low PS 6037 的四张材质贴图依次是
    /// Bubble Texture / DistortTex / FlowTex / BaseColor；后三张只存在于根材质默认值。
    public bool IsYutuEar =>
        ParentChain.Any(p => p.Equals("M_Gra_Yutu_Ear_Lighting", StringComparison.OrdinalIgnoreCase));

    /// 克莱因龙的玻璃液体材质。游戏资产里的 `Fulid/Fuild` 就是这个拼写，不能按正确的
    /// Fluid 去匹配；目标 Low PS 42877 直接以 COLOR_0.g 乘最终覆盖率。
    public bool IsFakeFluid =>
        ParentChain.Any(p => p.Contains("FakeFulid", StringComparison.OrdinalIgnoreCase));

    /// 克莱因龙外壳使用的 MatCap 遮罩材质。目标 Low color PS 19654 先算
    /// `BaseColor * LightRamp + MatCap`，再接 Rim/FlatEmissive/Main/Selection，
    /// 最终 alpha 恒为 1；基础 OpacityMask 由同 resource 的 Early-Z depth PS 15293
    /// 以 `max(MatCap亮度,Fresnel) >= 0.3333` 写深度。过去把它当“无基色纯特效”并按
    /// HDR tint 判成加色层，会绕过遮罩且在所有内层液体之后盖上一整层白膜。
    public bool IsMatcapMasked =>
        ParentChain.Any(p => p.Equals("M_P_MatCap_Masked", StringComparison.OrdinalIgnoreCase));

    public float[] MatcapMaskedBaseColor =>
        FirstVector("BaseColor")
        ?? RootDefaults?.Vectors.GetValueOrDefault("BaseColor")
        ?? [1f, 1f, 1f, 0f];

    public float[] MatcapMaskedLightRamp =>
        FirstVector("LightRampColor")
        ?? RootDefaults?.Vectors.GetValueOrDefault("LightRampColor")
        ?? [1f, 1f, 1f, 0f];

    public float[] MatcapMaskedFlatEmissive =>
        FirstVector("Flat_EmissiveColor")
        ?? RootDefaults?.Vectors.GetValueOrDefault("Flat_EmissiveColor")
        ?? [1f, 1f, 1f, 1f];

    public float[] MatcapMaskedMainColor =>
        FirstVector("MainColor")
        ?? RootDefaults?.Vectors.GetValueOrDefault("MainColor")
        ?? [1f, 1f, 1f, 1f];

    public float[] MatcapMaskedSelectionColor =>
        FirstVector("SelectionColor")
        ?? RootDefaults?.Vectors.GetValueOrDefault("SelectionColor")
        ?? [0f, 0f, 0f, 0f];

    /// PS 19654 中 cb3[5].xy / cb3[13].z / cb3[14].w 的确切参数映射。
    public float[] MatcapMaskedRimShape =>
    [
        Scalar("Rim Power", RootScalar("Rim Power", 0.4f)),
        Scalar("Rim Soft Edge", RootScalar("Rim Soft Edge", 0.3f)),
        Scalar("Rim Intensity", RootScalar("Rim Intensity", 0f)),
        Scalar("FresnelPow", RootScalar("FresnelPow", 3f)),
    ];

    /// PS 19654 的 Flat/Main 与 Xray 门。最后一项是 Xray/Common_Xray 的 max，
    /// 对应 uniform preshader scalar-slot[11] 的原式。
    public float[] MatcapMaskedSurfaceShape =>
    [
        RootScalar("Flat_EmissiveIntensity", 1f),
        RootScalar("Flat_EmissiveRatio", 0f),
        RootScalar("MainBright", 1f),
        Math.Max(RootScalar("Xray", 0f), RootScalar("Common_Xray", 0f)),
    ];

    public string? YutuBubbleTexture => IsYutuEar ? FirstTexture("Bubble Texture") : null;
    public string? YutuDistortTexture => IsYutuEar
        ? RootDefaults?.Textures.GetValueOrDefault("DistortTex") : null;
    public string? YutuFlowTexture => IsYutuEar
        ? RootDefaults?.Textures.GetValueOrDefault("FlowTex") : null;

    public float[] YutuBubbleColor => FirstVector("Bubble Color") ?? [0f, 0.508735f, 1f, 1f];
    public float[] YutuFlowColor => FirstVector("FlowColor") ?? [1f, 1f, 1f, 0f];
    public float[] YutuFresnelColor => FirstVector("FresnelCol") ?? [1f, 1f, 1f, 0f];
    public float[] YutuInnerColor => FirstVector("InColor") ?? [1f, 1f, 1f, 1f];
    public float[] YutuOverallColor => FirstVector("OverAllColor") ?? [1f, 1f, 1f, 0f];
    public float[] YutuRampColor => FirstVector("RampColor") ?? [1f, 1f, 1f, 0f];
    public float[] YutuTopColor => FirstVector("TopColor2") ?? [0f, 0f, 0f, 0f];
    public float[] YutuBubbleShape =>
    [
        Scalar("Bubble Speed 1", 0.05f), Scalar("Bubble Speed 2", 0.05f),
        Scalar("Bubbles Scale", 5f), Scalar("FlowDistort", 0.2f),
    ];
    public float[] YutuFlowShape =>
    [
        Scalar("U_Speed1", 0.1f), Scalar("V_Speed1", -0.5f),
        Scalar("U_Tiling1", 1f), Scalar("V_Tiling1", 0.8f),
    ];
    public float[] YutuLightShape =>
    [
        Scalar("Flow Int", 0.3f), Scalar("Fres ExponentIn", 1f),
        Scalar("Fres Int", 1f), Scalar("InColor Size", 0f),
    ];
    public float[] YutuTopShape =>
    [
        Scalar("TopColor Offset", 0f), Scalar("TopColor Size", 0f),
        Scalar("TopColor Size2", 1f), Scalar("Contrast Soft 软硬", 0f),
    ];

    public float[] FluidEdgeColor => FirstVector("EdgeColor") ?? [1f, 1f, 1f, 1f];
    public float[] FluidFresnelColor => FirstVector("FresnelColor") ?? [1f, 1f, 1f, 0f];
    public float[] FluidPlaneColor => FirstVector("FulidPlaneColor") ?? [1f, 1f, 1f, 1f];
    public float[] FluidGradient1 => FirstVector("GradientColor01") ?? [1f, 1f, 1f, 1f];
    public float[] FluidGradient2 => FirstVector("GradientColor02") ?? [1f, 1f, 1f, 1f];
    public float[] FluidHeightTiling => FirstVector("HeightNoiseTiling") ?? [1f, 1f, 0f, 0f];
    public float[] FluidPlaneAxis => FirstVector("PlaneAxis") ?? [0f, 0f, 1f, 1f];
    public float[] FluidPlaneCenter => FirstVector("PlaneCenter") ?? [0f, 0f, 0f, 0f];
    public float[] FluidBodyShape =>
    [
        Scalar("BodyEdgeArea", 5f), Scalar("BodyEdgeOffset", 0.8f),
        Scalar("BodyEdgeSmooth", 0.1f), Scalar("HeightNoiseIntensity", 5f),
    ];
    public float[] FluidGradientShape =>
    [
        Scalar("GradientOffset", 0.5f), Scalar("GradientSmooth", 0.01f),
        Scalar("FresnelOffset", 0.3f), Scalar("FresnelSmooth", 0.2f),
    ];
    public float[] FluidTopShape =>
    [
        Scalar("TopEdgeOffset", 0.3f), Scalar("TopEdgeSmooth", 0.05f),
        RootScalar("RippleOpacity", 1f), Scalar("FadeDistance", 30f),
    ];

    public float[] XiaoYouBaseColor1 =>
        FirstVector("BaseColor1") ?? [0f, 0f, 0f, 1f];

    public float[] XiaoYouBaseColor2 =>
        FirstVector("BaseColor2") ?? [0f, 0f, 0f, 1f];

    public float[] XiaoYouFlowColor1 =>
        FirstVector("FlowNoiseColor1") ?? [0f, 0f, 0f, 1f];

    public float[] XiaoYouFlowColor2 =>
        FirstVector("FlowNoiseColor2") ?? [0f, 0f, 0f, 1f];

    public float[] XiaoYouStarColor =>
        FirstVector("StarColor") ?? [0f, 0f, 0f, 0f];

    /// 原材质两组 panner 速度，参数名与目标 PS 使用的两条时间坐标一一对应。
    public float[] XiaoYouNoiseFlow =>
    [
        Scalar("USpeedTex01"), Scalar("VSpeedTex01"),
        Scalar("USpeedTex02"), Scalar("VSpeedTex02"),
    ];

    /// [两层 flow 强度, RG 星点阈值强度, 闪烁速度]。
    public float[] XiaoYouShape =>
    [
        Scalar("FlowNoseInt1", 1f), Scalar("FlowNoiseInt2", 1f),
        Scalar("Star_RG_Int", 1f), Scalar("Star_RG_TwinkleSpeed", 0f),
    ];

    public float[] XiaoYouStarUv =>
        FirstVector("Star_RG_UV_Control") ?? [1f, 0f, 1f, 0f];

    public string? NoiseTexture =>
        (IsYutuEar ? YutuDistortTexture : null)
        ?? (IsFakeFluid ? FirstTexture("BubbleColorLutTex") : null)
        ?? FirstTexture("Noise", "NoiseTex", "FlowTexture", "GlassyNoiseTex")
        ?? (IsGlassyInner ? RootDefaults?.Textures.GetValueOrDefault("GlassyNoiseTex") : null);

    public float[] GlassyFlowColor01 =>
        FirstVector("GlassyFlowColor01")
        ?? RootDefaults?.Vectors.GetValueOrDefault("GlassyFlowColor01")
        ?? [1f, 1f, 1f, 1f];

    public float[] GlassyFlowColor02 =>
        FirstVector("GlassyFlowColor02")
        ?? RootDefaults?.Vectors.GetValueOrDefault("GlassyFlowColor02")
        ?? [1f, 1f, 1f, 1f];

    public float[] GlassyFresnelColor =>
        FirstVector("GlassyFresnelColor")
        ?? RootDefaults?.Vectors.GetValueOrDefault("GlassyFresnelColor")
        ?? [1f, 1f, 1f, 1f];

    /// [速度, UV 尺度, GlassyNoiseRefract 原参数, 深度]。四个槽逐一对应 71636 的
    /// cb4[7]/[18]；其中 shader 实际读取的折射 eta 是 preshader 求出的
    /// `1 / (1 + GlassyNoiseRefract)`，运行时保留原参数是为了兼容已经导出的包。
    public float[] GlassyNoiseParams =>
    [
        RootScalar("GlassyNoiseSpeed", -0.1f),
        RootScalar("GlassyNoiseUVScale", 1f),
        RootScalar("GlassyNoiseRefract", 0.2f),
        RootScalar("GlassyNoiseDepth", 30f),
    ];

    /// [Fresnel 次数,阈值起点,过渡宽度,三向混合强度]。
    public float[] GlassyMaskParams =>
    [
        RootScalar("GlassyNoiseFresnelMaskPow", 1f),
        RootScalar("GlassyNoiseFresnelMaskOffset", 0.7f),
        RootScalar("GlassyNoiseFresnelMaskSmooth", 0.1f),
        RootScalar("GlassyNoiseTriPlannarBlendInt", 0f),
    ];

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
    /// **判据是「明确的 `XingGuang_UVFlow` 色带族,或静态开关
    /// `是否需要BaseColor流动` 打开」。** 不能把所有名字含 `UVFlow` 的父材质都算进来:
    /// `MI_P_Object_UVFlow_WPO_NoMetal` 的 `FlowTexture` 在目标 Low shader 中接的是
    /// **法线扰动**,不是 BaseColor。水灵的蝴蝶结与身体走这支；把那张蓝黑遮罩当颜色混入，
    /// 就会把本来干净的红蝴蝶结染蓝。`XingGuang_UVFlow` 才是把贴图接到颜色的那一支。
    /// 原来只看「美术给了流速」,那会多出 17 个火焰族材质(火花/迪莫/守夜烛):它们的
    /// `Flow_U_Speed` 是给**特效层自己的噪声卷动**用的,不是给固有色叠色带。
    public string? FlowTexture =>
        (ParentChain.Any(p => p.Contains("XingGuang_UVFlow", StringComparison.OrdinalIgnoreCase))
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
    /// 实机里这层**不流动**,运行时和 `StarStickTex` 走同一条路,只是贴图与着色换成这一族
    /// 自己的。全量只有 3 个材质是这一族(幽星光的身体)。
    ///
    /// **注意两族的公式其实不一样**(反汇编查实的,见 pet.wgsl `star_light`):`StarStickTex`
    /// 那张是彩色星形色块图集,这一族的 `NoiseTex` 是纯黑底、r/g/b 分别是阈值/相位/幅度。
    /// 运行时把两层并成一份时公式也并成了一套,那是已知的简化 —— 不是「同一份遮罩」。
    public bool IsFakeTrans =>
        ParentChain.Any(p => p.Contains("FakeTrans", StringComparison.OrdinalIgnoreCase));

    /// 发光强度(火焰族有);没有就 1。
    public float Glow => Scalar("Glow Intensity", 1f);

    /// 自发光:`Emitter Color` × `Emitter Intensity`。**根默认强度是 0**,也就是这一层
    /// 默认关闭、要用的宠物自己开 —— 所以只对开了的那些生效,风险有界。
    ///
    /// 汇编里它是 `材质颜色 × 一个遮罩` 加进结果(水蓝蓝 body 的 shader 33729:
    /// `mad r5.xyz, cb6[94].xyzx, r2.y, r5.xyzx`,r5 随后加进颜色);那个遮罩由若干标量
    /// 拼出的 ramp 给,**输入还没追到**,运行时先用菲涅尔当代理(见 pet.wgsl)。
    ///
    /// 证据:全库唯二开着这一项的(波波拉 0.3/0.4 蓝、火神 0.5 橙)正好是 17 只实机对照里
    /// **唯二的非构图色差离群项**(调色板 0.329 / 0.162),而关着的那些都在 0.02~0.11。
    /// **水体预设里 `Emitter Intensity` 不是自发光强度,是 `Color1` 那层的增益。**
    /// 查实于水蓝蓝 `_Fx`(父 `MI_P_Object_Water_NoMetal`)的 shader 35663 —— 配到该材质
    /// 块 15(`V=83`、`dcl cb5[106]`、`83 + ⌈90/4⌉ + 1 = 107`),那一步是
    /// `mad r4.xyz, r4.xyzx, cb5[83].x, r5.xyzx`,而 `cb5[83].x` 的名字就是 `Emitter Intensity`,
    /// `r4` 是 `mask × Color1`。完整公式见 rocom-capture/docs/shader.md「水体预设」。
    ///
    /// 所以这一族不能输出自发光层 —— 原来当通用自发光加了「白 × 0.4 × 菲涅尔」。
    /// **实测也支持**:关掉自发光后波波拉的调色板距离一动不动(0.337),
    /// 而火神从 0.090 恶化到 0.178 —— 火神那边它确实是自发光,所以只排除水体。
    /// (火神的图里也有 `Color1`/`Color2`/`FresnelInt`,可能是同一个共享图层,
    /// 但它的 shader 还没读,没有证据前不动。)
    public bool IsWater =>
        ParentChain.Any(p => p.Contains("Water", StringComparison.OrdinalIgnoreCase));

    public float EmissiveIntensity => IsWater ? 0f : Scalar("Emitter Intensity", 0f);


    /// 水体预设(`ML_P_StylizedWater` 图层)。整条链是从 shader 35663 读出来的,
    /// 公式见 rocom-capture/docs/shader.md「水体预设」;这里只把参数搬出来。
    ///
    /// `Color1` 的**增益就是 `Emitter Intensity`**(见上面),所以合成一个 rgb + a 传出去;
    /// `Main Color` 的 **a 是末尾那步 lerp 的混合系数**(波波拉是 0 ⇒ 空操作)——
    /// 这个「rgb 存颜色、a 存混合量」的套路在这套材质里反复出现,别只取 rgb。
    public float[]? WaterColor1 => !IsWater ? null
        : Vectors.TryGetValue("Color1", out var c)
            ? [c[0], c[1], c[2], Scalar("Emitter Intensity", 0f)]
            : null;

    public float[]? WaterColor2 =>
        !IsWater ? null : Vectors.TryGetValue("Color2", out var c) ? [c[0], c[1], c[2], 0f] : null;

    /// `Main Color` 原样带 a(a = 混合系数)。
    public float[]? WaterMain =>
        !IsWater ? null : Vectors.TryGetValue("Main Color", out var c) ? c : null;

    /// caustics 的 `[u 平铺, v 平铺, u 速度, v 速度]`。
    public float[] WaterCaustics =>
    [
        Scalar("U_Tiling_Caustics", 1f), Scalar("V_Tiling_Caustics", 1f),
        Scalar("U_Speed_Caustics", 0f), Scalar("V_Speed_Caustics", 0f),
    ];

    /// `[CausticsInt, FlowDistort, FresnelInt, FresnelPower]`。
    public float[] WaterShape =>
    [
        Scalar("CausticsInt", 1f), Scalar("FlowDistort", 0f),
        Scalar("FresnelInt", 1f), Scalar("FresnelPower", 1f),
    ];

    public float[]? EmissiveColor =>
        EmissiveIntensity > 0f && Vectors.TryGetValue("Emitter Color", out var c) ? c : null;

    /// 是不是半透材质。**有基色的材质也可能是半透**——暮星辰的裙子(`Fx1`)与那两个球(`Fx2`)
    /// 都是 `MI_P_Object_Trans_*` 家族、`BLEND_Translucent`,当成不透明画就是死板的实心块。
    public bool IsTranslucent =>
        BlendMode is EBlendMode.BLEND_Translucent or EBlendMode.BLEND_AlphaComposite;

    /// 半透族(`M_P_Object_Trans`)的星点层门:**`RampID >= 0.4`,这条是从汇编读出来的**。
    ///
    /// shader 51670(`M_P_Object_Trans` 的世界 base pass、`Opacity or OpacityMask` 那个排列,
    /// 7 个 uniform buffer ⇒ 材质 cb6):
    ///
    /// ```text
    /// ge  r2.y, cb6[84].z, l(0.4)          ← 门
    /// and r5.w, r2.y, l(1065353216)        ← 门 → 1.0f / 0.0f
    /// mad r4.xyw, r5.w, r9.xyxz, r4.xyxw   ← 高光层按门混
    /// …
    /// max r1.z, r1.z, r8.w                 ← r8.w = 星点遮罩 m
    /// mad r1.z, r5.w, r1.z, r1.w           ← **星点对不透明度的贡献也按同一个门混**
    /// ```
    ///
    /// `cb6[84].z` 的名字是查出来的不是猜的:按 `uniexpr.py` 的两条判据(V = 83、
    /// V + ⌈S/4⌉ + 1 = 120 ≥ 声明的 cb6[119])这条 shader 唯一配到冻结块 9,块 9 里
    /// `cb[84].z = 标量 6 = RampID`(根默认 0)。**交叉验证**:同块 `cb[88].z = StarStickTiling = 4`,
    /// 而汇编里星点的 UV 正是 `mul r3.yw, v2.xxxy, cb6[88].z` —— 槽位对得上。
    ///
    /// 实机两张截图也对得上:春兔 `_Fx`(耳膜,`RampID` = 0)看不到星点,
    /// 果冻 `_By`(`RampID` = **0.5**,实例显式开的)那层是开着的 —— 而它在这一族里
    /// **只改不透明度、不画彩色星星**(见上面汇编:`m` 只进 `r1.z` 那条 alpha 链),
    /// 所以实机看着也只是「果冻更实了一点」,不是一身四角星。
    /// **这条「只改 alpha」运行时还没实现**(见 docs/design.md §1.1),现在只做门。
    private bool TransStarGate =>
        !ParentChain.Any(p => p.Contains("Object_Trans", StringComparison.OrdinalIgnoreCase))
        || RootScalar("RampID", 0f) >= 0.4f;

    /// 星点贴图:游戏里身上那些细碎星光。共享图 `Tex_PetGlassyStar_004` 一类。
    ///
    /// **几乎每个宠物材质都挂着这张图,但绝大多数并没有真的启用它**——游戏靠静态开关
    /// 与遮罩通道决定要不要叠。半透族那道门现在**读出来了**(见 `TransStarGate`);
    /// 不透明族(`M_P_Object`)那道是**逐像素**的 —— shader 51377 里整段星点包在
    /// `if (法线贴图.a >= 0.4)` 里(`sample_l r3.xyzw, v2.xyxx, t3` 之后 `mad r3.xy, r3.xy, 2, -1`
    /// 再 `nz = sqrt(1-x²-y²)`,是标准切空间法线,所以 t3 就是法线图、`.w` 是塞在里面的遮罩),
    /// 运行时还没实现,所以那一族仍然退回下面这条启发式:
    /// 「美术是否显式设了向量 `StarStickTiling`」——设了(暮星辰的裙子 = 4×4)才当启用。
    /// 一开始无条件叠,结果整只宠物被星点冲白。
    ///
    /// 这里**故意只查向量**那份 `StarStickTiling`:平铺该读标量(见 `StarTiling`),但把标量
    /// 也算进这个门会让更多材质新启用这一层 —— 那是未经验证的行为改动,没做。
    public string? StarTexture =>
        !TransStarGate ? null
        : IsXiaoYou ? FirstTexture("StarTex")
        : Vectors.ContainsKey("StarStickTiling") ? FirstTexture("StarStickTex", "ShinyStarTex", "StarTex")
        : IsFakeTrans ? FirstTexture("NoiseTex", "Noise")
        : null;

    /// **这个材质的图里到底有没有星贴层。** 判据是**读出来的**:参数名表(uexp 里 shader map
    /// 自带那张,见 rocom-capture 的 `scripts/matparams.py`)就是「这个图实际用到哪些参数」,
    /// 而眼睛/嘴走的 `M_P_Eyes` 整张表只有 42 条、**一个 `Star*`/`Stick*` 都没有** ——
    /// 所以那两个槽压根不可能有这一层。
    ///
    /// 之所以要这道门:下面「一个形态只有一份星点遮罩」那段统一会把星点盖到**所有**材质上,
    /// 连眼睛和嘴一起刷。星光族三只实测就是这样(包里 `_Es`/`_Mh` 也带着 `star_tex`)。
    public bool GraphHasStickLayer =>
        RootDefaults is { } rd
        && (rd.Scalars.ContainsKey("Stick_Intensity") || rd.Scalars.ContainsKey("StarStickTiling"));

    /// 星点平铺(前两位是 uv 平铺)。
    ///
    /// **`StarStickTiling` 在材质图里同名存在标量与向量两份**,根默认是**标量** 4。汇编里星点
    /// 的采样是 `mul rX.zw, v2.xxxy, cb6[130].w` —— 网格 UV0 乘**一个标量**,u/v 同一个数,
    /// 所以标量那份才是它,向量那份是同名的另一个参数。
    ///
    /// 原来只查向量表,于是幽星光一族(标量覆盖 5.3 / 向量 (4,4) / 无覆盖)全掉进
    /// `NoiseTilingSpeed` 兜底,拿到 1.8/2.5 —— 偏小一半，运行时靠一个手挑的 ×3 补回来。
    /// 现在按标量优先,三只得到 4 / 5.3 / 4(与用户目视「三只星点大小间距差不多」一致),
    /// 运行时那个 ×3 也就撤掉了。
    public float[] StarTiling =>
        Scalars.TryGetValue("StarStickTiling", out var s) && s > 0 ? [s, s]
        : Vectors.TryGetValue("StarStickTiling", out var v) && v[0] > 0 ? [v[0], v[1]]
        : RootDefaults?.Scalars.TryGetValue("StarStickTiling", out var r) == true && r > 0 ? [r, r]
        : IsFakeTrans && Vectors.TryGetValue("NoiseTilingSpeed", out var n) && n[0] > 0 ? [n[0], n[1]]
        : [1f, 1f];

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
            // **HDR 量级要保留,不能归一化。** 原来除掉了峰值(曜星光 `Color02` =
            // (10, 8.07, 9.04) ⇒ (1, 0.807, 0.904)),那是因为当时运行时的强度写死 1.5、
            // 不除会糊白。现在强度是**读出来的** `Mat_NoiseIntensity` = 0.05,
            // 而它本就是配着原始 HDR 值用的:
            //     实机 (10, 8.07, 9.04) × 0.05 = (0.50, 0.40, 0.45)
            //     归一化后          × 0.05 = (0.05, 0.04, 0.045)   ← 暗十倍
            // 用户实测「幽星光身上的星点不明显」就是这十倍。
            if (!IsFakeTrans || FirstVector("Color02") is not { } hdr) return null;
            return [hdr[0], hdr[1], hdr[2], 1f];
        }
    }

    /// MatCap:球面反射查找表。暮星辰那两个球的玻璃感就是它 + `MatCapColor=(3,3,3)` 的 HDR 白。
    ///
    /// **判据直接用静态开关 `是否使用MatCap`**(全量 17 个材质开着)。很多材质的 MatCap 槽绑的
    /// 压根不是反射图(幽星光的 `By` 绑的是 `Fx_ID` 描边图),无条件当高光叠会把宠物冲成一片白。
    /// 原来拿「美术有没有显式设 `MatCapColor`」当判据,数目正好也是 17 个但对错各有两处
    /// (多算了果冻与翡翠水母、漏了莫比乌乌与风铃鲨三阶)—— 开关是明写的答案,不必再推断。
    public string? MatcapTexture =>
        IsFakeFluid
            ? FirstTexture("MatCapTex")
            : Switch("是否使用MatCap") ? FirstTexture("MatCap", "MatCapTex") : null;

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

    /// 内部星光的着色:**`CrossStarColor`**(根默认 (0.5, 0.1, 0.8) 紫)。
    ///
    /// **原来取的是 `StarColor`(0.33, 0.67, 2) —— 错的。** 汇编里球内星层那一步是
    /// `lerp(底, cb5[41], 星点强度)`,而 `cb5[41]` 在 `MI_Ill_XingGuang1_001_Fx1` 块 10 里
    /// 解出来是 `CrossStarColor`;`StarColor` 根本不在这条链上。
    /// 早先一版把 `cb5[36]` 读成 `StarColor` 并据此说「运行时用 StarColor 是对的」——
    /// 那是解析 bug 造成的**槽位名整体错位一格**(见 rocom-capture 的 `uniexpr.param_pair`),
    /// 修完 `cb5[36]` 是 `BlackMagicRimColor`。全库没有实例覆盖过 `CrossStarColor`。
    public float[]? InteriorColor =>
        FirstVector("CrossStarColor") ?? RootDefaults?.Vectors.GetValueOrDefault("CrossStarColor")
        ?? FirstVector("StarColor") ?? RootDefaults?.Vectors.GetValueOrDefault("StarColor");

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

    /// 半透族的颜色边缘参数。根材质默认是 0.4 / 0.3,实例会逐只覆盖
    /// (果冻 = 1.4 / 0.2)。目标实机选中的 Low alpha 链不读取这层；它只用于颜色。
    public float RimPower => RootScalar("Rim Power", RootScalar("RimPower", 0.4f));

    public float RimSoftEdge => RootScalar("Rim Soft Edge", 0.3f);

    /// 半透族高光覆盖率。公式来自 `M_P_Object_Trans` 的有/无方向光两个排列:
    /// `smoothstep(.4, .5, pow(max(N·H, 0), HighLightSpecPow)) × HighLight SpecInt`。
    /// Offset 是 UE 的 Z-up 向量,导出时换成运行时 glTF 的 Y-up `(x,z,y)`。
    public float[] HighlightOffset
    {
        get
        {
            var v = FirstVector("HighLight Offset")
                    ?? RootDefaults?.Vectors.GetValueOrDefault("HighLight Offset")
                    ?? [0f, 0f, 0f, 1f];
            return [v[0], v[2], v[1]];
        }
    }

    public float[] HighlightSpecColor
    {
        get
        {
            var v = FirstVector("HighLight SpecCol")
                    ?? RootDefaults?.Vectors.GetValueOrDefault("HighLight SpecCol")
                    ?? [1f, 1f, 1f, 0f];
            return [v[0], v[1], v[2]];
        }
    }

    public float HighlightSpecPower => RootScalar("HighLightSpecPow", 10f);

    public float HighlightSpecIntensity => RootScalar("HighLight SpecInt", 1f);

    /// 0 = 使用 max(基础 alpha,高光覆盖),1 = 强制退回基础 alpha。
    public float ForceUseDefaultOpacity => RootScalar("ForceUseDefOpacity", 0f);

    /// `M_P_Object_Trans` 场景深度淡化的距离(UE 厘米)。实机 ES3.1/Low 的 LOD0
    /// shader 在基础 alpha / 高光覆盖之后计算
    /// `saturate((sceneDepth - pixelDepth) / OpacityDepthDistance)`。
    public float OpacityDepthDistance => RootScalar("OpacityDepthDistance", 40f);

    /// 是否把上面的深度淡化加进 alpha；果冻实例明确覆盖为 1。
    public float OpenDepthDistance => RootScalar("OpenDepthDistance", 0f);

    /// 目标设备的 ES3.1/Low 基础半透明排列。只认**经由 `MI_P_Object_Trans` 这个共同中间父**
    /// 的那一支:果冻外壳、春兔耳膜与同类 `_WPO` 变体。
    ///
    /// **`_XingGuang_*` 与 `_Trans_MatCap` 必须挡掉。** 它们的父链里**也有**
    /// `MI_P_Object_Trans`(例如 `MI_Ill_XingGuang3_001_Fx1` 是
    /// `[_Trans_XingGuang_WPO, _Trans_WPO, MI_P_Object_Trans, M_P_Object_Trans]`),
    /// 所以只写 `ParentChain.Any(== "MI_P_Object_Trans")` 拦不住 —— 得按名字排除。
    /// 而「只认直接父」也不行:**果冻自己的直接父就是 `_WPO`**。
    ///
    /// 挡不住的代价量过:暮星辰的裙子会走上这条为果冻反汇编出来的短链,
    /// 调色板距离 0.068 → 0.082。这条 shader map(`ACB16DBC…`)只对果冻外壳验过,
    /// 别往没验过的分支上摊。
    public bool IsObjectTransLow =>
        IsTranslucent
        && BaseColorTexture is not null
        && ParentChain.Any(p => p.Equals("MI_P_Object_Trans", StringComparison.OrdinalIgnoreCase))
        && !ParentChain.Any(p => p.Contains("XingGuang", StringComparison.OrdinalIgnoreCase)
                                 || p.Contains("MatCap", StringComparison.OrdinalIgnoreCase));

    /// Low shader 2109/55790 的 t3/t4；绑定顺序由 cooked
    /// `UniformTextureParameters` 明确给出，而不是按文件名猜。
    public string? ObjectTransLightMaskTexture =>
        IsObjectTransLow ? FirstTexture("MaskTex") : null;

    public string? ObjectTransRampTexture =>
        IsObjectTransLow
            ? FirstTexture("RampTex") ?? RootDefaults?.Textures.GetValueOrDefault("RampTex")
            : null;

    /// `cb6[26].y`，原图中以 `0.1 * SoftEdge` 作为明暗过渡宽度。
    public float ObjectTransSoftEdge => RootScalar("SoftEdge", 0.5f);

    /// 原 shader 尾部 `cb6[29].xyz = MainColor.rgb * MainBright`。
    public float[] ObjectTransMainColor =>
        FirstVector("MainColor")
        ?? RootDefaults?.Vectors.GetValueOrDefault("MainColor")
        ?? [1f, 1f, 1f, 1f];

    public float ObjectTransMainBright => RootScalar("MainBright", 1f);

    /// **假半透族那层星点不走网格 UV0。** 材质图里有个明写的开关 `UseNoiseUV0`,根默认 **0**;
    /// 配套 `Mat_NoiseTilingX/Y = 5 / 2.5`、`Mat_NoiseSpeedX/Y = 0.1 / -0.1`、
    /// `Mat_NoiseIntensity = 0.05`。四条实机观察逐条对上:很淡、像蒙在镜头前、
    /// 拖动旋转时星点不随着转、略微上浮(`SpeedY` 为负 ⇒ 采样坐标下移 ⇒ 图案上浮)。
    ///
    /// **必须用 `RootScalar` 取。** 这几个参数全库没有任何实例覆盖过,只存在于根默认里,
    /// 而 `Scalar()` **不查根默认**(那是刻意的,见 `RootDefaults` 那条注释)。
    /// 踩过两次:两轮都拿到兜底值 `[0,0,1,1]`,还一度误判成"解包数据里没有"。
    public float[] NoiseUv =>
    [
        RootScalar("Mat_NoiseSpeedX", 0f), RootScalar("Mat_NoiseSpeedY", 0f),
        RootScalar("Mat_NoiseIntensity", 1f), RootScalar("UseNoiseUV0", 1f),
    ];

    /// 同上那套里的平铺;0 = 这个材质没有这套参数。
    public float[] NoiseTiling => [RootScalar("Mat_NoiseTilingX", 0f), RootScalar("Mat_NoiseTilingY", 0f)];

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
