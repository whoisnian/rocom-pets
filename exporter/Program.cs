// rocom-pets 宠物包导出器(Phase 0 S3)。
//
//   dotnet run --project exporter -- --species 3001 --out /tmp/packs
//
// 输入:游戏 pak(资产,经 CUE4Parse)+ rocom-capture 解包出的配置 JSON(表结构,见 Config.cs)。
// 输出:一条进化链一个包目录 `<链名>/`,含 manifest.toml、每个形态的 model.glb 与贴图。
// 素材版权属原发行方:包是本地生成物,不入仓库、不分发(docs/design.md §11)。

using CUE4Parse.Compression;
using CUE4Parse.Encryption.Aes;
using CUE4Parse.FileProvider;
using CUE4Parse.FileProvider.Vfs;
using CUE4Parse.UE4.Assets.Exports.Animation;
using CUE4Parse.UE4.Assets.Exports.SkeletalMesh;
using CUE4Parse.UE4.Objects.Core.Math;
using CUE4Parse.UE4.Objects.Core.Misc;
using CUE4Parse.UE4.Objects.RenderCore;
using CUE4Parse.UE4.Versions;
using CUE4Parse_Conversion.Textures;
using System.Text;
using CUE4Parse_Conversion.Writers.Gltf;
using RocomPets.Export;
using Serilog;
using Serilog.Events;

const string usage = """
    用法: rocom-pets-export --species <宠物id>[,<id>…] [选项]

      --species <ids>   要导的宠物(给链上任一成员即可,自动补全整条进化链)
      --paks <路径>     游戏 pak 目录或 .apk(默认 ~/Downloads/rocom/Paks)
      --parsed <路径>   rocom-capture 的解包根,用于读配置 JSON
                        (默认 $ROCOM_PARSED 或 ~/Downloads/rocom/parsed)
      --out <目录>      包输出目录(默认 ./packs)
      --aes <hex>       pak 主密钥(默认内置)
      --lod <n>         用第几级 LOD(默认 0)
      --all-clips       导出 ANIM_CONF 里的全部动作,而不是桌宠动作白名单
      --all             导出全部宠物(按图鉴号归并成包);与 --species 二选一
      --index           只列出归并后的包名与形态构成(不碰 pak,不导东西)
      --limit <n>       配合 --all:只导前 n 条链(试跑用)
      --skip-existing   跳过已经有 manifest.toml 的包(增量重跑)
      -j <n>            并行度(默认 CPU 核数;每个并行任务会同时持有一个形态的数据,
                        内存吃紧就调小)
      --no-voice        不导叫声(默认导:要 vgmstream-cli 与 ffmpeg,缺了会自动跳过)
      --zip             额外打成 <链名>.rkpet(包目录仍然留着)
      --zip-only        同上,但打完就删掉包目录 —— 只要归档时用这个。
                        全量导出的产物从 3.3GB(目录)+2.0GB(归档)降到只剩 2.0GB;
                        --skip-existing 认得 .rkpet,所以照样能分批续跑
      -h, --help        本帮助

    批量导出会在输出目录写 report.txt:每个形态的动作命中/缺失、体积、警告,末尾是汇总。
    """;

// 桌宠真正用得到的动作:名字是 ANIM_ID_CONF 里的逻辑名。
// 其余(Attack/Skill/Hit/Die/CG 演出)对桌宠没用,占体积,默认不导。
string[] defaultClips =
[
    "Idle", "Walk", "Run", "Happy", "Anger", "Sad", "Fear", "Shock", "Show", "Relax", "Alert",
    "SleepStart", "SleepLoop", "SleepEnd", "SleepStand", "CallOut",
    // 拖放松手之后的落地(design.md §9 Phase 1 遗留)。ANIM_CONF 里有这个逻辑名,
    // 但原来不在白名单里 ⇒ 全库 831 个形态一个都没导出来。
    "JumpFall",
];

var home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
var paksPath = Path.Combine(home, "Downloads", "rocom", "Paks");
var parsedPath = Environment.GetEnvironmentVariable("ROCOM_PARSED")
                 ?? Path.Combine(home, "Downloads", "rocom", "parsed");
var outDir = Path.GetFullPath("packs");
var aesKey = "0x34254D23E47299B3B7F6C4CFDE9BD0688703446D9D8F37B2EBDDDE5B06ED5ADF";
var species = new List<int>();
var lodIndex = 0;
var allClips = false;
var noVoice = false;
var zip = false;
var zipOnly = false;
var all = false;
var limit = int.MaxValue;
var skipExisting = false;
var jobs = Environment.ProcessorCount;
var probeAsset = (string?)null;
var indexOnly = false;

for (var i = 0; i < args.Length; i++)
{
    switch (args[i])
    {
        case "--species":
            foreach (var part in Next(ref i).Split(',', StringSplitOptions.RemoveEmptyEntries))
                species.Add(int.Parse(part.Trim()));
            break;
        case "--paks": paksPath = Next(ref i); break;
        case "--parsed": parsedPath = Next(ref i); break;
        case "--out": outDir = Path.GetFullPath(Next(ref i)); break;
        case "--aes": aesKey = Next(ref i); break;
        case "--lod": lodIndex = int.Parse(Next(ref i)); break;
        case "--all-clips": allClips = true; break;
        case "--no-voice": noVoice = true; break;
        case "--all": all = true; break;
        case "--limit": limit = int.Parse(Next(ref i)); break;
        case "--skip-existing": skipExisting = true; break;
        case "-j": jobs = Math.Max(1, int.Parse(Next(ref i))); break;
        case "--zip": zip = true; break;
        case "--zip-only": zip = zipOnly = true; break;
        case "--probe-material": probeAsset = Next(ref i); break;
        case "--index": indexOnly = true; break;
        case "-h" or "--help": Console.WriteLine(usage); return 0;
        default:
            Console.Error.WriteLine($"未知参数: {args[i]}\n{usage}");
            return 1;
    }
}
if (species.Count == 0 && !all && probeAsset is null && !indexOnly)
{
    Console.Error.WriteLine($"缺 --species(或 --all)\n{usage}");
    return 1;
}

// --index:只读配置表,把归并后的包列出来就走 —— **不碰 pak**,所以没有游戏安装也能跑。
// 拿来和 tools/petindex.py 对账:两边同一套规则各实现了一遍,结果必须一字不差。
if (indexOnly)
{
    foreach (var pack in new GameConfig(parsedPath).Packs())
        Console.WriteLine($"{pack.Book:D3}-{pack.Name}.rkpet\t" +
                          string.Join("/", pack.Forms.Select(f => f.Name)));
    return 0;
}

// **上游必须先打补丁**,见 patches/0001-fix-FPackedNormal-quantize.patch。
// 未打补丁时法线会被静默写成切线(整只宠物的光照/matcap/边缘光全错,而模型看着仍然正常),
// 所以在这儿硬拦一道:导出坏包比导出失败更糟。
if (!PackedNormalRoundTrips())
{
    Console.Error.WriteLine("""
        CUE4Parse 的 FPackedNormal(FVector) 构造函数是坏的(上游 bug),法线会被写成切线。
        先给 CUE4Parse 克隆打补丁:
          git -C "$CUE4PARSE_DIR" apply <本仓库>/exporter/patches/0001-fix-FPackedNormal-quantize.patch
        细节见 docs/design.md §1「法线」那几行。
        """);
    return 1;
}

// 同理再拦一道顶点色。没打这个补丁时,**没有顶点色缓冲的网格**会拿到 (0,0,0,0) 而不是白,
// 而 XiaoYou / FakeFulid / YutuEar 三族都拿 `COLOR_0` 当遮罩 —— 整层被静默关掉
// (春兔耳朵里那泡液体渲成黑的就是这么来的),模型照样导得出来,更该硬拦。
if (!MissingVertexColorIsWhite())
{
    Console.Error.WriteLine("""
        CUE4Parse 在网格没有顶点色缓冲时把 COLOR_0 填成 0(该是白)。
        先给 CUE4Parse 克隆打补丁:
          git -C "$CUE4PARSE_DIR" apply <本仓库>/exporter/patches/0002-fix-FColor-missing-vertex-colors.patch
        细节见 docs/design.md §1.1「六条实机反馈」那节。
        """);
    return 1;
}

// 只留 Fatal:CUE4Parse 会为每个材质实例刷一串 OverflowException(本作的已知上游 bug,
// 贴图不走材质所以无碍,见 Textures.cs),否则输出全是噪声
Log.Logger = new LoggerConfiguration().MinimumLevel.Is(LogEventLevel.Fatal).WriteTo.Console().CreateLogger();
// Detex/oodle 原生解码库是 Windows dll;托管解码器全平台可用,覆盖 PC 端 BC 与安卓 ASTC
TextureDecoder.UseAssetRipperTextureDecoder = true;
var cacheDir = Path.Combine(home, ".cache", "nrc-unpack");
Directory.CreateDirectory(cacheDir);
try { OodleHelper.Initialize(Path.Combine(cacheDir, OodleHelper.OodleFileName)); }
catch (Exception e) { Console.Error.WriteLine($"[warn] Oodle 初始化失败: {e.Message}"); }
try { ZlibHelper.Initialize(Path.Combine(cacheDir, ZlibHelper.DllName)); }
catch (Exception e) { Console.Error.WriteLine($"[warn] zlib-ng 初始化失败: {e.Message}"); }

var config = new GameConfig(parsedPath);

var hex = aesKey.StartsWith("0x", StringComparison.OrdinalIgnoreCase) ? aesKey[2..] : aesKey;
var version = new VersionContainer(EGame.GAME_RocoKingdomWorld);
AbstractVfsFileProvider provider;
if (File.Exists(paksPath) && paksPath.EndsWith(".apk", StringComparison.OrdinalIgnoreCase))
    provider = new ApkFileProvider(paksPath, versions: version, pathComparer: StringComparer.OrdinalIgnoreCase);
else if (Directory.Exists(paksPath))
    provider = new DefaultFileProvider(paksPath, SearchOption.AllDirectories, version, StringComparer.OrdinalIgnoreCase);
else
{
    Console.Error.WriteLine($"--paks 路径不存在: {paksPath}");
    return 1;
}
// 材质探针需要读取 cooked material resource，才能把质量档/FeatureLevel 精确对到
// shader archive 里的 map hash。正常导包不读这块（体积大，而且与素材导出无关）。
provider.ReadShaderMaps = probeAsset is not null;
provider.Initialize();
provider.SubmitKey(new FGuid(), new FAesKey(hex));
if (provider.Files.Count == 0)
{
    Console.Error.WriteLine("挂载后没有任何文件:检查 pak 目录与 AES 密钥");
    return 1;
}
Console.WriteLine($"挂载 {provider.MountedVfs.Count} 个包,{provider.Files.Count} 个文件");

if (probeAsset is not null)
{
    MaterialProbe.Run(provider, probeAsset);
    return 0;
}

// 源指纹:同一版本的 pak 组合应当稳定,换版本就会变。写进 manifest 便于日后排查
// 「这个包是哪个版本导的」。用 pak 文件名+长度的哈希,不去读内容(那要几十秒)。
var sourceVersion = Fingerprint(paksPath, provider.Files.Count);

// 索引「哪些资产目录真的有动画」,按族名分组:
// 实测 197/827 个形态自己没有 Animation/(变体资产,如 Win_ShiJiu1**Ar**_001、
// 或换了属性前缀的 Gra_DiMo2_001),它们与同族的基础资产共用骨架与动画。
var animIndex = BuildAnimIndex(provider);
Console.WriteLine($"动画索引: {animIndex.Count} 个族,{animIndex.Sum(kv => kv.Value.Count)} 个带动画的资产");

// 按图鉴号归并出全部包(见 Config.cs 的 `Chain`);--species 从里面挑。
// **算一次就够** —— 归并要读三张表、遍历全部宠物,每个 --species 各算一遍是白费。
var allPacks = config.Packs();
var chainErrors = new List<string>();
var targets = new List<Chain>();
var seenChains = new HashSet<string>();
var skipped = 0;
IEnumerable<Chain> wanted;
if (all)
{
    wanted = allPacks;
}
else
{
    var picked = new List<Chain>();
    foreach (var petId in species)
    {
        var hit = allPacks.FirstOrDefault(p => p.Forms.Any(f => f.Id == petId))
                  ?? allPacks.FirstOrDefault(p => p.Forms.Any(f => f.Asset == config.AssetOf(petId)));
        if (hit is null) chainErrors.Add($"宠物 {petId} 不在任何包里(未实装?占位行?)");
        else picked.Add(hit);
    }
    wanted = picked;
}
foreach (var chain in wanted)
{
    if (!seenChains.Add($"{chain.Book:D3}-{chain.Name}")) continue;
    // 先按「已导出」过滤再计 limit:否则 --limit 永远只覆盖前 n 条链,分批续跑推不动
    if (skipExisting && ChainAlreadyExported(outDir, chain))
    {
        skipped++;
        continue;
    }
    targets.Add(chain);
    if (targets.Count >= limit) break;
}
Console.WriteLine($"待导 {targets.Count} 条进化链,{targets.Sum(c => c.Forms.Count)} 个形态,并行度 {jobs}" +
                  (chainErrors.Count > 0 ? $"({chainErrors.Count} 条归链失败)" : ""));

var report = new StringBuilder();
report.AppendLine($"# rocom-pets 导出报告  {DateTime.Now:yyyy-MM-dd HH:mm}");
report.AppendLine($"# 源指纹 {sourceVersion}");
foreach (var e in chainErrors) report.AppendLine($"[归链失败] {e}");

var failed = 0;
var doneForms = 0;
var formSkipped = 0;
var chainSkipped = 0;
var totalBytes = 0L;
var stopwatch = System.Diagnostics.Stopwatch.StartNew();
// 并行导出:每条链一个任务。链之间没有共享可变状态(各写自己的包目录),
// provider 的并行读在 rocom-capture 的解包脚本里已经压过(16 线程),这里同样只读。
// 控制台与报告文本先按链攒着,跑完按原顺序合并 —— 并行下直接打印会交错到没法看。
// 包目录名 = `<图鉴号>-<链首名>`。**图鉴号就是去重手段**:同名的两条链(海盔虫的
// 「本来的样子」与「磨损的样子」)本来就是同一只宠物,归并阶段已经并成一个包了;
// 而真的不同宠物碰了同名的(大耳帽兜、逗逗)图鉴号不同,自然分开 —— 于是原来那套
// 「重名就追加链首 id」连同它的批次漂移问题一起没有了。
// 没有图鉴号的都记 000,那一档理论上还能撞;实测这份数据没撞,撞了就报出来别硬导。
var packDirName = new Func<Chain, string>(chain => $"{chain.Book:D3}-{SafeName(chain.Name)}");
var clashes = allPacks.GroupBy(packDirName).Where(g => g.Count() > 1).ToList();
foreach (var clash in clashes)
    Console.WriteLine($"[警告] 包名 {clash.Key} 被 {clash.Count()} 个包共用,它们会互相覆盖");

var results = new ChainResult[targets.Count];
var completed = 0;
Parallel.For(0, targets.Count, new ParallelOptions { MaxDegreeOfParallelism = jobs }, index =>
{
    var result = ExportChain(targets[index]);
    results[index] = result;
    // 进度只打一行,详细内容留给最后按序输出
    var done = Interlocked.Increment(ref completed);
    Console.WriteLine($"[{done}/{targets.Count}] {targets[index].Name}{result.Summary}");
});

foreach (var result in results)
{
    if (result is null) continue;
    report.Append(result.Report);
    doneForms += result.Forms;
    formSkipped += result.FormsSkipped;
    totalBytes += result.Bytes;
    if (result.Failed) failed++;
    if (result.NoForms) chainSkipped++;
    if (result.Detail.Length > 0) Console.Write(result.Detail);
}

report.AppendLine();
report.AppendLine($"# 汇总:{targets.Count - failed - chainSkipped} 条链成功、{skipped} 条已存在跳过、" +
                  $"{chainSkipped} 条无可用形态、{failed} 条失败;{doneForms} 个形态导出成功、" +
                  $"{formSkipped} 个形态跳过;glb 合计 {totalBytes / 1024 / 1024}MB," +
                  $"用时 {stopwatch.Elapsed.TotalMinutes:F1} 分钟");
Directory.CreateDirectory(outDir);
var reportPath = Path.Combine(outDir, "report.txt");
File.WriteAllText(reportPath, report.ToString());
Console.WriteLine($"\n{targets.Count - failed - chainSkipped} 条链成功、{skipped} 已存在跳过、" +
                  $"{chainSkipped} 条无可用形态、{failed} 失败;{doneForms} 个形态成功、{formSkipped} 个跳过;" +
                  $"glb 合计 {totalBytes / 1024 / 1024}MB,用时 {stopwatch.Elapsed.TotalMinutes:F1} 分钟");
Console.WriteLine($"报告: {reportPath}");
return failed == 0 ? 0 : 2;

/// 导出一条进化链。**不碰任何共享可变状态**,这样才能并行跑。
ChainResult ExportChain(Chain chain)
{
    var detail = new StringBuilder();
    var chainReport = new StringBuilder();
    var forms = new List<FormReport>();
    var formsSkipped = 0;
    var bytes = 0L;
    try
    {
        var packDir = Path.Combine(outDir, packDirName(chain));
        detail.AppendLine($"=== {chain.Name}(链首 {chain.RootId},{chain.Forms.Count} 个形态)");

        chainReport.AppendLine();
        chainReport.AppendLine($"## {chain.Name}(链首 {chain.RootId})");
        foreach (var form in chain.Forms)
        {
            // 一个形态缺资产(有些进化阶段这版本根本没做)不该拖垮整条链
            FormReport formReport;
            try
            {
                formReport = ExportForm(provider, form, packDir, lodIndex, allClips ? null : defaultClips,
                    config.PinyinOf(form.Id));
            }
            catch (Exception e)
            {
                detail.AppendLine($"  {form.Name}({form.Asset}): 跳过 — {e.Message}");
                chainReport.AppendLine($"  {form.Name}({form.Asset}) stage {form.Stage}: 跳过 — {e.Message}");
                formsSkipped++;
                continue;
            }
            // **一个动作都没有的形态不出包**:桌宠靠 Idle/Walk 活着,零动作的形态上了台
            // 就是一尊不动的雕像,形态菜单里还白占一格。实测约三分之一的王者形态
            // (`…Bo_001`)在 pak 里压根没有 Animation/ 目录 —— 叶冕魔力猫、圣水守护、
            // 千棘海针都是。**这一档以前碰不到**:王者形态不在 evolution_pet_id 那条路上,
            // 改按 PET_EVOLUTION_CONF 归并之后才进得来。
            if (formReport.Clips.Count == 0)
            {
                // 模型/贴图这会儿已经落盘了(有没有动作要导完才知道),连目录一起删掉 ——
                // 留着就是包里一份谁也不引用的 glb
                var dead = Path.Combine(packDir, "forms", form.Asset);
                if (Directory.Exists(dead)) Directory.Delete(dead, recursive: true);
                detail.AppendLine($"  {form.Name}({form.Asset}): 跳过 — 一个动作都没有(资产里没有 Animation/)");
                chainReport.AppendLine($"  {form.Name}({form.Asset}) stage {form.Stage}: 跳过 — 零动作");
                formsSkipped++;
                continue;
            }
            forms.Add(formReport);
            detail.AppendLine(
                $"  {form.Name}(id {form.Id} stage {form.Stage} {form.Asset}): " +
                $"{formReport.Clips.Count}/{form.Clips.Count} 个动作,glb {formReport.GlbBytes / 1024}KB," +
                $"{formReport.Textures.Count} 张贴图");
            foreach (var warning in formReport.Warnings) detail.AppendLine($"    [warn] {warning}");

            var got = formReport.Clips.Select(c => c.Logical).ToHashSet();
            var wanted = (allClips ? form.Clips.Select(c => c.Logical) : defaultClips)
                .Distinct().ToList();
            var missing = wanted.Where(w => !got.Contains(w)).ToList();
            chainReport.AppendLine(
                $"  {form.Name}({form.Asset}) stage {form.Stage}: " +
                $"动作 {formReport.Clips.Count}/{wanted.Count},glb {formReport.GlbBytes / 1024}KB," +
                $"贴图 {formReport.Textures.Count},高 {formReport.HeightCm:F0}cm");
            if (missing.Count > 0) chainReport.AppendLine($"    缺动作: {string.Join(", ", missing)}");
            foreach (var w in formReport.Warnings) chainReport.AppendLine($"    [warn] {w}");
            bytes += formReport.GlbBytes;
        }

        if (forms.Count == 0)
        {
            detail.AppendLine($"  {chain.Name}: 一个形态都没导出来,跳过整条链");
            chainReport.AppendLine("  (整条链没有可用形态)");
            return new ChainResult(":无可用形态", detail.ToString(), chainReport.ToString(),
                0, formsSkipped, 0, false, true);
        }

        var manifest = Manifest.Render(chain, forms, lodIndex, sourceVersion);
        Directory.CreateDirectory(packDir);
        File.WriteAllText(Path.Combine(packDir, "manifest.toml"), manifest);

        if (zip)
        {
            var rkpet = packDir + ".rkpet";
            if (File.Exists(rkpet)) File.Delete(rkpet);
            // 压缩级别保持默认(Optimal)。量过:换 SmallestSize 只小 0.3%,时间多 57%;
            // 而把已经压过的 png/ogg 改成仅存储反而**更大**(deflate 还能从 PNG 里挤出一点)。
            System.IO.Compression.ZipFile.CreateFromDirectory(packDir, rkpet);
            detail.AppendLine($"  → {rkpet}({new FileInfo(rkpet).Length / 1024}KB)");
            if (zipOnly)
            {
                // 归档已经落盘(上一行读过它的大小),这才敢删掉源目录
                Directory.Delete(packDir, true);
                detail.AppendLine($"  已删掉 {packDir}/(--zip-only)");
            }
        }

        return new ChainResult(
            $":{forms.Count} 个形态 {bytes / 1024 / 1024}MB", detail.ToString(), chainReport.ToString(),
            forms.Count, formsSkipped, bytes, false, false);
    }
    catch (Exception e)
    {
        detail.AppendLine($"{chain.Name} 导出失败: {e.Message}");
        chainReport.AppendLine();
        chainReport.AppendLine($"## {chain.Name}:导出失败 — {e.Message}");
        return new ChainResult($":失败({e.Message})", detail.ToString(), chainReport.ToString(),
            0, formsSkipped, 0, true, false);
    }
}

FormReport ExportForm(
    AbstractVfsFileProvider fileProvider,
    Form form,
    string packDir,
    int lod,
    string[]? whitelist,
    string? pinyin)
{
    const string petsRoot = "NRC/Content/ArtRes/AnimSequence/Pets";
    var assetDir = $"{petsRoot}/{form.Asset}";
    var warnings = new List<string>();

    // 网格名多数是 SKM_<资产>_Skin,但不能硬编码:枚举目录直属的 SKM_* 更稳
    // (LOD_/ABP_ 前缀的是别的东西,要排掉)
    var meshCandidates = Textures.TopLevelFiles(fileProvider, assetDir)
        .Select(path => Path.GetFileNameWithoutExtension(path))
        .Where(name => name.StartsWith("SKM_", StringComparison.Ordinal))
        .OrderByDescending(name => name.EndsWith("_Skin", StringComparison.Ordinal))
        .ToList();
    if (meshCandidates.Count == 0)
        throw new InvalidOperationException($"{form.Asset} 目录下没有 SKM_*(这个形态这版本没做?)");
    var mesh = fileProvider.LoadPackageObject<USkeletalMesh>($"{assetDir}/{meshCandidates[0]}");

    // 逻辑动作 → AnimSequence:文件名去掉类别前缀(World_/Common_/Fight_…)再忽略下划线大小写比对
    // 有些形态自己没有 Animation/ 目录(如 Gra_DiMo2_001),动画挂在共享同一 anim_conf_id
    // 的另一个资产下——按 anim_conf_id 找过去,这也解释了 anim_conf_id 为什么能与 model 不同
    var animDir = $"{assetDir}/Animation";
    var byNormalized = new Dictionary<string, string>(StringComparer.Ordinal);
    void Collect(string dir)
    {
        foreach (var path in Textures.TopLevelFiles(fileProvider, dir))
            byNormalized.TryAdd(Normalize(Path.GetFileNameWithoutExtension(path)), path);
    }
    Collect(animDir);
    if (byNormalized.Count == 0)
    {
        // 先试同 anim_conf_id 的资产(配置层面的显式共享)
        foreach (var sibling in config.AssetsSharingAnimConf(form.AnimConfId, form.Asset))
        {
            Collect($"{petsRoot}/{sibling}/Animation");
            if (byNormalized.Count > 0)
            {
                warnings.Add($"自己没有 Animation/,借用同 anim_conf {form.AnimConfId} 的 {sibling}");
                break;
            }
        }
    }
    if (byNormalized.Count == 0)
    {
        // 再试同族资产:优先同阶段,其次任意阶段。骨架不匹配时 GlbBuilder 会按骨骼名对不上
        // 直接跳过那段动画,所以借错了只会少动作,不会渲出鬼东西
        var (family, stage) = FamilyOf(form.Asset);
        if (animIndex.TryGetValue(family, out var candidates))
        {
            foreach (var (sibling, siblingStage) in candidates
                         .Where(c => !c.Asset.Equals(form.Asset, StringComparison.OrdinalIgnoreCase))
                         .OrderByDescending(c => c.Stage == stage))
            {
                Collect($"{petsRoot}/{sibling}/Animation");
                if (byNormalized.Count > 0)
                {
                    warnings.Add(
                        $"自己没有 Animation/,借用同族 {sibling}(族 {family},阶段 {siblingStage} vs {stage})");
                    break;
                }
            }
        }
    }

    var clips = new List<(string Logical, string Clip, UAnimSequence Sequence)>();
    foreach (var clip in form.Clips)
    {
        if (whitelist is not null && !whitelist.Contains(clip.Logical, StringComparer.OrdinalIgnoreCase))
            continue;
        if (!byNormalized.TryGetValue(Normalize(clip.Logical), out var file))
        {
            warnings.Add($"动作 {clip.Logical} 在 {form.Asset}/Animation 里找不到对应资产");
            continue;
        }
        try
        {
            // file 是完整虚拟路径(可能借自别的资产目录),去掉扩展名喂给加载器
            var objectPath = file[..file.LastIndexOf('.')];
            clips.Add((clip.Logical, Path.GetFileNameWithoutExtension(file),
                fileProvider.LoadPackageObject<UAnimSequence>(objectPath)));
        }
        catch (Exception e)
        {
            warnings.Add($"动作 {clip.Logical}({file}) 加载失败: {e.Message}");
        }
    }

    var (glb, written, buildWarnings) = GlbBuilder.Build(mesh, clips, lod);
    warnings.AddRange(buildWarnings);

    var formDir = Path.Combine(packDir, "forms", form.Asset);
    Directory.CreateDirectory(formDir);
    File.WriteAllBytes(Path.Combine(formDir, "model.glb"), glb);
    var texDir = Path.Combine(formDir, "tex");
    var textures = Textures.Export(fileProvider, assetDir, texDir, warnings);

    // 材质:哪个槽画哪张贴图、alpha 该不该当遮罩剔。这一步取代原来的命名约定猜法
    // (实测全量 2043 个槽里 258 个猜错或猜不到,详见 docs/design.md §1)。
    var resolved = Materials.Load(mesh, warnings);
    // **材质资产全部悬空 = 这只宠物没做完。** 实测 13 个形态如此,而且都是未实装的:
    // 4 个名字里直接带「占位」,全部 legal_petbase / completeness 皆空,id 集中在最新未上线段。
    // 与其猜贴图硬渲出来,不如照「这个形态这版本没做」处理(和缺 SKM 那条一样)。
    if (resolved.Count > 0 && resolved.Values.All(m => !m.Resolved))
        throw new InvalidOperationException(
            $"{form.Asset} 的材质资产在 pak 里全部缺失(疑似未实装的宠物)");

    var materials = new List<MaterialEntry>();
    // 这个形态的星点遮罩(见下面统一那一段):优先用「假半透」族给的那张,它是宠物自己的
    // 星点图(幽星光一族 = `T_Ill_XingGuang1_001_Fx_D`);没有才退用共享的 `StarStickTex`。
    (string Tex, float[] Tiling, float[]? Color, float[] NoiseUv)? starLayer = null;
    var starFromFakeTrans = false;
    // 平铺数单独挑一份:见下面统一那一段末尾的说明 —— 贴图跟着「假半透」那份走,
    // 平铺数则跟着**实例里显式覆盖过**的那份走(两者不一定在同一个材质上)。
    float[]? explicitTiling = null;
    foreach (var (name, info) in resolved)
    {
        // 个别槽悬空:不写进材质表,运行时会跳过那一片(总比拿别的贴图硬凑好)
        if (!info.Resolved) continue;
        // 游戏自带的描边材质我们不用(自己按法线外扩画描边)
        if (name.EndsWith("_Ol", StringComparison.OrdinalIgnoreCase)) continue;
        string? baseColor = null;
        if (info.BaseColorTexture is { } objectPath)
        {
            // 基色贴图可能不在本资产的 Tex/ 下(共享图集/别的槽的贴图),那就补导一份
            var file = Textures.ExportByObjectPath(fileProvider, objectPath, texDir, textures, warnings);
            // 路径写成**包内相对**(和上面的 model 字段一致),运行时是拿包目录去 join 的;
            // 注意别跟 [forms.textures] 那节的 form 内相对路径搞混
            if (file is not null) baseColor = $"forms/{form.Asset}/tex/{file}";
            else warnings.Add($"材质 {name} 的基色贴图导不出来: {objectPath}");
        }
        // 特效层没有固有色贴图,靠主色 + 遮罩/噪声近似;那两张贴图要补导出来
        string? maskFile = null;
        string? noiseFile = null;
        if (baseColor is null)
        {
            maskFile = ExportEffectTexture(info.MaskTexture);
            noiseFile = ExportEffectTexture(info.NoiseTexture);
        }
        else if (info.WaterColor1 is not null || info.IsXiaoYou)
        {
            // 水体预设的 caustics 也走 `noise_tex` 那个槽(它有基色,但没有色带,槽是空的)
            noiseFile = ExportEffectTexture(info.NoiseTexture);
        }
        var yutuEar = info.IsYutuEar
            ? new YutuEarMaterial(
                maskFile, noiseFile, ExportEffectTexture(info.YutuFlowTexture),
                info.YutuBubbleColor, info.YutuFlowColor, info.YutuFresnelColor,
                info.YutuInnerColor, info.YutuOverallColor, info.YutuRampColor, info.YutuTopColor,
                info.YutuBubbleShape, info.YutuFlowShape, info.YutuLightShape, info.YutuTopShape)
            : null;
        var fakeFluid = info.IsFakeFluid
            ? new FakeFluidMaterial(
                info.FluidEdgeColor, info.FluidFresnelColor, info.FluidPlaneColor,
                info.FluidGradient1, info.FluidGradient2, info.FluidHeightTiling,
                info.FluidPlaneAxis, info.FluidPlaneCenter, info.FluidBodyShape,
                info.FluidGradientShape, info.FluidTopShape)
            : null;
        var matcapMasked = info.IsMatcapMasked
            ? new MatcapMaskedMaterial(
                info.MatcapMaskedBaseColor, info.MatcapMaskedLightRamp,
                info.MatcapMaskedFlatEmissive, info.MatcapMaskedMainColor,
                info.MatcapMaskedSelectionColor, info.MatcapMaskedRimShape,
                info.MatcapMaskedSurfaceShape)
            : null;
        materials.Add(new MaterialEntry(name, baseColor, info.IsFacePatch,
            info.OpacityMaskClipValue, info.BlendMode.ToString(), info.ParentChain,
            info.Tint, info.Opacity, info.Glow, info.Flow, maskFile, noiseFile, info.MaskIsMatcap,
            info.IsTranslucent,
            ExportEffectTexture(info.StarTexture), info.StarTiling, info.StarColor, info.StickIntensity,
            info.IsFakeTrans,
            info.MaskIsMatcap ? null : ExportEffectTexture(info.MatcapTexture), info.MatcapColor,
            info.RimColor, info.RimIntensity, info.EmissiveColor, info.EmissiveIntensity,
            info.RimPower, info.RimSoftEdge,
            info.HighlightOffset, info.HighlightSpecColor,
            info.HighlightSpecPower, info.HighlightSpecIntensity, info.ForceUseDefaultOpacity,
            info.OpacityDepthDistance, info.OpenDepthDistance,
            info.IsObjectTransLow,
            ExportEffectTexture(info.ObjectTransLightMaskTexture),
            ExportEffectTexture(info.ObjectTransRampTexture),
            info.ObjectTransSoftEdge, info.ObjectTransMainColor, info.ObjectTransMainBright,
            info.AlphaIsOpacity,
            ExportEffectTexture(info.FlowTexture), info.FlowPower,
            ExportEffectTexture(info.MaskIdTexture), info.MaskIdRange,
            info.WaterColor1, info.WaterColor2, info.WaterMain,
            info.WaterCaustics, info.WaterShape,
            ExportEffectTexture(info.InteriorTexture), info.InteriorColor,
            info.Refraction, info.RefractDepth, info.FlickerSpeed, info.FlickerPower,
            info.NoiseUv,
            info.IsGlassyInner,
            info.GlassyFlowColor01, info.GlassyFlowColor02, info.GlassyFresnelColor,
            info.GlassyNoiseParams, info.GlassyMaskParams,
            info.IsXiaoYou,
            info.XiaoYouBaseColor1, info.XiaoYouBaseColor2,
            info.XiaoYouFlowColor1, info.XiaoYouFlowColor2, info.XiaoYouStarColor,
            info.XiaoYouNoiseFlow, info.XiaoYouShape, info.XiaoYouStarUv,
            yutuEar, fakeFluid, matcapMasked, info.OutlineWidth ?? 0f, info.IsPaintOrder));

        if (info.StarTexture is not null && ExportEffectTexture(info.StarTexture) is { } starTex
            && (starLayer is null || (info.IsFakeTrans && !starFromFakeTrans)))
        {
            // 假半透族的平铺该用 `Mat_NoiseTilingX/Y`(5 / 2.5),不是 `StarStickTiling`(4)。
            var tile = info.IsFakeTrans && info.NoiseTiling[0] > 0f && info.NoiseTiling[1] > 0f
                ? info.NoiseTiling : info.StarTiling;
            starLayer = (starTex, tile, info.StarColor, info.NoiseUv);
            starFromFakeTrans = info.IsFakeTrans;
        }
        // 实例上**显式覆盖过**的 `StarStickTiling`(不是继承来的根默认 4)优先。
        // 曜星光就是这种情形:5.3 写在 `_By` 上,而星点贴图来自 `_Fx`(假半透那份),
        // 只按贴图那一份挑会把 5.3 丢掉、退回根默认 4。
        if (info.Scalars.ContainsKey("StarStickTiling") || info.Vectors.ContainsKey("StarStickTiling"))
            explicitTiling ??= info.StarTiling;

        string? ExportEffectTexture(string? objectPath)
        {
            if (objectPath is null) return null;
            var file = Textures.ExportByObjectPath(fileProvider, objectPath, texDir, textures, warnings);
            return file is null ? null : $"forms/{form.Asset}/tex/{file}";
        }
    }

    // **一个形态只有一份星点遮罩,而且盖在整只宠物上。** 各材质自己写的贴图与平铺数并不一致
    // (暮星辰:裙子是共享的 `Tex_PetGlassyStar_004`、身体是自己的 `Fx_D`),照各自的画就成了
    // 两种星点两种密度叠在一只宠物上。那两颗球身上的星星也是这层 —— 球的基色在图集里是
    // **一片平色圆盘**,星形完全来自这层(所以幽星光一颗球是星、另一颗是圆点)。
    //
    // 这是我们的简化:游戏那边**每个材质各有一份**(各自的 cb),靠静态开关与遮罩通道决定
    // 要不要画。真按每材质走要先解出那些开关,现在还做不到。
    //
    // 贴图与颜色跟「假半透」那份走(它是宠物自己的星点图);**平铺数单独挑**,跟着实例里
    // 显式覆盖过的那份走 —— 两者不一定在同一个材质上(见上面 `explicitTiling`)。
    //
    // **但不是所有材质都刷**:只刷**自己就开着这一层**的(`StarTexture is not null`,
    // 判据见 `Materials.StarTexture`:美术显式设了向量 `StarStickTiling`,或者是假半透族)。
    //
    // ~~原来的判据是「图里有这一层」(`GraphHasStickLayer`)~~ —— **那道门太松,是个 bug**:
    // 根默认里就有 `Stick_Intensity`,于是 `M_P_Object` 族的每个材质都过,包括从来没开过
    // 这一层的白身体。春兔实机报的「身上几乎看不到星点」就是这么来的:它只有 `_Fx`
    // (半透的耳朵/披风)显式写了 `StarStickTiling = (4,4)`,`_By` 一个星点参数都没覆盖过
    // (探针:全是「根num」),却被这里刷上了整层星点,渲出来一身四角星。
    // 统一的本意是「一只宠物别出现两种星点两种密度」,不是「给没开的材质开一层」。
    if (starLayer is { } star)
    {
        var tiling = explicitTiling ?? star.Tiling;
        var hasLayer = resolved
            .Where(kv => kv.Value.Resolved && kv.Value.StarTexture is not null)
            .Select(kv => kv.Key)
            .ToHashSet(StringComparer.OrdinalIgnoreCase);
        for (var i = 0; i < materials.Count; i++)
        {
            if (!hasLayer.Contains(materials[i].Name)) continue;
            // XiaoYou 的 StarTex 是该材质自身 PS 的 t4，并不属于通用 Stick 星点层；
            // 跨材质统一会把中间层的专用星图误换成另一个 Fx 遮罩。
            if (materials[i].XiaoYou) continue;
            materials[i] = materials[i] with
            {
                StarTexture = star.Tex,
                StarTiling = tiling,
                StarColor = star.Color,
                // 坐标系/滚动/浓度跟着星点层一起统一发,否则 `_By` 拿不到(见 Manifest.cs)。
                // **但只在星点层来自假半透族时发** —— 这套参数是那一族的,
                // 另一族走 `StarStickTex` + 自己的公式,误套过去会让全库过曝 5 → 11(踩过)。
                NoiseUv = starFromFakeTrans ? star.NoiseUv : [0f, 0f, 1f, 1f],
                // **`StarFakeTrans` 不在这里统一** —— 它是**按材质**的:
                // 汇编里带四段渐变的三条 shader(23766 / 27803 / 34270,V=116)全部来自
                // `_By`(`MI_P_Object` 那一族),而 `_Fx`(`FakeTrans`)的 shader 一条都没有。
                // 两族公式不同,所以这个标记必须跟着材质自己的父链走。
            };
        }
    }

    // 叫声:拿不到就是 null(39 个 bnk 查无此宠,还有形态压根没有 Pet_Vo_* 库),不算失败
    var voice = noVoice || pinyin is null
        ? null
        : Voice.Export(fileProvider, pinyin, formDir, $"forms/{form.Asset}", warnings);

    var bounds = mesh.ImportedBounds;
    return new FormReport(form, written, textures, materials, glb.Length, bounds.BoxExtent.Z * 2f,
        warnings, voice);
}

/// 上游的 `FPackedNormal(FVector)` 是否能把向量原样存取回来。
///
/// 坏掉的版本少了括号、又踩了 C# 里 `+` 比 `<<` 紧的优先级,三个分量会被搅成一个数;
/// 高精度切线基(`FPackedRGBA16N`)正是经它降到 8 位的,于是**法线与切线变成同一个向量**。
/// 拿一个不对称的向量试:三个分量都不同、且能区分 X/Y/Z 顺序错位。
static bool PackedNormalRoundTrips()
{
    var packed = new FPackedNormal(new FVector(0.25f, -0.5f, 0.75f));
    // 8 位量化的步长是 1/127.5,留两步余量
    return Math.Abs(packed.X - 0.25f) < 0.02f
        && Math.Abs(packed.Y - -0.5f) < 0.02f
        && Math.Abs(packed.Z - 0.75f) < 0.02f;
}

/// 没有顶点色缓冲的网格,`COLOR_0` 该是白的(UE 的顶点工厂就是这么补的),上游给的是 0。
/// 判据直接问那个结构体:不传颜色时它填什么。
static bool MissingVertexColorIsWhite() =>
    new VertexColorXTextureX([]).Color == System.Numerics.Vector4.One;

// "World_Idle" → "idle";"Common_Sleep_Loop" → "sleeploop";逻辑名 "SleepLoop" → "sleeploop"
static string Normalize(string name)
{
    string[] categories = ["World", "Common", "Fight", "Ride", "Battle", "Scene"];
    var parts = name.Split('_', StringSplitOptions.RemoveEmptyEntries).ToList();
    if (parts.Count > 1 && categories.Contains(parts[0], StringComparer.OrdinalIgnoreCase))
        parts.RemoveAt(0);
    return string.Concat(parts).ToLowerInvariant();
}

/// 扫一遍 VFS,找出所有「Animation/ 下有东西」的宠物资产,按族名分组。
/// 族名 = 资产名中段去掉末尾的变体后缀(Ar)与阶段数字:
/// `Win_ShiJiu1Ar_001` → 族 shijiu / 阶段 1;`Gra_DiMo2_001` → 族 dimo / 阶段 2。
static Dictionary<string, List<(string Asset, int Stage)>> BuildAnimIndex(AbstractVfsFileProvider provider)
{
    const string prefix = "NRC/Content/ArtRes/AnimSequence/Pets/";
    var assets = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
    foreach (var file in provider.Files.Values)
    {
        var path = file.Path;
        if (!path.StartsWith(prefix, StringComparison.OrdinalIgnoreCase)) continue;
        var rest = path[prefix.Length..];
        var slash = rest.IndexOf('/');
        if (slash < 0) continue;
        var asset = rest[..slash];
        var tail = rest[(slash + 1)..];
        // 只认目录直属的 Animation/*.uasset(子目录里是 CG/BlendSpace 之类)
        if (!tail.StartsWith("Animation/", StringComparison.OrdinalIgnoreCase)) continue;
        if (tail.IndexOf('/', "Animation/".Length) >= 0) continue;
        if (!tail.EndsWith(".uasset", StringComparison.OrdinalIgnoreCase)) continue;
        assets.Add(asset);
    }

    var index = new Dictionary<string, List<(string, int)>>(StringComparer.Ordinal);
    foreach (var asset in assets)
    {
        var (family, stage) = FamilyOf(asset);
        if (family.Length == 0) continue;
        if (!index.TryGetValue(family, out var list)) index[family] = list = [];
        list.Add((asset, stage));
    }
    return index;
}

/// 资产名 → (族名, 阶段)。`Win_ShiJiu1Ar_001` → ("shijiu", 1)。
static (string Family, int Stage) FamilyOf(string asset)
{
    var parts = asset.Split('_');
    if (parts.Length < 2) return ("", 0);
    var core = parts[1];
    // 变体后缀:Ar(骑乘/变体皮)等,去掉后才是同族
    if (core.EndsWith("Ar", StringComparison.Ordinal)) core = core[..^2];
    var digits = new string(core.SkipWhile(c => !char.IsDigit(c)).TakeWhile(char.IsDigit).ToArray());
    var family = new string(core.TakeWhile(c => !char.IsDigit(c)).ToArray()).ToLowerInvariant();
    return (family, int.TryParse(digits, out var stage) ? stage : 0);
}

/// pak 目录(或 apk)的指纹:文件名 + 长度 + 挂载后的文件数。
static string Fingerprint(string paksPath, int fileCount)
{
    var parts = new List<string>();
    if (Directory.Exists(paksPath))
        foreach (var file in Directory.EnumerateFiles(paksPath).OrderBy(f => f, StringComparer.Ordinal))
            parts.Add($"{Path.GetFileName(file)}:{new FileInfo(file).Length}");
    else if (File.Exists(paksPath))
        parts.Add($"{Path.GetFileName(paksPath)}:{new FileInfo(paksPath).Length}");
    parts.Add($"files:{fileCount}");
    var bytes = System.Text.Encoding.UTF8.GetBytes(string.Join('|', parts));
    var hash = System.Security.Cryptography.SHA256.HashData(bytes);
    return Convert.ToHexString(hash)[..12].ToLowerInvariant();
}

/// --skip-existing 用:两种命名(裸名字 / 名字-链首id)任一存在就算导过。
/// 去重命名依赖全量名字统计,而这个判断发生在统计之前,所以两种都查。
///
/// **目录和 `.rkpet` 都要查**:`--zip-only` 导完就没有目录了,只认目录的话
/// 续跑会把整库重导一遍。
static bool ChainAlreadyExported(string outDir, Chain chain)
{
    var name = $"{chain.Book:D3}-{SafeName(chain.Name)}";
    if (File.Exists(Path.Combine(outDir, name, "manifest.toml"))) return true;
    return File.Exists(Path.Combine(outDir, name + ".rkpet"));
}

/// 物种名直接当目录名:大多是中文,但个别名字可能带斜杠之类,做一层净化。
static string SafeName(string name)
{
    var invalid = Path.GetInvalidFileNameChars();
    var safe = new string(name.Select(c => invalid.Contains(c) ? '_' : c).ToArray()).Trim();
    return safe.Length == 0 ? "pet" : safe;
}

string Next(ref int i)
{
    if (i + 1 >= args.Length)
    {
        Console.Error.WriteLine($"{args[i]} 缺少参数值\n{usage}");
        Environment.Exit(1);
    }
    return args[++i];
}

/// 一条链的导出结果。文本先攒起来,避免并行时控制台/报告乱序。
record ChainResult(
    string Summary,
    string Detail,
    string Report,
    int Forms,
    int FormsSkipped,
    long Bytes,
    bool Failed,
    bool NoForms);
