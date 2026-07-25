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
    List<ClipInfo> Clips);

/// 逻辑动作:名字取自 ANIM_ID_CONF,时长取自 ANIM_CONF(毫秒)。
public record ClipInfo(string Logical, int AnimId, int Ms);

/// 一条进化链。
public record Chain(int RootId, string Name, List<Form> Forms);

public class GameConfig
{
    private readonly JObject _petBase;
    private readonly JObject _model;
    private readonly JObject _anim;
    private readonly Dictionary<int, string> _animNames = new();

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
        foreach (var (key, value) in Rows(binDir, "ANIM_ID_CONF"))
        {
            var name = value?["anim_name"]?.Value<string>();
            if (name is not null && int.TryParse(key, out var id)) _animNames[id] = name;
        }
    }

    private static JObject Rows(string dir, string table)
    {
        var path = Path.Combine(dir, table + ".json");
        if (!File.Exists(path)) throw new FileNotFoundException($"缺配置表 {path}");
        var root = JObject.Parse(File.ReadAllText(path));
        return root["RocoDataRows"] as JObject
               ?? throw new InvalidDataException($"{table}.json 没有 RocoDataRows");
    }

    /// 从任意一个成员出发,拿到它所属的完整进化链(按 stage 升序)。
    public Chain ResolveChain(int petId)
    {
        // 先顺着 evolution_pet_id 往前找链首:链首 = 没有别人指向它的那个
        var incoming = new Dictionary<int, int>();
        foreach (var (key, row) in _petBase)
        {
            if (!int.TryParse(key, out var id) || !IsRealPet(id, row)) continue;
            foreach (var next in Evolutions(row))
                incoming.TryAdd(next, id);
        }

        var root = petId;
        var guard = 0;
        while (incoming.TryGetValue(root, out var prev) && guard++ < 16) root = prev;

        var forms = new List<Form>();
        var cursor = root;
        guard = 0;
        while (guard++ < 16)
        {
            var row = _petBase[cursor.ToString()];
            if (row is null) break;
            forms.Add(BuildForm(cursor, row));
            var next = Evolutions(row).FirstOrDefault();
            if (next == 0) break;
            cursor = next;
        }

        return new Chain(root, forms[0].Name, forms);
    }

    /// 过滤测试行与重复行:PETBASE_CONF 里混着「测试喵喵1」和 32000001 这类影子行。
    private static bool IsRealPet(int id, JToken row)
    {
        if (id is < 1000 or > 99999) return false; // 正式宠物是 4 位/5 位 id
        var name = row["name"]?.Value<string>() ?? "";
        if (name.Length == 0 || name.Contains("测试") || name.Contains("Test")) return false;
        return row["legal_petbase"]?.Value<int>() != 0;
    }

    private static IEnumerable<int> Evolutions(JToken row) =>
        row["evolution_pet_id"] is JArray arr ? arr.Select(t => t.Value<int>()) : [];

    private Form BuildForm(int id, JToken row)
    {
        var name = row["name"]?.Value<string>() ?? id.ToString();
        var stage = row["stage"]?.Value<int>() ?? 0;
        var modelConfId = row["model_conf"]?.Value<int>()
                          ?? throw new InvalidDataException($"宠物 {id}({name}) 没有 model_conf");
        var model = _model[modelConfId.ToString()]
                    ?? throw new InvalidDataException($"MODEL_CONF 缺 {modelConfId}");

        // path 形如 Blueprint'/Game/ArtRes/BP/Pets/Gra_MiaoMiao1_001/BP_Gra_MiaoMiao1_001.…_C'
        var path = model["path"]?.Value<string>() ?? "";
        var asset = ExtractAsset(path)
                    ?? throw new InvalidDataException($"从 MODEL_CONF {modelConfId} 的 path 认不出资产名: {path}");
        // anim_conf_id 可以不等于 model_conf 的 id(如珀尔鼬 model 14765 / anim 14641)
        var animConfId = model["anim_conf_id"]?.Value<int>() ?? modelConfId;
        var scale = (model["model_scale"]?.Value<float>() ?? 100f) / 100f;

        return new Form(
            id, name, stage, asset, modelConfId, animConfId, scale,
            row["move_type"]?.Value<string>() ?? "",
            ClipsOf(animConfId));
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
