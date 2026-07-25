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
using CUE4Parse.UE4.Assets.Exports.Texture;
using CUE4Parse.UE4.Assets.Objects;

namespace RocomPets.Export;

/// 一个材质槽解析出来的结果。
public record MaterialInfo(
    string Name,
    /// 参数名 → 贴图对象路径(已顺父链合并,子覆盖父)。
    Dictionary<string, string> Textures,
    EBlendMode BlendMode,
    float OpacityMaskClipValue,
    /// 父链上所有材质的名字,由近及远;排查用。
    List<string> ParentChain)
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
}

public static class Materials
{
    /// UE 默认的遮罩阈值;材质没覆盖时用它。
    private const float DefaultMaskClip = 0.3333f;

    /// 解析一个资产目录下 `Mat/` 里的全部材质实例。键是材质名(与 glb 里的材质名一致)。
    public static Dictionary<string, MaterialInfo> Load(
        AbstractVfsFileProvider provider,
        string assetDir,
        List<string> warnings)
    {
        var result = new Dictionary<string, MaterialInfo>(StringComparer.OrdinalIgnoreCase);
        var matDir = $"{assetDir}/Mat";
        foreach (var path in Textures.TopLevelFiles(provider, matDir))
        {
            var name = Path.GetFileNameWithoutExtension(path);
            try
            {
                var material = provider.LoadPackageObject<UMaterialInstance>(path[..path.LastIndexOf('.')]);
                // **键用对象名而不是文件名。** 本作的 pak 里两者大小写能对不上,而且方向还不一致:
                // 喵呜的文件是 `MI_Gra_MiaoMiao2_001_By`、对象名是 `…Miaomiao2…`,魔力猫正好反过来。
                // glb 里的材质名取的是对象名,键不一致运行时就查不到 → 整只宠物一片都画不出来。
                var key = string.IsNullOrEmpty(material.Name) ? name : material.Name;
                result[key] = Resolve(key, material);
            }
            catch (Exception e)
            {
                warnings.Add($"材质 {name} 解析失败: {e.Message}");
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
            // BasePropertyOverrides 只在「勾了 override」时才有意义,但本作的实例普遍不写
            // bOverride_* 标记,所以按「有值就用」处理:BLEND_Opaque 是 0,等于没覆盖。
            var overrides = mi.BasePropertyOverrides;
            if (overrides is not null)
            {
                if (overrides.BlendMode != EBlendMode.BLEND_Opaque) blend = overrides.BlendMode;
                if (overrides.OpacityMaskClipValue > 0) maskClip = overrides.OpacityMaskClipValue;
            }
        }
        return new MaterialInfo(name, textures, blend, maskClip, parents);
    }
}
