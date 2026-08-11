// 把一个形态导成带动画的单个 glb。
//
// 分两步:① CUE4Parse 的 MeshExporter 出「网格 + 蒙皮 + 骨架」的 glb(它顺手做了
// UE → glTF 的轴与单位转换);② 用 SharpGLTF 打开这份 glb,把 AnimSequence 的关键帧
// 按**同一套转换**加成 glTF animation。
//
// 为什么不走 psa:CUE4Parse 的动画导出只出 psa/ueanim,而 psa 保持 UE 坐标系,
// 与 glb 已转过的坐标系不一致,合并时还得自己补变换;直接在 glb 上加通道少一道坑。
//
// **上游 bug(必须知道,否则动画一定错)**:UE 是 Z-up 左手系,glTF 是 Y-up 右手系,
// CUE4Parse 的转换是交换 Y/Z——这是个**反射**(det = -1)。位置按 (x, z, y) 交换是对的,
// 但旋转不能照抄:反射共轭 M R M 会把旋转方向反过来,正确的四元数是 (-x, -z, -y, w),
// 而上游写的是 (x, z, y, w),刚好是它的共轭(即逆旋转)。
//
// 为什么上游没暴露这个问题:绑定姿势下蒙皮矩阵 = world × inverse(world) = I,骨骼旋转
// 错了也照样渲染正确,而 CUE4Parse 根本不导 glTF 动画,于是永远看不出来。我们一加动画就露馅
// (实测:整只宠物的耳朵尾巴被拉成面条)。
//
// 所以这里除了写动画,还要:① 用正确映射改写所有骨骼节点的局部旋转;
// ② 按改过的绑定姿势重算 inverseBindMatrices(否则 world_anim × IBM_bind 依然是乱的)。
// 位置/顶点/UV 沿用上游(本来就对):位置 = SwapYZ(v) * 0.01(cm → m)。

using System.Numerics;
using CUE4Parse.UE4.Assets.Exports.Animation;
using CUE4Parse.UE4.Assets.Exports.SkeletalMesh;
using CUE4Parse.UE4.Objects.Core.Math;
using CUE4Parse_Conversion.Animations;
using CUE4Parse_Conversion.Dto;
using CUE4Parse_Conversion.Formats.Meshes;
using CUE4Parse_Conversion.Options;
using CUE4Parse_Conversion.Writers.ActorX.Structs.Animations;
using SharpGLTF.Schema2;
using SharpGLTF.Transforms;

namespace RocomPets.Export;

/// 一段已写进 glb 的动画。
public record ClipResult(string Logical, string Clip, float Seconds, int Frames, float RootMotionCm);

public static class GlbBuilder
{
    /// UE 单位是厘米,glTF 用米。
    private const float CmToM = 0.01f;

    /// 位移小于这个值(厘米)就认为动画是原地循环,位移由程序推进。
    private const float InPlaceThresholdCm = 1.0f;

    public static Vector3 SwapYz(FVector v) => new(v.X * CmToM, v.Z * CmToM, v.Y * CmToM);

    /// UE 四元数 → glTF:Y/Z 交换是反射,轴要跟着换且旋转方向取反,故取负而非照抄。
    public static Quaternion SwapYz(FQuat q) => new(-q.X, -q.Z, -q.Y, q.W);

    /// 导出网格 glb,再把 `clips`(逻辑名 → AnimSequence)加成动画通道。
    /// 眼睛族那套逐眼 UV 变换的一份参数(见 `Materials.MaterialInfo.EyeUvLeft`)。
    /// `Left`/`Right` 都是 `[缩放H, 缩放V, 平移H, 平移V]`。
    public readonly record struct EyeUv(float[] Left, float[] Right, float OffsetScale);

    public static (byte[] Glb, List<ClipResult> Written, List<string> Warnings) Build(
        USkeletalMesh mesh,
        IReadOnlyList<(string Logical, string Clip, UAnimSequence Sequence)> clips,
        int lodIndex,
        bool pullBackParkedBones = false,
        IReadOnlyDictionary<string, EyeUv>? eyeUv = null)
    {
        var warnings = new List<string>();
        // **不走 `SkeletalMeshExporter`**。上游 2026-08 那次重构(`Delete duplicated classes`)
        // 把导出器改成了「`ExportSession` + 异步写盘」的形状:`BuildExportFiles` 是 protected,
        // 拿字节只能先落盘再读回来。而我们要的就是内存里那份 glb —— 转换格式本身是公开的
        // (`GltfMeshFormat.BuildSkeletalMesh` 返回 `ExportFile.Data`),直接调它即可,
        // 少一次落盘、也不必给每个形态造一个 session。
        //
        // `EMeshQuality` 取代了旧的 `ELodFormat`:`Highest` = 只要第一级(旧 `FirstLod`)、
        // `All` = 全部(旧 `AllLods`),语义一一对应。
        var quality = lodIndex == 0 ? EMeshQuality.Highest : EMeshQuality.All;
        var options = new ExportOptions(
            meshFormat: EMeshFormat.Gltf2,
            meshQuality: quality,
            exportMaterials: false, // 贴图另走命名约定,见 Textures.cs
            // 不导 morph target(表情 blend shape)。我们只播骨骼动画,从不驱动它们
            // (glb 里既没有 morph 通道也没有 mesh.weights),纯属死重;
            // 更要命的是 CUE4Parse 会把空的 morph 写成**没有 bufferView 的 accessor**——
            // 按 glTF 规范那等价于「全零」是合法的,但 Rust 的 gltf crate 判定
            // 「accessors[N].bufferView: Missing data」直接拒绝加载。
            // 实测全量 826 个形态里 32 个带 morph target,这 32 个**全部**加载失败。
            // 将来若真要做表情,除了打开这个开关还得把 AnimSequence 的曲线转成 morph 通道。
            exportMorphTargets: false);

        using var dto = new SkeletalMeshDto(mesh, quality);
        var files = new GltfMeshFormat().BuildSkeletalMesh(mesh.Name, options, dto);
        if (files.Count == 0)
            throw new InvalidOperationException($"{mesh.Name}: glTF 导出没产出 LOD");
        var lod = lodIndex < files.Count ? files[lodIndex] : files[0];
        if (lodIndex >= files.Count)
            warnings.Add($"要 LOD{lodIndex} 但只有 {files.Count} 级,退回 LOD0");

        var model = ModelRoot.ParseGLB(lod.Data);
        FixBindPose(model, mesh, warnings);
        if (eyeUv is { Count: > 0 }) BakeEyeUv(model, eyeUv, warnings);
        var nodes = new Dictionary<string, Node>(StringComparer.Ordinal);
        foreach (var node in model.LogicalNodes)
            if (!string.IsNullOrEmpty(node.Name))
                nodes.TryAdd(node.Name, node);

        // 借来的动画可能属于**另一副骨架**(见 Program.cs 的借用链)。那时动画里的平移是
        // 源骨架的骨骼长度,照抄过来等于把本形态按源骨架的比例重新拼一遍。
        var meshSkeleton = mesh.Skeleton?.ResolvedObject?.GetPathName();

        // 真的有肉挂在上面的骨骼。用来判断「这根骨骼的平移写错了要不要紧」——
        // 挂了 0 个顶点的定位骨(`locator_ball_1`、乐乐的 `Bone_Yu_01`)写歪了也看不见,
        // 而腿上写歪了就是一条腿飞出去(见 `ConstantTranslationOffenders`)。
        var skinned = SkinnedJointNames(model);
        // 「停得太远」的判据(米):见 `ParkedThreshold`。用绑定姿势包围盒的高度,
        // 与动画无关,所以每个形态是个定值。0 = 这一档不开(宠物那边不开,原因见下)。
        var parked = pullBackParkedBones ? ParkedThreshold(mesh) : 0f;
        // 被「停得太远」判据拉回绑定姿势的骨骼 → 原来偏了多远(米)。
        var parkedBones = new Dictionary<string, float>(StringComparer.Ordinal);
        // 骨骼名 → 它在这个形态的所有动作里被「恒定挪出绑定姿势」的最大距离(米)。
        // 全形态汇总成一条警告(见 Build 末尾),不是一段动作报一条 —— 这毛病要么整只都有、
        // 要么一根都没有,按段报只会把报告刷满。
        var suspects = new Dictionary<string, float>(StringComparer.Ordinal);
        // 中招的动作 → 错位起点与位数(见 `DetectTrackShift`)。同样是整形态汇总一条。
        var shifted = new List<(string Logical, TrackShift Shift)>();
        // 这个资产里别的动作带来的**真轨道映射**(轨道数 → 每条轨道属于哪根骨骼),
        // 用来修「轨道数少于骨骼数、映射却是恒等」那批。见 `BorrowTrackMap`。
        var trackMaps = CollectTrackMaps(clips, mesh.ReferenceSkeleton.FinalRefBoneInfo.Length);
        var borrowed = new List<string>();

        var written = new List<ClipResult>();
        var foreignWarned = false;
        foreach (var (logical, clipName, sequence) in clips)
        {
            try
            {
                var foreign = meshSkeleton is not null &&
                              sequence.Skeleton?.ResolvedObject?.GetPathName() is { } animSkeleton &&
                              !animSkeleton.Equals(meshSkeleton, StringComparison.OrdinalIgnoreCase);
                // 名字对不上的骨骼**全程停在绑定姿势**(黑猫巫师的尾巴/帽子就是这么僵直的)。
                // 这是借用的固有代价,报告里记一行数字,免得日后又要重新量一遍。
                if (foreign && !foreignWarned)
                {
                    foreignWarned = true;
                    var animBones = sequence.Skeleton?.Load<USkeleton>()?.ReferenceSkeleton
                        .FinalRefBoneInfo.Select(b => b.Name.Text) ?? [];
                    var joints = model.LogicalSkins.Count > 0 ? model.LogicalSkins[0].JointsCount : nodes.Count;
                    warnings.Add($"动画借自别的骨架:{joints} 根骨骼里 {animBones.Count(nodes.ContainsKey)} 根对得上名字," +
                                 "其余全程保持绑定姿势(平移已重定基到本形态)");
                }
                var result = AddAnimation(
                    model, nodes, logical, clipName, sequence, foreign, skinned, parked, parkedBones,
                    suspects, shifted, trackMaps, borrowed, warnings);
                if (result is not null) written.Add(result);
            }
            catch (Exception e)
            {
                warnings.Add($"动作 {logical}({clipName}) 加入失败: {e.Message}");
            }
        }


        if (parkedBones.Count > 0)
        {
            var worstParked = parkedBones.OrderByDescending(kv => kv.Value).ToList();
            warnings.Add(
                $"{parkedBones.Count} 根骨骼整段停在离绑定姿势 {parked * 100f:F0}cm 以外,已拉回绑定姿势:" +
                string.Join("、", worstParked.Take(5).Select(kv => $"{kv.Key}({kv.Value * 100f:F0}cm)")) +
                (worstParked.Count > 5 ? "…" : ""));
        }
        if (borrowed.Count > 0)
        {
            var kinds = trackMaps.Keys.OrderBy(x => x);
            warnings.Add(
                $"{borrowed.Count} 段动画的轨道映射被上游读成了恒等(轨道数少于骨骼数时那是不可能的)," +
                $"已换用同资产别的动作带的真映射(轨道数 {string.Join("/", kinds)} 各一份," +
                "见 GlbBuilder.BorrowTrackMap)");
        }
        if (shifted.Count > 0)
        {
            var k = shifted[0].Shift.K;
            var from = shifted.Min(x => x.Shift.Start);
            warnings.Add(
                $"{shifted.Count} 段动画的骨骼数据从第 {from} 根起整体错位 {k} 位" +
                "(上游 CUE4Parse 的 ACL 解码,见 GlbBuilder.DetectTrackShift),已按错位量搬回");
        }

        // **有肉的骨骼被「恒定地」挪出了绑定姿势。**
        // 恒定 = 这段动画根本没在移动它,那这个偏移就不是动作,而是「动画里记的骨骼位置
        // 和这个网格的不一样」—— 照抄过来就是那块肉被拉飞。
        //
        // 这是上面那条错位修正的**兜底哨兵**:错位修完之后可丽希亚只剩一根
        // (`Bone_R_UpperArm_Twist2`,16cm,看不出来),修之前是 17 根、最大 115cm。
        // 别的形态本来就只有裙摆/救生圈那几根小幅命中。**只报不改** —— 单看「恒定偏移」
        // 判不出对错:可丽希亚坐下那一段(`SleepLoop`)的根骨骼就是恒定压低 68cm,那是对的。
        // 报出来让人去渲一张看看,好过悄悄猜错。
        if (suspects.Count > 0)
        {
            var worst = suspects.OrderByDescending(kv => kv.Value).ToList();
            warnings.Add(
                $"{suspects.Count} 根有蒙皮的骨骼被恒定挪出绑定姿势,最大 {worst[0].Value * 100f:F0}cm:" +
                string.Join("、", worst.Take(5).Select(kv => $"{kv.Key}({kv.Value * 100f:F0}cm)")) +
                (worst.Count > 5 ? "…" : "") +
                " —— 动画没在动它们,多半是上游解码填了别的骨骼的平移;超过一个骨节长度的那几根会被拉飞");
        }

        using var stream = new MemoryStream();
        model.WriteGLB(stream);
        return (stream.ToArray(), written, warnings);
    }

    /// **把眼睛族在着色器里做的那步 UV 变换烘进 UV0。**
    ///
    /// NPC 的两颗眼球在网格里各占**半张贴图**(左眼 u∈[0,0.5]、右眼 u∈[0.5,1]),而瞳孔
    /// 贴图上只画了**一只、居中**。照 UV0 原样采,两只眼睛各自只取到瞳孔靠内的那一小条,
    /// 于是一整套 NPC 全是内斜视 —— 实机反馈的第一条。
    ///
    /// 游戏那边由 `M_C_Eyes` 的像素着色器补上这一步(目标 PS 51613):
    ///
    /// ```text
    /// mul r1.xy, v3.xyxx, l(2.0, 1.0, 0, 0)    ← u 平铺 2、v 不动。**2 是字面量**
    /// frc r1.xy, r1.xyxx
    /// add r1.xy, r1.xyxx, l(-0.5, -0.5, 0, 0)
    /// ge  r0.w, l(0.5), v3.x                   ← u ≤ 0.5 选「右」那组参数
    /// …
    /// mad r1.xy, r1.xyxx, r1.zwzz, l(0.5, 0.5, 0, 0)   ← ×(1+缩放)后移回
    /// mul r2.x, r2.w, cb6[28].x                ← 平移H × UVOffsetScale
    /// add r1.xy, r1.xyxx, -r2.xyxx
    /// ```
    ///
    /// **烘进顶点是安全的**:整段只与顶点的 UV 有关,不含任何运行时状态;而且两颗眼球在
    /// UV 上是两个互不相交的岛(实测露西亚:[0.015,0.484] 与 [0.515,0.984],外加两片
    /// 12 顶点的高光小面片都落在 [0.011,0.032]),没有三角形跨 u=0.5 那道缝。
    /// 烘完 `verify_glb.py`/`sweep.py` 看到的就是实机的样子,运行时也不必为这一族开分支。
    private static void BakeEyeUv(
        ModelRoot model, IReadOnlyDictionary<string, EyeUv> eyeUv, List<string> warnings)
    {
        // **这一片的 UV0 不能是和别的片共用的那一份。** 上游给每个 section 各写一份顶点
        // (实测眼睛那片 152 个顶点、身体 5428,accessor 各一条),但共用是 glTF 完全合法的
        // 写法 —— 真共用了还照改,就是把身体的 UV 一起翻倍。宁可不改也不能改坏。
        var shared = model.LogicalMeshes
            .SelectMany(m => m.Primitives)
            .Where(pr => pr.Material?.Name is not { } n || !eyeUv.ContainsKey(n))
            .Select(pr => pr.GetVertexAccessor("TEXCOORD_0")?.LogicalIndex)
            .Where(i => i is not null)
            .ToHashSet();

        foreach (var mesh in model.LogicalMeshes)
        foreach (var primitive in mesh.Primitives)
        {
            var name = primitive.Material?.Name;
            if (name is null || !eyeUv.TryGetValue(name, out var p)) continue;
            var accessor = primitive.GetVertexAccessor("TEXCOORD_0");
            if (accessor is null)
            {
                warnings.Add($"{name}: 眼睛族材质但这一片没有 UV0,跳过瞳孔 UV 修正");
                continue;
            }
            if (shared.Contains(accessor.LogicalIndex))
            {
                warnings.Add($"{name}: 这一片的 UV0 与非眼睛材质共用一条 accessor,跳过瞳孔 UV 修正");
                continue;
            }
            var uv = accessor.AsVector2Array();
            for (var i = 0; i < uv.Count; i++)
            {
                var (u, v) = (uv[i].X, uv[i].Y);
                // 左右两组参数按**原 u** 分,不是按烘完的
                var side = u <= 0.5f ? p.Right : p.Left;
                var t = new Vector2(u * 2f, v);
                t = new Vector2(t.X - MathF.Floor(t.X), t.Y - MathF.Floor(t.Y));
                uv[i] = new Vector2(
                    (t.X - 0.5f) * (1f + side[0]) + 0.5f - side[2] * p.OffsetScale,
                    (t.Y - 0.5f) * (1f + side[1]) + 0.5f - side[3]);
            }
            // **改完必须重算 accessor 的 min/max。** 那两个数是 glTF 的必填元数据,
            // SharpGLTF 在 `WriteGLB` 时会拿它校验,对不上直接抛
            // `Accessor[79] memory: Value[0] is out of bounds` —— 踩过一次:
            // 全量 70 个形态里 11 个当场导不出来(乐乐/万事通/罗兰…),
            // 而剩下 59 个恰好因为烘完的范围仍落在原区间里,一声不吭地过了。
            accessor.UpdateBounds();
        }
    }

    /// 用正确的四元数映射改写骨骼节点旋转,并重算 inverseBindMatrices(见文件头的上游 bug 说明)。
    private static void FixBindPose(ModelRoot model, USkeletalMesh mesh, List<string> warnings)
    {
        var refSkeleton = mesh.ReferenceSkeleton;
        var poseByName = new Dictionary<string, FQuat>(StringComparer.Ordinal);
        for (var i = 0; i < refSkeleton.FinalRefBoneInfo.Length; i++)
            poseByName.TryAdd(refSkeleton.FinalRefBoneInfo[i].Name.Text,
                refSkeleton.FinalRefBonePose[i].Rotation);

        var fixedCount = 0;
        foreach (var node in model.LogicalNodes)
        {
            if (node.Name is null || !poseByName.TryGetValue(node.Name, out var rotation)) continue;
            // 必须保持 SRT 表示(而不是塞一个矩阵),否则后面读 LocalTransform.Rotation 会抛
            var local = node.LocalTransform.GetDecomposed();
            node.LocalTransform = new AffineTransform(local.Scale, SwapYz(rotation), local.Translation);
            fixedCount++;
        }

        // 绑定姿势变了,IBM 必须按新的世界变换重算,否则动画时 world × IBM 依然错位
        foreach (var skin in model.LogicalSkins)
        {
            var joints = new Node[skin.JointsCount];
            for (var i = 0; i < joints.Length; i++) joints[i] = skin.GetJoint(i).Joint;
            skin.BindJoints(Matrix4x4.Identity, joints);
        }

        if (fixedCount == 0) warnings.Add("没有匹配到任何骨骼节点,绑定姿势未修正(动画大概率是错的)");
    }

    /// `foreignSkeleton`:这段动画属于别的骨架(借来的)。那时**平移要重定基**:
    /// 写进 glb 的是「本形态的绑定姿势 + 动画相对源参考姿势的位移」,而不是动画里的绝对值。
    /// 这正是 UE 的 `AnimationRelative` 重定向,骨架相同时它逐位等价于照抄(差值恒为 0),
    /// 所以只在借用时打开、不动其余 600 多个形态。
    private static ClipResult? AddAnimation(
        ModelRoot model,
        Dictionary<string, Node> nodes,
        string logical,
        string clipName,
        UAnimSequence sequence,
        bool foreignSkeleton,
        IReadOnlySet<string> skinned,
        float parked,
        Dictionary<string, float> parkedBones,
        Dictionary<string, float> suspects,
        List<(string Logical, TrackShift Shift)> shifted,
        IReadOnlyDictionary<int, int[]> trackMaps,
        List<string> borrowed,
        List<string> warnings)
    {
        if (sequence.AdditiveAnimType != EAdditiveAnimationType.AAT_None)
        {
            warnings.Add($"动作 {logical}({clipName}) 是叠加动画,跳过");
            return null;
        }

        var set = sequence.ConvertAnims();
        if (set.Sequences.Count == 0)
        {
            warnings.Add($"动作 {logical}({clipName}) 转换后没有序列,跳过");
            return null;
        }
        var seq = set.Sequences[0];
        var frames = Math.Max(seq.NumFrames, 1);
        var fps = seq.FramesPerSecond > 0 ? seq.FramesPerSecond : 30f;
        var refSkeleton = set.Skeleton.ReferenceSkeleton;

        // 这根骨骼的数据在 `seq.Tracks` 的第几格;-1 = 没有,保持绑定姿势。
        // 三条来源,按可信度排:①同资产别的动作带来的**真映射**;②`DetectTrackShift` 的推断;
        // ③什么都不做(`FindTrackForBoneIndex`)。见 `BorrowTrackMap` 与 `DetectTrackShift`。
        var boneCount = refSkeleton.FinalRefBoneInfo.Length;
        var sourceOf = BorrowTrackMap(sequence, boneCount, trackMaps);
        var shift = new TrackShift(0, 0);
        var borrowedMap = sourceOf is not null;
        if (sourceOf is not null)
        {
            borrowed.Add(logical);
        }
        else
        {
            // 上游 ACL 解码从某根骨骼起整体错位(见 `DetectTrackShift`):往回搬 K 位。
            // 这毛病是**按资产**的(中招的资产每一段动画都中),所以汇总成一条,不是一段报一条。
            shift = DetectTrackShift(sequence, seq, refSkeleton);
            if (shift.Any) shifted.Add((logical, shift));
            sourceOf = new int[boneCount];
            for (var bone = 0; bone < boneCount; bone++)
                sourceOf[bone] = shift.Any
                    ? shift.SourceOf(bone)
                    : sequence.FindTrackForBoneIndex(bone) < 0 ? -1 : bone;
        }

        var animation = model.CreateAnimation(logical);
        var rootStart = FVector.ZeroVector;
        var rootEnd = FVector.ZeroVector;
        var matched = 0;

        for (var bone = 0; bone < refSkeleton.FinalRefBoneInfo.Length; bone++)
        {
            // 骨架资产的骨骼数可以多于网格(喵喵:47 vs 44,多出的是末端 Nub),按名字对齐
            var boneName = refSkeleton.FinalRefBoneInfo[bone].Name.Text;
            if (!nodes.TryGetValue(boneName, out var node)) continue;

            // 错位时这根骨骼改读别处的轨道;压根没数据的(-1)保持绑定姿势。
            // **不能再拿 `FindTrackForBoneIndex(bone) < 0` 当「没数据」**:轨道表比骨骼少几条时
            // 它对末尾那几根恒返回 -1,而借来的映射说它们的数据就在最后几条轨道里 ——
            // 菲尔特的整条右小腿(`R-Calf`/`R-Foot`/`R-Toe0`)就是这么被整段丢掉的。
            var source = bone < sourceOf.Length ? sourceOf[bone] : -1;
            var track = source >= 0 && source < seq.Tracks.Count ? seq.Tracks[source] : null;
            // **缩放那一路没跟着错位。** ACL 把旋转/平移/缩放分三份存,三份各自把「整段恒定」的
            // 轨道剔掉;道具骨的旋转平移恒定(被剔了)而缩放不是(要靠它藏起来),于是缩放那份
            // 没被压缩、下标仍然对着骨骼序。所以错位的动作里,**旋转平移读 `source`、缩放读 `bone`**。
            //
            // 这不是推的,是量出来的。菲尔特(`NPC_01401`)那 49 段错位动作,每一段都恰好
            // 给两根骨骼写缩放,值分毫不差地固定在 0.0978 与 0.0976;而按错位读会落到
            // `Bone032` 与 **`Bip001-L-Calf`** 上 —— 左小腿缩成十分之一,靴子整只不见,
            // 正是实机反馈的「待机少了左脚」。三条证据说那两个值属于 75/77 号骨骼
            // (`Bone_Bottle01` 与 `Bone_Spoon01`):
            //   ① 干杯(`GanBei`,映射本来就对的一段)里 `Bone_Bottle01` 的缩放**就是 0.0978**;
            //   ② 另外 21 段映射正确的动作里,`Bone032` 与 `L-Calf` 一次都没被缩放过 ——
            //      真是它俩的话不会 49 段全有、21 段全无;
            //   ③ `World_ShaoZi_Idle`(「拿着勺子」的待机,87 条轨道、映射正确)**不缩放任何骨骼**,
            //      而普通 `World_Idle` 缩放 77 号 —— 一个拿着勺子、一个把勺子藏起来,正好对上。
            var scaleTrack = source == bone || bone >= seq.Tracks.Count ? null : seq.Tracks[bone];
            // 道具骨自己**没有**旋转/平移轨道(压缩器整条剔掉了),但缩放那一格有 ——
            // 那正是「这一段把它藏起来」的开关,不能连它一起跳过,否则酒瓶和勺子会一直挂在身上。
            //
            // **只在这段动作的映射被修过时才捞**(`corrected`),而且**只写缩放那一条**:
            // 没修过的动作里 `FindTrackForBoneIndex < 0` 就是「这根骨骼没有数据」的正解,
            // 照捞会把两样东西弄坏 —— ① 借来别人骨架的动画里,`Tracks[骨骼]` 那一格未必是这根
            // 骨骼的;② 顺手写出去的平移是**动画骨架的参考姿势**,与本网格的绑定姿势不一样,
            // 于是一根本来不该动的骨骼被挪走。多西三阶(`Mac_DuoXi3_001`,借的动画)实测就是
            // 这样:不设这道门会多出 23 条平移通道 + 39 条缩放通道,而它一段动作都没被修过。
            var corrected = borrowedMap || shift.Any;
            if (track is null && !(corrected && scaleTrack is { KeyScale.Length: > 0 })) continue;
            var refPose = refSkeleton.FinalRefBonePose[bone];
            // 借来的动画:平移改成「本形态绑定姿势 + 相对源参考姿势的位移」。恒定的平移轨道
            // (绝大多数骨骼)于是正好落回绑定姿势、整条通道被丢掉,身体比例保持本形态的。
            var rebase = foreignSkeleton
                ? node.LocalTransform.GetDecomposed().Translation - SwapYz(refPose.Translation)
                : Vector3.Zero;
            var rotations = new Dictionary<float, Quaternion>();
            var translations = new Dictionary<float, Vector3>();
            var scales = new Dictionary<float, Vector3>();
            var scaled = false;

            for (var frame = 0; frame < frames; frame++)
            {
                var rotation = refPose.Rotation;
                var position = refPose.Translation;
                var scale = FVector.OneVector;
                track?.GetBoneTransform(frame, frames, ref rotation, ref position, ref scale);
                // **缩放不跟着错位走** —— 见 `scaleTrack`。旋转/平移已经从 `track` 拿到了,
                // 这一步只把 scale 覆盖成本骨骼那一格的。
                if (scaleTrack is not null)
                {
                    var ignoredRotation = refPose.Rotation;
                    var ignoredPosition = refPose.Translation;
                    scale = FVector.OneVector;
                    scaleTrack.GetBoneTransform(
                        frame, frames, ref ignoredRotation, ref ignoredPosition, ref scale);
                }

                var time = frame / fps;
                rotations[time] = SwapYz(rotation);
                translations[time] = SwapYz(position) + rebase;
                scales[time] = new Vector3(scale.X, scale.Z, scale.Y);
                if (MathF.Abs(scale.X - 1f) > 1e-4f || MathF.Abs(scale.Y - 1f) > 1e-4f ||
                    MathF.Abs(scale.Z - 1f) > 1e-4f)
                    scaled = true;

                if (bone == 0)
                {
                    if (frame == 0) rootStart = position;
                    if (frame == frames - 1) rootEnd = position;
                }
            }

            // 体积优化:绝大多数骨骼在一段动画里只转不移,平移/缩放轨道全程恒定。
            // 恒定且等于绑定姿势 → 整条通道不写;恒定但不等 → 只写一个关键帧。
            // 实测这一步把喵喵链的 glb 从 4–6.5MB 压到 1MB 上下。
            //
            // **只为缩放捞回来的那些骨骼(`track is null`)只写缩放**:它们的旋转/平移压根
            // 没有数据,这里手上那份是**动画骨架的参考姿势** —— 与本网格的绑定姿势不一定相等,
            // 写出去就是把一根不该动的骨骼挪走。
            if (track is not null)
            {
                WriteRotation(animation, node, rotations);
                var drift = ConstantTranslationDrift(node, translations, skinned);
                if (parked > 0f && drift > parked)
                {
                    // **停得太远,退回绑定姿势。** 见 `ParkedThreshold`。
                    var bind = node.LocalTransform.GetDecomposed().Translation;
                    foreach (var time in translations.Keys.ToList()) translations[time] = bind;
                    parkedBones[boneName] = MathF.Max(parkedBones.GetValueOrDefault(boneName), drift);
                    drift = 0f;
                }
                WriteTranslation(animation, node, translations);
                if (drift > 0f) suspects[boneName] = MathF.Max(suspects.GetValueOrDefault(boneName), drift);
            }
            if (scaled) WriteScale(animation, node, scales);
            matched++;
        }

        if (matched == 0)
        {
            warnings.Add($"动作 {logical}({clipName}) 没有一根骨骼对上网格骨架,跳过");
            return null;
        }

        var rootMotion = (rootEnd - rootStart).Size();
        return new ClipResult(logical, clipName, frames / fps, frames, rootMotion);
    }

    public static bool IsInPlace(float rootMotionCm) => rootMotionCm < InPlaceThresholdCm;

    /// **同一个资产里,别的动作带着一份真映射。**
    ///
    /// `TrackToSkeletonMapTable` 是「第 i 条轨道属于哪根骨骼」的权威答案。压缩器会把整段恒定的
    /// 骨骼(道具、末端 Nub)整条丢掉,于是轨道数少于骨骼数、映射非恒等 —— 这种动作 CUE4Parse
    /// 处理得**是对的**(菲尔特的 `World_Walk` 双脚俱全)。
    ///
    /// 坏的是另一种:**轨道数少于骨骼数,映射却是恒等的**。那在语义上说不通(恒等 = 丢掉的是
    /// 末尾那几根),而数据也说它是错的。菲尔特(`NPC_01401`,87 根骨骼)一眼可见:
    ///
    /// | 轨道数 | 映射 | 段数 | |
    /// | --- | --- | --- | --- |
    /// | 87 | 恒等 | 16 | 一根不少,对 |
    /// | 84 | 非恒等 | 6 | 丢了 `Bone_Bottle01`/`Bone_Cork01`/`Bone_Spoon01` 三根道具骨,对 |
    /// | 84 | **恒等** | **67** | **坏的** —— `World_Idle` 在里面 |
    /// | 79 | 非恒等 | 1 | |
    ///
    /// 那 6 段给出的映射是 `轨75→骨78, 轨76→骨79(L-Thigh), … 轨83→骨86(R-Toe0)`,
    /// 而坏掉的 `World_Idle` 的**数据恰好也是这个排布**(逐根比首帧:轨79 的旋转与
    /// `L-Toe0` 的参考姿势差 1°、轨80 与 `R-Thigh` 差 12°、轨83 与 `R-Toe0` 差 1°,
    /// 没有第二根骨骼能对上)。两条独立证据指同一份映射,所以直接借过来用。
    ///
    /// **按轨道数配对,不是按资产一份。** 同一个资产不同动作丢的骨骼数不一样:
    /// 可丽希亚(109 根)有 107 条的一族(丢右臂两根 twist)与 106 条的一族(丢的是另外三根),
    /// 各自的映射不同、但**同一个轨道数下的映射完全一致**(107 条那 6 段逐条相同,
    /// 106 条那 4 段也是)。所以键取轨道数;同一个键下两段不一致就整条作废(宁可退回推断)。
    ///
    /// **宠物库里也有**(这条不是 NPC 专用):全库 201 个包 / 617 个形态重导逐字节比,
    /// 权杖-Ⅱ(`Mac_QuanZhangll2_001`)命中这条、瞌睡王与钨丝贝贝命中下面那条推断,
    /// 三只的渲图都是改好 —— 瞌睡王身上那根竖着的灰板子没了,另外两只原先被撑爆的包围盒
    /// 把宠物挤成画面里一小点。其余 614 个形态逐字节不变。
    ///
    /// 返回「骨骼 → 该读第几条轨道」;null = 这段动作不需要借(映射本来就对,或者借不到)。
    private static int[]? BorrowTrackMap(
        UAnimSequence sequence, int boneCount, IReadOnlyDictionary<int, int[]> trackMaps)
    {
        var map = sequence.GetTrackMap();
        // 轨道数与骨骼数一致,或者映射本来就是非恒等的 —— 两种都不用管
        if (map.Length >= boneCount) return null;
        for (var i = 0; i < map.Length; i++)
            if (map[i].BoneTreeIndex != i)
                return null;
        if (!trackMaps.TryGetValue(map.Length, out var good)) return null;

        var sourceOf = new int[boneCount];
        Array.Fill(sourceOf, -1);
        for (var track = 0; track < good.Length; track++)
            if (good[track] >= 0 && good[track] < boneCount)
                sourceOf[good[track]] = track;
        return sourceOf;
    }

    /// 收集这个资产里**可信的**轨道映射:轨道数 → 「第 i 条轨道属于哪根骨骼」。
    /// 只收非恒等的那些(恒等的要么本来就对、要么正是要修的那种);同一个轨道数下
    /// 两段给的映射不一致就把这个键作废。
    private static Dictionary<int, int[]> CollectTrackMaps(
        IReadOnlyList<(string Logical, string Clip, UAnimSequence Sequence)> clips, int boneCount)
    {
        var maps = new Dictionary<int, int[]>();
        var rejected = new HashSet<int>();
        foreach (var (_, _, sequence) in clips)
        {
            FTrackToSkeletonMap[] map;
            try { map = sequence.GetTrackMap(); }
            catch { continue; }
            if (map.Length >= boneCount) continue;
            var bones = map.Select(m => m.BoneTreeIndex).ToArray();
            if (bones.Select((b, i) => b == i).All(x => x)) continue;   // 恒等:不可信
            if (rejected.Contains(map.Length)) continue;
            if (!maps.TryGetValue(map.Length, out var seen)) maps[map.Length] = bones;
            else if (!seen.SequenceEqual(bones)) { maps.Remove(map.Length); rejected.Add(map.Length); }
        }
        return maps;
    }

    /// **上游 ACL 解码从某根骨骼起整体错位**:从哪根开始、错几位。
    ///
    /// 症状(可丽希亚 `NPC_01301`,用 `--probe-anim NPC_01301:World_Idle` 逐根摊开量到的):
    /// 从骨骼 81 起,**骨骼 N 拿到的是骨骼 N+2 的整份数据**(旋转与平移都是)。
    /// 平移那一半特别好认 —— 后面这批骨骼在动画里本来就不平移,于是
    /// 「骨骼 N 的平移恰好等于骨骼 N+2 的**参考姿势**」:`Bip001-L-Thigh` 拿到 `Bip001-L-Foot` 的、
    /// `Bip001-R-Foot` 拿到 `locator_ball_1` 的(右脚被摆到 100cm 外,整条腿甩出体外)。
    /// 旋转那一半从数值上看不出规律,但把数据整体移回 2 位之后,腿部旋转与**同一个角色的
    /// 另一套资产**(`NPC_10801`,同一份 biped、同样的站立待机)的差从 106~179° 掉到 0.0~1.5°,
    /// 骨骼 83 以后 23 根的中位差 68.1° → 0.8°。这个资产的 86 段动画全部如此。
    ///
    /// **不是我们这侧的索引问题**,排除法做完了:网格与骨架的骨骼数都是 109、参考姿势逐根一致,
    /// 轨道映射是恒等的 107 条,`Tracks` 按骨骼序排(CUE4Parse 三条填充路径都是
    /// `for boneIndex in 0..BoneCount` + `FindTrackForBoneIndex`)。错在
    /// `CUE4Parse-Natives` 的 `nReadACLData` —— ACL 那侧的轨道数与 UE 的轨道表对不上,
    /// 从某处起整体差了 2 条。
    ///
    /// **判据要求「整批一致」而不是「某一根可疑」**:从后往前找「平移恰好等于后面第 k 根的
    /// 参考姿势」的骨骼,至少要有 [`ShiftMinBones`] 根**能判别**的(那两个参考姿势不相同,
    /// 否则说明不了问题)。单根撞上是巧合,二十几根同一个 k 不是。真的恒定平移
    /// (可丽希亚坐下时根骨骼恒定压低 68cm,骨骼 1)落在区间外,不会被误伤。
    ///
    /// 中间**允许 [`ShiftGapTolerance`] 根说不清的**:错位区间里也有本来就在动的骨骼
    /// (可丽希亚的披风 `Bone_cloak_001`、右臂的两根 twist),它们的真值不是参考姿势,
    /// 判别不出来。不留这点余地的话扫描会停在 86,而真正的起点是 81 ——
    /// 少修的那几根正是右上臂那两根 twist(与另一套资产比差 121~125°)。
    ///
    /// 修法:区间起点往后第 k 根开始,每根骨骼改读**前面第 k 根**的轨道(旋转+平移+缩放一起),
    /// 起点那 k 根(ACL 那侧丢掉的两条,数据根本没解出来)退回自己的参考姿势。
    private const int ShiftMinBones = 3;
    private const int ShiftMaxDistance = 4;
    private const int ShiftGapTolerance = 3;

    /// `Start` 起、错 `K` 位。`K == 0` 表示没有错位。
    private readonly record struct TrackShift(int Start, int K)
    {
        public bool Any => K > 0;

        /// 这根骨骼该读哪根骨骼的轨道;返回 -1 表示「没有可信数据,用自己的参考姿势」。
        public int SourceOf(int bone)
        {
            if (K == 0 || bone < Start) return bone;
            return bone < Start + K ? -1 : bone - K;
        }
    }

    private static TrackShift DetectTrackShift(
        UAnimSequence sequence, CAnimSequence seq, FReferenceSkeleton refSkeleton)
    {
        var count = Math.Min(refSkeleton.FinalRefBoneInfo.Length, seq.Tracks.Count);
        var pose = refSkeleton.FinalRefBonePose;
        // 每根骨骼首帧解出来的平移(没轨道的记 null)。判别只看首帧就够 ——
        // 要求整段连续区间一致,巧合撞不出来。
        var first = new FVector?[count];
        for (var bone = 0; bone < count; bone++)
        {
            if (sequence.FindTrackForBoneIndex(bone) < 0) continue;
            var rotation = pose[bone].Rotation;
            var position = pose[bone].Translation;
            var scale = FVector.OneVector;
            seq.Tracks[bone].GetBoneTransform(0, Math.Max(seq.NumFrames, 1), ref rotation, ref position, ref scale);
            first[bone] = position;
        }

        static bool Same(FVector a, FVector b) => (a - b).SizeSquared() <= 1e-4f;

        var best = new TrackShift(0, 0);
        var bestScore = 0;
        for (var k = 1; k <= ShiftMaxDistance; k++)
        {
            // 从最后往前扫,记住最靠前的那次「判别性命中」;中间允许几根说不清的
            var start = -1;
            var discriminating = 0;
            var gap = 0;
            for (var i = count - k - 1; i >= 0; i--)
            {
                if (first[i] is not { } value) break;   // 没轨道:到头了
                if (!Same(pose[i + k].Translation, pose[i].Translation)
                    && Same(value, pose[i + k].Translation))
                {
                    discriminating++;
                    start = i;
                    gap = 0;
                }
                else if (++gap > ShiftGapTolerance) break;
            }
            if (start >= 0 && discriminating >= ShiftMinBones && discriminating > bestScore)
            {
                bestScore = discriminating;
                best = new TrackShift(start, k);
            }
        }
        return best;
    }

    /// 这根骨骼在这段动画里被「恒定挪出绑定姿势」多远(米);不算数就返回 0。
    /// 判据的来由见 `Build` 末尾那段说明。
    ///
    /// 门槛 10cm:比它小的在桌面上那点像素里看不出来。**根骨骼那一档不排除** ——
    /// 排除它要靠猜下标,而恒定压低根骨骼(坐下)本来就是合法的,报出来让人看一眼就好。
    /// 「停得太远」的门槛(米):**四分之一身高**,再兜一个 30cm 的下限。判据要挡住两类东西:
    /// 美术把用不到的道具挪出画面藏起来(远行商人的武器在 10.5 米外),以及上游解码把别的骨骼
    /// 的平移填了进来。按身高而不是绝对值 —— 同样偏 40cm,对 1.8 米的人是半条腿,
    /// 对 40cm 的小个子是三个身子。
    ///
    /// **只对 NPC 开(`pullBackParkedBones`)。** 宠物那边量过:已发布的 201 个包 / 617 个形态里
    /// 34 个形态、304 条通道会被这条碰到,而抽查下来它们是**对的** —— 伏地兽那条伸出去 57cm 的
    /// 舌头正是这只宠物的样子,鸭吉吉挂在身上的炸弹、阿瓦鲨嘴边的小鱼也一样。宠物个头小,
    /// 「四分之一身高」在那边太紧。
    private static float ParkedThreshold(USkeletalMesh mesh) =>
        MathF.Max(0.30f, mesh.ImportedBounds.BoxExtent.Z * 2f * CmToM * 0.25f);

    private const float SuspectOffsetM = 0.10f;

    private static float ConstantTranslationDrift(
        Node node, Dictionary<float, Vector3> translations, IReadOnlySet<string> skinned)
    {
        if (node.Name is null || !skinned.Contains(node.Name)) return 0f;
        var first = translations.First().Value;
        if (!translations.Values.All(v => Close(v, first))) return 0f;   // 在动 = 是真动作
        var drift = (first - node.LocalTransform.GetDecomposed().Translation).Length();
        return drift > SuspectOffsetM ? drift : 0f;
    }

    /// 真的有顶点权重挂在上面的骨骼名。定位骨/道具挂点权重为 0,写歪了也看不见。
    private static IReadOnlySet<string> SkinnedJointNames(ModelRoot model)
    {
        var names = new HashSet<string>(StringComparer.Ordinal);
        foreach (var mesh in model.LogicalMeshes)
            foreach (var primitive in mesh.Primitives)
            {
                var joints = primitive.GetVertexAccessor("JOINTS_0")?.AsVector4Array();
                var weights = primitive.GetVertexAccessor("WEIGHTS_0")?.AsVector4Array();
                if (joints is null || weights is null) continue;
                var skin = model.LogicalSkins.Count > 0 ? model.LogicalSkins[0] : null;
                if (skin is null) continue;
                for (var v = 0; v < joints.Count && v < weights.Count; v++)
                {
                    var j = joints[v];
                    var w = weights[v];
                    Span<float> js = [j.X, j.Y, j.Z, j.W];
                    Span<float> ws = [w.X, w.Y, w.Z, w.W];
                    for (var k = 0; k < 4; k++)
                    {
                        if (ws[k] <= 0.01f) continue;
                        var index = (int)js[k];
                        if (index < 0 || index >= skin.JointsCount) continue;
                        var name = skin.GetJoint(index).Joint.Name;
                        if (!string.IsNullOrEmpty(name)) names.Add(name);
                    }
                }
            }
        return names;
    }

    /// 关键帧值相等的判定阈值:位置单位是米,1e-5 ≈ 0.01mm,旋转是单位四元数分量。
    private const float KeyEpsilon = 1e-5f;

    private static void WriteRotation(Animation animation, Node node, Dictionary<float, Quaternion> keys)
    {
        var first = keys.First().Value;
        if (keys.Values.All(v => Close(v, first)))
        {
            if (Close(first, node.LocalTransform.GetDecomposed().Rotation)) return;
            keys = new Dictionary<float, Quaternion> { [0f] = first };
        }
        animation.CreateRotationChannel(node, keys);
    }

    private static void WriteTranslation(Animation animation, Node node, Dictionary<float, Vector3> keys)
    {
        var first = keys.First().Value;
        if (keys.Values.All(v => Close(v, first)))
        {
            if (Close(first, node.LocalTransform.GetDecomposed().Translation)) return;
            keys = new Dictionary<float, Vector3> { [0f] = first };
        }
        animation.CreateTranslationChannel(node, keys);
    }

    private static void WriteScale(Animation animation, Node node, Dictionary<float, Vector3> keys)
    {
        var first = keys.First().Value;
        if (keys.Values.All(v => Close(v, first)))
        {
            if (Close(first, node.LocalTransform.GetDecomposed().Scale)) return;
            keys = new Dictionary<float, Vector3> { [0f] = first };
        }
        animation.CreateScaleChannel(node, keys);
    }

    private static bool Close(Vector3 a, Vector3 b) => (a - b).LengthSquared() < KeyEpsilon * KeyEpsilon;

    private static bool Close(Quaternion a, Quaternion b) =>
        MathF.Abs(a.X - b.X) < KeyEpsilon && MathF.Abs(a.Y - b.Y) < KeyEpsilon &&
        MathF.Abs(a.Z - b.Z) < KeyEpsilon && MathF.Abs(a.W - b.W) < KeyEpsilon;
}
