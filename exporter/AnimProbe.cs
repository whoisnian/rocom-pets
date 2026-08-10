// 动画调查用的探针(`--probe-anim <资产名>`)。
//
// 起因:一批形态的 Idle/Run/JumpFall 把**整只宠物**平移了半个身位(阿米亚特 −0.75m 横向、
// 加尔 −0.50m 纵向),而同一形态的其余动作都规规矩矩落在原点。游戏里没有这种事,所以问题
// 出在「我们怎么读动画」而不是「美术怎么做的动画」。
//
// 这个探针把三样东西摆在一起看:
// ① `USkeleton.BoneTree` 的**平移重定向模式**(`EBoneTranslationRetargetingMode`)——
//    UE 播动画时,标成 `Skeleton` 的骨骼**直接用骨架参考姿势的平移,丢掉动画里那份**;
// ② 每段动画里各骨骼平移相对参考姿势的偏移;
// ③ 动画自己的 root motion 开关。
//
// 只读、不写文件,输出给人看。

using CUE4Parse.FileProvider;
using CUE4Parse.FileProvider.Vfs;
using CUE4Parse.UE4.Assets.Exports.Animation;
using CUE4Parse.UE4.Assets.Exports.SkeletalMesh;
using CUE4Parse.UE4.Objects.Core.Math;
using CUE4Parse_Conversion.Animations;
using CUE4Parse_Conversion.Writers.ActorX.Structs.Animations;

namespace RocomPets.Export;

public static class AnimProbe
{
    private const string PetsRoot = "NRC/Content/ArtRes/AnimSequence/Pets";

    /// `--probe-anim ALL`:全库普查「一个逻辑动作名撞上几个资产」。
    /// 撞名的来源是 `Normalize` 剥掉的类别前缀 —— `World_Idle` 与 `Ride_Idle` 剥完都是 idle。
    public static void Survey(AbstractVfsFileProvider provider)
    {
        // 值用 SortedSet:`provider.Files.Values` 里同一条路径会被补丁 pak 重复列出
        // (`Textures.TopLevelFiles` 也是这么去重的),不去重会把撞名数灌到没法看。
        var byAsset = new SortedDictionary<string, SortedDictionary<string, SortedSet<string>>>(
            StringComparer.OrdinalIgnoreCase);
        foreach (var file in provider.Files.Values)
        {
            var path = file.Path;
            if (!path.StartsWith(PetsRoot + "/", StringComparison.OrdinalIgnoreCase)) continue;
            var tail = path[(PetsRoot.Length + 1)..];
            var slash = tail.IndexOf('/');
            if (slash < 0) continue;
            var asset = tail[..slash];
            var rest = tail[(slash + 1)..];
            if (!rest.StartsWith("Animation/", StringComparison.OrdinalIgnoreCase)) continue;
            if (rest.IndexOf('/', "Animation/".Length) >= 0) continue;
            if (!path.EndsWith(".uasset", StringComparison.OrdinalIgnoreCase)) continue;
            var name = Path.GetFileNameWithoutExtension(rest);
            if (!byAsset.TryGetValue(asset, out var byLogical))
                byAsset[asset] = byLogical = new SortedDictionary<string, SortedSet<string>>(StringComparer.OrdinalIgnoreCase);
            var key = AnimNames.Normalize(name);
            if (!byLogical.TryGetValue(key, out var list))
                byLogical[key] = list = new SortedSet<string>(StringComparer.OrdinalIgnoreCase);
            list.Add(name);
        }

        // 只关心桌宠白名单里那些逻辑名的撞名情况:别的动作我们本来就不导。
        var clashes = new SortedDictionary<string, SortedDictionary<string, int>>(StringComparer.OrdinalIgnoreCase);
        var assetsWithClash = 0;
        foreach (var (asset, byLogical) in byAsset)
        {
            var any = false;
            foreach (var (logical, files) in byLogical)
            {
                if (files.Count < 2) continue;
                any = true;
                if (!clashes.TryGetValue(logical, out var variants))
                    clashes[logical] = variants = new SortedDictionary<string, int>(StringComparer.OrdinalIgnoreCase);
                foreach (var f in files)
                {
                    var prefix = f.Contains('_') ? f[..f.IndexOf('_')] : "(无前缀)";
                    variants[prefix] = variants.GetValueOrDefault(prefix) + 1;
                }
            }
            if (any) assetsWithClash++;
        }
        Console.WriteLine($"{byAsset.Count} 个资产有 Animation/,其中 {assetsWithClash} 个存在撞名");
        Console.WriteLine("逻辑名 → 各类别前缀出现次数(次数即「有多少个资产给出了这个前缀的版本」):");
        foreach (var (logical, variants) in clashes)
            Console.WriteLine($"  {logical,-14} {string.Join("  ", variants.Select(v => $"{v.Key}×{v.Value}"))}");
    }

    public static void Run(AbstractVfsFileProvider provider, string asset)
    {
        if (asset.Equals("ALL", StringComparison.OrdinalIgnoreCase))
        {
            Survey(provider);
            return;
        }
        var assetDir = $"{PetsRoot}/{asset}";
        var meshName = Textures.TopLevelFiles(provider, assetDir)
            .Select(Path.GetFileNameWithoutExtension)
            .Where(n => n is not null && n.StartsWith("SKM_", StringComparison.Ordinal))
            .OrderByDescending(n => n!.EndsWith("_Skin", StringComparison.Ordinal))
            .FirstOrDefault();
        if (meshName is null)
        {
            Console.Error.WriteLine($"{assetDir} 下没有 SKM_*");
            return;
        }
        var mesh = provider.LoadPackageObject<USkeletalMesh>($"{assetDir}/{meshName}");
        var skeleton = mesh.Skeleton?.Load<USkeleton>();
        // 骨架**所在的资产目录**才是「谁的动画能直接套上来」的判据(见 design.md
        // 「黑猫巫师身体偏短」):黑猫巫师的网格挂的是 `Com_HeiMaoBo_001` 那份骨架。
        Console.WriteLine($"网格 {meshName},骨架 {skeleton?.Name ?? "(网格自带)"}" +
                          $" @ {mesh.Skeleton?.ResolvedObject?.GetPathName() ?? "?"}");

        var refSkeleton = mesh.ReferenceSkeleton;
        var names = refSkeleton.FinalRefBoneInfo.Select(b => b.Name.Text).ToArray();
        var modes = skeleton?.BoneTree;
        if (modes is null)
        {
            Console.WriteLine("骨架没有 BoneTree(读不到重定向模式)");
        }
        else
        {
            var byMode = modes.Select((m, i) => (Mode: m, Index: i))
                .GroupBy(x => x.Mode)
                .OrderBy(g => g.Key);
            Console.WriteLine($"BoneTree {modes.Length} 根骨骼的平移重定向模式:");
            foreach (var group in byMode)
            {
                var sample = group.Take(8)
                    .Select(x => x.Index < names.Length ? names[x.Index] : $"#{x.Index}");
                Console.WriteLine($"  {group.Key,-18} {group.Count(),3} 根: {string.Join(", ", sample)}" +
                                  (group.Count() > 8 ? " …" : ""));
            }
        }

        foreach (var path in Textures.TopLevelFiles(provider, $"{assetDir}/Animation")
                     .OrderBy(p => p, StringComparer.OrdinalIgnoreCase))
        {
            UAnimSequence sequence;
            try
            {
                sequence = provider.LoadPackageObject<UAnimSequence>(path[..path.LastIndexOf('.')]);
            }
            catch (Exception e)
            {
                Console.WriteLine($"[{Path.GetFileNameWithoutExtension(path)}] 加载失败: {e.Message}");
                continue;
            }
            Console.WriteLine($"[{Path.GetFileNameWithoutExtension(path)}] {sequence.NumFrames} 帧" +
                              $" 叠加={sequence.AdditiveAnimType} 重定向源={sequence.RetargetSource}");
            CAnimSet set;
            try
            {
                set = sequence.ConvertAnims();
            }
            catch (Exception e)
            {
                Console.WriteLine($"   转换失败: {e.Message}");
                continue;
            }
            if (set.Sequences.Count == 0) continue;
            var seq = set.Sequences[0];
            var frames = Math.Max(seq.NumFrames, 1);
            // 每根有轨道的骨骼:第一帧平移相对参考姿势偏了多少(厘米)。只打**偏得离谱**的,
            // 一段动画里这种骨骼要么没有、要么是整只一起偏。
            for (var bone = 0; bone < refSkeleton.FinalRefBoneInfo.Length && bone < seq.Tracks.Count; bone++)
            {
                if (sequence.FindTrackForBoneIndex(bone) < 0) continue;
                var refPose = refSkeleton.FinalRefBonePose[bone];
                var rotation = refPose.Rotation;
                var position = refPose.Translation;
                var scale = FVector.OneVector;
                seq.Tracks[bone].GetBoneTransform(0, frames, ref rotation, ref position, ref scale);
                var delta = position - refPose.Translation;
                if (delta.Size() < 10f) continue; // 10cm 以内是正常的动作幅度
                var mode = modes is not null && bone < modes.Length ? modes[bone].ToString() : "?";
                Console.WriteLine($"   {names[bone],-24} Δ={delta} ({delta.Size():F1}cm) 重定向={mode}");
            }
        }
    }
}
