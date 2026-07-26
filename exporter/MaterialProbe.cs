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
using CUE4Parse.UE4.Assets.Objects;
using CUE4Parse.UE4.Assets.Objects.Properties;

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
        const string petsRoot = "NRC/Content/ArtRes/AnimSequence/Pets";
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

    public static void Run(AbstractVfsFileProvider provider, string asset)
    {
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
            Console.WriteLine($"=== {obj.Name}({obj.ExportType})的原始属性树");
            DumpProperties(obj, "  ");
            return;
        }
        const string petsRoot = "NRC/Content/ArtRes/AnimSequence/Pets";
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
                Console.WriteLine($"    tex {param,-20} → {tex}");
            foreach (var (param, c) in info.Vectors.OrderBy(kv => kv.Key))
                Console.WriteLine($"    col {param,-20} = ({c[0]:0.###}, {c[1]:0.###}, {c[2]:0.###}, {c[3]:0.###})");
            foreach (var (param, v) in info.Scalars.OrderBy(kv => kv.Key))
                Console.WriteLine($"    num {param,-20} = {v:0.####}");
            // 根材质的默认值:实例与中间层都没覆盖的那些参数(顺父链看不到,见 RootDefaults.cs)
            if (info.RootDefaults is { } rd)
            {
                foreach (var (param, tex) in rd.Textures.OrderBy(kv => kv.Key))
                    if (!info.Textures.ContainsKey(param))
                        Console.WriteLine($"    根tex {param,-18} → {tex}");
                foreach (var (param, c) in rd.Vectors.OrderBy(kv => kv.Key))
                    if (!info.Vectors.ContainsKey(param))
                        Console.WriteLine($"    根col {param,-18} = ({c[0]:0.###}, {c[1]:0.###}, {c[2]:0.###}, {c[3]:0.###})");
            }
        }
        foreach (var w in warnings) Console.WriteLine($"  [warn] {w}");

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
}
