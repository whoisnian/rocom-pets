// 贴图导出:解码 → 修正 BC7 通道序 → PNG。
//
// 两个已知坑(见 docs/design.md §1):
// ① CUE4Parse 的 AssetRipper 托管解码分支对 PF_BC7 用 ColorRGBA 解码却标成 B8G8R8A8,
//    R/B 全图对调,必须换回来;
// ② `UMaterialInstance.Deserialize` 在本作抛 OverflowException,材质里的贴图槽拿到的是
//    父材质默认值,所以贴图不能顺着材质走,只能按命名约定:
//    材质名后缀 `_By/_Es/_Mh` ↔ 贴图 `T_<Asset>_<槽>_D`。

using CUE4Parse.FileProvider;
using CUE4Parse.FileProvider.Vfs;
using CUE4Parse.UE4.Assets.Exports.Texture;
using CUE4Parse_Conversion.Textures;
using SkiaSharp;

namespace RocomPets.Export;

public record TextureFile(string Name, string RelativePath, int Width, int Height, string Slot, string Kind);

public static class Textures
{
    /// 导出 `<资产目录>/Tex/` 下的贴图到 `outDir`,返回写出的清单。
    public static List<TextureFile> Export(
        AbstractVfsFileProvider provider,
        string assetDir,
        string outDir,
        List<string> warnings)
    {
        var result = new List<TextureFile>();
        var texDir = $"{assetDir}/Tex";
        Directory.CreateDirectory(outDir);

        foreach (var path in TopLevelFiles(provider, texDir))
        {
            var name = Path.GetFileNameWithoutExtension(path);
            try
            {
                var texture = provider.LoadPackageObject<UTexture>(path[..path.LastIndexOf('.')]);
                var decoded = texture.Decode()
                              ?? throw new InvalidOperationException($"解码返回空({texture.Format})");
                FixBc7ChannelOrder(texture, decoded);
                var png = decoded.Encode(ETextureFormat.Png, false, out _);
                File.WriteAllBytes(Path.Combine(outDir, name + ".png"), png);
                var (slot, kind) = Classify(name);
                result.Add(new TextureFile(name, $"tex/{name}.png", decoded.Width, decoded.Height, slot, kind));
            }
            catch (Exception e)
            {
                warnings.Add($"贴图 {name} 导出失败: {e.Message}");
            }
        }
        return result;
    }

    /// 按资产对象路径导一张贴图(材质参数给的就是这种路径)。已经导过就直接返回文件名。
    /// 用于基色贴图不在本资产 `Tex/` 下的情况:共享图集,或槽名与贴图名对不上。
    /// 返回文件名(不含目录),失败返回 null。
    public static string? ExportByObjectPath(
        AbstractVfsFileProvider provider,
        string objectPath,
        string outDir,
        List<TextureFile> exported,
        List<string> warnings)
    {
        // 对象路径形如 `路径/T_Xxx.T_Xxx`,取包路径部分
        var packagePath = objectPath.Contains('.') ? objectPath[..objectPath.LastIndexOf('.')] : objectPath;
        var name = Path.GetFileNameWithoutExtension(packagePath);
        var already = exported.FirstOrDefault(t => t.Name.Equals(name, StringComparison.OrdinalIgnoreCase));
        if (already is not null) return Path.GetFileName(already.RelativePath);

        try
        {
            var texture = provider.LoadPackageObject<UTexture>(packagePath);
            var decoded = texture.Decode()
                          ?? throw new InvalidOperationException($"解码返回空({texture.Format})");
            FixBc7ChannelOrder(texture, decoded);
            Directory.CreateDirectory(outDir);
            var png = decoded.Encode(ETextureFormat.Png, false, out _);
            File.WriteAllBytes(Path.Combine(outDir, name + ".png"), png);
            var (slot, kind) = Classify(name);
            exported.Add(new TextureFile(name, $"tex/{name}.png", decoded.Width, decoded.Height, slot, kind));
            return name + ".png";
        }
        catch (Exception e)
        {
            warnings.Add($"贴图 {name} 补导失败: {e.Message}");
            return null;
        }
    }

    /// 只取目录直属文件(跳过 CG/ 之类子目录里的另一套资产)。
    public static IEnumerable<string> TopLevelFiles(AbstractVfsFileProvider provider, string dir)
    {
        var prefix = dir + "/";
        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var file in provider.Files.Values)
        {
            var path = file.Path;
            if (!path.StartsWith(prefix, StringComparison.OrdinalIgnoreCase)) continue;
            if (path.IndexOf('/', prefix.Length) >= 0) continue;
            if (!path.EndsWith(".uasset", StringComparison.OrdinalIgnoreCase)) continue;
            if (seen.Add(path)) yield return path;
        }
    }

    /// 贴图名形如 `T_<Asset>_<槽>_<用途>`:槽 = By/Es/Mh…,用途 = D(基色)/M(遮罩)/ID(分色)。
    private static (string Slot, string Kind) Classify(string name)
    {
        var parts = name.Split('_');
        if (parts.Length < 2) return ("", "");
        var kind = parts[^1];
        var slot = parts.Length >= 3 ? parts[^2] : "";
        return (slot, kind);
    }

    // 上游 bug 修正:双重条件防御——上游若改 colorType 修复,此处自动失效;
    // 若改成 ColorBGRA 修复,则必须删掉本函数。
    private static void FixBc7ChannelOrder(UTexture texture, CTexture decoded)
    {
        if (!TextureDecoder.UseAssetRipperTextureDecoder) return;
        if (texture.Format != EPixelFormat.PF_BC7) return;
        if (decoded.PixelFormat != EPixelFormat.PF_B8G8R8A8) return;
        var data = decoded.Data;
        for (var i = 0; i + 2 < data.Length; i += 4)
            (data[i], data[i + 2]) = (data[i + 2], data[i]);
    }

    /// 供将来把基色塞进 glb 用(现在贴图独立成文件,见 docs/design.md §4.2)。
    public static SKBitmap? Ignored => null;
}
