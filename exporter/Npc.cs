// NPC 出包:把 `ArtRes/AnimSequence/Human/NPC/` 那棵树接进宠物那条管线。
//
// **为什么能直接接**:两棵树的资产结构逐项对得上 —— 同样是一个角色一个目录,里面
// `SKM_<码>_Skin` + `SK_<码>_Skin` + `Animation/*.uasset` + `Mat/` + `Tex/`,骨骼名同样是
// `Bip001-*` 那套通用命名,动画文件同样带 `World_`/`Fight_` 场景类别前缀。于是网格、材质、
// 贴图、glb 合并这四步一行都不用改,只要把「资产根」从常量变成参数(见 `AssetRoots`)。
//
// 真正要新写的只有两件事:
//
// ① **配置侧的入口不一样。** 宠物走 `PETBASE_CONF → MODEL_CONF → ANIM_CONF`,一条进化链一个包;
//    NPC 走 `NPC_CONF.model_conf → MODEL_CONF.path(里面写着 NPC_xxxxx)→ ANIM_CONF`,
//    **一个角色一个包、一个模型目录一个形态**。同一个角色在 `NPC_CONF` 里往往有几十行
//    (可丽希亚 100 行),那是「同一个人在不同任务/不同场景的实例」,不是不同外观 ——
//    按模型目录归并之后才是真正的形态数(芙蕾雅 2、可丽希亚 2、乐乐 1)。
//    `BP/Scene/…` 与 `BP/Battle/…` 是同一份模型的两个蓝图,`anim_conf` 内容实测一字不差
//    (12270 vs 10252、12290 vs 10270、12009 vs 10012 三对都比过),所以按目录合并、取并集。
//
// ② **动作名对不上。** 宠物的逻辑名(`Idle`/`Happy`/`SleepStart`)就是运行时那套;NPC 的是
//    `Happy1`/`Hello1`/`SitDownStart` 这种,而且没有 `Sleep*`、没有 `JumpFall`、没有技能循环段。
//    于是这里有一张**别名表**(`NpcClips.Aliases`):运行时动作名 → 候选 NPC 动作名(按优先级)。
//    翻译发生在导出期,写进包里的键与宠物包**完全一致** —— 运行时、配置窗口、下载站的预览
//    一行都不用改。
//
// 音频只能给一部分:见 `NpcClips.VoiceBanks`。

using System.Text.RegularExpressions;
using Newtonsoft.Json.Linq;

namespace RocomPets.Export;

public static class NpcClips
{
    /// 运行时动作名 → NPC 动作名的候选(按优先级,**第一个在资产里找得到的**胜出)。
    ///
    /// 键取自 `stage::RUNTIME_CLIPS`(src/stage.rs),值取自 `ANIM_ID_CONF` 的逻辑名。
    /// 候选之所以要给好几个,是因为同一个角色的不同外观做的动作并不齐:
    /// 「放松」在可丽希亚/乐乐那儿是 `Idle_Relax_1`、在 07401 版芙蕾雅那儿是没有后缀的
    /// `Idle_Relax`、在 08501 版那儿一个都没有(那就真缺,运行时按 `stage::fallbacks` 退)。
    ///
    /// **`Sleep*` 接的是「坐下」那三段。** NPC 没有睡觉动作(乐乐的 `ANIM_CONF` 里挂着
    /// `DeepSleepLoop`,但资产里没有这个文件);坐下起坐三段的结构与 `SleepStart/Loop/End`
    /// 一模一样,桌面上的观感也最接近「歇着」。这是**判断**,不是数据直说的,所以写在这儿。
    ///
    /// **`JumpFall` 与 `Skill*Loop` 没有对应,故意不给。** NPC 的 `Fight_*` 是入场演出
    /// (`Start`/`Start_Show`/`Command`),不是能按住重播的循环段,硬接上去就是每隔几秒
    /// 瞬移一次 —— 与宠物那边「只要 Loop 不要 Start/End」的理由是同一条。
    public static readonly (string Runtime, string[] Sources, string Label)[] Aliases =
    [
        ("Idle", ["Idle"], "待机"),
        ("Walk", ["Walk"], "行走"),
        ("Run", ["Run"], "奔跑"),
        ("Happy", ["Happy1", "Happy2"], "开心"),
        ("Anger", ["Anger1", "Anger2"], "生气"),
        ("Sad", ["Sad1", "Sad2"], "难过"),
        ("Fear", ["Fear1", "Fear0"], "害怕"),
        ("Shock", ["Shock1", "Shock0"], "受惊"),
        ("Show", ["Hello1", "Greet1"], "展示"),
        ("Relax", ["IdleRelax1", "IdleRelax", "CrossarmsLoop", "Crossarms"], "放松"),
        ("Alert", ["Think", "WatchOut"], "警觉"),
        ("CallOut", ["CallOut"], "召唤"),
        ("SleepStart", ["SitDownStart", "SitDown2Start"], "入睡"),
        ("SleepLoop", ["SitDownLoop", "SitDown2Loop"], "睡着"),
        ("SleepEnd", ["SitDownEnd", "SitDown2End"], "醒来"),
    ];

    /// 动作名 → 中文标签,给配置窗口那张动作表用。
    ///
    /// **上面那张别名表管的是「运行时靠它活着」的十五段;这张管的是别的全部。**
    /// NPC 的动作比宠物丰富得多(全库 190 个名字,单个角色最多 86 段):对话有五段常规
    /// 加七种带情绪的、姿态是「起手/循环/收手」三段一组(抱臂/叉腰/抚胸/托腮/伸手)、
    /// 还有行礼、干杯、挣扎、晕倒、战斗入场演出。这些宠物一个都没有,所以**不必和宠物那套对齐** ——
    /// 导出器把资产目录里**除了已经被别名表用掉的之外的每一段**都收进包,名字就用
    /// `AnimNames.LogicalOf` 从文件名算出来的那个,标签查这张表(查不到就用名字本身)。
    ///
    /// 表里没列全也不影响导出,只是配置窗口里显示成英文名。
    public static readonly Dictionary<string, string> Labels = new(StringComparer.OrdinalIgnoreCase)
    {
        // 情绪的第二版(第一版被别名表拿去当 Happy/Anger/Sad 了)
        ["Happy1"] = "开心一", ["Happy2"] = "开心二",
        ["Anger1"] = "生气一", ["Anger2"] = "生气二",
        ["Sad1"] = "难过一", ["Sad2"] = "难过二",
        ["Fear0"] = "害怕·轻", ["Fear1"] = "害怕",
        ["Shock0"] = "吃惊·轻", ["Shock1"] = "吃惊",
        ["Hello1"] = "打招呼", ["Greet1"] = "问好",
        ["Think"] = "思索", ["Provoke"] = "挑衅", ["WatchOut"] = "警戒",
        ["Faint"] = "晕倒", ["Die"] = "倒下",
        ["IdleRelax"] = "歇着", ["IdleRelax1"] = "歇着",
        // 对话:五段常规 + 七种带情绪的
        ["Dialogue1"] = "说话一", ["Dialogue2"] = "说话二", ["Dialogue3"] = "说话三",
        ["Dialogue4"] = "说话四", ["Dialogue5"] = "说话五",
        ["DialogueAnger1"] = "说话·生气", ["DialogueFear1"] = "说话·害怕",
        ["DialogueHappy1"] = "说话·开心", ["DialogueSad1"] = "说话·难过",
        ["DialogueShock1"] = "说话·吃惊", ["DialogueThink1"] = "说话·思索",
        ["DialogueHandOnHead1"] = "说话·挠头",
        // 姿态:起手 / 循环 / 收手 三段一组
        ["Crossarms"] = "抱臂", ["CrossarmsStart"] = "抱臂·起", ["CrossarmsLoop"] = "抱臂·持",
        ["CrossarmsEnd"] = "抱臂·收",
        ["CrossarmsGesture"] = "抱臂手势", ["CrossarmsGestureStart"] = "抱臂手势·起",
        ["CrossarmsGestureLoop"] = "抱臂手势·持", ["CrossarmsGestureEnd"] = "抱臂手势·收",
        ["CrossarmsGesture2"] = "抱臂手势二", ["CrossarmsGesture2End"] = "抱臂手势二·收",
        ["HandSonhips"] = "叉腰", ["HandSonhipsStart"] = "叉腰·起",
        ["HandSonhipsLoop"] = "叉腰·持", ["HandSonhipsEnd"] = "叉腰·收",
        ["HandSonhipsGestureEnd"] = "叉腰手势·收",
        ["HandOnchest"] = "抚胸", ["HandOnchestStart"] = "抚胸·起",
        ["HandOnchestLoop"] = "抚胸·持", ["HandOnchestEnd"] = "抚胸·收",
        ["HandOnchest2End"] = "抚胸二·收",
        ["HandOnchin"] = "托腮", ["HandOnchinStart"] = "托腮·起",
        ["HandOnchinLoop"] = "托腮·持", ["HandOnchinEnd"] = "托腮·收",
        ["HandOnchin2End"] = "托腮二·收",
        ["HandReach"] = "伸手", ["HandReachStart"] = "伸手·起",
        ["HandReachLoop"] = "伸手·持", ["HandReachEnd"] = "伸手·收",
        // 坐下(别名表把它接成了 Sleep*,这里是第二套)
        ["SitDownStart"] = "坐下·起", ["SitDownLoop"] = "坐着", ["SitDownEnd"] = "起身",
        ["SitDown2Start"] = "坐下二·起", ["SitDown2Loop"] = "坐着二", ["SitDown2End"] = "起身二",
        // 零散的世界动作
        ["XingLi"] = "行礼", ["XingLiStart"] = "行礼·起", ["XingLiLoop"] = "行礼·持",
        ["XingLiEnd"] = "行礼·收",
        ["GanBei"] = "干杯", ["ZhengZha"] = "挣扎", ["MagicBoss"] = "施法",
        ["ItemGive"] = "递东西", ["Command"] = "下令",
        ["RolePlayAnger"] = "扮演·生气", ["RolePlayThreaten"] = "扮演·威吓",
        // 战斗演出(前缀留着,见 AnimNames.LogicalOf)
        ["FightStart"] = "出场", ["FightStart2"] = "出场二",
        ["FightStart31"] = "出场三·前", ["FightStart32"] = "出场三·后",
        ["FightStartShow"] = "登场亮相", ["FightCommand"] = "下令",
        ["FightLose"] = "战败", ["FightCallOut"] = "召唤", ["FightCallBack"] = "收回",
        ["FightCallBackLose"] = "收回·败", ["FightYingDui"] = "应对",
        ["FightMagicBloodUp"] = "强化", ["FightMagicBlack"] = "黑魔法", ["FightLeave"] = "退场",
    };

    /// 不收进「其余全收」那一批的动作。**别名表不受这里影响** ——
    /// `CallOut` 的源就是 `Fight_CallOut`,那是运行时要用的,照旧翻过来。
    ///
    /// - `Face_*`:表情/口型是**脸上的叠加曲线**,不是身体动作
    ///   (`Face_Expression_PoseAsset` 根本不是 `UAnimSequence`,加载就报错);
    /// - `Fight_*`:战斗入场演出(出场/亮相/下令/战败/应对)。桌宠用不上,而且**它们正是
    ///   上游 ACL 解码最容易坏的那一批** —— 罗兰的 `Fight_Command` 整条腿的平移是别的骨骼的,
    ///   渲出来身体折叠炸开;而同一个角色的 `World_*` 一段都没坏。全量导出 70 个形态里
    ///   11 个的身体骨报警,坏的全在这一族。**少一段动作,好过多一段炸开的。**
    ///   真要看战斗演出就 `--all-clips`(那一档按 `ANIM_CONF` 的原始逻辑名导,不走这张表)。
    public static bool Skip(string fileName) =>
        fileName.StartsWith("Face_", StringComparison.OrdinalIgnoreCase)
        || fileName.StartsWith("Fight_", StringComparison.OrdinalIgnoreCase);

    /// 这段动作在配置窗口里叫什么。查不到就用动作名本身。
    public static string LabelOf(string logical) => Labels.GetValueOrDefault(logical, logical);

    /// 角色名 → Wwise 库名(`NPC_Vo_<库名>.bnk`)。
    ///
    /// **这张表是人工填的,因为解包数据里查不到这条链。** 全树 grep `Claria` 只命中
    /// `Data/Audio/dataconfig_audio.bytes`(那里面是事件名 → id 的表),没有任何配置表把
    /// 中文名接到库名上;蓝图里也只有一个光秃秃的 `AudioComponent`。能确认的只有:
    /// 库一共 14 个(`NPC_Vo_{Annie,Bode,Claria,Collen,EC,Envious,Envy,Enzo,Felter,Griffin,
    /// Hate,Iris,Louis,Pica}`),每个带八种情绪(`_Calm/_Happy/_Scorn/_Shock/_Anger/_Fear/
    /// _Sad/_Shy`),其中 `Hate` 对得上 `NPC_07701` 那个名字就叫「恨」的角色、
    /// `Felter` 对得上 `NPC_01401`(配置里写作「菲尔特, 费尔特」)—— 这两条说明库名是
    /// **意译/音译的英文名**,不是拼音。
    ///
    /// 所以 `可丽希亚 → Claria` 是**推断**:她是主线里戏份最重的角色之一(NPC_CONF 100 行、
    /// 304 段动画),十四个库里只剩这一个对得上的女性名字,而 `MAGE_CONF` 给她的英文名
    /// `KRITHIA` 也是同一个中文名的另一种转写。**拿不准就把这一行删掉** —— 删了只是没声音。
    ///
    /// 芙蕾雅与乐乐**没有库**(十四个里没有 Freya/Fuleiya/Lele/Yueyue),她们的台词在
    /// 174 个按章节打包的 `Story_*.bnk` 里,不按角色分,挑不干净。
    public static readonly Dictionary<string, string> VoiceBanks = new(StringComparer.Ordinal)
    {
        ["可丽希亚"] = "Claria",
    };
}

public partial class GameConfig
{
    /// `NPC_CONF`。18293 行,只有导 NPC 时才读。
    private JObject? _npcRows;

    private JObject Npc => _npcRows ??= Rows(_binDir, "NPC_CONF");

    /// `MODEL_CONF.path` 里的 NPC 资产码。宠物那条是 `…/Pets/<资产>/…`,
    /// NPC 这条是 `Blueprint'/Game/ArtRes/BP/Scene/NPC_08501/BP_Scene_NPC_08501…'` ——
    /// 蓝图路径里的目录名就是资产码,而资产码同时也是 `Human/NPC/` 下的目录名。
    private static readonly Regex NpcAssetPattern = new(@"NPC_(\d{5})", RegexOptions.Compiled);

    private static string? NpcAssetOf(JToken? model)
    {
        var m = NpcAssetPattern.Match(model?["path"]?.Value<string>() ?? "");
        return m.Success ? $"NPC_{m.Groups[1].Value}" : null;
    }

    /// 明显不是角色的行:数据自己标的测试/占位,以及做占位用的问号名。
    private static bool IsExportableNpcName(string name) =>
        name.Length > 0
        && !name.Contains("测试") && !name.Contains("Test", StringComparison.OrdinalIgnoreCase)
        && !name.Contains("占位") && !name.Contains("废弃")
        && !name.Contains('？') && !name.Contains('?');

    /// 一个模型目录在表里的样子:出现过的名字与次数、最小行号、用到的 model_conf。
    private sealed class NpcDir
    {
        public readonly Dictionary<string, int> Names = new(StringComparer.Ordinal);
        public readonly SortedSet<int> Models = [];
        public int MinRowId = int.MaxValue;
    }

    /// **归并单位是模型目录,不是 `NPC_CONF.name`。** 那个字段一半是编辑器备注
    /// (`7天版本-黑巫团成员31-大道半岛北半岛`、`A2拾遗_侦探社_罗宾`、`干劲值+5`),
    /// 直接按它建包会出三百个「角色」,而且同一份模型被切成几十份。
    private Dictionary<string, NpcDir> NpcDirs()
    {
        var dirs = new Dictionary<string, NpcDir>(StringComparer.Ordinal);
        foreach (var (key, row) in Npc)
        {
            if (row is null || !int.TryParse(key, out var npcId)) continue;
            var modelId = row["model_conf"]?.Value<int>();
            if (modelId is null) continue;
            var asset = NpcAssetOf(_model[modelId.Value.ToString()]);
            if (asset is null) continue;
            if (!dirs.TryGetValue(asset, out var dir)) dirs[asset] = dir = new NpcDir();
            dir.Models.Add(modelId.Value);
            dir.MinRowId = Math.Min(dir.MinRowId, npcId);
            var name = row["name"]?.Value<string>() ?? "";
            if (name.Length > 0) dir.Names[name] = dir.Names.GetValueOrDefault(name) + 1;
        }
        return dirs;
    }

    /// 「干净的名字」:没有数字/连字符/下划线/引号括号、不超过八个字、不带测试占位字样。
    /// 那些编辑器备注(`7天版本-黑巫团成员31-…`、`A2拾遗_侦探社_罗宾`、`干劲值+5`)
    /// 全都带这些标记,而真名(`芙蕾雅`/`可丽希亚`/`乐乐`)一个都不带。
    private static bool IsCleanNpcName(string n) =>
        IsExportableNpcName(n) && n.Length <= 8
        && n.All(c => !char.IsAsciiDigit(c)
                      && c is not ('-' or '_' or '"' or '“' or '”' or '(' or ')' or '（' or '）'));

    /// 每个模型目录 → 它该叫什么:干净的名字里**这个目录用得最多**的那个。
    /// 空串 = 一个干净名字都没有(纯路人),`--npc-all` 跳过。
    ///
    /// ~~原来还想按「这个名字被几个目录共用」降权~~,以为能压掉 `居民`/`黑衣人` 这类
    /// 职业称呼 —— **那条规则是错的**:真人也会横跨几个目录(芙蕾雅两套外观、恩佐四套),
    /// 一降权她们反而各自捡到了 `芙蕾雅线索` 这种一次性备注名。词频本身就够:
    /// 芙蕾雅 29 行 vs 芙蕾雅线索 1 行。
    private static Dictionary<string, string> NpcDirNames(Dictionary<string, NpcDir> dirs) =>
        dirs.ToDictionary(
            kv => kv.Key,
            kv => kv.Value.Names
                      .Where(n => IsCleanNpcName(n.Key))
                      .OrderByDescending(n => n.Value)
                      .ThenBy(n => n.Key.Length)
                      .ThenBy(n => n.Key, StringComparer.Ordinal)
                      .Select(n => n.Key)
                      .FirstOrDefault() ?? "",
            StringComparer.Ordinal);

    /// 角色名 → 它用到的资产目录(按码升序)。`--npc-list` 与 `--npc-all` 用。
    /// 同一个角色的几套外观(芙蕾雅的 NPC_07401 / NPC_08501)在这里并成一条。
    public SortedDictionary<string, List<string>> NpcCatalog()
    {
        var dirs = NpcDirs();
        var names = NpcDirNames(dirs);
        var byName = new SortedDictionary<string, SortedSet<string>>(StringComparer.Ordinal);
        foreach (var (asset, name) in names)
        {
            if (name.Length == 0) continue;
            if (!byName.TryGetValue(name, out var set))
                byName[name] = set = new SortedSet<string>(StringComparer.Ordinal);
            set.Add(asset);
        }
        return new SortedDictionary<string, List<string>>(
            byName.ToDictionary(kv => kv.Key, kv => kv.Value.ToList()), StringComparer.Ordinal);
    }

    /// 按角色名建包。一个角色一个 [`Chain`],一个模型目录一个 [`Form`]。
    ///
    /// `aliases` 为真时,`NPC_CONF` 里出现过的任何一个别名也能命中(`--npc` 用:
    /// 拿表里读到的名字直接喂进来就该能用);`--npc-all` 只认规范名,免得
    /// 「居民」这种职业称呼把一堆不相干的模型拽进同一个包。
    public List<Chain> NpcPacks(IEnumerable<string> names, bool aliases = true)
    {
        var wanted = names.ToHashSet(StringComparer.Ordinal);
        var dirs = NpcDirs();
        var canonicalOf = NpcDirNames(dirs);
        var perName = new Dictionary<string, SortedDictionary<string, NpcDir>>(StringComparer.Ordinal);
        foreach (var (asset, dir) in dirs)
        {
            var canonical = canonicalOf[asset];
            var keys = aliases ? dir.Names.Keys.Append(canonical) : [canonical];
            var hit = keys.FirstOrDefault(n => n.Length > 0 && wanted.Contains(n));
            if (hit is null) continue;
            var packName = canonical.Length > 0 ? canonical : hit;
            if (!perName.TryGetValue(packName, out var byAsset))
                perName[packName] = byAsset = new SortedDictionary<string, NpcDir>(StringComparer.Ordinal);
            byAsset[asset] = dir;
        }

        var packs = new List<Chain>();
        foreach (var (name, byAsset) in perName.OrderBy(kv => kv.Key, StringComparer.Ordinal))
        {
            var forms = new List<Form>();
            var stage = 0;
            foreach (var (asset, dirInfo) in byAsset)
            {
                var (minId, models) = (dirInfo.MinRowId, dirInfo.Models);
                stage++;
                // Scene 与 Battle 两个蓝图各有一个 model_conf,指的是同一份模型;取码最小的
                // 那个当代表(缩放两边一致),动作清单取并集(实测两边一字不差,并集只是保险)。
                var modelConfId = models.Min();
                var model = _model[modelConfId.ToString()]!;
                var animConfId = model["anim_conf_id"]?.Value<int>() ?? modelConfId;
                var clips = new List<ClipInfo>();
                var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
                foreach (var id in models)
                {
                    var conf = _model[id.ToString()]?["anim_conf_id"]?.Value<int>() ?? id;
                    foreach (var clip in ClipsOf(conf))
                        if (seen.Add(clip.Logical)) clips.Add(clip);
                }
                // **同一个角色的几套外观得能分辨**:数据里没给外观名(不像宠物有 MEGAMAP_CONF
                // 的「本来的样子」),只能拿资产码当标签 —— 它至少是可查的。
                var formName = byAsset.Count > 1 ? $"{name}({asset})" : name;
                forms.Add(new Form(
                    minId, formName, stage, asset, modelConfId, animConfId,
                    (model["model_scale"]?.Value<float>() ?? 100f) / 100f,
                    // NPC 没有 `move_type` 字段,全是两条腿走路 —— 运行时的 `ground` 就是这个。
                    "步行", clips,
                    Lord: false, Root: AssetRoots.Npc,
                    VoiceBank: NpcClips.VoiceBanks.GetValueOrDefault(name)));
            }
            packs.Add(new Chain(0, name, forms[0].Id, forms, PackKind.Npc));
        }
        return packs;
    }

    /// 共享同一个 `anim_conf_id` 的其他 NPC 资产目录。与宠物那条同理
    /// (`AssetsSharingAnimConf`),只是路径的取法不同。
    public IEnumerable<string> NpcAssetsSharingAnimConf(int animConfId, string exclude)
    {
        foreach (var (_, model) in _model)
        {
            if (model?["anim_conf_id"]?.Value<int>() != animConfId) continue;
            var asset = NpcAssetOf(model);
            if (asset is null || asset == exclude) continue;
            yield return asset;
        }
    }
}
