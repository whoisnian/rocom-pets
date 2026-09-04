// 眼神曲线与材质槽的配对调查(`--probe-face <资产名|ALL>`)。
//
// 起因:全库的 `EC_*` 曲线不止 `EC_Eye`/`EC_Mouth` 两条,还有 `EC_Eye_1`/`EC_Eye_2`/
// `EC_Eye_By`/`EC_Mouth_By`/`EC_Dynamic1..3` 等一批。要接它们就得先回答一个问题:
// **一条带后缀的曲线,刷的是哪个材质槽的 `Number`?**
//
// 这个探针把三样东西摆在一起看:① 网格的材质槽(**按槽序**);② 每段动画里出现的
// `EC_*` 曲线;③ 盖在整段上的 `ANS_SetFacialExpressionIntegrated` 通知实例的属性
// (`FacialExpressionConfigs`:类型 + 下标)。
//
// 只读、不写文件,输出给人看。

using CUE4Parse.FileProvider.Vfs;
using CUE4Parse.UE4.Assets.Exports.Animation;
using CUE4Parse.UE4.Assets.Exports.SkeletalMesh;

namespace RocomPets.Export;

public static class FaceProbe
{
    private const string PetsRoot = "NRC/Content/ArtRes/AnimSequence/Pets";

    public static void Run(AbstractVfsFileProvider provider, string asset)
    {
        if (asset.Equals("ALL", StringComparison.OrdinalIgnoreCase)) { Survey(provider); return; }

        var assetDir = $"{PetsRoot}/{asset}";
        var mesh = LoadMesh(provider, assetDir);
        if (mesh is null) { Console.Error.WriteLine($"{assetDir} 下没有 SKM_*"); return; }
        Console.WriteLine($"=== {asset} 的材质槽(按槽序)");
        for (var i = 0; i < mesh.Materials.Length; i++)
            Console.WriteLine($"  [{i}] {mesh.Materials[i].Name}");

        Console.WriteLine("=== 动画");
        foreach (var path in Textures.TopLevelFiles(provider, $"{assetDir}/Animation")
                     .OrderBy(p => p, StringComparer.OrdinalIgnoreCase))
        {
            UAnimSequence sequence;
            try { sequence = provider.LoadPackageObject<UAnimSequence>(path[..path.LastIndexOf('.')]); }
            catch (Exception e) { Console.WriteLine($"  {Path.GetFileNameWithoutExtension(path)} 读不了: {e.Message}"); continue; }

            var curves = (sequence.CompressedCurveData?.FloatCurves ?? [])
                .Select(c => c.CurveName.Text)
                .Where(n => n.StartsWith("EC_", StringComparison.OrdinalIgnoreCase))
                .ToArray();
            if (curves.Length == 0) continue;
            Console.WriteLine($"  {Path.GetFileNameWithoutExtension(path)}  曲线: {string.Join(",", curves)}");
            foreach (var curve in sequence.CompressedCurveData?.FloatCurves ?? [])
            {
                if (!curve.CurveName.Text.StartsWith("EC_", StringComparison.OrdinalIgnoreCase)) continue;
                var keys = curve.FloatCurve?.Keys ?? [];
                Console.WriteLine($"      {curve.CurveName.Text}: "
                    + string.Join(" ", keys.Select(k => $"{k.Time:F2}={k.Value:F0}")));
            }
            foreach (var notify in sequence.Notifies ?? [])
            {
                var obj = notify.NotifyStateClass?.ResolvedObject?.Load() ?? notify.Notify?.ResolvedObject?.Load();
                if (obj is null) continue;
                var name = obj.ExportType;
                if (!name.Contains("Facial", StringComparison.OrdinalIgnoreCase)) continue;
                Console.WriteLine($"    {name} @{notify.GetTime():F2}s+{notify.Duration:F2}s");
                foreach (var tag in obj.Properties)
                {
                    if (tag.Tag?.GenericValue is CUE4Parse.UE4.Assets.Objects.UScriptArray array)
                    {
                        Console.WriteLine($"      {tag.Name.Text}:");
                        foreach (var item in array.Properties)
                        {
                            var fields = (item.GenericValue as CUE4Parse.UE4.Assets.Objects.FStructFallback)?.Properties
                                ?? ((item.GenericValue as CUE4Parse.UE4.Assets.Objects.FScriptStruct)?.StructType
                                    as CUE4Parse.UE4.Assets.Objects.FStructFallback)?.Properties;
                            if (fields is null) { Console.WriteLine($"        {item.GenericValue}"); continue; }
                            Console.WriteLine("        " + string.Join("  ",
                                fields.Select(f => $"{f.Name.Text}={f.Tag?.GenericValue}")));
                        }
                        continue;
                    }
                    Console.WriteLine($"      {tag.Name.Text} = {tag.Tag?.GenericValue?.ToString()}");
                }
            }
        }
    }

    /// 全库普查:带后缀的 `EC_*` 曲线各出现在哪些资产上,那些资产的材质槽长什么样。
    private static void Survey(AbstractVfsFileProvider provider)
    {
        var assets = new SortedSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var file in provider.Files.Values)
        {
            var path = file.Path;
            if (!path.StartsWith(PetsRoot + "/", StringComparison.OrdinalIgnoreCase)) continue;
            var tail = path[(PetsRoot.Length + 1)..];
            var slash = tail.IndexOf('/');
            if (slash > 0) assets.Add(tail[..slash]);
        }
        Console.WriteLine($"资产 {assets.Count} 个");
        foreach (var asset in assets)
        {
            var assetDir = $"{PetsRoot}/{asset}";
            var names = new SortedSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach (var path in Textures.TopLevelFiles(provider, $"{assetDir}/Animation"))
            {
                UAnimSequence sequence;
                try { sequence = provider.LoadPackageObject<UAnimSequence>(path[..path.LastIndexOf('.')]); }
                catch { continue; }
                foreach (var curve in sequence.CompressedCurveData?.FloatCurves ?? [])
                    if (curve.CurveName.Text.StartsWith("EC_", StringComparison.OrdinalIgnoreCase))
                        names.Add(curve.CurveName.Text);
            }
            if (names.Count == 0) continue;
            var mesh = LoadMesh(provider, assetDir);
            var slots = mesh is null
                ? "(没有网格)"
                : string.Join(",", mesh.Materials.Select(m => m.Name));
            Console.WriteLine($"{asset}\t{string.Join(",", names)}\t{slots}");
        }
    }

    private static USkeletalMesh? LoadMesh(AbstractVfsFileProvider provider, string assetDir)
    {
        var meshName = Textures.TopLevelFiles(provider, assetDir)
            .Select(Path.GetFileNameWithoutExtension)
            .Where(n => n is not null && n.StartsWith("SKM_", StringComparison.Ordinal))
            .OrderByDescending(n => n!.EndsWith("_Skin", StringComparison.Ordinal))
            .FirstOrDefault();
        if (meshName is null) return null;
        try { return provider.LoadPackageObject<USkeletalMesh>($"{assetDir}/{meshName}"); }
        catch { return null; }
    }
}
