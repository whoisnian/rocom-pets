// 眼神曲线:游戏把「这段动作眼睛/嘴是哪一格」写在 AnimSequence 自己的浮点曲线上。
//
// 每段动画带 `EC_Eye`(眼)与 `EC_Mouth`(嘴)两条曲线,值 = **眼神图集第几格 × 100**
// (1..8,编号与 `M_P_Eyes_Mesh` 的顶点色卡号是同一套:`col + 2·row + 1`)。整段动画上还
// 盖着一个 `ANS_SetFacialExpressionIntegrated` 的 AnimNotifyState —— 那就是把曲线值刷到
// 材质 `Number` 参数上的那个东西(`M_P_Eyes` 根材质上默认值为 1 的标量)。
//
// 全库实测(1000 个宠物资产 / 23557 段动画):带 `EC_Eye` 的 22884 段(97.1%)、
// 带 `EC_Mouth` 的 8901 段(只有做了嘴图集 `_Mh` 槽的宠物才有)。取值分布几乎全落在八档:
// 100:44923 200:6031 300:2178 400:10430 500:13500 600:1792 700:5809 800:1633
// (中间那些零星值是关键帧插值采到的,四舍五入即可)。
//
// **眼和嘴是两条独立曲线**:8636 段两条都带的动画里,主值不一致的约占三成
// (眼1嘴2 181 段、眼4嘴1 138 段、眼2嘴1 101 段…)。所以运行时必须给两个槽各一个偏移,
// 不能像原来那样一个偏移喂两个槽。
//
// 时间上也是**逐帧**的:待机段里 100↔500 来回跳就是眨眼。所以这里不压成「一段一张脸」,
// 原样留成阶梯关键帧。

using CUE4Parse.UE4.Assets.Exports.Animation;
using CUE4Parse.UE4.Assets.Exports.SkeletalMesh;

namespace RocomPets.Export;

/// 一个阶梯关键帧:从 `Ms` 起,这个槽画第 `Card` 格,直到下一个关键帧。
public record FaceKey(int Ms, int Card);

/// 一条眼神曲线。空 = 这段动画没有这条曲线(或者整段都是默认格,不值得写)。
public record FaceTrack(IReadOnlyList<FaceKey> Keys);

public static class FaceCurves
{
    /// 图集是 2 列 × 4 行,格号 1..8。
    private const int MaxCard = 8;

    /// 一个材质槽跟哪条曲线走 —— **判据是材质名后缀 + 同后缀里的第几个**。
    ///
    /// 游戏那边的对应关系写在 `ANS_SetFacialExpressionIntegratedBase` 上:它有一张
    /// `LayerFunctions` 表(`Eye → ML_FacialModelClip_Eye`、`Mouth → …_Mouth`、
    /// `Dynamic1..6` 各一条),而每段动画上盖的那个通知实例带一串
    /// `FacialExpressionConfigs`,每条是 `{类型, 下标, 目标, 目标下标}`。实测(`--probe-face`):
    /// 一窝蜂三阶的 5 条配置正好是 `(Eye,0) (Eye,1) (Eye,2) (Mouth,1) (Mouth,2)`,
    /// 与那段动画里的 `EC_Eye / EC_Eye_1 / EC_Eye_2 / EC_Mouth_1 / EC_Mouth_2` 逐条对上 ——
    /// **曲线名就是 `EC_{类型}` 加上下标**(下标 0 不写后缀)。
    ///
    /// 「下标 → 哪个材质」这一步是**推的**:游戏是按材质里挂的那层
    /// (`ML_FacialModelClip_*`)去找的,而 cooked 包里读不到图层栈(材质图是 editor-only,
    /// 见 MaterialProbe.cs 的说明),所以这里按**槽序**数同后缀的材质 —— 第 k 个 `_Es`
    /// 跟 `EC_Eye_k`。一窝蜂二/三阶(两个 `_Es`)、加油海葵(两个 `_Es` 两个 `_Mh`)、
    /// 卡波(三个 `_Dynamic*`)都对得上;全库没有第三个同后缀的槽。
    ///
    /// 数不到的槽(如动画只有 `EC_Eye` 而形态有两个 `_Es`)就没有曲线,运行时按性格那张脸
    /// 画 —— 和游戏一致:通知只覆盖配置过的那几个,其余保持出生时设的那张。
    ///
    /// **`EC_*_By` 这一路没接**:那是把脸刷到**本体**材质(`_By`)上的,配置里的
    /// `FacialExpressionTarget=By`(雪熊三阶、妮妮二/三阶等 17 个资产)。本体材质是整只的
    /// 贴图,不能整片偏 UV —— 要接得先把那层脸图层单独抠出来,见 findings.md。
    public static Dictionary<string, string> SlotTracks(USkeletalMesh mesh)
    {
        var tracks = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        var seen = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
        foreach (var slot in mesh.Materials)
        {
            var name = slot?.Name;
            if (string.IsNullOrEmpty(name)) continue;
            var kind = KindOf(name);
            if (kind is null) continue;
            var ordinal = seen.GetValueOrDefault(kind);
            seen[kind] = ordinal + 1;
            tracks[name] = ordinal == 0 ? kind : $"{kind}_{ordinal}";
        }
        return tracks;
    }

    /// 材质名后缀 → 曲线族。`_Es` 眼、`_Mh` 嘴、`_Dynamic<n>` 各自一族
    /// (全库有一个只写了 `_Dynamic` 的,按 1 算)。别的后缀不是脸槽。
    private static string? KindOf(string material)
    {
        var name = material.TrimEnd();
        var underscore = name.LastIndexOf('_');
        if (underscore < 0) return null;
        var suffix = name[(underscore + 1)..].ToLowerInvariant();
        if (suffix.Length == 0) return null;
        // `_Es` / `_Es1`、`_Mh` / `_Mh2`:尾数是美术编号,不是这里要的序号(序号按槽序数)
        if (suffix.StartsWith("es") && suffix[2..].All(char.IsAsciiDigit)) return "eye";
        if (suffix.StartsWith("mh") && suffix[2..].All(char.IsAsciiDigit)) return "mouth";
        if (suffix.StartsWith("dynamic") && suffix[7..].All(char.IsAsciiDigit))
            return "dynamic" + (suffix.Length > 7 ? suffix[7..] : "1");
        return null;
    }

    /// 曲线名(manifest 里的键 → 动画里的曲线名):`eye_1` → `EC_Eye_1`。
    private static string CurveName(string key)
    {
        var underscore = key.IndexOf('_');
        var kind = underscore < 0 ? key : key[..underscore];
        var tail = underscore < 0 ? "" : key[underscore..];
        var name = kind switch
        {
            "eye" => "Eye",
            "mouth" => "Mouth",
            _ => char.ToUpperInvariant(kind[0]) + kind[1..],   // dynamic1 → Dynamic1
        };
        return $"EC_{name}{tail}";
    }

    /// 这段动画里,`keys` 那几个槽各自的曲线。没有内容的槽不出现在结果里。
    public static List<(string Key, FaceTrack Track)> Extract(
        UAnimSequence sequence, float seconds, IEnumerable<string> keys)
    {
        var result = new List<(string, FaceTrack)>();
        foreach (var key in keys.Distinct(StringComparer.OrdinalIgnoreCase).OrderBy(k => k, StringComparer.Ordinal))
            if (Extract(sequence, CurveName(key), seconds) is { } track)
                result.Add((key, track));
        return result;
    }

    /// 把一条曲线读成阶梯关键帧。返回 null = 没有这条曲线,或者整段都是第 1 格
    /// (「默认」——运行时那时用性格给的那张脸,写出来只是白占体积)。
    private static FaceTrack? Extract(UAnimSequence sequence, string name, float seconds)
    {
        var curves = sequence.CompressedCurveData?.FloatCurves;
        if (curves is null) return null;
        var curve = curves.FirstOrDefault(c =>
            c.CurveName.Text.Equals(name, StringComparison.OrdinalIgnoreCase));
        if (curve?.FloatCurve?.Keys is not { Length: > 0 } keys) return null;

        // **时间要先归到 [0, 时长]**。一部分动画(Eat_End、Fear_Loop…)的关键帧时间是负的
        // ——那是蒙太奇里按段偏移过的坐标。负时间的那几帧只影响「t=0 时是哪一格」,
        // 所以全部折到 0;超出时长的直接丢。
        var steps = new List<FaceKey>();
        foreach (var key in keys.OrderBy(k => k.Time))
        {
            var ms = (int)MathF.Round(MathF.Max(key.Time, 0f) * 1000f);
            if (key.Time > seconds + 1e-3f) break;
            var card = Math.Clamp((int)MathF.Round(key.Value / 100f), 1, MaxCard);
            // 折到 0 之后可能撞上前一帧:后来的那个才是 t=0 生效的值
            if (steps.Count > 0 && steps[^1].Ms == ms) steps[^1] = new FaceKey(ms, card);
            else steps.Add(new FaceKey(ms, card));
        }
        if (steps.Count == 0) return null;

        // 相邻同值的合并掉(曲线里成片的 0 很多),再把「从头到尾都是默认格」整条丢掉
        var merged = new List<FaceKey> { steps[0] };
        foreach (var step in steps.Skip(1))
            if (step.Card != merged[^1].Card)
                merged.Add(step);
        if (merged.Count == 1 && merged[0].Card == 1) return null;
        // 第一帧不在 0 上时补一格默认:运行时是「查表取当前生效的那一格」,
        // 没有起点的话开头那一小段会取不到值。
        if (merged[0].Ms > 0) merged.Insert(0, new FaceKey(0, 1));
        return new FaceTrack(merged);
    }
}
