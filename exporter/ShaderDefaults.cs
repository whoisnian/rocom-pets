// 读**实机那份 cooked 排列自带的 uniform 参数表**,当作「没人覆盖时 GPU 拿到什么」的答案。
//
// 为什么需要:`RootDefaults`(根材质的 `CachedExpressionData`)按**参数名**建表,而
// 一个根材质里**允许有两个同名参数**。`M_P_Object` 里就有两个都叫 `FlowColor` 的向量:
//
//   * `CachedExpressionData` 留下的那条(GUID `C0DE08B7…`)默认 **(0.5, 0, 0.6)** —— 紫;
//   * 实机排列(quality=Num / lod=0 / dsid=0,波波拉 `_By` 的 resource `0F1003EB…`)
//     绑的却是另一条,默认 **(1, 1, 1)** —— 白。
//
// 哪一条是真的,汇编说了算:加性流动层那一句是
//
//     mul r8.xyz, r8.xyzx, cb6[8].xyzx        ← PS 49966 第 116 行
//
// 而同一份 resource 的 `vector-slot[8]` 字节码是 `04 02 00 …` ⇒ 取 **vector-param[2]**,
// 也就是默认 (1,1,1) 的那条。**波波拉与圣水守护的水环因此被我们涂成了紫色**
// (水灵没事是因为它的实例把 `FlowColor` 显式写成了 (1,1,1))。
//
// 所以查参数默认值的顺序是:**实例链 → 这里(实机排列的编译期默认)→ `RootDefaults`**。
// 编译期表只列这条排列**用得上**的参数,列不到的仍旧回退到 `RootDefaults`。
//
// 代价实测:整条 `--species` 导出打开 `ReadShaderMaps` 后 15.6s → 16.1s(+3%)。

using CUE4Parse.FileProvider;
using CUE4Parse.UE4.Assets.Exports.Material;

namespace RocomPets.Export;

/// 一份材质在**实机排列**下的参数默认值(参数名 → 值)。
public record ShaderDefaults(
    Dictionary<string, float[]> Vectors,
    Dictionary<string, float> Scalars)
{
    public static readonly ShaderDefaults Empty = new([], []);
}

public static class ShaderMapDefaults
{
    private static readonly Dictionary<string, ShaderDefaults> Cache = new(StringComparer.OrdinalIgnoreCase);

    /// 排列四元组的后两项 `(LODUsed, DynamicSwitchId)` —— CUE4Parse 不解这两个字段,
    /// 只能回到 uexp 的原始字节:它们是 `CookedShaderMapIdHash` 前面那 24 字节
    /// (6 个 uint32)的第 4、5 项。与 `MaterialProbe.PermutationKey` 是同一段判据。
    private static (uint Lod, uint Dsid)? PermutationKey(byte[]? raw, string? sha)
    {
        if (raw is null || string.IsNullOrEmpty(sha)) return null;
        byte[] needle;
        try { needle = Convert.FromHexString(sha); }
        catch { return null; }
        for (var at = 24; at + needle.Length <= raw.Length; at++)
        {
            var hit = true;
            for (var k = 0; k < needle.Length && hit; k++)
                if (raw[at + k] != needle[k]) hit = false;
            if (hit) return (BitConverter.ToUInt32(raw, at - 12), BitConverter.ToUInt32(raw, at - 8));
        }
        return null;
    }

    /// 取这份材质**实机那条排列**(quality=Num ∧ LODUsed=0 ∧ DSId=0)的编译期参数默认值。
    ///
    /// 拿不到就返回 `Empty` —— 调用方照旧回退 `RootDefaults`,行为与加这一层之前一致。
    /// 拿不到的两种情形都见过:① 材质没有内联 shader map(`MI_Wor_LangZhu1_001_Fx`,
    /// 它的 uexp 里一条 map 哈希都没有);② `ReadShaderMaps` 没打开(导出器以外的调用)。
    public static ShaderDefaults Of(UMaterialInterface material)
    {
        var key = material.GetPathName();
        lock (Cache)
        {
            if (Cache.TryGetValue(key, out var hit)) return hit;
            var parsed = Parse(material);
            Cache[key] = parsed;
            return parsed;
        }
    }

    private static ShaderDefaults Parse(UMaterialInterface material)
    {
        if (material.LoadedMaterialResources.Count == 0) return ShaderDefaults.Empty;
        byte[]? raw = null;
        if (material.Owner?.Provider is IFileProvider provider)
        {
            var path = material.Owner.Name;
            try { raw = provider.SaveAsset(path + ".uexp"); }
            catch { raw = null; }
        }
        foreach (var resource in material.LoadedMaterialResources)
        {
            if (resource.LoadedShaderMap is not { } map) continue;
            // **CUE4Parse 对 `GAME_RocoKingdomWorld` 会把这两个字段对调**,对我们这份包是反的
            // (核对过程见 docs/findings.md §1.1「排列标签」);这里换回来,与探针同一套。
            var quality = (EMaterialQualityLevel) (int) map.ShaderMapId.FeatureLevel;
            if (quality != EMaterialQualityLevel.Num) continue;
            if (PermutationKey(raw, map.ShaderMapId.CookedShaderMapIdHash?.ToString())
                is not { Lod: 0, Dsid: 0 }) continue;
            if (map.Content is not FMaterialShaderMapContent content) continue;
            var set = content.MaterialCompilationOutput.UniformExpressionSet;
            var vectors = new Dictionary<string, float[]>(StringComparer.OrdinalIgnoreCase);
            var scalars = new Dictionary<string, float>(StringComparer.OrdinalIgnoreCase);
            // **同名重复参数在同一条排列里也可能同时出现**(`M_P_Object` 的两个 `FlowColor`
            // 在 dsid=4/6 那几条里就是一起编进来的)。那种情形无从分辨,取先出现的那条并
            // 保持沉默会让人以为没有歧义 —— 所以只在实机那条排列上用这张表,而它是单条的。
            foreach (var p in set.UniformVectorParameters)
            {
                var name = p.ParameterInfo?.Name.Text ?? p.ParameterName;
                if (string.IsNullOrEmpty(name)) continue;
                var v = p.DefaultValue;
                vectors.TryAdd(name, [v.R, v.G, v.B, v.A]);
            }
            foreach (var p in set.UniformScalarParameters)
            {
                var name = p.ParameterInfo?.Name.Text ?? p.ParameterName;
                if (string.IsNullOrEmpty(name)) continue;
                scalars.TryAdd(name, p.DefaultValue);
            }
            return new ShaderDefaults(vectors, scalars);
        }
        return ShaderDefaults.Empty;
    }
}
