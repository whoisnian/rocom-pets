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
    public static (byte[] Glb, List<ClipResult> Written, List<string> Warnings) Build(
        USkeletalMesh mesh,
        IReadOnlyList<(string Logical, string Clip, UAnimSequence Sequence)> clips,
        int lodIndex)
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
        var nodes = new Dictionary<string, Node>(StringComparer.Ordinal);
        foreach (var node in model.LogicalNodes)
            if (!string.IsNullOrEmpty(node.Name))
                nodes.TryAdd(node.Name, node);

        // 借来的动画可能属于**另一副骨架**(见 Program.cs 的借用链)。那时动画里的平移是
        // 源骨架的骨骼长度,照抄过来等于把本形态按源骨架的比例重新拼一遍。
        var meshSkeleton = mesh.Skeleton?.ResolvedObject?.GetPathName();

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
                var result = AddAnimation(model, nodes, logical, clipName, sequence, foreign, warnings);
                if (result is not null) written.Add(result);
            }
            catch (Exception e)
            {
                warnings.Add($"动作 {logical}({clipName}) 加入失败: {e.Message}");
            }
        }

        using var stream = new MemoryStream();
        model.WriteGLB(stream);
        return (stream.ToArray(), written, warnings);
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

        var animation = model.CreateAnimation(logical);
        var rootStart = FVector.ZeroVector;
        var rootEnd = FVector.ZeroVector;
        var matched = 0;

        for (var bone = 0; bone < refSkeleton.FinalRefBoneInfo.Length; bone++)
        {
            // 骨架资产的骨骼数可以多于网格(喵喵:47 vs 44,多出的是末端 Nub),按名字对齐
            var boneName = refSkeleton.FinalRefBoneInfo[bone].Name.Text;
            if (!nodes.TryGetValue(boneName, out var node)) continue;
            if (sequence.FindTrackForBoneIndex(bone) < 0) continue; // 无轨道 = 保持绑定姿势

            var track = seq.Tracks[bone];
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
                track.GetBoneTransform(frame, frames, ref rotation, ref position, ref scale);

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
            WriteRotation(animation, node, rotations);
            WriteTranslation(animation, node, translations);
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
