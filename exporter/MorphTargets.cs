// 形变目标(blend shape):**一部分宠物的脸不是贴图,是几何**。
//
// 全库 1000 个宠物资产里 37 个带形变目标(214 个 target),其中 22 个挂的是同一套
// 脸的 blendshape:`Xi 喜 / Jing 惊 / Nu 怒 / Shui 睡 / Ai 哀 / Shou 收 / Yun 晕`
// (少数还有 `Zheng 正`,以及按网格名加前缀的变体 `<资产>_Mh_Xi`)。权重由**同名的动画
// 曲线**逐帧驱动 —— 和眼睛那条 `EC_Eye` 是一个机制,只是驱动的是几何而不是 UV。
//
// 典型的一只:里奥一阶(Win_LiAo1_001)。它**没有嘴的图集槽**(游戏里只有 By/By_Ol/Es
// 三个材质),嘴是本体网格上的形变:七个 target 各影响本体那一段的 184~234 个顶点、
// 最大位移 2.1~4.4cm,影响点包围盒 X[-4.2,4.2] Y[13.6,16.4] Z[26.7,33.6] ——
// 67.7cm 高的模型上正好是头前面口鼻那一块。不导它,里奥的嘴就永远是贴图上画死的那条线。
//
// ## 为什么自己注入,而不是打开 CUE4Parse 的 `exportMorphTargets`
//
// 上游那条路有三处过不去(`CUE4Parse-Conversion/Writers/Gltf/Gltf.cs`):
//
// 1. `UseMorphTarget(j)` 用的是**原始下标**,跳过的空 target 会在中间留洞 ⇒ 写出
//    **没有 bufferView 的 accessor**。按 glTF 规范那等价于全零、是合法的,但 Rust 的
//    `gltf` crate 判定「Missing data」直接拒绝加载 —— 这正是原来那条注释里
//    「32 个带 morph 的形态全部加载失败」的原因。
// 2. `FindVert` 是对**整份顶点表**的线性扫描,每个 delta 扫一遍(O(n²));而且只比位置、
//    只取第一个命中 —— UV 接缝处一个位置对应多个顶点,那些复制点不会跟着动,会撕开。
// 3. `targetNames` 那串 extras 的逗号与下标都和跳过逻辑对不上,名字会错位。
//
// 这里改成:按位置建一张 `位置 → 该位置上所有顶点` 的表(**所有复制点一起动**),
// 逐图元写成标准的 morph target accessor。位置的算法与上游逐位一致
// (`SwapYZ(pos × 0.01)`),所以能精确对上;对不上的 delta 会计数并报进 warnings。

using System.Numerics;
using CUE4Parse.UE4.Assets.Exports.Animation;
using CUE4Parse.UE4.Assets.Exports.SkeletalMesh;
using CUE4Parse_Conversion.Dto;
using SharpGLTF.Schema2;

namespace RocomPets.Export;

/// 这个形态导出了哪些形变目标。`Names` 的顺序**就是 glb 里 morph target 的顺序**,
/// 权重通道按这个顺序排。空 = 这只没有形变目标。
public record MorphInfo(string[] Names);

public static class MorphTargets
{
    private const float CmToM = 0.01f;

    /// 最多导几个。这套脸的 blendshape 最多 8 个(七情 + 正),运行时的权重也按 8 个排。
    /// 超出的丢掉并报一条 —— 与其让运行时的 uniform 溢出,不如少一个形变目标。
    private const int MaxTargets = 8;

    /// 把 LOD0 的形变目标写进已经解析好的 glb,返回它们的名字(即 morph target 顺序)。
    public static MorphInfo Inject(
        ModelRoot model,
        SkeletalMeshDto dto,
        USkeletalMesh mesh,
        int lodIndex,
        List<string> warnings)
    {
        var targets = mesh.MorphTargets;
        if (targets is not { Length: > 0 }) return new MorphInfo([]);
        if (lodIndex >= dto.LODs.Count) return new MorphInfo([]);
        var lod = dto.LODs[lodIndex];
        var sourceLod = (int)lod.SourceLodIndex;

        // 位置 → 这个位置上的所有 (图元, 顶点下标)。**一个位置可以对应多个顶点**:
        // UV/法线接缝会把同一个点拆开,拆出来的复制点必须一起动,否则接缝会撕开。
        var byPosition = new Dictionary<Vector3, List<(int Prim, int Vertex)>>();
        var prims = model.LogicalMeshes.SelectMany(m => m.Primitives).ToList();
        var positions = new List<IList<Vector3>>();
        for (var p = 0; p < prims.Count; p++)
        {
            var accessor = prims[p].GetVertexAccessor("POSITION");
            if (accessor is null) { positions.Add([]); continue; }
            var array = accessor.AsVector3Array();
            positions.Add(array);
            for (var v = 0; v < array.Count; v++)
            {
                if (!byPosition.TryGetValue(array[v], out var list))
                    byPosition[array[v]] = list = [];
                list.Add((p, v));
            }
        }

        var names = new List<string>();
        var deltas = new List<Vector3[][]>();   // [target][图元][顶点]
        var missed = 0;
        var total = 0;
        foreach (var reference in targets)
        {
            if (names.Count >= MaxTargets)
            {
                warnings.Add($"形变目标多于 {MaxTargets} 个,只导前 {MaxTargets} 个");
                break;
            }
            var target = reference.Load<UMorphTarget>();
            var lodModel = target?.MorphLODModels is { Length: > 0 } models && sourceLod < models.Length
                ? models[sourceLod]
                : null;
            if (target is null || lodModel is null || lodModel.Vertices.Length == 0) continue;

            var perPrim = prims.Select((_, p) => new Vector3[positions[p].Count]).ToArray();
            var hit = false;
            foreach (var delta in lodModel.Vertices)
            {
                if (delta.SourceIdx >= lod.Vertices.Length) continue;
                total++;
                // 与上游写 glb 顶点时**逐位相同**的算法,所以能当键精确查表
                var key = SwapYz(lod.Vertices[delta.SourceIdx].Position * CmToM);
                if (!byPosition.TryGetValue(key, out var hits)) { missed++; continue; }
                var offset = SwapYz(delta.PositionDelta * CmToM);
                foreach (var (prim, vertex) in hits) perPrim[prim][vertex] = offset;
                hit = true;
            }
            if (!hit) continue;
            names.Add(target.Name);
            deltas.Add(perPrim);
        }
        if (names.Count == 0) return new MorphInfo([]);
        if (missed > 0)
            warnings.Add($"形变目标有 {missed}/{total} 个顶点在 glb 里找不到对应位置,那几点不会动");

        // glTF 要求**同一个 mesh 的所有图元 morph target 个数相同**,所以没被这个 target
        // 碰到的图元也要写一份(全零)。零填充的那份很小(眼睛那几片才一百来个顶点)。
        for (var t = 0; t < names.Count; t++)
            for (var p = 0; p < prims.Count; p++)
            {
                if (positions[p].Count == 0) continue;
                prims[p].SetMorphTargetAccessors(t, new Dictionary<string, Accessor>
                {
                    ["POSITION"] = PositionAccessor(model, $"{names[t]}#{p}", deltas[t][p]),
                });
            }
        return new MorphInfo([.. names]);
    }

    /// 一段动画里各形变目标的权重曲线 → glTF 的 `weights` 通道。
    ///
    /// 曲线名就是形变目标名(大小写不较真)。一段动画通常只驱动其中两三个,没被驱动的
    /// 恒为 0 —— 但 glTF 的权重通道是**整组一起给**的,所以每个关键时刻都要写满一组。
    /// 相邻两组相同就跳过(原始曲线是逐帧的,成片的 0 占了大半)。
    public static void WriteWeights(
        Animation animation,
        Node node,
        UAnimSequence sequence,
        MorphInfo morph,
        float seconds)
    {
        if (morph.Names.Length == 0) return;
        var curves = sequence.CompressedCurveData?.FloatCurves;
        if (curves is null) return;
        var picked = morph.Names
            .Select(n => curves.FirstOrDefault(c =>
                c.CurveName.Text.Equals(n, StringComparison.OrdinalIgnoreCase)))
            .ToArray();
        if (picked.All(c => c?.FloatCurve?.Keys is not { Length: > 0 })) return;

        var times = new SortedSet<float>();
        foreach (var curve in picked)
            foreach (var key in curve?.FloatCurve?.Keys ?? [])
                if (key.Time >= 0f && key.Time <= seconds + 1e-3f)
                    times.Add(key.Time);
        if (times.Count == 0) return;

        var keyframes = new Dictionary<float, float[]>();
        float[]? previous = null;
        foreach (var time in times)
        {
            var weights = picked
                .Select(c => Math.Clamp(c?.FloatCurve?.Eval(time) ?? 0f, 0f, 1f))
                .ToArray();
            // 逐帧曲线里成片的 0 没必要写;只留变化点(首尾两端各留一个)
            if (previous is not null && weights.Zip(previous).All(p => Math.Abs(p.First - p.Second) < 1e-4f))
                continue;
            keyframes[time] = weights;
            previous = weights;
        }
        if (keyframes.Count == 0) return;
        // 起点没有关键帧的话,glTF 采样器在 0 到第一帧之间取的是第一帧的值,那正是想要的;
        // 但补一帧 0 更省心 —— 权重曲线是从静止开始的。
        if (!keyframes.ContainsKey(0f))
            keyframes[0f] = keyframes[times.Min];
        animation.CreateMorphChannel(node, keyframes, morph.Names.Length);
    }

    private static Vector3 SwapYz(CUE4Parse.UE4.Objects.Core.Math.FVector v) =>
        new(v.X, v.Z, v.Y);

    private static Accessor PositionAccessor(ModelRoot model, string name, Vector3[] values)
    {
        var view = model.CreateBufferView(values.Length * 12, 0, BufferMode.ARRAY_BUFFER);
        var accessor = model.CreateAccessor(name);
        accessor.SetData(view, 0, values.Length, DimensionType.VEC3, EncodingType.FLOAT, false);
        var array = accessor.AsVector3Array();
        for (var i = 0; i < values.Length; i++) array[i] = values[i];
        accessor.UpdateBounds();
        return accessor;
    }
}
