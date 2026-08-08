// 读游戏配置表:进化链归组、形态的资产目录、每个形态的逻辑动作清单。
//
// 配置表是 RocoBinData(.bytes),CUE4Parse 只提供 FRocoBinData 解码器而**不解 .non schema**,
// 全仓唯一的 .non 实现在 rocom-capture 的 scripts/bin2json.py 里。所以这里直接读它产出的
// JSON(解包根 = --parsed / $ROCOM_PARSED / ~/Downloads/rocom/parsed),不重复实现一遍。
// 见 docs/design.md §8。

using Newtonsoft.Json.Linq;

namespace RocomPets.Export;

/// 一个形态(进化链上的一环)。
public record Form(
    int Id,
    string Name,
    int Stage,
    string Asset,
    int ModelConfId,
    int AnimConfId,
    float ModelScale,
    string MoveType,
    List<ClipInfo> Clips,
    /// 王者形态(资产名写作 `…Bo_001`)。排序时一律垫底 —— 它们的 `stage` 与普通三阶撞号。
    bool Lord = false);

/// 逻辑动作:名字取自 ANIM_ID_CONF,时长取自 ANIM_CONF(毫秒)。
public record ClipInfo(string Logical, int AnimId, int Ms);

/// 一个包。**按图鉴号归并之后的**一条或几条进化链(海盔虫的「本来的样子」与「磨损的样子」
/// 是两条链、一个包),包名写成 `<图鉴号>-<链首名>`。没有图鉴号的记 0(`000-…`)。
///
/// 归并规则与全量清单见 docs/petindex.md,那份清单由 tools/petindex.py 生成 ——
/// **这里是它的 C# 实现,两边的结果必须一致**(改了任一边都要跑 `tools/petindex.py --check`)。
public record Chain(int Book, string Name, int RootId, List<Form> Forms);

public class GameConfig
{
    private readonly JObject _petBase;
    private readonly JObject _model;
    private readonly JObject _anim;
    /// 进化链的权威表:`evolution_chain` 是普通形态,`lordevo_chain` 是王者形态。
    /// **不顺着 `PETBASE_CONF.evolution_pet_id` 爬**:那字段在分支链上是一串
    /// (矿晶虫指着六个),只取第一个会把另外五种外观整条丢掉,王者形态也不在那条路上。
    private readonly JObject _evolution;
    /// 宠物 id → 外观标签(「本来的样子」)。来自 MEGAMAP_CONF:`genre` 写成
    /// 「刺盔虫_本来的样子」,`icon` 就是 petbase_id。
    private readonly Dictionary<int, string> _skins = new();
    private readonly Dictionary<int, string> _animNames = new();
    /// 宠物 id → 拼音。叫声的 SoundBank 按拼音命名(`Pet_Vo_MiaoMiao.bnk`),
    /// **每个形态各有一个**(点点 DianDian / 珀尔鼬 BoErYou),不是按物种。
    private readonly Dictionary<int, string> _pinyin = new();

    public GameConfig(string parsedRoot)
    {
        var binDir = Path.Combine(parsedRoot,
            "NRC", "Content", "ScriptC", "Data", "Bin", "BinDataCompressed");
        if (!Directory.Exists(binDir))
            throw new DirectoryNotFoundException(
                $"找不到配置目录 {binDir}\n" +
                "先在 rocom-capture 里跑 scripts/unpack.sh(会自动把 .bytes 解成 .json),再用 --parsed 指过来");

        _petBase = Rows(binDir, "PETBASE_CONF");
        _model = Rows(binDir, "MODEL_CONF");
        _anim = Rows(binDir, "ANIM_CONF");
        _evolution = Rows(binDir, "PET_EVOLUTION_CONF");
        foreach (var (_, row) in Rows(binDir, "MEGAMAP_CONF"))
        {
            var genre = row?["genre"]?.Value<string>();
            var icon = row?["icon"]?.Value<string>();
            var cut = genre?.IndexOf('_') ?? -1;
            if (genre is null || cut < 0 || !int.TryParse(icon, out var pid)) continue;
            // 头一条说了算:同一只宠物在大地图上可能出现在好几行
            _skins.TryAdd(pid, genre[(cut + 1)..]);
        }
        foreach (var (key, value) in Rows(binDir, "ANIM_ID_CONF"))
        {
            var name = value?["anim_name"]?.Value<string>();
            if (name is not null && int.TryParse(key, out var id)) _animNames[id] = name;
        }
        foreach (var (key, value) in Rows(binDir, "PET_NAME_MAP_CONF"))
        {
            var name = value?["name"]?.Value<string>();
            if (!string.IsNullOrEmpty(name) && int.TryParse(key, out var id)) _pinyin[id] = name;
        }
    }

    /// 这个形态的拼音;表里没有就返回 null(实测 667 条覆盖不全)。
    public string? PinyinOf(int petId) => _pinyin.GetValueOrDefault(petId);

    private static JObject Rows(string dir, string table)
    {
        var path = Path.Combine(dir, table + ".json");
        if (!File.Exists(path)) throw new FileNotFoundException($"缺配置表 {path}");
        var root = JObject.Parse(File.ReadAllText(path));
        return root["RocoDataRows"] as JObject
               ?? throw new InvalidDataException($"{table}.json 没有 RocoDataRows");
    }

    /// 资产名的构成:`元素_物种拼音 + 阶段 + 可选 Ar + _变体号`,阶段位是 `Bo` 就是王者形态。
    private static readonly System.Text.RegularExpressions.Regex AssetPattern =
        new(@"^([A-Za-z]+_[A-Za-z]+?)(\d+|Bo)(Ar)?_(\d+)$");

    /// `Gra_RuoYeXi1_001` → `Gra_RuoYeXi`。一条链共用的那截。
    private static string AssetStem(string asset)
    {
        var m = AssetPattern.Match(asset);
        return m.Success ? m.Groups[1].Value : asset;
    }

    /// 资产名里的阶段位;王者形态(`…Bo_001`)与认不出的返回 null。
    ///
    /// **排序优先用它,而不是配置里的 `stage`**:那个字段是相对本条链的,半路起头的链会从 1
    /// 数起(路路尼那条链只有它一个,写着 stage 1,可它明明是二阶)。资产名里的数字是绝对的。
    private static int? AssetStage(string asset)
    {
        var m = AssetPattern.Match(asset);
        if (!m.Success || m.Groups[2].Value == "Bo") return null;
        return int.Parse(m.Groups[2].Value);
    }

    private static bool IsLordAsset(string asset)
    {
        var m = AssetPattern.Match(asset);
        return m.Success && m.Groups[2].Value == "Bo";
    }

    /// 这个形态用哪份模型资产。**认不出返回空串** —— 那种行不该出包
    /// (`MODEL_CONF.path` 里没有 `/Pets/…` 的只有四行:幸运惊喜盒 ×3、随机精灵,是界面占位)。
    public string AssetOf(int petId)
    {
        var modelId = _petBase[petId.ToString()]?["model_conf"]?.Value<int>();
        if (modelId is null) return "";
        var path = _model[modelId.Value.ToString()]?["path"]?.Value<string>() ?? "";
        return ExtractAsset(path) ?? "";
    }

    /// 链名里的括号:「矿晶虫进化链(西瓜碧玺的样子)」→「西瓜碧玺的样子」。
    /// 「…分支」不算外观(果冻那三条链说的是分支去向、不是长相)。
    private static string? ChainLabel(string chainName)
    {
        var left = chainName.IndexOf('（');
        var right = chainName.LastIndexOf('）');
        if (left < 0 || right <= left) return null;
        var label = chainName[(left + 1)..right];
        return label.Length == 0 || label.EndsWith("分支") ? null : label;
    }

    /// 建包中间态:形态按**资产**去重(同一个王者形态在表里有三四行,`model_conf` 各不相同,
    /// 指的却是同一份模型),外观标签跟着资产走。
    private sealed class PackBuilder
    {
        public int Book;
        public string Name = "";
        public int RootId;
        public readonly Dictionary<string, Form> ByAsset = new();
        public readonly Dictionary<string, string?> Skins = new();
        public readonly Dictionary<string, int> Order = new();
    }

    /// 按图鉴号归并出全部包。与 tools/petindex.py 同算法,见 [`Chain`] 的说明。
    public List<Chain> Packs()
    {
        // 有图鉴号的先来,它们说了算 —— 没图鉴号的那批里有一堆**借着别人的模型占位**
        // (`Com_YaJiJi1_001` 是鸭吉吉的模型,却被 51 个还没做模型的宠物顶着用)。
        // 不先把有主的资产占掉,那批会连成一个二十几形态的怪包。
        var chains = _evolution
            .Properties()
            .Select(p => (Id: int.Parse(p.Name), Row: p.Value))
            .Where(c => c.Row["evolution_chain"] is JArray { Count: > 0 })
            // 链名自己写着「废弃」的不要(两条:野外首领梦想三三/雪影娃娃);
            // 它们仨成员指的是同一份 BOSS 模型,收进来白白多两个只有一份模型的包
            .Where(c => !new[] { "废弃", "占位", "测试" }
                .Any(mark => (c.Row["name"]?.Value<string>() ?? "").Contains(mark)))
            .OrderBy(c => BookOfChain(c.Row) is null)
            .ThenBy(c => c.Id)
            .ToList();
        var bookedAssets = chains
            .Where(c => BookOfChain(c.Row) is not null)
            .SelectMany(c => Members(c.Row).Select(m => AssetOf(m.Id)))
            .Where(a => a.Length > 0)
            .ToHashSet();

        var packs = new Dictionary<(int Book, string Key), PackBuilder>();
        var taken = new HashSet<string>();
        foreach (var (chainId, row) in chains)
        {
            var members = Members(row).ToList();
            var root = members[0];
            var book = BookOfChain(row);
            // 没图鉴号的用**链首资产的词干**分包:同根的几条分支链(菌宝那四条)并成一个,
            // 换外观的几条(雪毛角羚牛那三条)也并成一个。**链首顶着别人的模型时不能用词干** ——
            // 那样五条毫不相干的链会因为都指着 `Com_YaJiJi1_001` 而并到一起,
            // 这时退回按链首 id 各归各的。
            var key = "";
            if (book is null)
            {
                var rootAsset = AssetOf(root.Id);
                key = rootAsset.Length > 0 && !bookedAssets.Contains(rootAsset)
                    ? AssetStem(rootAsset)
                    : $"pet{root.Id}";
            }
            if (!packs.TryGetValue((book ?? 0, key), out var pack))
                packs[(book ?? 0, key)] = pack = new PackBuilder
                {
                    Book = book ?? 0, Name = root.Name, RootId = root.Id,
                };

            foreach (var (id, name, lord) in members)
            {
                var asset = AssetOf(id);
                if (asset.Length == 0) continue;
                // 没图鉴号的不许抢已经有主的资产;有图鉴号的照收
                // (同一份模型在几个包里各占一格是正常的,千棘海针那种)
                if (book is null && !taken.Add(asset)) continue;
                if (book is not null) taken.Add(asset);
                if (!pack.ByAsset.ContainsKey(asset))
                {
                    pack.ByAsset[asset] = BuildForm(id, _petBase[id.ToString()]!, name, asset, lord);
                    pack.Order[asset] = pack.Order.Count;
                }
                pack.Skins.TryAdd(asset, _skins.GetValueOrDefault(id) ?? ChainLabel(row["name"]?.Value<string>() ?? ""));
            }
        }

        var built = packs.Values.Where(p => p.ByAsset.Count > 0).ToList();
        AdoptUnbooked(built, taken);
        // **排序放在 Finish 之后**:`000` 那一档里链首被挡掉的包要等 Finish 才定下名字
        // (超级鳄椰、长翎将军、具足秘剑那几个),先排就是拿旧名字排。
        // 与 tools/petindex.py 同序,两边好逐行对账
        return built
            .Select(Finish)
            .OrderBy(p => p.Book)
            .ThenBy(p => p.Name, StringComparer.Ordinal)
            .ToList();
    }

    /// 没图鉴号、又不在任何进化链里的那些形态。
    ///
    /// 词干和某个已有的包对得上就**并进那个包** —— 那是它缺的一环(赤毛鸡仔那条链缺三阶
    /// 伊丽莎白、小鼠獭缺王者卷发巨獭);丢进 000 会把一条链劈成两个包,和归并的初衷反着来。
    /// 对不上的自成一包。
    private void AdoptUnbooked(List<PackBuilder> packs, HashSet<string> taken)
    {
        var byStem = new Dictionary<string, PackBuilder>();
        foreach (var pack in packs)
            foreach (var asset in pack.ByAsset.Keys)
                byStem.TryAdd(AssetStem(asset), pack);

        foreach (var (key, row) in _petBase.Properties().Select(p => (p.Name, p.Value)).OrderBy(p => p.Name))
        {
            if (!int.TryParse(key, out var id) || !IsExportable(id, row)) continue;
            if (row["pictorial_book_id"] is not null && row["pictorial_book_id"]!.Type != JTokenType.Null) continue;
            var asset = AssetOf(id);
            if (asset.Length == 0 || !taken.Add(asset)) continue;
            var name = row["name"]!.Value<string>()!;
            var stem = AssetStem(asset);
            if (!byStem.TryGetValue(stem, out var pack))
            {
                packs.Add(pack = new PackBuilder { Book = 0, Name = name, RootId = id });
                byStem[stem] = pack;
            }
            var form = BuildForm(id, row, name, asset, IsLordAsset(asset));
            // 包名跟着最靠前的那一环走:路路尼(二阶)收进一阶的 路路 之后该改叫 路路。
            // **要在挂进去之前比**,不然拿自己和自己比,永远不会更靠前
            var earlier = pack.ByAsset.Values.Where(f => !f.Lord).Select(f => f.Stage).DefaultIfEmpty(99).Min();
            if (!form.Lord && form.Stage < earlier) pack.Name = name;
            pack.ByAsset[asset] = form;
            pack.Order[asset] = pack.Order.Count;
            pack.Skins.TryAdd(asset, _skins.GetValueOrDefault(id));
        }
    }

    /// 收尾:排序、包名归位、给同名的多种外观加上区分后缀。
    private static Chain Finish(PackBuilder pack)
    {
        var forms = pack.ByAsset
            .OrderBy(kv => kv.Value.Lord)
            .ThenBy(kv => kv.Value.Stage)
            .ThenBy(kv => pack.Order[kv.Key])
            .Select(kv => kv.Value)
            .ToList();

        // **包名要在加外观后缀之前定下来**:不然「喵喵」这种链首自己有多种外观的,
        // 包名会变成 `002-喵喵(本来的样子)`
        var name = forms.Any(f => f.Name == pack.Name) ? pack.Name : forms[0].Name;
        var rootId = forms.FirstOrDefault(f => f.Name == name)?.Id ?? pack.RootId;

        // **同名的多种外观要能分辨**:一个包里六个都叫「晶石蜗」的形态,运行时的形态菜单
        // 就成了六条一模一样的项。带上外观标签,没标签的退用资产名。
        // **按改名前的名字统计**:边数边改的话,头一个改完之后剩下那个就成了「独苗」,
        // 于是 `刺盔虫(本来的样子)` 有后缀、`刺盔虫(磨损的样子)` 反而没有(实测踩过)
        var dupes = forms.GroupBy(f => f.Name).Where(g => g.Count() > 1).Select(g => g.Key).ToHashSet();
        foreach (var name0 in dupes)
        {
            // 基础那一版常常没登记名字(游戏只给「特殊的那一版」起名),补「本来的样子」——
            // 这四个字正是游戏自己在肯登记时用的说法。**只补孤零零一个**没名字的:
            // 两个都没名字就不是「基础版 + 特殊版」那种结构,瞎补会把话说错,那时退用资产名。
            var group = forms.Where(f => f.Name == name0).Select(f => f.Asset).ToList();
            var blank = group.Where(a => pack.Skins.GetValueOrDefault(a) is null).ToList();
            if (blank.Count == 1) pack.Skins[blank[0]] = "本来的样子";
        }
        for (var i = 0; i < forms.Count; i++)
        {
            if (!dupes.Contains(forms[i].Name)) continue;
            var label = pack.Skins.GetValueOrDefault(forms[i].Asset) ?? forms[i].Asset;
            forms[i] = forms[i] with { Name = $"{forms[i].Name}({label})" };
        }
        return new Chain(pack.Book, name, rootId, forms);
    }

    private IEnumerable<(int Id, string Name, bool Lord)> Members(JToken chain)
    {
        foreach (var m in chain["evolution_chain"] as JArray ?? [])
            yield return (m["petbase_id"]!.Value<int>(), m["pet_name"]?.Value<string>() ?? "", false);
        foreach (var m in chain["lordevo_chain"] as JArray ?? [])
            yield return (m["lord_petbase_id"]!.Value<int>(), m["lord_pet_name"]?.Value<string>() ?? "", true);
    }

    private int? BookOfChain(JToken chain)
    {
        var root = (chain["evolution_chain"] as JArray)?.FirstOrDefault()?["petbase_id"]?.Value<int>();
        if (root is null) return null;
        return _petBase[root.Value.ToString()]?["pictorial_book_id"]?.Value<int?>();
    }

    /// 这一行值不值得出包。比原来的 `IsRealPet` 多挡两类:名字带「占位」的
    /// (数据自己这么写的)、以及 `首领-xxx`(BOSS 行,和本体同一份模型)。
    private static bool IsExportable(int id, JToken row)
    {
        if (id is < 1000 or > 99999) return false;
        var name = row["name"]?.Value<string>() ?? "";
        if (name.Length == 0 || name.Contains("测试") || name.Contains("Test")) return false;
        if (name.Contains("占位") || name.StartsWith("首领")) return false;
        return row["legal_petbase"]?.Value<int>() != 0;
    }

    /// 某个宠物 id 属于哪个包(`--species` 用)。找不到返回 null。
    public Chain? PackFor(int petId)
    {
        var asset = AssetOf(petId);
        return Packs().FirstOrDefault(p =>
            p.Forms.Any(f => f.Id == petId || (asset.Length > 0 && f.Asset == asset)));
    }

    /// 共享同一个 `anim_conf_id` 的其他资产目录名。
    /// 有些形态自己没有 Animation/ 目录,动画挂在同组的另一个资产下(见 Program.cs 的用法)。
    public IEnumerable<string> AssetsSharingAnimConf(int animConfId, string exclude)
    {
        foreach (var (_, model) in _model)
        {
            if (model?["anim_conf_id"]?.Value<int>() != animConfId) continue;
            var asset = ExtractAsset(model?["path"]?.Value<string>() ?? "");
            if (asset is null || asset == exclude) continue;
            yield return asset;
        }
    }

    private Form BuildForm(int id, JToken row, string name, string asset, bool lord)
    {
        if (name.Length == 0) name = row["name"]?.Value<string>() ?? id.ToString();
        var modelConfId = row["model_conf"]?.Value<int>()
                          ?? throw new InvalidDataException($"宠物 {id}({name}) 没有 model_conf");
        var model = _model[modelConfId.ToString()]
                    ?? throw new InvalidDataException($"MODEL_CONF 缺 {modelConfId}");
        // anim_conf_id 可以不等于 model_conf 的 id(如珀尔鼬 model 14765 / anim 14641)
        var animConfId = model["anim_conf_id"]?.Value<int>() ?? modelConfId;
        var scale = (model["model_scale"]?.Value<float>() ?? 100f) / 100f;
        // 王者形态不套资产阶段位(它们的资产名是 `…Bo_001`,没有数字),
        // 退回配置里的 stage,再靠 `Lord` 一律垫底
        var stage = (lord ? null : AssetStage(asset)) ?? row["stage"]?.Value<int>() ?? 0;

        return new Form(
            id, name, stage, asset, modelConfId, animConfId, scale,
            row["move_type"]?.Value<string>() ?? "",
            ClipsOf(animConfId), lord);
    }

    private static string? ExtractAsset(string path)
    {
        const string marker = "/Pets/";
        var i = path.IndexOf(marker, StringComparison.Ordinal);
        if (i < 0) return null;
        var rest = path[(i + marker.Length)..];
        var j = rest.IndexOf('/');
        return j < 0 ? null : rest[..j];
    }

    /// ANIM_CONF[anim_conf_id].anim_info → 逻辑动作清单(名字优先取 ANIM_ID_CONF)。
    private List<ClipInfo> ClipsOf(int animConfId)
    {
        var clips = new List<ClipInfo>();
        if (_anim[animConfId.ToString()]?["anim_info"] is not JArray infos) return clips;
        foreach (var info in infos)
        {
            var animId = info["anim_id"]?.Value<int>() ?? 0;
            var name = animId != 0 && _animNames.TryGetValue(animId, out var n)
                ? n
                : info["anim_name"]?.Value<string>();
            if (string.IsNullOrEmpty(name)) continue;
            clips.Add(new ClipInfo(name, animId, info["anim_len"]?.Value<int>() ?? 0));
        }
        return clips;
    }
}
