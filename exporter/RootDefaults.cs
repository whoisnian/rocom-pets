// 读**根材质**(`UMaterial`)的参数默认值。
//
// 为什么需要:参数是顺 `Parent` 链继承的,但链只能走到根**之前** —— 根不是
// `UMaterialInstance`,`Materials.Resolve` 到它就停了。于是「只在根上给了默认、没有任何实例
// 覆盖过」的参数,导出器**完全看不见**。那两颗球的 `StarTex`(= `T_EMeng003`,球里那颗
// 单独在动的四角星)就是这么漏掉的,固有色候选(红橙 F94728、紫 64358B…)也在这儿。
//
// 数据在 cooked 包里:`UMaterial.CachedExpressionData.Parameters` 有三组有序数组
// (149 个标量 / 43 个向量 / 13 张贴图,以 M_P_Object_Trans 为例)。
//
// **名字被剥了**,只剩 `NameHashes`;但同结构里有一份与值数组**同序**的 `ExpressionGuids`,
// 而实例那边每条参数同时带名字和 `ExpressionGUID`。所以拿 data/param-guids.tsv
// (由 `--probe-material ALL` 全量扫实例生成)按 GUID 一对,就能给根默认值配上名字。
//
// **刻意不合并进 `MaterialInfo` 的 Textures/Vectors/Scalars。** 那些判据是「美术显式设了没有」,
// 混进根默认会把判据整片翻转 —— 例如 `StarStickTiling` 的根默认是 4,一混进去每个玻璃材质
// 都会被判成「开了星点层」。要用就显式查这里。

using CUE4Parse.FileProvider;
using CUE4Parse.FileProvider.Vfs;
using CUE4Parse.UE4.Assets.Exports.Material;
using CUE4Parse.UE4.Assets.Exports.Texture;
using CUE4Parse.UE4.Assets.Objects;
using CUE4Parse.UE4.Objects.Core.Misc;
using CUE4Parse.UE4.Objects.Core.Math;
using CUE4Parse.UE4.Objects.Engine;
using CUE4Parse.UE4.Objects.UObject;

namespace RocomPets.Export;

/// 一个根材质的参数默认值(参数名 → 值)。
public record RootDefaults(
    Dictionary<string, string> Textures,
    Dictionary<string, float[]> Vectors,
    Dictionary<string, float> Scalars)
{
    public static readonly RootDefaults Empty = new([], [], []);
}

public static class RootMaterial
{
    /// GUID → 参数名。由 `--probe-material ALL` 全量扫实例生成(exporter/data/param-guids.tsv),
    /// 因为根材质那边名字被剥了、只能靠 GUID 反查。
    private static readonly Lazy<Dictionary<string, string>> GuidNames = new(() =>
    {
        var map = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        var path = Path.Combine(AppContext.BaseDirectory, "data", "param-guids.tsv");
        if (!File.Exists(path)) path = Path.Combine("exporter", "data", "param-guids.tsv");
        if (!File.Exists(path)) return map;
        foreach (var line in File.ReadLines(path))
        {
            var parts = line.Split('\t');
            if (parts.Length == 2) map.TryAdd(parts[0].Trim(), parts[1].Trim());
        }
        return map;
    });

    private static readonly Dictionary<string, RootDefaults> Cache = new(StringComparer.OrdinalIgnoreCase);

    /// 顺 `Parent` 链走到根(第一个不是 `UMaterialInstance` 的),读它的默认值。
    public static RootDefaults Of(UMaterialInstance material)
    {
        object current = material;
        for (var depth = 0; depth < 8; depth++)
        {
            if (current is not UMaterialInstance mi) break;
            var parent = mi.Parent;
            if (parent is null) break;
            current = parent;
        }
        if (current is not UMaterial root) return RootDefaults.Empty;
        lock (Cache)
        {
            if (Cache.TryGetValue(root.Name, out var hit)) return hit;
            var parsed = Parse(root);
            Cache[root.Name] = parsed;
            return parsed;
        }
    }

    private static RootDefaults Parse(UMaterial root)
    {
        var cached = root.GetOrDefault<FStructFallback>("CachedExpressionData")
            ?.GetOrDefault<FStructFallback>("Parameters");
        if (cached is null) return RootDefaults.Empty;

        // RuntimeEntries 是**按参数类型分组**的:0=标量、1=向量、2=贴图(与 UE 的
        // EMaterialParameterType 同序),每组里的 ExpressionGuids 与对应的值数组同序。
        //
        // **它是 UE 的定长数组属性**(`FMaterialCachedParameterEntry RuntimeEntries[NumTypes]`),
        // 序列化成**三个同名属性、靠 ArrayIndex 区分**,不是一个 TArray ——
        // 所以 `GetOrDefault<FStructFallback[]>("RuntimeEntries")` 取不到任何东西(踩过)。
        var entries = cached.Properties
            .Where(p => p.Name.Text == "RuntimeEntries")
            .OrderBy(p => p.ArrayIndex)
            .Select(p => p.Tag?.GetValue(typeof(FStructFallback)) as FStructFallback)
            .ToArray();
        var textures = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        var vectors = new Dictionary<string, float[]>(StringComparer.OrdinalIgnoreCase);
        var scalars = new Dictionary<string, float>(StringComparer.OrdinalIgnoreCase);

        void Fill<T>(int group, IReadOnlyList<T> values, Action<string, T> add)
        {
            if (group >= entries.Length || entries[group] is null) return;
            var guids = entries[group]!.GetOrDefault<FGuid[]>("ExpressionGuids", []);
            for (var i = 0; i < guids.Length && i < values.Count; i++)
                if (GuidNames.Value.TryGetValue(guids[i].ToString(), out var name))
                    add(name, values[i]);
        }

        Fill(0, cached.GetOrDefault<float[]>("ScalarValues", []), (n, v) => scalars.TryAdd(n, v));
        Fill(1, cached.GetOrDefault<FLinearColor[]>("VectorValues", []),
            (n, c) => vectors.TryAdd(n, [c.R, c.G, c.B, c.A]));
        Fill(2, cached.GetOrDefault<FPackageIndex[]>("TextureValues", []), (n, t) =>
        {
            if (t?.Load<UTexture>()?.GetPathName() is { } p) textures.TryAdd(n, p);
        });
        return new RootDefaults(textures, vectors, scalars);
    }
}
