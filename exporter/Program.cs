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
using CUE4Parse.UE4.Objects.Core.Misc;
using CUE4Parse.UE4.Versions;
using CUE4Parse_Conversion.Textures;
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
      --zip             额外打成 <链名>.rkpet
      -h, --help        本帮助
    """;

// 桌宠真正用得到的动作:名字是 ANIM_ID_CONF 里的逻辑名。
// 其余(Attack/Skill/Hit/Die/CG 演出)对桌宠没用,占体积,默认不导。
string[] defaultClips =
[
    "Idle", "Walk", "Run", "Happy", "Anger", "Sad", "Fear", "Shock", "Show", "Relax", "Alert",
    "SleepStart", "SleepLoop", "SleepEnd", "SleepStand", "CallOut",
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
var zip = false;

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
        case "--zip": zip = true; break;
        case "-h" or "--help": Console.WriteLine(usage); return 0;
        default:
            Console.Error.WriteLine($"未知参数: {args[i]}\n{usage}");
            return 1;
    }
}
if (species.Count == 0)
{
    Console.Error.WriteLine($"缺 --species\n{usage}");
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
provider.Initialize();
provider.SubmitKey(new FGuid(), new FAesKey(hex));
if (provider.Files.Count == 0)
{
    Console.Error.WriteLine("挂载后没有任何文件:检查 pak 目录与 AES 密钥");
    return 1;
}
Console.WriteLine($"挂载 {provider.MountedVfs.Count} 个包,{provider.Files.Count} 个文件");

var failed = 0;
foreach (var petId in species)
{
    try
    {
        var chain = config.ResolveChain(petId);
        Console.WriteLine($"\n=== {chain.Name}(链首 {chain.RootId},{chain.Forms.Count} 个形态)");
        var packDir = Path.Combine(outDir, chain.Name);
        var forms = new List<FormReport>();

        foreach (var form in chain.Forms)
        {
            var report = ExportForm(provider, form, packDir, lodIndex, allClips ? null : defaultClips);
            forms.Add(report);
            Console.WriteLine(
                $"  {form.Name}(id {form.Id} stage {form.Stage} {form.Asset}): " +
                $"{report.Clips.Count}/{form.Clips.Count} 个动作,glb {report.GlbBytes / 1024}KB," +
                $"{report.Textures.Count} 张贴图");
            foreach (var warning in report.Warnings) Console.WriteLine($"    [warn] {warning}");
        }

        var manifest = Manifest.Render(chain, forms, lodIndex);
        Directory.CreateDirectory(packDir);
        File.WriteAllText(Path.Combine(packDir, "manifest.toml"), manifest);
        Console.WriteLine($"  → {packDir}");

        if (zip)
        {
            var rkpet = packDir + ".rkpet";
            if (File.Exists(rkpet)) File.Delete(rkpet);
            System.IO.Compression.ZipFile.CreateFromDirectory(packDir, rkpet);
            Console.WriteLine($"  → {rkpet}({new FileInfo(rkpet).Length / 1024}KB)");
        }
    }
    catch (Exception e)
    {
        Console.Error.WriteLine($"宠物 {petId} 导出失败: {e.Message}");
        failed++;
    }
}
return failed == 0 ? 0 : 2;

FormReport ExportForm(
    AbstractVfsFileProvider fileProvider,
    Form form,
    string packDir,
    int lod,
    string[]? whitelist)
{
    const string petsRoot = "NRC/Content/ArtRes/AnimSequence/Pets";
    var assetDir = $"{petsRoot}/{form.Asset}";
    var warnings = new List<string>();

    var meshPath = $"{assetDir}/SKM_{form.Asset}_Skin";
    var mesh = fileProvider.LoadPackageObject<USkeletalMesh>(meshPath);

    // 逻辑动作 → AnimSequence:文件名去掉类别前缀(World_/Common_/Fight_…)再忽略下划线大小写比对
    var byNormalized = new Dictionary<string, string>(StringComparer.Ordinal);
    foreach (var path in Textures.TopLevelFiles(fileProvider, $"{assetDir}/Animation"))
    {
        var name = Path.GetFileNameWithoutExtension(path);
        byNormalized.TryAdd(Normalize(name), name);
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
            clips.Add((clip.Logical, file,
                fileProvider.LoadPackageObject<UAnimSequence>($"{assetDir}/Animation/{file}")));
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
    var textures = Textures.Export(fileProvider, assetDir, Path.Combine(formDir, "tex"), warnings);

    var bounds = mesh.ImportedBounds;
    return new FormReport(form, written, textures, glb.Length, bounds.BoxExtent.Z * 2f, warnings);
}

// "World_Idle" → "idle";"Common_Sleep_Loop" → "sleeploop";逻辑名 "SleepLoop" → "sleeploop"
static string Normalize(string name)
{
    string[] categories = ["World", "Common", "Fight", "Ride", "Battle", "Scene"];
    var parts = name.Split('_', StringSplitOptions.RemoveEmptyEntries).ToList();
    if (parts.Count > 1 && categories.Contains(parts[0], StringComparer.OrdinalIgnoreCase))
        parts.RemoveAt(0);
    return string.Concat(parts).ToLowerInvariant();
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
