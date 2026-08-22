// 异色(游戏里的 `MDT_SHINING`)。
//
// **异色不是着色,是整套材质替换。** 客户端 `PetMutationUtils.SetColorDiffMutation` 把网格
// 每个槽位的材质换成 `character.DiffMaterials[EPetMaterialDifferenceType.ColorDiff]` 里的那一份,
// 而那份清单**写在宠物蓝图上**(`BP_Pet_<资产>` 的 `DiffMaterials`),内容是
// `<资产>/Yise/Mat/MI_…_101_*` —— 美术为这只宠物另做的一套材质与贴图。
//
// 所以这一层没有任何逆向:读蓝图拿清单 → 走**和默认材质完全相同**的那条解析链
// (`Materials.Load` / `Program.BuildMaterials`)→ 写进 manifest 的 `[forms.shiny_materials]`。
// 运行时按同一个 glb 画,只是查材质表时查另一张。
//
// 一条要点:`DiffMaterials` 的两张清单是**按槽序**对齐的,而 glb 里的材质名取自默认那套。
// 所以导出来之后要把键换回默认槽的名字,否则运行时按 glb 材质名查不到任何一条。

using CUE4Parse.FileProvider.Vfs;
using CUE4Parse.UE4.Assets.Exports;
using CUE4Parse.UE4.Assets.Exports.Material;
using CUE4Parse.UE4.Assets.Objects;
using CUE4Parse.UE4.Assets.Objects.Properties;

namespace RocomPets.Export;

public static class Shiny
{
    /// 蓝图里那张表的键名。`Default` 是普通外观,`ColorDiff` 就是异色。
    private const string ColorDiffKey = "EPetMaterialDifferenceType::ColorDiff";
    private const string DefaultKey = "EPetMaterialDifferenceType::Default";

    /// 从宠物蓝图里读异色材质清单,返回「默认材质对象名 → 异色材质」。
    ///
    /// 拿不到就返回空表 —— **多数宠物没有异色**(全库 3297 行 `MODEL_CONF` 里只有 177 行的
    /// `shiny_icon` 与普通图不同),没有不是错误。
    public static Dictionary<string, UMaterialInstance> Materials(
        AbstractVfsFileProvider provider,
        string? blueprintPath,
        List<string> warnings)
    {
        var result = new Dictionary<string, UMaterialInstance>(StringComparer.OrdinalIgnoreCase);
        if (string.IsNullOrEmpty(blueprintPath)) return result;

        UObject? cdo;
        try
        {
            // `MODEL_CONF.path` 给的是 `Blueprint'/Game/…/BP_X.BP_X_C'`,末尾那个 `_C`
            // 是生成类;`DiffMaterials` 挂在它的 **CDO** 上,而 CDO 的导出名是
            // `Default__BP_X_C` —— 和包名不同,所以 `LoadPackageObject(包路径)` 取不到
            // (它按包名找同名导出)。这里改成扫整个包的导出,挑 `Default__` 那个。
            cdo = provider.LoadPackage(PackagePath(blueprintPath))
                .GetExports()
                .FirstOrDefault(o => o.Name.StartsWith("Default__", StringComparison.Ordinal));
        }
        catch (Exception e)
        {
            warnings.Add($"异色:蓝图 {blueprintPath} 读不了({e.Message})");
            return result;
        }
        if (cdo is null)
        {
            warnings.Add($"异色:{blueprintPath} 里没有 Default__ 导出(CDO),跳过");
            return result;
        }
        var defaults = SlotList(cdo, DefaultKey);
        var colorDiff = SlotList(cdo, ColorDiffKey);
        if (colorDiff.Count == 0) return result;
        if (defaults.Count != colorDiff.Count)
        {
            // 两张清单按槽序对齐是这条路成立的前提。对不上就整只不做异色 ——
            // 错位地换材质会得到「身体套了眼睛的材质」那种结果,比没有异色糟得多。
            warnings.Add(
                $"异色:蓝图里 Default({defaults.Count})与 ColorDiff({colorDiff.Count})槽数对不上,跳过");
            return result;
        }

        for (var i = 0; i < colorDiff.Count; i++)
        {
            var key = ObjectName(defaults[i]);
            if (key is null) continue;
            try
            {
                if (Load(provider, colorDiff[i]) is { } material) result[key] = material;
                else warnings.Add($"异色:{colorDiff[i]} 不是材质实例或在 pak 里没有资产");
            }
            catch (Exception e)
            {
                warnings.Add($"异色材质 {colorDiff[i]} 读不了: {e.Message}");
            }
        }
        return result;
    }

    /// 解析成 `Materials.Load` 那样的表(键是**默认**槽的对象名,glb 里用的就是它)。
    public static Dictionary<string, MaterialInfo> Resolve(
        Dictionary<string, UMaterialInstance> materials,
        List<string> warnings)
    {
        var result = new Dictionary<string, MaterialInfo>(StringComparer.OrdinalIgnoreCase);
        foreach (var (key, material) in materials)
        {
            try
            {
                result[key] = Export.Materials.ResolveInstance(key, material);
            }
            catch (Exception e)
            {
                warnings.Add($"异色材质 {material.Name} 解析失败: {e.Message}");
            }
        }
        return result;
    }

    /// 取 `DiffMaterials[key].Materials` —— 一组 `TSoftObjectPtr<UMaterialInterface>`。
    ///
    /// 两处形状要点(都是实测出来的,不是照 UE 的类型名想当然):
    /// ① `DiffMaterials` 要用 `GetOrDefault<UScriptMap>` 取,`TryGetValue` 拿不到;
    /// ② map 的值是 `FScriptStruct`,**结构体本身在它的 `StructType` 里**,
    ///    直接往 `FStructFallback` 上匹配会一条都取不到(整个异色静默变成"这只没有")。
    private static List<string> SlotList(UObject cdo, string key)
    {
        var list = new List<string>();
        var map = cdo.GetOrDefault<UScriptMap>("DiffMaterials");
        if (map is null) return list;
        foreach (var pair in map.Properties)
        {
            if (pair.Key?.GenericValue?.ToString() != key) continue;
            if (pair.Value?.GenericValue is not FScriptStruct script) continue;
            if (script.StructType is not FStructFallback entry) continue;
            var array = entry.GetOrDefault<UScriptArray>("Materials");
            if (array is null) continue;
            foreach (var item in array.Properties)
                if (SoftPath(item) is { } path)
                    list.Add(path);
        }
        return list;
    }

    /// `TSoftObjectPtr` 的资产路径,形如 `/Game/…/MI_X.MI_X`。
    private static string? SoftPath(FPropertyTagType? property)
    {
        var value = property?.GenericValue?.ToString();
        return string.IsNullOrEmpty(value) ? null : value;
    }

    private static UMaterialInstance? Load(AbstractVfsFileProvider provider, string softPath) =>
        provider.LoadPackageObject(PackagePath(softPath)) as UMaterialInstance;

    /// `Blueprint'/Game/A/B.B_C'`、`/Game/A/B.B` → `NRC/Content/A/B`。
    /// `/Game/` 是 UE 的项目内容根,在本作的解包树里就是 `NRC/Content/`。
    private static string PackagePath(string reference)
    {
        var quoted = reference.Split('\'');
        var inner = quoted.Length >= 2 ? quoted[1] : reference;
        var dot = inner.IndexOf('.');
        if (dot >= 0) inner = inner[..dot];
        return inner.StartsWith("/Game/", StringComparison.OrdinalIgnoreCase)
            ? "NRC/Content/" + inner["/Game/".Length..]
            : inner;
    }

    /// `/Game/…/MI_X.MI_X` → `MI_X`。glb 里的材质名就是这个对象名。
    private static string? ObjectName(string softPath)
    {
        var dot = softPath.LastIndexOf('.');
        var name = dot >= 0 ? softPath[(dot + 1)..] : softPath[(softPath.LastIndexOf('/') + 1)..];
        return string.IsNullOrEmpty(name) ? null : name;
    }
}
