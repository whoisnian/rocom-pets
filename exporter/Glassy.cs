// 炫彩(游戏里的 `MDT_GLASS`)要用的**共享**贴图。
//
// 炫彩不换材质,它在原材质上打开动态开关 `GlassySwitch` 再覆盖几个参数;要覆盖的贴图
// 只有两张槽 —— `MainTex`(花纹)与 `StarStickTex`(粒子),而这两张都是**全库共用**的:
//
// - 常规炫彩根本不覆盖 `MainTex`,用的是材质自己的 `Tex_PetGlassy_007_D`;
//   `StarStickTex` 从 `PARTICLE_RANDOM_CONF` 那 4 张里选一张;
// - 隐藏/赛季款各自带一对(`T_PetGlassyNoise*` + 各自的星图),在 `HIDDEN_GLASS_CONF` 里。
//
// 所以它们**不进宠物包**:塞进 201 个包要多背 120MB,而做成包目录旁边的一份 `glassy/`
// 共享目录只要 3.7MB,已经导好的包也不用重导就能用上炫彩。
// 运行时按名字取,见 `src/pet/glassy.rs` 的 `assets_dir`。

using CUE4Parse.FileProvider.Vfs;
using Newtonsoft.Json.Linq;

namespace RocomPets.Export;

public static class Glassy
{
    /// 材质自带的 `MainTex`。全库共享的红/绿双通道斑点图,常规炫彩的花纹就是它。
    /// 名字是从 `--probe-material` 读出来的(`MI_P_Object` 一族的 `MainTex` 槽),不是猜的。
    private const string DefaultMainTex =
        "NRC/Content/ArtRes/Material/Characters/Pets/CommonTexture/Tex_PetGlassy_007_D";

    /// 导出全部炫彩共享贴图到 `outDir`。返回写出的张数。
    ///
    /// 清单**从配置表读**而不是写死:粒子表与隐藏款表哪天加了新款,这里跟着走。
    public static int Export(AbstractVfsFileProvider provider, string parsedRoot, string outDir)
    {
        var paths = new List<string> { DefaultMainTex };
        var binDir = Path.Combine(parsedRoot,
            "NRC", "Content", "ScriptC", "Data", "Bin", "BinDataCompressed");

        // `egg_particle_res` 只给随机蛋用,桌宠不画蛋,不导。
        foreach (var (_, row) in Rows(binDir, "PARTICLE_RANDOM_CONF"))
            if (AssetPath(row?["particle_res"]?.Value<string>()) is { } path)
                paths.Add(path);

        foreach (var (_, row) in Rows(binDir, "HIDDEN_GLASS_CONF"))
            foreach (var tex in row?["tex_param"] ?? Enumerable.Empty<JToken>())
                if (AssetPath(tex["tex_param_path"]?.Value<string>()) is { } path)
                    paths.Add(path);

        // **`season_pet_tex` 刻意不导。** 赛季款给 4 只宠物(加灵/加益/加尔/黑化加尔)
        // 另配了专属 `MainTex`,但那是整张身体图 —— 单张 2~9MB,7 张就把这份共享目录
        // 从 3.6MB 顶到 34MB,而受益的只有那 4 只。
        // 少了它并不是缺功能:游戏对**不在 `season_pet` 里**的宠物走的正是通用覆盖
        // (`PetMutationUtils` 里那个 `bSeasonButNotCustomPet` 分支),我们对所有宠物
        // 都走那一条,行为与游戏对绝大多数宠物的行为一致。

        Directory.CreateDirectory(outDir);
        var exported = new List<TextureFile>();
        var warnings = new List<string>();
        var written = 0;
        foreach (var path in paths.Distinct(StringComparer.OrdinalIgnoreCase))
        {
            if (Textures.ExportByObjectPath(provider, path, outDir, exported, warnings) is not null)
                written++;
        }
        foreach (var warning in warnings) Console.Error.WriteLine($"  警告: {warning}");
        return written;
    }

    /// `Texture2D'/Game/…/Tex_X.Tex_X'` → `NRC/Content/…/Tex_X`。
    /// `/Game/` 是 UE 的项目内容根,在本作的解包树里就是 `NRC/Content/`。
    private static string? AssetPath(string? longPath)
    {
        if (string.IsNullOrEmpty(longPath)) return null;
        var quoted = longPath.Split('\'');
        var inner = quoted.Length >= 2 ? quoted[1] : longPath;
        if (!inner.StartsWith("/Game/", StringComparison.OrdinalIgnoreCase)) return null;
        return "NRC/Content/" + inner["/Game/".Length..];
    }

    private static JObject Rows(string binDir, string table)
    {
        var path = Path.Combine(binDir, table + ".json");
        if (!File.Exists(path))
            throw new FileNotFoundException(
                $"找不到 {path}\n先在 rocom-capture 里跑 scripts/unpack.sh,再用 --parsed 指过来");
        var root = JObject.Parse(File.ReadAllText(path));
        return (JObject) (root["RocoDataRows"] ?? new JObject());
    }
}
