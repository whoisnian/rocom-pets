// 宠物音频:从 Wwise SoundBank 里找到事件对应的 .wem,转成 ogg 进包。
//
// 关联链路是 rocom-capture/docs/audio.md 查实的,rocom-petvo 的网页版已经跑通一遍;
// 这里是那条链路的最小移植 —— 只要「事件名 → wem」与「音调 RTPC 曲线」两件事。
//
// **不必另外解包音频**:WwiseAudio 那 3.3GB 就在 pak 里,导出器手上已经有 provider。
//
// ## 两族库 = 两层声音
//
// 同一只宠物、同一段情绪,游戏里叠着放两条:
//
// | 库 | 内容 | 变调 |
// | --- | --- | --- |
// | `Pet_Vo_<拼音>.bnk` | 叫声(嗓子发出来的) | 跟着 `Pet_Vo_Pitch` |
// | `Pet_Action_<拼音>.bnk` | 动作音效(身体动静:落地、扑翅、拖尾巴) | **不变调** |
//
// 两条**不是同一段音频的两份拷贝**:同名事件的 10ms RMS 包络相关只有 0.1~0.4,
// 而「同一段」的判据是 0.87 以上(rocom-capture/docs/audio.md §10)。动作那层还明显更轻
// (RMS 约 0.015 比 0.045),听感上是垫在叫声底下的一层。
//
// 「不变调」也是查出来的而不是猜的:621 个 `Pet_Vo_*` 里 619 个挂着 `Pet_Vo_Pitch` 曲线,
// 而 650 个 `Pet_Action_*` 里**只有 1 个**有。那个 Game Parameter 是给嗓子用的。
//
// 偏移全是对本作(BKHD version 135 / Wwise 2021.1)的实测值,换游戏不保证适用。

using System.Diagnostics;
using System.Text;
using CUE4Parse.FileProvider.Vfs;

namespace RocomPets.Export;

/// 一段音频在包里的样子。`Key` 是**动作逻辑名**,与 `[forms.clips]` 同一把键。
public record AudioClip(string Key, string RelativePath, int Ms);

/// 一个形态的声音:叫声若干段 + 动作音效若干段 + 音调曲线两端。
public record AudioInfo(List<AudioClip> Voice, List<AudioClip> Sfx, int CentsLow, int CentsHigh);

public static class Audio
{
    private const string WwiseDir = "NRC/Content/NewRoco/WwiseAudio/Windows/";

    /// 桌宠会播的动作 → Wwise 事件后缀。
    ///
    /// **键就是动作逻辑名**(`[forms.clips]` 那一张表的键),不另起一套触发点名字:
    /// 运行时是「播哪段动作」决定「出什么声」,两张表同一把键才不会各对各的。
    /// 剩下的动作(Idle/Walk/Run/JumpFall/Sleep*)在两族库里都没有对应事件。
    ///
    /// 后缀大小写在源数据里不统一(`Common_SAd` / `Fight_Callout` 都有),但**这里不用管**:
    /// Wwise 的 FNV-1 是**先转小写**再哈希的,各种写法命中同一个 Event id。
    ///
    /// 全库覆盖率(621 个 `Pet_Vo_*` / 650 个 `Pet_Action_*`):八个 `Common_*` 都是
    /// 617~648 只有,`Fight_CallOut` 612/644。也就是说**降级基本用不上** ——
    /// 原来那张「取不到 Happy 就退 Show」的导出期降级表是空转,已经去掉;
    /// 真缺一段时由运行时按它自己的动作降级表(`stage::fallbacks`)退,那才是一套。
    private static readonly (string Key, string Event)[] Wanted =
    [
        ("Happy", "Common_Happy"),
        ("Shock", "Common_Shock"),
        ("Fear", "Common_Fear"),
        ("Sad", "Common_Sad"),
        ("Anger", "Common_Anger"),
        ("Show", "Common_Show"),
        ("Relax", "Common_Relax"),
        ("Alert", "Common_Alert"),
        ("CallOut", "Fight_CallOut"),
    ];

    /// Game Parameter 名:游戏把 -100~100 的 `voice` 属性喂给它,由 RTPC 曲线实时变调。
    private const string PitchParam = "Pet_Vo_Pitch";

    /// 外部工具在不在。缺了就整批跳过音频(包仍然可用),而不是让导出失败。
    private static readonly Lazy<bool> ToolsReady = new(() =>
        Which("vgmstream-cli") && Which("ffmpeg"));

    public static bool Available => ToolsReady.Value;

    /// 导出一个形态的音频。拿不到就返回 null(不是错误:39 个 bnk 查无此宠,
    /// 还有形态压根没有 `Pet_Vo_*` 库)。
    public static AudioInfo? Export(
        AbstractVfsFileProvider provider,
        string pinyin,
        string formDir,
        string relativeDir,
        List<string> warnings)
    {
        if (!Available) return null;
        var voiceBank = Load(provider, $"Pet_Vo_{pinyin}");
        var sfxBank = Load(provider, $"Pet_Action_{pinyin}");

        var voice = Rip(provider, voiceBank, $"Pet_Vo_{pinyin}",
            formDir, relativeDir, "voice", warnings);
        var sfx = Rip(provider, sfxBank, $"Pet_Action_{pinyin}",
            formDir, relativeDir, "sfx", warnings);
        if (voice.Count == 0 && sfx.Count == 0) return null;

        // 音调曲线:三点线性(x = -100 / 0 / +100),两端的音分就是「粗嗓门 / 婉转声」。
        // 只问叫声那族库 —— 动作音效不跟着变调。读不到就按 0 走(不变调),声音照样能用
        var curve = voiceBank?.PitchCurve(PitchParam);
        var low = curve is null ? 0 : CentsAt(curve, -100);
        var high = curve is null ? 0 : CentsAt(curve, 100);
        return new AudioInfo(voice, sfx, low, high);
    }

    private static Bank? Load(AbstractVfsFileProvider provider, string stem) =>
        provider.TrySaveAsset($"{WwiseDir}{stem}.bnk", out var bytes) ? new Bank(bytes) : null;

    /// 把一族库里认得的事件全转成 ogg,写进 `<形态>/<sub>/`。
    private static List<AudioClip> Rip(
        AbstractVfsFileProvider provider,
        Bank? bank,
        string eventPrefix,
        string formDir,
        string relativeDir,
        string sub,
        List<string> warnings)
    {
        var clips = new List<AudioClip>();
        if (bank is null) return clips;
        var dir = Path.Combine(formDir, sub);
        // 同一段 wem 被两个事件共用是有的(全库 621 只里 1 只),转一次就够
        var done = new Dictionary<uint, AudioClip>();
        foreach (var (key, suffix) in Wanted)
        {
            // 同一个事件通常挂 3 个随机变体,**取最小 id** 那条:导出要可复现
            var wems = bank.EventWems($"{eventPrefix}_{suffix}");
            if (wems.Count == 0) continue;
            var source = wems.Min();
            if (done.TryGetValue(source, out var same))
            {
                clips.Add(same with { Key = key });
                continue;
            }
            if (!provider.TrySaveAsset($"{WwiseDir}{source}.wem", out var wem)) continue;

            Directory.CreateDirectory(dir);
            var outPath = Path.Combine(dir, key + ".ogg");
            var ms = Transcode(wem, outPath, warnings);
            if (ms <= 0) continue;
            var clip = new AudioClip(key, $"{relativeDir}/{sub}/{key}.ogg", ms);
            done[source] = clip;
            clips.Add(clip);
        }
        return clips;
    }

    /// 曲线上 x 处的音分(线性内插;超出端点就取端点)。
    private static int CentsAt(List<(float X, float Y)> curve, float x)
    {
        if (curve.Count == 0) return 0;
        if (x <= curve[0].X) return (int)MathF.Round(curve[0].Y);
        for (var i = 1; i < curve.Count; i++)
        {
            if (x > curve[i].X) continue;
            var (x0, y0) = curve[i - 1];
            var (x1, y1) = curve[i];
            var t = x1 - x0 <= 0 ? 0 : (x - x0) / (x1 - x0);
            return (int)MathF.Round(y0 + (y1 - y0) * t);
        }
        return (int)MathF.Round(curve[^1].Y);
    }

    /// wem → ogg。**ffmpeg 解不了 Wwise Vorbis**,得先用 vgmstream 解成 wav。
    /// 返回时长(毫秒),失败返回 0。
    private static int Transcode(byte[] wem, string outPath, List<string> warnings)
    {
        var temp = Path.Combine(Path.GetTempPath(), $"rocom-voice-{Guid.NewGuid():N}");
        var wemPath = temp + ".wem";
        var wavPath = temp + ".wav";
        try
        {
            File.WriteAllBytes(wemPath, wem);
            if (!Run("vgmstream-cli", ["-o", wavPath, wemPath], out var err))
            {
                warnings.Add($"音频解码失败: {err}");
                return 0;
            }
            // 单声道 + 最低质量档:桌宠的叫声是一两秒的短音,再高听不出来,而全库 800 多个
            // 形态、每个形态最多 18 段,每 10KB 都要乘上万。
            //
            // **别顺手裁尾巴**:源里每段末尾都挂着 0.6~1.5 秒的数字静音(占时长三成),
            // 看着像白占地方 —— 实测裁掉只省 1~5% 字节,vorbis 编静音本来就几乎不花钱。
            if (!Run("ffmpeg", [
                    "-hide_banner", "-loglevel", "error", "-y",
                    "-i", wavPath, "-ac", "1", "-c:a", "libvorbis", "-q:a", "0", outPath
                ], out err))
            {
                warnings.Add($"音频转码失败: {err}");
                return 0;
            }
            return WavMs(wavPath);
        }
        finally
        {
            File.Delete(wemPath);
            File.Delete(wavPath);
        }
    }

    /// 从 wav 头算时长。只读自己刚生成的文件,不做通用 wav 解析。
    private static int WavMs(string path)
    {
        try
        {
            var bytes = File.ReadAllBytes(path);
            if (bytes.Length < 44) return 0;
            var rate = BitConverter.ToUInt32(bytes, 24);
            var bytesPerSec = BitConverter.ToUInt32(bytes, 28);
            if (rate == 0 || bytesPerSec == 0) return 0;
            return (int)((bytes.Length - 44L) * 1000 / bytesPerSec);
        }
        catch (IOException)
        {
            return 0;
        }
    }

    /// 这个可执行文件在不在。**只看进程起不起得来,不看退出码** ——
    /// vgmstream-cli 没有 `--version`,任何「没给输入文件」的调用都退 1。
    private static bool Which(string exe)
    {
        var psi = new ProcessStartInfo(exe)
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        };
        psi.ArgumentList.Add("-h");
        try
        {
            using var p = Process.Start(psi);
            if (p is null) return false;
            p.StandardOutput.ReadToEnd();
            p.StandardError.ReadToEnd();
            p.WaitForExit();
            return true;
        }
        catch (Exception)
        {
            return false;
        }
    }

    private static bool Run(string exe, string[] args, out string error)
    {
        var psi = new ProcessStartInfo(exe)
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        };
        foreach (var a in args) psi.ArgumentList.Add(a);
        try
        {
            using var p = Process.Start(psi);
            if (p is null) { error = $"{exe} 起不来"; return false; }
            var stderr = p.StandardError.ReadToEnd();
            p.StandardOutput.ReadToEnd();
            p.WaitForExit();
            error = p.ExitCode == 0 ? "" : $"{exe} 退出码 {p.ExitCode}: {Tail(stderr)}";
            return p.ExitCode == 0;
        }
        catch (Exception e)
        {
            error = $"{exe}: {e.Message}";
            return false;
        }
    }

    private static string Tail(string text)
    {
        var lines = text.Split('\n', StringSplitOptions.RemoveEmptyEntries);
        return lines.Length == 0 ? "" : lines[^1].Trim();
    }

    // ── Wwise SoundBank 最小解析 ─────────────────────────────────────

    /// HIRC 对象类型。
    private const byte Sound = 2, Action = 3, Event = 4, RanSeq = 5, Switch = 6, ActorMixer = 7;

    private static bool IsContainer(byte type) => type is RanSeq or Switch or ActorMixer;

    private sealed class Bank
    {
        private readonly byte[] _buf;
        private readonly Dictionary<uint, (byte Type, int Start, int End)> _objs = new();
        private readonly Dictionary<uint, List<uint>> _children = new();

        public Bank(byte[] buf)
        {
            _buf = buf;
            var p = 0;
            while (p < buf.Length - 8)
            {
                var size = BitConverter.ToUInt32(buf, p + 4);
                if (Encoding.ASCII.GetString(buf, p, 4) == "HIRC") ReadHirc(p + 8);
                p += 8 + (int)size;
            }
            // 靠 directParentID 反建父子树。**别用「扫描对象体里的 4 字节、命中已知 id 就当
            // 子节点」那种启发式** —— 引用会一路爬到 ActorMixer 根,把整个 bnk 的 Sound
            // 全吞进来(rocom-capture 那边踩过:三个不同事件返回完全相同的 67 个 wem)
            foreach (var (id, (type, start, end)) in _objs)
            {
                var parent = DirectParent(type, start, end);
                if (parent is not null)
                {
                    if (!_children.TryGetValue(parent.Value, out var list))
                        _children[parent.Value] = list = [];
                    list.Add(id);
                }
            }
        }

        private void ReadHirc(int off)
        {
            var n = BitConverter.ToUInt32(_buf, off);
            var p = off + 4;
            for (var i = 0; i < n && p + 5 <= _buf.Length; i++)
            {
                var type = _buf[p];
                var size = (int)BitConverter.ToUInt32(_buf, p + 1);
                var id = BitConverter.ToUInt32(_buf, p + 5);
                _objs[id] = (type, p + 5, p + 5 + size);
                p += 5 + size;
            }
        }

        /// NodeBaseParams.directParentID。偏移 = 前缀 + nFX 块 + 2 + 4;
        /// Sound 前缀 14B、容器无前缀,nFX = 0 时实测落在 Sound @25 / 容器 @11。
        private uint? DirectParent(byte type, int start, int end)
        {
            var off = type == Sound ? 25 : IsContainer(type) ? 11 : -1;
            if (off < 0 || start + off + 4 > end || start + off + 4 > _buf.Length) return null;
            var parent = BitConverter.ToUInt32(_buf, start + off);
            return _objs.ContainsKey(parent) ? parent : null;
        }

        /// 事件名 → 它会播到的所有 wem 的 sourceID(去重,保留遍历序)。
        public List<uint> EventWems(string eventName)
        {
            var eid = Fnv1(eventName);
            if (!_objs.TryGetValue(eid, out var ev) || ev.Type != Event) return [];
            var count = _buf[ev.Start + 4];   // Event 的 action 数量是 uint8
            var targets = new List<uint>();
            for (var k = 0; k < count; k++)
            {
                var at = ev.Start + 5 + 4 * k;
                if (at + 4 > _buf.Length) break;
                var aid = BitConverter.ToUInt32(_buf, at);
                if (_objs.TryGetValue(aid, out var action) && action.Type == Action)
                    targets.Add(BitConverter.ToUInt32(_buf, action.Start + 6));
            }

            var seen = new HashSet<uint>();
            var outIds = new List<uint>();
            void Walk(uint id)
            {
                if (!seen.Add(id) || !_objs.TryGetValue(id, out var obj)) return;
                if (obj.Type == Sound) outIds.Add(BitConverter.ToUInt32(_buf, obj.Start + 9));
                if (!_children.TryGetValue(id, out var kids)) return;
                foreach (var c in kids.OrderBy(x => x)) Walk(c);
            }
            foreach (var t in targets) Walk(t);
            return outIds.Distinct().ToList();
        }

        /// 容器/ActorMixer 上 `param` 对 Pitch 的 RTPC 曲线 → [(x, 音分)]。
        ///
        /// 没做完整的 RTPC 段游标解析(各节点结构差异大),而是全 buf 搜 Game Parameter 的 id,
        /// 再用「宿主是容器 + ParameterID == 2(Pitch) + 点数与取值在合理范围」三重校验筛掉
        /// 误命中。效果器插件上的同名 RTPC 会因为宿主不是容器被排除 —— 那边的 ParameterID
        /// 是插件私有下标,和这里的枚举不是一套。
        public List<(float X, float Y)>? PitchCurve(string param)
        {
            var key = BitConverter.GetBytes(Fnv1(param));
            var spans = _objs.Values.ToList();
            for (var o = 0; o + 4 <= _buf.Length; o++)
            {
                if (_buf[o] != key[0] || _buf[o + 1] != key[1] ||
                    _buf[o + 2] != key[2] || _buf[o + 3] != key[3]) continue;
                var owner = spans.FirstOrDefault(s => s.Start <= o && o < s.End);
                if (!IsContainer(owner.Type)) continue;
                if (o + 14 > _buf.Length || _buf[o + 6] != 2) continue;
                var npts = BitConverter.ToUInt16(_buf, o + 12);
                if (npts is < 2 or > 8) continue;
                if (o + 14 + 12 * npts > _buf.Length) continue;
                var pts = new List<(float, float)>(npts);
                var sane = true;
                for (var k = 0; k < npts; k++)
                {
                    var x = BitConverter.ToSingle(_buf, o + 14 + 12 * k);
                    var y = BitConverter.ToSingle(_buf, o + 18 + 12 * k);
                    if (MathF.Abs(x) > 200 || MathF.Abs(y) > 4800) { sane = false; break; }
                    pts.Add((x, y));
                }
                if (sane) return pts;
            }
            return null;
        }
    }

    /// Wwise 用 FNV-1(不是 1a)32 位哈希,且**先转小写**。
    private static uint Fnv1(string name)
    {
        var h = 2166136261u;
        foreach (var b in Encoding.UTF8.GetBytes(name.ToLowerInvariant()))
            h = (h * 16777619u) ^ b;
        return h;
    }
}
