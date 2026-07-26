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
    EBlendMode BlendMode,
    float OpacityMaskClipValue,
    /// 父链上所有材质的名字,由近及远;排查用。
    List<string> ParentChain,
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
    public float[]? Tint => FirstVector("Color01", "MainColor", "BaseColor", "BaseColor1",
        "Emitter Color", "FresnelColor", "PatternColor", "BackColor");

    /// 半透强度;没写就当全不透明。
    public float Opacity => Scalars.TryGetValue("Opacity", out var v) ? v : 1f;

    /// 半透材质的整体着色。暮星辰的裙子 `MainColor` = (0.39, 0.4, 0.63) —— 蓝紫调,
    /// 不乘上去裙子会偏白。只在显式给了、且不是纯白时才用。
    public float[]? MainColor =>
        Vectors.TryGetValue("MainColor", out var c) && (c[0] < 0.99f || c[1] < 0.99f || c[2] < 0.99f)
            ? c : null;

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

    /// **卷动色带**:一张渐变图沿 UV 滚过表面,给固有色叠上流动的颜色。
    /// 暮星辰的环带就是它——`MI_P_Object_XingGuang_UVFlow_Morph` 给 `FlowTexture`
    /// = `T_..._Fx_D`(青↔粉竖条纹渐变)+ `Flow_U_Speed` = 0.25,于是青粉渐变绕着环跑;
    /// 基色贴图里环带那一条是**纯粉的**,渐变完全来自这张图。
    ///
    /// 判据取「美术真给了流速」:`FlowTexture` 槽几乎人人都挂着,但只有 UVFlow 族在用。
    public string? FlowTexture =>
        Scalar("Flow_U_Speed") != 0f || Scalar("Flow_V_Speed") != 0f
            ? FirstTexture("FlowTexture")
            : null;

    /// 色带的混入强度(暮星辰环带 0.8)。
    public float FlowPower => Scalar("FlowPower", 1f);

    /// **「假半透」族的内部星光**:身体看着半透、内里飘着星星。做法是拿 `NoiseTex`
    /// (黑底 + 粉白星点)× HDR 的 `Color02`(幽星光 = (15,15,15)、暮星辰 = (14.8,11,15))
    /// 按 `NoiseTilingSpeed` 卷动,叠在固有色上。
    ///
    /// 只认 `..._FakeTrans*` 父材质——全量只有 3 个材质用(幽星光一族的身体),
    /// 按参数名放宽会误伤一堆把 `Color02` 当别的用的材质。
    public bool IsFakeTrans =>
        ParentChain.Any(p => p.Contains("FakeTrans", StringComparison.OrdinalIgnoreCase));

    public string? InnerTexture => IsFakeTrans ? FirstTexture("NoiseTex", "Noise") : null;

    public float[]? InnerColor => IsFakeTrans ? FirstVector("Color02") : null;

    /// `NoiseTilingSpeed` 是 (平铺U, 平铺V, 速度U, 速度V);换成与 `Flow` 一致的
    /// [速度U, 速度V, 平铺U, 平铺V] 布局,运行时两者共用同一组 uniform 字段。
    public float[] InnerFlow =>
        Vectors.TryGetValue("NoiseTilingSpeed", out var v) && v[0] > 0
            ? [v[2], v[3], v[0], v[1]]
            : [0f, 0f, 1f, 1f];

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
        Vectors.ContainsKey("StarStickTiling") ? FirstTexture("StarStickTex", "ShinyStarTex", "StarTex") : null;

    /// 星点平铺;`StarStickTiling` 是 vec4,前两位是 uv 平铺(暮星辰 = (4,4))。
    public float[] StarTiling =>
        Vectors.TryGetValue("StarStickTiling", out var v) && v[0] > 0 ? [v[0], v[1]] : [1f, 1f];

    public float[]? StarColor => FirstVector("StarColor", "StarStickColor");

    /// MatCap:球面反射查找表。暮星辰那两个球的玻璃感就是它 + `MatCapColor=(3,3,3)` 的 HDR 白。
    ///
    /// 同样**只在美术显式设了 `MatCapColor` 时才算启用**。很多材质的 MatCap 槽绑的压根不是
    /// 反射图(幽星光的 `By` 绑的是 `Fx_ID` 描边图),无条件当高光叠会把宠物冲成一片白。
    public string? MatcapTexture =>
        Vectors.ContainsKey("MatCapColor") ? FirstTexture("MatCap", "MatCapTex") : null;

    public float[]? MatcapColor => FirstVector("MatCapColor");

    /// 边缘光颜色/强度。暮星辰的球有一圈紫边(`Rim LightColor` = (0.67, 0.11, 1))。
    public float[]? RimColor => FirstVector("Rim LightColor", "RimLightColor", "FresnelColor");

    public float RimIntensity => Scalar("Rim Intensity", Scalar("RimIntensity", 0f));

    /// 边缘光的衰减次数:`pow(1 - N·V, RimPower)`。**小于 1 就不是「一圈边」而是整片泛色**——
    /// 幽星光那两个球是 0.35,整颗都透着红,只写 RimColor 不写这个会画成一圈细红边。
    public float RimPower => Scalar("Rim Power", Scalar("RimPower", 3f));

    private float Scalar(string name, float fallback = 0f) =>
        Scalars.TryGetValue(name, out var v) ? v : fallback;

    private float[]? FirstVector(params string[] names) =>
        names.Select(n => Vectors.TryGetValue(n, out var v) ? v : null).FirstOrDefault(v => v is not null);

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
                    result[slot.Name.Text] = new MaterialInfo(slot.Name.Text, [], [], [],
                        EBlendMode.BLEND_Opaque, DefaultMaskClip, [], Resolved: false);
                    continue;
                }
                // **键用对象名。** 本作的 pak 里对象名与资产文件名的大小写能对不上,方向还不一致:
                // 喵呜的文件是 `MI_Gra_MiaoMiao2_001_By`、对象名是 `…Miaomiao2…`,魔力猫正好反过来。
                // glb 里的材质名取的是对象名,键不一致运行时就查不到 → 整只宠物一片都画不出来。
                var key = material.Name;
                if (!string.IsNullOrEmpty(key)) result[key] = Resolve(key, material);
            }
            catch (Exception e)
            {
                warnings.Add($"材质槽 {slot.Name} 解析失败: {e.Message}");
            }
        }
        return result;
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
            }
            foreach (var param in mi.GetOrDefault<FVectorParameterValue[]>("VectorParameterValues", []))
            {
                var c = param.ParameterValue;
                if (!string.IsNullOrEmpty(param.Name) && c is not null)
                    vectors[param.Name] = [c.Value.R, c.Value.G, c.Value.B, c.Value.A];
            }
            foreach (var param in mi.GetOrDefault<FScalarParameterValue[]>("ScalarParameterValues", []))
                if (!string.IsNullOrEmpty(param.Name))
                    scalars[param.Name] = param.ParameterValue;
            // BasePropertyOverrides 只在「勾了 override」时才有意义,但本作的实例普遍不写
            // bOverride_* 标记,所以按「有值就用」处理:BLEND_Opaque 是 0,等于没覆盖。
            var overrides = mi.BasePropertyOverrides;
            if (overrides is not null)
            {
                if (overrides.BlendMode != EBlendMode.BLEND_Opaque) blend = overrides.BlendMode;
                if (overrides.OpacityMaskClipValue > 0) maskClip = overrides.OpacityMaskClipValue;
            }
        }
        return new MaterialInfo(name, textures, vectors, scalars, blend, maskClip, parents);
    }
}
