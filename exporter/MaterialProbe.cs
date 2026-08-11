// 材质调查用的一次性探针(`--probe-material <资产名>`)。
//
// 起因:`UMaterialInstance.Deserialize` 在本作抛 OverflowException,于是贴图槽只能按命名
// 约定硬接(见 Textures.cs),眼睛等指向共享图集的槽只能退用本体贴图,水蓝蓝/幽星光那种
// 半透与加色材质更是完全没法判。这个探针把材质对象**尽可能原样**打印出来,用来回答:
// ① 到底是哪一步溢出;② 绕过反序列化后能不能拿到贴图参数与混合模式。
//
// 只读、不写文件,输出给人看。

using CUE4Parse.FileProvider;
using CUE4Parse.FileProvider.Vfs;
using CUE4Parse.UE4.Assets.Exports;
using CUE4Parse.UE4.Assets.Exports.Material;
using CUE4Parse.UE4.Assets.Exports.SkeletalMesh;
using CUE4Parse.UE4.Assets.Exports.Texture;
using CUE4Parse.UE4.Assets.Objects;
using CUE4Parse.UE4.Assets.Objects.Properties;
using CUE4Parse.UE4.Objects.Core.Math;

namespace RocomPets.Export;

/// 一个特效父族的参数汇总(调查用)。
internal class EffectFamily
{
    public int Count;
    public string? Sample;
    public readonly HashSet<string> Blends = new(StringComparer.OrdinalIgnoreCase);
    public readonly HashSet<string> Textures = new(StringComparer.OrdinalIgnoreCase);
    public readonly HashSet<string> Vectors = new(StringComparer.OrdinalIgnoreCase);
    public readonly HashSet<string> Scalars = new(StringComparer.OrdinalIgnoreCase);
}

public static class MaterialProbe
{
    /// 全量普查:跑遍所有宠物资产,统计父材质种类、贴图参数名、混合模式,
    /// 以及**基色贴图有多少落在资产目录之外**(= 共享贴图,原来按命名约定接不到的那批)。
    public static void Survey(AbstractVfsFileProvider provider, int limit)
    {
        const string petsRoot = AssetRoots.Pets;
        var assets = provider.Files.Values
            .Select(f => f.Path)
            .Where(p => p.StartsWith(petsRoot + "/", StringComparison.OrdinalIgnoreCase))
            .Select(p =>
            {
                var rest = p[(petsRoot.Length + 1)..];
                var slash = rest.IndexOf('/');
                return slash < 0 ? null : rest[..slash];
            })
            .Where(a => a is not null)
            .Distinct(StringComparer.OrdinalIgnoreCase)
            .OrderBy(a => a, StringComparer.OrdinalIgnoreCase)
            .Take(limit)
            .ToList();
        Console.WriteLine($"普查 {assets.Count} 个资产目录");

        var parentCount = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
        var paramsByParent = new Dictionary<string, HashSet<string>>(StringComparer.OrdinalIgnoreCase);
        var blendCount = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
        var slotParam = new Dictionary<string, Dictionary<string, int>>(StringComparer.OrdinalIgnoreCase);
        var shared = new List<string>();
        var nonOpaque = new List<string>();
        var effectFamily = new Dictionary<string, EffectFamily>(StringComparer.OrdinalIgnoreCase);
        var failed = 0;
        var materialCount = 0;
        // 静态开关普查:开/关各多少,以及「开」的那些落在哪些材质上
        var switchOn = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
        var switchOff = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
        var switchSample = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        var switchWho = new Dictionary<string, List<string>>(StringComparer.OrdinalIgnoreCase);
        // GUID → 参数名。全量收一遍,用来给根材质 CachedExpressionData 里那些
        // 「名字被剥掉、只剩哈希」的默认值配名字(见 Materials.cs 的 ParameterGuids)
        var guidNames = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);

        foreach (var asset in assets)
        {
            var assetDir = $"{petsRoot}/{asset}";
            var warnings = new List<string>();
            Dictionary<string, MaterialInfo> mats;
            try { mats = Materials.Load(LoadMesh(provider, assetDir)!, warnings); }
            catch { failed++; continue; }
            failed += warnings.Count;
            foreach (var (name, info) in mats)
            {
                materialCount++;
                var root = info.ParentChain.Count > 0 ? info.ParentChain[^1] : "(无父)";
                parentCount[root] = parentCount.GetValueOrDefault(root) + 1;
                if (!paramsByParent.TryGetValue(root, out var set))
                    paramsByParent[root] = set = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
                foreach (var p in info.Textures.Keys) set.Add(p);
                blendCount[info.BlendMode.ToString()] = blendCount.GetValueOrDefault(info.BlendMode.ToString()) + 1;
                if (info.BlendMode != EBlendMode.BLEND_Opaque)
                    nonOpaque.Add($"{asset}/{name}: {info.BlendMode} 阈值={info.OpacityMaskClipValue:0.###}");

                // 槽名(材质名最后一段)→ 它有哪些贴图参数
                var slot = name.Split('_')[^1];
                if (!slotParam.TryGetValue(slot, out var counts))
                    slotParam[slot] = counts = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
                foreach (var p in info.Textures.Keys) counts[p] = counts.GetValueOrDefault(p) + 1;

                // 特效层:按父族攒参数名,看要写几套近似公式
                if (info.BaseColorTexture is null && !name.EndsWith("_Ol", StringComparison.OrdinalIgnoreCase))
                {
                    var family = info.ParentChain.Count > 0 ? info.ParentChain[^1] : "(无父)";
                    if (!effectFamily.TryGetValue(family, out var acc))
                        effectFamily[family] = acc = new EffectFamily();
                    acc.Count++;
                    acc.Sample ??= $"{asset}/{name}";
                    acc.Blends.Add(info.BlendMode.ToString());
                    foreach (var t in info.Textures.Keys) acc.Textures.Add(t);
                    foreach (var v in info.Vectors.Keys) acc.Vectors.Add(v);
                    foreach (var sc in info.Scalars.Keys) acc.Scalars.Add(sc);
                }

                foreach (var (pname, guid) in info.ParameterGuids) guidNames.TryAdd(guid, pname);

                foreach (var (sw, on) in info.Switches)
                {
                    if (on)
                    {
                        switchOn[sw] = switchOn.GetValueOrDefault(sw) + 1;
                        switchSample.TryAdd(sw, $"{asset}/{name}");
                        if (!switchWho.TryGetValue(sw, out var who)) switchWho[sw] = who = [];
                        who.Add($"{asset}/{name}");
                    }
                    else switchOff[sw] = switchOff.GetValueOrDefault(sw) + 1;
                }

                // 基色候选落在别的目录 = 共享贴图
                foreach (var (param, tex) in info.Textures)
                    if (IsBaseColorParam(param) && !tex.StartsWith(assetDir + "/", StringComparison.OrdinalIgnoreCase))
                        shared.Add($"{asset}/{name}: {param} → {tex}");
            }
        }

        Console.WriteLine($"\n=== 材质 {materialCount} 个,解析失败 {failed} 个");
        Console.WriteLine("\n=== 根材质分布");
        foreach (var (k, v) in parentCount.OrderByDescending(kv => kv.Value))
            Console.WriteLine($"  {v,6}  {k}   贴图参数: {string.Join(", ", paramsByParent[k].OrderBy(x => x))}");
        Console.WriteLine($"\n=== GUID → 参数名({guidNames.Count} 条;拿去给根材质的默认值配名字)");
        foreach (var (g, n) in guidNames.OrderBy(kv => kv.Value, StringComparer.OrdinalIgnoreCase))
            Console.WriteLine($"  {g}\t{n}");
        Console.WriteLine("\n=== 静态开关(「这个特性到底开没开」的明写答案)");
        foreach (var sw in switchOn.Keys.Concat(switchOff.Keys).Distinct(StringComparer.OrdinalIgnoreCase)
                     .OrderByDescending(s => switchOn.GetValueOrDefault(s)))
            Console.WriteLine($"  开 {switchOn.GetValueOrDefault(sw),5} / 关 {switchOff.GetValueOrDefault(sw),5}  {sw}" +
                              (switchSample.TryGetValue(sw, out var s) ? $"   例: {s}" : ""));
        // 开着的数量少的,把是谁全列出来 —— 这批正是「哪些材质真的启用了某个特性」
        foreach (var (sw, who) in switchWho.Where(kv => kv.Value.Count <= 20).OrderBy(kv => kv.Value.Count))
            Console.WriteLine($"    [{sw}] 开在: {string.Join(", ", who)}");
        Console.WriteLine("\n=== 混合模式分布");
        foreach (var (k, v) in blendCount.OrderByDescending(kv => kv.Value))
            Console.WriteLine($"  {v,6}  {k}");
        Console.WriteLine("\n=== 非不透明的材质(逐个列出:半透/加色的范围有多大)");
        foreach (var s in nonOpaque) Console.WriteLine("  " + s);
        Console.WriteLine("\n=== 材质槽 → 贴图参数");
        foreach (var (slot, counts) in slotParam.OrderByDescending(kv => kv.Value.Values.Sum()).Take(14))
            Console.WriteLine($"  {slot,-12} {string.Join(", ", counts.OrderByDescending(c => c.Value).Select(c => $"{c.Key}×{c.Value}"))}");
        // 特效层按「父材质族」归类:每一族要写一个近似公式,所以先看有几族、各多少
        Console.WriteLine("\n=== 特效层(无基色参数)按父材质族分布");
        foreach (var (k, v) in effectFamily.OrderByDescending(kv => kv.Value.Count))
        {
            Console.WriteLine($"  {v.Count,5} 个  父族 {k}");
            Console.WriteLine($"        贴图参数: {string.Join(", ", v.Textures.OrderBy(x => x))}");
            Console.WriteLine($"        颜色参数: {string.Join(", ", v.Vectors.OrderBy(x => x))}");
            Console.WriteLine($"        标量参数: {string.Join(", ", v.Scalars.OrderBy(x => x).Take(12))}");
            Console.WriteLine($"        混合: {string.Join(", ", v.Blends.OrderBy(x => x))}  例: {v.Sample}");
        }

        Console.WriteLine($"\n=== 基色指向共享贴图(资产目录之外)的 {shared.Count} 处");
        foreach (var s in shared.Take(20)) Console.WriteLine("  " + s);

        // 决定性对比:材质给出的基色贴图 vs 现在按命名约定找到的
        Console.WriteLine("\n=== 材质解析 vs 命名约定(只看有基色参数的材质)");
        int same = 0, matOnly = 0, convOnly = 0, differ = 0, neither = 0;
        var examples = new List<string>();
        foreach (var asset in assets)
        {
            var assetDir = $"{petsRoot}/{asset}";
            var texNames = Textures.TopLevelFiles(provider, $"{assetDir}/Tex")
                .Select(Path.GetFileNameWithoutExtension)
                .Where(n => n is not null)
                .ToList();
            Dictionary<string, MaterialInfo> mats;
            try { mats = Materials.Load(LoadMesh(provider, assetDir)!, []); }
            catch { continue; }
            foreach (var (name, info) in mats)
            {
                if (name.EndsWith("_Ol", StringComparison.OrdinalIgnoreCase)) continue; // 游戏自带描边材质,我们不用
                var slot = name.Split('_')[^1];
                var fromMaterial = info.Textures
                    .Where(kv => IsBaseColorParam(kv.Key))
                    .Select(kv => Path.GetFileNameWithoutExtension(kv.Value.Split('.')[0]))
                    .FirstOrDefault();
                // 命名约定:`T_<任意>_<槽>_D`,大小写不敏感(现在运行时就是这么找的)
                var fromConvention = texNames.FirstOrDefault(n =>
                {
                    var parts = n!.Split('_');
                    return parts.Length >= 3
                           && parts[^1].Equals("D", StringComparison.OrdinalIgnoreCase)
                           && parts[^2].Equals(slot, StringComparison.OrdinalIgnoreCase);
                });
                if (fromMaterial is null && fromConvention is null) neither++;
                else if (fromMaterial is null) convOnly++;
                else if (fromConvention is null)
                {
                    matOnly++;
                    if (examples.Count < 12) examples.Add($"  [只有材质给得出] {asset}/{name} → {fromMaterial}");
                }
                else if (fromMaterial.Equals(fromConvention, StringComparison.OrdinalIgnoreCase)) same++;
                else
                {
                    differ++;
                    if (examples.Count < 12) examples.Add($"  [两者不同] {asset}/{name}: 材质={fromMaterial} 约定={fromConvention}");
                }
            }
        }
        Console.WriteLine($"  一致 {same} / 只有材质给得出 {matOnly} / 只有约定给得出 {convOnly} / 两者不同 {differ} / 都没有 {neither}");
        foreach (var e in examples) Console.WriteLine(e);
    }

    /// 找资产目录直属的 `SKM_*` 蒙皮网格(与导出器同一套规则)。
    private static USkeletalMesh? LoadMesh(AbstractVfsFileProvider provider, string assetDir)
    {
        var candidates = Textures.TopLevelFiles(provider, assetDir)
            .Select(Path.GetFileNameWithoutExtension)
            .Where(n => n is not null && n.StartsWith("SKM_", StringComparison.OrdinalIgnoreCase))
            .Distinct(StringComparer.OrdinalIgnoreCase)
            .OrderByDescending(n => n!.EndsWith("_Skin", StringComparison.OrdinalIgnoreCase))
            .ToList();
        if (candidates.Count == 0) return null;
        return provider.LoadPackageObject<USkeletalMesh>($"{assetDir}/{candidates[0]}");
    }

    /// 哪些参数名承载「基色」。按普查结果定:本体是 BaseTex,眼/嘴是 EyeTex。
    private static bool IsBaseColorParam(string param) =>
        param.Equals("BaseTex", StringComparison.OrdinalIgnoreCase)
        || param.Equals("EyeTex", StringComparison.OrdinalIgnoreCase);

    /// 全库普查描边材质(`--probe-material OUTLINES`)。
    ///
    /// 想回答的是「实机的描边到底有多宽、是不是逐宠物调的」。判据不能只看实例覆盖了什么:
    /// **实例里的覆盖是按参数名查根材质的**,名字对不上就是一条死设定。本作正好有这么一条
    /// (`OutLine Offset`),而且几乎每份 `_Ol` 都写了 0 —— 照着它做会得出「全库都没有描边」
    /// 的错误结论。所以这里对每个标量同时打印:链上的覆盖值、根材质默认值,以及
    /// **名字在根材质里存不存在**。
    private static void SurveyOutlines(AbstractVfsFileProvider provider)
    {
        const string petsRoot = AssetRoots.Pets;
        var files = provider.Files.Values
            .Select(f => f.Path)
            .Where(p => p.StartsWith(petsRoot + "/", StringComparison.OrdinalIgnoreCase)
                        && p.EndsWith("_Ol.uasset", StringComparison.OrdinalIgnoreCase))
            .Distinct(StringComparer.OrdinalIgnoreCase)
            .OrderBy(p => p, StringComparer.OrdinalIgnoreCase)
            .ToList();
        Console.WriteLine($"=== {files.Count} 份 _Ol 描边材质");

        string[] watch =
        [
            "OutlineWidthPC", "MaxWidthScale", "MinWidthScale", "OutlineOffset", "OutLine Offset",
            "Outline Intensity", "UseNormalVector", "IgnoreVertexColor", "DistanceUniform",
            "描边中心剔除范围", "MinID",
        ];
        // 参数名 → 「有效值 → 命中数」。有效值 = 链上覆盖过且名字在根里存在,否则根默认。
        var effective = watch.ToDictionary(w => w, _ => new Dictionary<string, int>(), StringComparer.OrdinalIgnoreCase);
        var deadNames = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
        var parents = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
        var sample = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        var failed = 0;

        foreach (var path in files)
        {
            UMaterialInstance? mi;
            try { mi = provider.LoadPackageObject(path[..path.LastIndexOf('.')]) as UMaterialInstance; }
            catch { failed++; continue; }
            if (mi is null) { failed++; continue; }

            // 顺父链合并:从最远的祖先开始写,近的覆盖远的(与 Materials.Resolve 同一套规则)
            var chain = new List<UMaterialInstance>();
            for (var cur = mi; cur is not null && chain.Count < 8; cur = cur.Parent as UMaterialInstance)
                chain.Add(cur);
            var scalars = new Dictionary<string, float>(StringComparer.OrdinalIgnoreCase);
            for (var i = chain.Count - 1; i >= 0; i--)
                foreach (var p in chain[i].GetOrDefault<FScalarParameterValue[]>("ScalarParameterValues", []))
                    if (!string.IsNullOrEmpty(p.Name)) scalars[p.Name] = p.ParameterValue;

            var root = RootMaterial.Of(mi);
            var rootName = chain[^1].Parent?.Name ?? "(无根)";
            parents[rootName] = parents.GetValueOrDefault(rootName) + 1;

            foreach (var name in watch)
            {
                var live = root.Scalars.ContainsKey(name);
                var value = live && scalars.TryGetValue(name, out var v) ? v
                    : root.Scalars.TryGetValue(name, out var d) ? d
                    : float.NaN;
                var key = float.IsNaN(value) ? "(根材质没有这个参数)" : $"{value:0.####}";
                if (scalars.ContainsKey(name) && !live)
                {
                    deadNames[name] = deadNames.GetValueOrDefault(name) + 1;
                    sample.TryAdd(name, Path.GetFileNameWithoutExtension(path));
                }
                effective[name][key] = effective[name].GetValueOrDefault(key) + 1;
                sample.TryAdd($"{name}={key}", Path.GetFileNameWithoutExtension(path));
            }
        }

        Console.WriteLine($"读取失败 {failed} 份;根材质分布:" +
                          string.Join("、", parents.Select(kv => $"{kv.Key} × {kv.Value}")));
        Console.WriteLine("\n--- 有效值分布(链上覆盖 → 根默认)");
        foreach (var name in watch)
            Console.WriteLine($"  {name,-22} " + string.Join("  ",
                effective[name].OrderByDescending(kv => kv.Value)
                    .Select(kv => $"{kv.Key} × {kv.Value}"
                                  + (kv.Value > 8 ? "" : $"({sample[$"{name}={kv.Key}"]})"))));
        Console.WriteLine("\n--- 死设定(实例写了,但根材质里没有同名参数 ⇒ 运行时查不到,不生效)");
        foreach (var (name, n) in deadNames.OrderByDescending(kv => kv.Value))
            Console.WriteLine($"  {name,-22} × {n}   例:{sample[name]}");
    }

    public static void Run(AbstractVfsFileProvider provider, string asset)
    {
        if (asset.StartsWith("FIND:", StringComparison.OrdinalIgnoreCase))
        {
            var needle = asset[5..];
            foreach (var path in provider.Files.Values.Select(f => f.Path)
                         .Where(p => p.Contains(needle, StringComparison.OrdinalIgnoreCase))
                         .Distinct(StringComparer.OrdinalIgnoreCase)
                         .OrderBy(p => p, StringComparer.OrdinalIgnoreCase))
                Console.WriteLine(path);
            return;
        }
        // 材质 shader 里的 MaterialCollection0/1 只保留缓冲名，不保留资产名。
        // 把 pak 中所有 MPC 资产连同原始属性顺序列出来，才能用 cb 槽位反向确认是哪一份
        // collection；这也能区分“资产默认值”和“运行时由蓝图写入”的参数。
        if (asset.Equals("COLLECTIONS", StringComparison.OrdinalIgnoreCase))
        {
            var collections = provider.Files.Values
                .Select(file => file.Path)
                .Where(path => path.EndsWith(".uasset", StringComparison.OrdinalIgnoreCase))
                .Where(path =>
                {
                    var name = Path.GetFileNameWithoutExtension(path);
                    return name.StartsWith("MPC_", StringComparison.OrdinalIgnoreCase)
                           || name.Equals("Buzhuo_Transfom", StringComparison.OrdinalIgnoreCase)
                           || name.Equals("EnvParamCollection", StringComparison.OrdinalIgnoreCase);
                })
                .Distinct(StringComparer.OrdinalIgnoreCase)
                .OrderBy(path => path, StringComparer.OrdinalIgnoreCase)
                .ToList();
            Console.WriteLine($"=== Material parameter collections: {collections.Count}");
            foreach (var path in collections)
            {
                var objectPath = path[..path.LastIndexOf('.')];
                try
                {
                    var obj = provider.LoadPackageObject(objectPath);
                    Console.WriteLine($"\n──── {objectPath} ({obj.ExportType})");
                    DumpProperties(obj, "  ");
                }
                catch (Exception e)
                {
                    Console.WriteLine($"\n──── {objectPath}: {e.GetType().Name}: {e.Message}");
                }
            }
            return;
        }

        if (asset.Equals("OUTLINES", StringComparison.OrdinalIgnoreCase))
        {
            SurveyOutlines(provider);
            return;
        }
        if (asset == "ALL")
        {
            Survey(provider, int.MaxValue);
            return;
        }
        if (asset.StartsWith("ALL:", StringComparison.OrdinalIgnoreCase))
        {
            Survey(provider, int.Parse(asset[4..]));
            return;
        }
        // 带 `/` 的当**对象路径**直接加载 —— 用来看**根材质**(`UMaterial`,如 M_P_Object_Trans)。
        // 顺父链合并到根就停了(根不是 UMaterialInstance),所以只写在根上的参数默认值
        // 平时是看不见的;而 cooked 包里 `CachedExpressionData` 存着有序的参数名与默认值。
        if (asset.Contains('/'))
        {
            var obj = provider.LoadPackageObject(asset);
            if (Environment.GetEnvironmentVariable("PROBE_HASHES") is not null)
            {
                DumpParameterHashes(obj);
                return;
            }
            Console.WriteLine($"=== {obj.Name}({obj.ExportType})的原始属性树");
            DumpProperties(obj, "  ");
            return;
        }
        // 宠物与 NPC 两棵树同构,按资产名前缀挑(见 Npc.cs)
        var petsRoot = AssetRoots.RootOf(asset);
        var assetDir = $"{petsRoot}/{asset}";

        // 先看这只宠物的资产目录里都有什么,材质到底放在哪
        Console.WriteLine($"\n=== {assetDir} 下的文件(前 40 个)");
        var files = provider.Files.Values
            .Where(f => f.Path.StartsWith(assetDir + "/", StringComparison.OrdinalIgnoreCase))
            .Select(f => f.Path)
            .OrderBy(p => p, StringComparer.OrdinalIgnoreCase)
            .ToList();
        foreach (var path in files.Where(p => !p.Contains("/Animation/", StringComparison.OrdinalIgnoreCase)).Take(40))
            Console.WriteLine("  " + path);
        Console.WriteLine($"  (共 {files.Count} 个文件,其中 Animation/ 下 " +
                          $"{files.Count(p => p.Contains("/Animation/", StringComparison.OrdinalIgnoreCase))} 个)");

        // 名字含 `SKM_` 的资产不一定是几何；例如 `LOD_SKM_*` 实际是
        // `SkeletalMeshLODSettings`。探针先打印真实导出类型，避免把 LOD 配置误认成
        // 第二套宠物网格，再把真正的 USkeletalMesh 各内部 LOD 并排列出。
        if (Environment.GetEnvironmentVariable("PROBE_MESHES") is not null)
        {
            Console.WriteLine("\n=== 蒙皮网格候选");
            foreach (var path in files
                         .Where(p => p.EndsWith(".uasset", StringComparison.OrdinalIgnoreCase))
                         .Where(p => Path.GetFileNameWithoutExtension(p)
                             .Contains("SKM_", StringComparison.OrdinalIgnoreCase))
                         .Distinct(StringComparer.OrdinalIgnoreCase))
            {
                var objectPath = path[..path.LastIndexOf('.')];
                try
                {
                    var obj = provider.LoadPackageObject(objectPath);
                    if (obj is not USkeletalMesh candidate)
                    {
                        Console.WriteLine($"  {Path.GetFileName(objectPath)}: {obj.ExportType}（不是几何）");
                        continue;
                    }
                    Console.WriteLine($"  {candidate.Name}: 材质 {candidate.Materials.Length}, " +
                                      $"LOD {candidate.LODModels?.Length ?? 0}, " +
                                      $"bounds={candidate.ImportedBounds.BoxExtent}");
                    Console.WriteLine("    slots: " + string.Join(", ",
                        candidate.Materials.Select((m, i) => $"{i}:{m?.Name ?? "(null)"}")));
                    if (candidate.LODModels is null) continue;
                    for (var i = 0; i < candidate.LODModels.Length; i++)
                    {
                        var lod = candidate.LODModels[i];
                        Console.WriteLine($"    LOD{i}: vertices={lod.NumVertices} " +
                                          $"triangles={lod.Sections.Sum(s => s.NumTriangles)} " +
                                          $"sections=[{string.Join(", ", lod.Sections.Select(s =>
                                              $"mat{s.MaterialIndex}:{s.NumTriangles}"))}]");
                    }
                }
                catch (Exception e)
                {
                    Console.WriteLine($"  {Path.GetFileName(objectPath)}: 读取失败 " +
                                      $"{e.GetType().Name}: {e.Message}");
                }
            }
        }

        // 材质候选:名字含 MI_ 或放在 Mat/ 目录下的 uasset
        var materials = files
            .Where(p => p.EndsWith(".uasset", StringComparison.OrdinalIgnoreCase))
            .Where(p => p.Contains("/Mat", StringComparison.OrdinalIgnoreCase)
                        || Path.GetFileName(p).StartsWith("MI_", StringComparison.OrdinalIgnoreCase)
                        || Path.GetFileName(p).StartsWith("M_", StringComparison.OrdinalIgnoreCase))
            // provider.Files 每个资产会按扩展名重复列出,去重免得同一个材质打三遍
            .Distinct(StringComparer.OrdinalIgnoreCase)
            // CG/ 是过场动画用的另一套,桌宠不用
            .Where(p => !p.Contains("/CG/", StringComparison.OrdinalIgnoreCase))
            .ToList();
        Console.WriteLine($"\n=== 材质候选 {materials.Count} 个");
        foreach (var path in materials) Console.WriteLine("  " + path);

        // 先打「解析后的结论」——这才是导出器要吃的东西
        Console.WriteLine("\n=== 解析结果(顺父链合并后)");
        var warnings = new List<string>();
        foreach (var (name, info) in Materials.Load(LoadMesh(provider, assetDir)!, warnings))
        {
            Console.WriteLine($"  {name}  混合={info.BlendMode} 遮罩阈值={info.OpacityMaskClipValue:0.####}");
            Console.WriteLine($"    父链: {string.Join(" ← ", info.ParentChain)}");
            // 静态开关放最前面:它是「这个特性到底开没开」的明写答案,比下面那堆参数值更有用
            if (info.Switches.Count > 0)
                Console.WriteLine("    开关 " + string.Join("  ",
                    info.Switches.OrderBy(kv => kv.Key).Select(kv => $"{kv.Key}={(kv.Value ? "开" : "关")}")));
            foreach (var (param, tex) in info.Textures.OrderBy(kv => kv.Key))
            {
                var objectPath = tex.Contains('.') ? tex[..tex.LastIndexOf('.')] : tex;
                var colorSpace = provider.LoadPackageObject(objectPath) is UTexture texture
                    ? $"  sRGB={(texture.SRGB ? 1 : 0)}"
                    : "";
                Console.WriteLine($"    tex {param,-20} → {tex}{colorSpace}");
            }
            foreach (var (param, c) in info.Vectors.OrderBy(kv => kv.Key))
                Console.WriteLine($"    col {param,-20} = ({c[0]:0.###}, {c[1]:0.###}, {c[2]:0.###}, {c[3]:0.###})");
            foreach (var (param, v) in info.Scalars.OrderBy(kv => kv.Key))
                Console.WriteLine($"    num {param,-20} = {v:0.####}");
            // 根材质的默认值:实例与中间层都没覆盖的那些参数(顺父链看不到,见 RootDefaults.cs)
            if (info.RootDefaults is { } rd)
            {
                // `PROBE_ROOT_ALL=1`:**不过滤**地把根材质的完整参数表打出来。
                // 平时那几行会跳过「实例已覆盖」的参数(因为那时根默认不是实际值),
                // 但要拿「这个图到底有哪些参数、按什么顺序」时,需要的恰恰是完整表 ——
                // 补丁表里的 `paramId` 疑似就是这张表某种顺序下的下标(见
                // rocom-capture/scripts/matparams.py)。
                if (Environment.GetEnvironmentVariable("PROBE_ROOT_ALL") is not null)
                {
                    Console.WriteLine($"    全根表: 贴图 {rd.Textures.Count} 向量 {rd.Vectors.Count} 标量 {rd.Scalars.Count}");
                    foreach (var (param, _) in rd.Textures) Console.WriteLine($"    ALLtex {param}");
                    foreach (var (param, _) in rd.Vectors) Console.WriteLine($"    ALLcol {param}");
                    foreach (var (param, _) in rd.Scalars) Console.WriteLine($"    ALLnum {param}");
                }
                foreach (var (param, tex) in rd.Textures.OrderBy(kv => kv.Key))
                    if (!info.Textures.ContainsKey(param))
                        Console.WriteLine($"    根tex {param,-18} → {tex}");
                foreach (var (param, c) in rd.Vectors.OrderBy(kv => kv.Key))
                    if (!info.Vectors.ContainsKey(param))
                        Console.WriteLine($"    根col {param,-18} = ({c[0]:0.###}, {c[1]:0.###}, {c[2]:0.###}, {c[3]:0.###})");
                // 标量默认值同样要能看到:汇编里读到一个 cb 标量槽却不知道是谁时,
                // 「实例没覆盖 ⇒ 根默认就是实际值」是唯一能对上号的线索(见 RootDefaults.cs)
                foreach (var (param, v) in rd.Scalars.OrderBy(kv => kv.Key))
                    if (!info.Scalars.ContainsKey(param))
                        Console.WriteLine($"    根num {param,-18} = {v:0.####}");
            }
        }
        foreach (var w in warnings) Console.WriteLine($"  [warn] {w}");

        // `uexp` 里会同时出现当前实例与父材质的 shader-map 哈希，单纯 memmem 会把它们
        // 全算成候选。cooked resource 本身保存着 (Quality, FeatureLevel, map hash)，这里把
        // 真正属于各材质资源的表打印出来，供 matshader.py 精确选排列。
        if (Environment.GetEnvironmentVariable("PROBE_SHADERS") is not null)
        {
            Console.WriteLine("\n=== Cooked shader resources");
            foreach (var path in materials)
            {
                var trimmed = path[..path.LastIndexOf('.')];
                try
                {
                    if (provider.LoadPackageObject(trimmed) is not UMaterialInterface material)
                        continue;
                    Console.WriteLine($"  {material.Name}: {material.LoadedMaterialResources.Count} resources");
                    for (var i = 0; i < material.LoadedMaterialResources.Count; i++)
                    {
                        var map = material.LoadedMaterialResources[i].LoadedShaderMap;
                        if (map is null)
                        {
                            Console.WriteLine($"    [{i}] (invalid)");
                            continue;
                        }
                        // `LayoutParams` 紧跟在 (Quality, Feature) 之后读。它是**判偏移对不对的
                        // 探针**:`MaxFieldAlignment` 该是 0xffffffff 或 4/8,`Flags` 该是个 1..31 的
                        // 小位掩码。这两个不合理 ⇒ 前面那两个枚举也读错了位置。
                        // `PROBE_RAWMAP=1`:回到 uexp 的原始字节,把 SHA 之前那几个 int 挖出来。
                        // **这是核对 (Quality, Feature) 偏移的唯一硬办法** —— CUE4Parse 对
                        // `GAME_RocoKingdomWorld` 有一步「把两者对调、再跳 16 字节」的特判,
                        // 而对调之后 FeatureLevel 会解成 SM6,与「整个项目只有一份
                        // `ShaderArchive-NRC-PCD3D_ES31`」矛盾。
                        if (Environment.GetEnvironmentVariable("PROBE_RAWMAP") is not null
                            && map.ShaderMapId.CookedShaderMapIdHash is { } sha)
                        {
                            var raw = provider.SaveAsset(trimmed + ".uexp");
                            var needle = Convert.FromHexString(sha.ToString());
                            for (var at = 0; at + needle.Length <= raw.Length; at++)
                            {
                                var hit = true;
                                for (var k = 0; k < needle.Length && hit; k++)
                                    if (raw[at + k] != needle[k]) hit = false;
                                if (!hit) continue;
                                var from = Math.Max(0, at - 24);
                                Console.WriteLine($"      raw@{at}: 前 24 字节 = " +
                                                  Convert.ToHexString(raw.AsSpan(from, at - from)) +
                                                  "  后 8 字节 = " +
                                                  Convert.ToHexString(raw.AsSpan(at + needle.Length,
                                                      Math.Min(8, raw.Length - at - needle.Length))));
                                break;
                            }
                        }
                        // **CUE4Parse 的 `GAME_RocoKingdomWorld` 特判会把这两个字段对调,
                        // 而对我们这份包来说那是反的 —— 这里换回来。** 核对过程见
                        // docs/design.md §1.1「排列标签」那节:SHA 之前的 24 字节是 6 个 uint32,
                        //     [第一个 int: 0 或 4 交替] [第二个 int: 恒 1] [1] [0xFFFFFFFF] [每两条 +1] [0]
                        // 而 SHA 之后紧跟 `08000000 29000000`(= MaxFieldAlignment 8 / Flags 0x29),
                        // 说明偏移本身没错。不对调 ⇒ quality ∈ {Low(0), Num(4)}、feature = 1 = ES3_1,
                        // 与「整个项目只有一份 `ShaderArchive-NRC-PCD3D_ES31`」一致;
                        // 对调 ⇒ feature 会解成 SM6,一个跑 ES3.1 的手游不会 cook 那个。
                        var quality = (EMaterialQualityLevel) (int) map.ShaderMapId.FeatureLevel;
                        var feature = (ERHIFeatureLevel) (int) map.ShaderMapId.QualityLevel;
                        var layout = map.ShaderMapId.LayoutParams;
                        Console.WriteLine(
                            $"    [{i}] quality={quality} feature={feature} " +
                            $"align=0x{layout?.MaxFieldAlignment:X} flags={layout?.Flags} " +
                            $"map={map.ShaderMapId.CookedShaderMapIdHash} " +
                            $"resource={map.ResourceHash}");
                        var detailIndexText = Environment.GetEnvironmentVariable("PROBE_SHADER_INDEX");
                        var wantsDetails = Environment.GetEnvironmentVariable("PROBE_SHADER_DETAILS") is not null
                                           && (detailIndexText is null
                                               || int.TryParse(detailIndexText, out var detailIndex)
                                               && detailIndex == i);
                        if (wantsDetails)
                        {
                            // 贴图参数数组的顺序就是材质 uniform-expression 的绑定顺序。
                            // 配合 DXBC 的 t 槽可以区分 BaseTex / MaskTex / RampTex；只按
                            // CachedReferencedTextures 猜顺序会把引擎纹理与材质纹理混在一起。
                            if (map.Content is not FMaterialShaderMapContent materialContent)
                                continue;
                            var expressions = materialContent.MaterialCompilationOutput.UniformExpressionSet;
                            Console.WriteLine(
                                $"      uniforms: vector={expressions.UniformVectorPreshaders.Length} " +
                                $"scalar={expressions.UniformScalarPreshaders.Length}");
                            Console.WriteLine("      collections: " + string.Join(", ",
                                expressions.ParameterCollections.Select(guid => guid.ToString())));
                            foreach (var (parameter, parameterIndex) in
                                     expressions.UniformVectorParameters.Select((value, index) => (value, index)))
                            {
                                var name = parameter.ParameterInfo?.Name.Text
                                           ?? parameter.ParameterName
                                           ?? "(unnamed)";
                                var value = parameter.DefaultValue;
                                Console.WriteLine(
                                    $"      vector-param[{parameterIndex}] {name}=" +
                                    $"({value.R:0.######},{value.G:0.######}," +
                                    $"{value.B:0.######},{value.A:0.######})");
                            }
                            foreach (var (parameter, parameterIndex) in
                                     expressions.UniformScalarParameters.Select((value, index) => (value, index)))
                            {
                                var name = parameter.ParameterInfo?.Name.Text
                                           ?? parameter.ParameterName
                                           ?? "(unnamed)";
                                Console.WriteLine(
                                    $"      scalar-param[{parameterIndex}] {name}={parameter.DefaultValue:0.######}");
                            }
                            var preshaderData = expressions.UniformPreshaderData.Data;
                            foreach (var (header, slot) in
                                     expressions.UniformVectorPreshaders.Select((value, index) => (value, index)))
                            {
                                Console.WriteLine(
                                    $"      vector-slot[{slot}] off={header.OpcodeOffset} " +
                                    $"size={header.OpcodeSize} code=" +
                                    Convert.ToHexString(preshaderData.AsSpan(
                                        checked((int) header.OpcodeOffset), checked((int) header.OpcodeSize))));
                            }
                            foreach (var (header, scalarIndex) in
                                     expressions.UniformScalarPreshaders.Select((value, index) => (value, index)))
                            {
                                Console.WriteLine(
                                    $"      scalar-slot[{scalarIndex}] off={header.OpcodeOffset} " +
                                    $"size={header.OpcodeSize} code=" +
                                    Convert.ToHexString(preshaderData.AsSpan(
                                        checked((int) header.OpcodeOffset), checked((int) header.OpcodeSize))));
                            }
                            for (var textureType = 0;
                                 textureType < expressions.UniformTextureParameters.Length;
                                 textureType++)
                            {
                                foreach (var texture in expressions.UniformTextureParameters[textureType])
                                {
                                    var parameter = texture.ParameterInfo?.Name.Text
                                                    ?? texture.ParameterName
                                                    ?? "(unnamed)";
                                    Console.WriteLine(
                                        $"      tex[{textureType}] {parameter}: " +
                                        $"index={texture.TextureIndex} sampler={texture.SamplerSource}");
                                }
                            }
                        }
                    }
                }
                catch (Exception e)
                {
                    Console.WriteLine($"  {Path.GetFileName(trimmed)}: shader resource 读取失败: " +
                                      $"{e.GetType().Name}: {e.Message}");
                }
            }
        }

        if (Environment.GetEnvironmentVariable("PROBE_RAW") is null)
        {
            Console.WriteLine("\n(原始属性树略;要看设 PROBE_RAW=1)");
            return;
        }

        foreach (var path in materials)
        {
            var trimmed = path[..path.LastIndexOf('.')];
            Console.WriteLine($"\n──── {Path.GetFileName(trimmed)}");
            // ① 强类型加载:复现 OverflowException,把完整堆栈打出来定位溢出点
            try
            {
                var obj = provider.LoadPackageObject(trimmed);
                Console.WriteLine($"  强类型加载 OK: {obj.GetType().Name} ExportType={obj.ExportType}");
                DumpProperties(obj, "  ");
            }
            catch (Exception e)
            {
                Console.WriteLine($"  强类型加载失败: {e.GetType().Name}: {e.Message}");
                Console.WriteLine("  " + string.Join("\n  ",
                    (e.StackTrace ?? "").Split('\n').Take(8).Select(l => l.TrimEnd())));
            }

            // ② 退一步:只读包、逐个 export 看能拿到什么(不触发那条会溢出的路径)
            try
            {
                var package = provider.LoadPackage(trimmed);
                Console.WriteLine($"  包内 export {package.GetExports().Count()} 个:");
                foreach (var export in package.GetExports())
                {
                    Console.WriteLine($"    · {export.Name} ({export.ExportType}) 属性 {export.Properties.Count} 条");
                    DumpProperties(export, "      ");
                }
            }
            catch (Exception e)
            {
                Console.WriteLine($"  读包也失败: {e.GetType().Name}: {e.Message}");
            }
        }
    }

    /// 递归打印属性树。参数值(贴图/标量/混合模式)都嵌在结构体里,不下钻就只能看到
    /// 一串 `FStructFallback`,所以这里按 tag 的实际类型逐层展开。
    private static void DumpProperties(UObject obj, string indent) =>
        DumpTags(obj.Properties, indent, 0);

    private static void DumpTags(IReadOnlyList<FPropertyTag> props, string indent, int depth)
    {
        if (depth > 4) return;
        foreach (var prop in props)
            DumpTag(prop.Name.Text, prop.Tag, indent, depth);
    }

    private static void DumpTag(string name, FPropertyTagType? tag, string indent, int depth)
    {
        switch (tag)
        {
            case StructProperty { Value: { } script }:
                Console.WriteLine($"{indent}{name} (struct {script.StructType?.GetType().Name}):");
                DumpStruct(script.StructType, indent + "  ", depth + 1);
                break;
            case ArrayProperty { Value: { } array }:
                Console.WriteLine($"{indent}{name} [{array.Properties.Count}]:");
                // 数组默认截到 32 项:根材质的 CachedExpressionData 有 149 个标量参数,
                // 想看全设 PROBE_ALL=1
                var cap = Environment.GetEnvironmentVariable("PROBE_ALL") is null ? 32 : int.MaxValue;
                for (var i = 0; i < array.Properties.Count && i < cap; i++)
                {
                    Console.WriteLine($"{indent}  [{i}]");
                    DumpTag("", array.Properties[i], indent + "    ", depth + 1);
                }
                break;
            default:
                Console.WriteLine($"{indent}{name} = {Shorten(tag?.GenericValue?.ToString())}");
                break;
        }
    }

    private static void DumpStruct(object? structType, string indent, int depth)
    {
        switch (structType)
        {
            case FStructFallback fallback:
                DumpTags(fallback.Properties, indent, depth);
                break;
            case FLinearColor color:
                Console.WriteLine($"{indent}({color.R:0.######}, {color.G:0.######}, " +
                                  $"{color.B:0.######}, {color.A:0.######})");
                break;
            case null:
                Console.WriteLine($"{indent}(null)");
                break;
            default:
                // 已知类型(FLinearColor / FGuid / FName 之类)直接打 ToString
                Console.WriteLine($"{indent}{Shorten(structType.ToString())}");
                break;
        }
    }

    private static string Shorten(string? s)
    {
        if (s is null) return "(null)";
        s = s.Replace('\n', ' ');
        return s.Length > 110 ? s[..110] + "…" : s;
    }

    /// 打印根材质 `CachedExpressionData` 里每组参数的 `(下标, NameHash, 默认值)`。
    ///
    /// **为什么要 NameHash**:cooked 包把参数名剥成了哈希(`ParameterInfos` 是空壳)。
    /// 但包的名字表里名字是全的 —— 把名字逐个哈希去对,就能把整组参数命名,
    /// 不再受 GUID 桥覆盖率的限制(GUID 桥只能命名「至少被某个实例覆盖过」的参数)。
    /// 值必须从这里打(C# 侧是完整浮点):属性树的 hex 转储是 8 位有损的,
    /// `StarColor` 的 (0.33, 0.67, **2.0**) 根本存不下。
    private static void DumpParameterHashes(UObject root)
    {
        var cached = root.GetOrDefault<FStructFallback>("CachedExpressionData")
            ?.GetOrDefault<FStructFallback>("Parameters");
        if (cached is null)
        {
            Console.Error.WriteLine("没有 CachedExpressionData.Parameters");
            return;
        }
        var entries = cached.Properties
            .Where(p => p.Name.Text == "RuntimeEntries")
            .OrderBy(p => p.ArrayIndex)
            .Select(p => p.Tag?.GetValue(typeof(FStructFallback)) as FStructFallback)
            .ToArray();
        var scalars = cached.GetOrDefault<float[]>("ScalarValues", []);
        var vectors = cached.GetOrDefault<FLinearColor[]>("VectorValues", []);
        Console.WriteLine($"=== {root.Name} 的参数哈希(组 0=标量 1=向量 2=贴图)");
        for (var g = 0; g < entries.Length; g++)
        {
            if (entries[g] is not { } e) continue;
            var hashes = e.GetOrDefault<ulong[]>("NameHashes", []);
            Console.WriteLine($"--- 组 {g}:{hashes.Length} 条");
            for (var i = 0; i < hashes.Length; i++)
            {
                var val = g == 0 && i < scalars.Length ? $"{scalars[i]:0.####}"
                    : g == 1 && i < vectors.Length
                        ? $"({vectors[i].R:0.####}, {vectors[i].G:0.####}, {vectors[i].B:0.####}, {vectors[i].A:0.####})"
                        : "";
                Console.WriteLine($"  [{i,3}] {hashes[i],22} {val}");
            }
        }
    }
}
