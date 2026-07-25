// 生成 manifest.toml。schema 见 docs/design.md §4.3。
//
// 手写 TOML 而不引 TOML 库:字段固定、只写不读,手写省一个依赖也少一层版本约束。
// 运行时那边用 Rust 的 toml crate 读。

using System.Globalization;
using System.Text;

namespace RocomPets.Export;

/// 一个材质槽写进 manifest 的内容:运行时按 glb 里的材质名查这张表。
public record MaterialEntry(
    string Name,
    /// 基色贴图在包内的相对路径;null = 这个材质不画固有色(纯 VFX),运行时目前整片跳过。
    string? BaseColor,
    /// 贴图 alpha 是不是真遮罩(眼/嘴的表情图集是,本体贴图不是)。
    bool MaskAlpha,
    float MaskClip,
    string Blend,
    /// 父链(由近及远),排查用;也是「这是哪一族特效」的线索(如 M_FX_Fire_Mat)。
    List<string> ParentChain,
    /// 特效层的主色(线性 RGBA)。**可能是 HDR**(火花的 Color01 = (6, 0.8, 0)),
    /// 任一通道 >1 就说明这层是加色发光,运行时据此走加色而不是半透。
    float[]? Tint,
    float Opacity,
    /// 发光强度(火焰族的 `Glow Intensity`)。
    float Glow,
    /// UV 卷动 [u速度, v速度, u平铺, v平铺];火焰靠它动。
    float[] Flow,
    /// 特效的形状/流动贴图,包内相对路径。
    string? MaskTexture,
    string? NoiseTexture);

public record FormReport(
    Form Form,
    List<ClipResult> Clips,
    List<TextureFile> Textures,
    List<MaterialEntry> Materials,
    int GlbBytes,
    float HeightCm,
    List<string> Warnings);

public static class Manifest
{
    /// manifest 格式版本;运行时 ABI 版本单独走,便于格式没变但语义变了的情况。
    private const int Schema = 1;
    private const int RuntimeAbi = 1;

    public static string Render(Chain chain, List<FormReport> forms, int lodIndex, string sourceVersion)
    {
        var sb = new StringBuilder();
        sb.AppendLine("# 由 rocom-pets-export 生成,勿手改(素材本地生成物,不入仓库)");
        sb.AppendLine($"schema = {Schema}");
        sb.AppendLine($"runtime_abi = {RuntimeAbi}");
        sb.AppendLine($"generated_at = {Quote(DateTime.UtcNow.ToString("yyyy-MM-dd'T'HH:mm:ss'Z'", CultureInfo.InvariantCulture))}");
        sb.AppendLine($"lod = {lodIndex}");
        // 导出时的 pak 指纹:换游戏版本重导后这里会变,便于排查「这包是哪版导的」
        sb.AppendLine($"source_version = {Quote(sourceVersion)}");
        sb.AppendLine();

        sb.AppendLine("[species]");
        sb.AppendLine($"id = {chain.RootId}");
        sb.AppendLine($"name = {Quote(chain.Name)}");
        sb.AppendLine($"chain = [{string.Join(", ", chain.Forms.Select(f => f.Id))}]");

        foreach (var report in forms)
        {
            var form = report.Form;
            sb.AppendLine();
            sb.AppendLine("[[forms]]");
            sb.AppendLine($"id = {form.Id}");
            sb.AppendLine($"name = {Quote(form.Name)}");
            sb.AppendLine($"stage = {form.Stage}");
            sb.AppendLine($"asset = {Quote(form.Asset)}");
            sb.AppendLine($"model = {Quote($"forms/{form.Asset}/model.glb")}");
            sb.AppendLine($"scale = {Num(form.ModelScale)}");
            sb.AppendLine($"height_cm = {Num(report.HeightCm)}   # 绑定姿势包围盒高度,用于换算屏幕像素");
            sb.AppendLine($"locomotion = {Quote(Locomotion(form.MoveType))}   # 原文 move_type = {Quote(form.MoveType)}");

            sb.AppendLine();
            sb.AppendLine($"  [forms.clips]   # 逻辑动作 → glb 里的 animation 名(同名)");
            foreach (var clip in report.Clips)
            {
                // 位移类动作额外给出 root motion 与由它算出的速度:实测本作的 Walk/Run
                // 自带位移(喵喵 Walk 53cm/1.13s、Run 180cm/0.6s),运行时可以直接用这个速度
                // 推进位置并原地循环播放,不必解析 root motion 曲线
                var moving = clip.Logical.StartsWith("Walk", StringComparison.Ordinal) ||
                             clip.Logical.StartsWith("Run", StringComparison.Ordinal);
                var extra = moving
                    ? $", in_place = {(GlbBuilder.IsInPlace(clip.RootMotionCm) ? "true" : "false")}" +
                      $", root_motion_cm = {Num(clip.RootMotionCm)}" +
                      $", speed_cm_s = {Num(clip.Seconds > 0 ? clip.RootMotionCm / clip.Seconds : 0f)}"
                    : "";
                sb.AppendLine(
                    $"  {ClipKey(clip.Logical)} = {{ clip = {Quote(clip.Logical)}, " +
                    $"ms = {(int)MathF.Round(clip.Seconds * 1000f)}, frames = {clip.Frames}{extra} }}");
            }

            if (report.Textures.Count > 0)
            {
                sb.AppendLine();
                sb.AppendLine("  [forms.textures]   # 材质槽(材质名后缀)→ 贴图,D=基色 M=遮罩 ID=分色");
                foreach (var tex in report.Textures)
                    sb.AppendLine(
                        $"  {ClipKey(tex.Name)} = {{ path = {Quote(tex.RelativePath)}, " +
                        $"slot = {Quote(tex.Slot)}, kind = {Quote(tex.Kind)}, " +
                        $"size = [{tex.Width}, {tex.Height}] }}");
            }

            if (report.Materials.Count > 0)
            {
                sb.AppendLine();
                sb.AppendLine("  [forms.materials]   # glb 里的材质名 → 该画什么。base_color 缺失 = 纯特效层");
                foreach (var mat in report.Materials)
                {
                    var parts = new List<string>();
                    if (mat.BaseColor is not null) parts.Add($"base_color = {Quote(mat.BaseColor)}");
                    parts.Add($"mask_alpha = {(mat.MaskAlpha ? "true" : "false")}");
                    parts.Add($"mask_clip = {Num(mat.MaskClip)}");
                    parts.Add($"blend = {Quote(mat.Blend)}");
                    if (mat.BaseColor is null)
                    {
                        // 特效层:主色 + 卷动 + 遮罩/噪声,运行时靠这些近似画出火焰/水壳/光晕
                        if (mat.Tint is { } t)
                            parts.Add($"tint = [{Num(t[0])}, {Num(t[1])}, {Num(t[2])}, {Num(t[3])}]");
                        parts.Add($"opacity = {Num(mat.Opacity)}");
                        parts.Add($"glow = {Num(mat.Glow)}");
                        parts.Add($"flow = [{string.Join(", ", mat.Flow.Select(Num))}]");
                        if (mat.MaskTexture is not null) parts.Add($"mask_tex = {Quote(mat.MaskTexture)}");
                        if (mat.NoiseTexture is not null) parts.Add($"noise_tex = {Quote(mat.NoiseTexture)}");
                        parts.Add($"parents = [{string.Join(", ", mat.ParentChain.Select(Quote))}]");
                    }
                    sb.AppendLine($"  {ClipKey(mat.Name)} = {{ {string.Join(", ", parts)} }}");
                }
            }

            if (report.Warnings.Count > 0)
            {
                sb.AppendLine();
                sb.AppendLine("  [forms.report]   # 导出时的缺口,运行时按需降级而不是报错");
                sb.AppendLine($"  warnings = [{string.Join(", ", report.Warnings.Select(Quote))}]");
            }
        }

        return sb.ToString();
    }

    /// PETBASE_CONF.move_type 是中文(步行/浮游/…),转成运行时用的枚举。
    private static string Locomotion(string moveType) => moveType switch
    {
        "步行" => "ground",
        "浮游" => "hover",
        "游泳" or "游动" => "swim",
        "飞行" => "fly",
        _ => "ground",
    };

    /// TOML 裸键只允许 [A-Za-z0-9_-],其余情况加引号。
    private static string ClipKey(string name) =>
        name.All(c => char.IsAsciiLetterOrDigit(c) || c is '_' or '-') ? name : Quote(name);

    private static string Quote(string s) =>
        "\"" + s.Replace("\\", "\\\\").Replace("\"", "\\\"").Replace("\n", "\\n") + "\"";

    private static string Num(float v) => v.ToString("0.####", CultureInfo.InvariantCulture);
}
