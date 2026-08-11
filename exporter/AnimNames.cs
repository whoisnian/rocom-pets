// 逻辑动作名 ↔ AnimSequence 资产名的对应规则。
//
// 资产名带**场景类别前缀**:`World_Idle`(大世界)、`Common_Anger`(通用)、`Fight_Idle`(战斗)、
// `Ride_Idle`(被骑乘)、`ThrowBall_Idle`(抛球演出)…而配置表里的逻辑名只有 `Idle`。
// 所以比对时把前缀剥掉(`Normalize`)。
//
// **剥完会撞名**:同一个形态往往同时有 `World_Idle` 与 `Ride_Idle`,剥完都是 idle。
// 撞了必须按类别挑(`Rank`),不能先到先得 —— 先到先得取决于 pak 的文件枚举顺序,
// 而枚举顺序恰好把 `Ride_*` 排在 `World_*` 前面。踩过的坑:加尔/黑化加尔/雷鸣小子/
// 阿米亚特/波波拉的 Idle、Run、JumpFall 全取到了 `Ride_*` 那一版 —— 骑乘动作是**照着
// 骑手的挂点**摆的,整只宠物偏出半个身位(加尔 Bip001 偏 X −28cm / Z −49cm),
// 于是桌面上待机时整只歪在一边、一做别的动作又瞬移回中间。

namespace RocomPets.Export;

public static class AnimNames
{
    /// 类别前缀,**按优先级从高到低**。桌宠演的是「跟在身边的宠物」,
    /// 所以大世界(World)那一版最贴切;通用(Common)其次;
    /// 战斗/骑乘/演出那几档都是**摆在别的东西旁边**的动作,位置不以自己为原点,垫底。
    private static readonly string[] Categories =
        ["World", "Common", "Scene", "Battle", "Fight", "Ride"];

    /// "World_Idle" → "idle";"Common_Sleep_Loop" → "sleeploop";逻辑名 "SleepLoop" → "sleeploop"
    public static string Normalize(string name)
    {
        var parts = name.Split('_', StringSplitOptions.RemoveEmptyEntries).ToList();
        if (parts.Count > 1 && Categories.Contains(parts[0], StringComparer.OrdinalIgnoreCase))
            parts.RemoveAt(0);
        return string.Concat(parts).ToLowerInvariant();
    }

    /// 资产文件名 → **写进包里的动作名**(保留大小写,给人看)。
    /// `World_Crossarms_Gesture_2_End` → `CrossarmsGesture2End`;
    /// `Fight_Start_Show` → `FightStartShow`。
    ///
    /// **只剥 `World_`**:大世界是「这个角色平时的样子」,那一档才是默认类别,剥掉最自然。
    /// 其余类别(`Fight_`/`Ride_`…)留着前缀 —— 一来 `Fight_Start`/`Fight_Lose` 剥完就成了
    /// 光秃秃的 `Start`/`Lose`,看不出是什么;二来剥完会和大世界的名字撞
    /// (`Fight_CallOut` 与 `World_CallOut`)。
    public static string LogicalOf(string fileName)
    {
        var parts = fileName.Split('_', StringSplitOptions.RemoveEmptyEntries).ToList();
        if (parts.Count > 1 && parts[0].Equals("World", StringComparison.OrdinalIgnoreCase))
            parts.RemoveAt(0);
        return string.Concat(parts);
    }

    /// 撞名时谁赢:数字小的赢。没有已知前缀的(`TakeOut`)与不认识的前缀(`Suits70_…`、
    /// `Bond305055_…`)垫底 —— 那些是联动/皮肤演出,更不该顶掉正经的大世界动作。
    ///
    /// 全库普查(`--probe-anim ALL`)下来,桌宠白名单里真会撞的只有 idle(252 个资产)、
    /// run(265)、jumpfall(141) 三个,每个都是 `Ride_*` 对 `World_*`;
    /// 剩下的撞名(flygliding/flyhover/swim*/movel/mover/hit2)都不在白名单里。
    public static int Rank(string name)
    {
        var cut = name.IndexOf('_');
        if (cut <= 0) return Categories.Length;
        var index = Array.FindIndex(Categories,
            c => c.Equals(name[..cut], StringComparison.OrdinalIgnoreCase));
        return index < 0 ? Categories.Length : index;
    }
}
