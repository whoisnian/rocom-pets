// 生成 manifest.toml。schema 见 docs/design.md §4.3。
//
// 手写 TOML 而不引 TOML 库:字段固定、只写不读,手写省一个依赖也少一层版本约束。
// 运行时那边用 Rust 的 toml crate 读。

using System.Globalization;
using System.Text;

namespace RocomPets.Export;

/// `M_Gra_Yutu_Ear_Lighting` 的目标 Low 材质输入。颜色/标量均保留游戏参数原值；
/// 三张贴图分别对应 PS 6037 的 t2/t3/t4。
public record YutuEarMaterial(
    string? BubbleTexture,
    string? DistortTexture,
    string? FlowTexture,
    float[] BubbleColor,
    float[] FlowColor,
    float[] FresnelColor,
    float[] InnerColor,
    float[] OverallColor,
    float[] RampColor,
    float[] TopColor,
    float[] BubbleShape,
    float[] FlowShape,
    float[] LightShape,
    float[] TopShape);

/// `M_P_FakeFulid`（原资产拼写）的液面/玻璃参数。FuildMask 与 BubbleColorLutTex
/// 继续复用 MaterialEntry 的 mask/noise 槽，避免同一贴图重复导出。
public record FakeFluidMaterial(
    float[] EdgeColor,
    float[] FresnelColor,
    float[] PlaneColor,
    float[] Gradient1,
    float[] Gradient2,
    float[] HeightTiling,
    float[] PlaneAxis,
    float[] PlaneCenter,
    float[] BodyShape,
    float[] GradientShape,
    float[] TopShape);

/// `M_P_MatCap_Masked` 的目标 Low PS 19654 输入。MatCap 贴图沿用
/// MaterialEntry.MaskTexture；这里保留原 uniform preshader 对应的颜色与标量。
public record MatcapMaskedMaterial(
    float[] BaseColor,
    float[] LightRampColor,
    float[] FlatEmissiveColor,
    float[] MainColor,
    float[] SelectionColor,
    float[] RimShape,
    float[] SurfaceShape);

/// `M_FairyBall_BallFront` 的目标 PS 52626 输入(沙漏/水晶球那层玻璃壳)。
/// 这一族只采一张贴图,就是 `Matcap`;其余全是这几个参数。
public record FairyBallMaterial(
    string? Matcap,
    float[] BaseColor,
    float[] MatcapColor,
    float[] RimDarkColor,
    float[] RimLightColor,
    float[] MainColor,
    float[] Shape);

/// 一个材质槽写进 manifest 的内容:运行时按 glb 里的材质名查这张表。
public record MaterialEntry(
    string Name,
    /// 基色贴图在包内的相对路径;null = 这个材质不画固有色(纯 VFX),运行时目前整片跳过。
    string? BaseColor,
    /// 贴图 alpha 是不是真遮罩(眼/嘴的表情图集是,本体贴图不是)。
    bool MaskAlpha,
    float MaskClip,
    string Blend,
    /// 父链(由近及远),排查用;也是「这是哪一族特效」的线索(如 M_FX_Fire_Mat)。
    List<string> ParentChain,
    /// 特效层的主色(线性 RGBA)。**可能是 HDR**(火花的 Color01 = (6, 0.8, 0)),
    /// 任一通道 >1 就说明这层是加色发光,运行时据此走加色而不是半透。
    float[]? Tint,
    float Opacity,
    /// 发光强度(火焰族的 `Glow Intensity`)。
    float Glow,
    /// UV 卷动 [u速度, v速度, u平铺, v平铺];火焰靠它动。
    float[] Flow,
    /// 特效的形状/流动贴图,包内相对路径。
    string? MaskTexture,
    string? NoiseTexture,
    /// 遮罩贴图是不是 MatCap(要按视空间法线采样,不能用网格 UV)。
    bool MaskIsMatcap,
    /// 以下对**所有**材质都有意义(有基色的也一样)。
    bool Translucent,
    /// 星点贴图 + 平铺 + 着色 + 强度:身上那些细碎星光。
    string? StarTexture,
    float[] StarTiling,
    float[]? StarColor,
    float StickIntensity,
    /// 星点层来自**「假半透」族**(`NoiseTex` + `Color02`),而不是 `StarStickTex` 那一族。
    /// 两族的着色不一样:前者用 `Color02`(即 `StarColor` 这一项),后者用
    /// `StickRandomColor01..04` 四段渐变。见 pet.wgsl 的 `stick_layer`。
    bool StarFakeTrans,
    /// MatCap 贴图 + 着色:玻璃/金属高光。
    string? MatcapTexture,
    float[]? MatcapColor,
    /// 边缘光。`RimPower` < 1 = 整片泛色而不是一圈细边。
    float[]? RimColor,
    float RimIntensity,
    /// 自发光色(线性)与强度;强度为 0 时不写出。见 Materials.cs 的 EmissiveColor。
    float[]? EmissiveColor,
    float EmissiveIntensity,
    float RimPower,
    float RimSoftEdge,
    /// `M_P_Object_Trans` 的高光/alpha 覆盖参数,全部来自材质实例或根默认。
    float[] HighlightOffset,
    float[] HighlightSpecColor,
    float HighlightSpecPower,
    float HighlightSpecIntensity,
    float ForceUseDefaultOpacity,
    /// `M_P_Object_Trans` 的场景深度淡化:距离(cm)与开启强度。
    float OpacityDepthDistance,
    float OpenDepthDistance,
    /// 目标设备 ES3.1/Low 的基础 `MI_P_Object_Trans` 局部链。
    bool ObjectTransLow,
    string? ObjectTransLightMaskTexture,
    string? ObjectTransRampTexture,
    float ObjectTransSoftEdge,
    float[] ObjectTransMainColor,
    float ObjectTransMainBright,
    /// 基色贴图的 alpha 是不透明度(而不是纹路遮罩)。
    bool AlphaIsOpacity,
    /// 卷动色带:渐变图 + 混入强度(暮星辰的环带靠它出青↔粉渐变)。
    string? FlowTexture,
    float FlowPower,
    /// 色带的 ID 遮罩:`MaskTex` + alpha 的取值区间。
    string? MaskIdTexture,
    float[] MaskIdRange,
    /// 水体预设(`ML_P_StylizedWater`):`Color1`(a = 增益 `Emitter Intensity`)、
    /// `Color2`、`Main Color`(a = 末尾 lerp 的混合系数)、caustics 的平铺/速度、
    /// `[CausticsInt, FlowDistort, FresnelInt, FresnelPower]`。公式见
    /// rocom-capture/docs/shader.md「水体预设」。
    float[]? WaterColor1,
    float[]? WaterColor2,
    float[]? WaterMain,
    float[] WaterCaustics,
    float[] WaterShape,
    /// 玻璃内部那颗星:四角星场贴图 + 着色 + 折射率 + march 深度。
    string? InteriorTexture,
    float[]? InteriorColor,
    float Refraction,
    float RefractDepth,
    /// 球内那颗星的闪烁:速度与次数(见 MaterialInfo.FlickerSpeed)。
    float FlickerSpeed,
    float FlickerPower,
    /// 假半透族星点层的 [速度X, 速度Y, 强度, 是否用UV0]。见 Materials.cs 的 NoiseUv。
    float[] NoiseUv,
    /// `M_ShuiMu_ByIn` 的原始流动内胆分支。
    bool GlassyInner,
    float[] GlassyFlowColor01,
    float[] GlassyFlowColor02,
    float[] GlassyFresnelColor,
    float[] GlassyNoiseParams,
    float[] GlassyMaskParams,
    /// `MI_P_Object_XiaoYou` 的目标 Low 材质输入。三张贴图复用 base/noise/star 槽，
    /// 这里仅记录原材质的颜色、panner 与星点控制参数。
    bool XiaoYou,
    float[] XiaoYouBaseColor1,
    float[] XiaoYouBaseColor2,
    float[] XiaoYouFlowColor1,
    float[] XiaoYouFlowColor2,
    float[] XiaoYouStarColor,
    float[] XiaoYouNoiseFlow,
    float[] XiaoYouShape,
    float[] XiaoYouStarUv,
    YutuEarMaterial? YutuEar,
    FakeFluidMaterial? FakeFluid,
    MatcapMaskedMaterial? MatcapMasked,
    FairyBallMaterial? FairyBall,
    /// 配套 `_Ol` 描边材质算出来的描边宽度(米);0 = 不画。见 `Materials.OutlineWidthOf`。
    float OutlineWidth = 0f,
    /// 按画家序画(不写深度),见 `MaterialInfo.IsPaintOrder`。
    bool PaintOrder = false);

public record FormReport(
    Form Form,
    List<ClipResult> Clips,
    List<TextureFile> Textures,
    List<MaterialEntry> Materials,
    int GlbBytes,
    float HeightCm,
    List<string> Warnings,
    /// 叫声与动作音效;null = 这个形态两族库都没有(或者外部工具不在)。
    AudioInfo? Audio = null,
    /// 动作名 → 中文标签。只有和动作名不同的才在里面(宠物那边一条都没有:
    /// 它的动作名就是运行时那套,中文名写在运行时的 `RUNTIME_CLIPS` 里)。
    Dictionary<string, string>? ClipLabels = null);

public static class Manifest
{
    /// manifest 格式版本;运行时 ABI 版本单独走,便于格式没变但语义变了的情况。
    private const int Schema = 1;
    private const int RuntimeAbi = 1;

    public static string Render(Chain chain, List<FormReport> forms, int lodIndex, string sourceVersion)
    {
        var sb = new StringBuilder();
        sb.AppendLine("# 由 rocom-pets-export 生成,勿手改(素材本地生成物,不入仓库)");
        sb.AppendLine($"schema = {Schema}");
        sb.AppendLine($"runtime_abi = {RuntimeAbi}");
        sb.AppendLine($"generated_at = {Quote(DateTime.UtcNow.ToString("yyyy-MM-dd'T'HH:mm:ss'Z'", CultureInfo.InvariantCulture))}");
        sb.AppendLine($"lod = {lodIndex}");
        // 导出时的 pak 指纹:换游戏版本重导后这里会变,便于排查「这包是哪版导的」
        sb.AppendLine($"source_version = {Quote(sourceVersion)}");
        sb.AppendLine();

        sb.AppendLine("[species]");
        sb.AppendLine($"id = {chain.RootId}");
        sb.AppendLine($"name = {Quote(chain.Name)}");
        sb.AppendLine($"chain = [{string.Join(", ", chain.Forms.Select(f => f.Id))}]");

        foreach (var report in forms)
        {
            var form = report.Form;
            sb.AppendLine();
            sb.AppendLine("[[forms]]");
            sb.AppendLine($"id = {form.Id}");
            sb.AppendLine($"name = {Quote(form.Name)}");
            sb.AppendLine($"stage = {form.Stage}");
            sb.AppendLine($"asset = {Quote(form.Asset)}");
            sb.AppendLine($"model = {Quote($"forms/{form.Asset}/model.glb")}");
            sb.AppendLine($"scale = {Num(form.ModelScale)}");
            sb.AppendLine($"height_cm = {Num(report.HeightCm)}   # 绑定姿势包围盒高度,用于换算屏幕像素");
            sb.AppendLine($"locomotion = {Quote(Locomotion(form.MoveType))}   # 原文 move_type = {Quote(form.MoveType)}");

            sb.AppendLine();
            sb.AppendLine($"  [forms.clips]   # 逻辑动作 → glb 里的 animation 名(同名)");
            foreach (var clip in report.Clips)
            {
                // 位移类动作额外给出 root motion 与由它算出的速度:实测本作的 Walk/Run
                // 自带位移(喵喵 Walk 53cm/1.13s、Run 180cm/0.6s),运行时可以直接用这个速度
                // 推进位置并原地循环播放,不必解析 root motion 曲线
                var moving = clip.Logical.StartsWith("Walk", StringComparison.Ordinal) ||
                             clip.Logical.StartsWith("Run", StringComparison.Ordinal);
                var extra = moving
                    ? $", in_place = {(GlbBuilder.IsInPlace(clip.RootMotionCm) ? "true" : "false")}" +
                      $", root_motion_cm = {Num(clip.RootMotionCm)}" +
                      $", speed_cm_s = {Num(clip.Seconds > 0 ? clip.RootMotionCm / clip.Seconds : 0f)}"
                    : "";
                // 中文标签:NPC 那批动作名(`Dialogue1`/`CrossarmsLoop`)运行时不认识,
                // 配置窗口要拿它显示。没有就不写,运行时退用动作名本身。
                var label = report.ClipLabels?.GetValueOrDefault(clip.Logical) is { } text
                    ? $", label = {Quote(text)}"
                    : "";
                sb.AppendLine(
                    $"  {ClipKey(clip.Logical)} = {{ clip = {Quote(clip.Logical)}, " +
                    $"ms = {(int)MathF.Round(clip.Seconds * 1000f)}, frames = {clip.Frames}{extra}{label} }}");
            }

            if (report.Audio is { } audio)
            {
                // 两节都用 `[forms.clips]` 那把键(动作逻辑名):谁在播这段动作,就出这个声
                if (audio.Voice.Count > 0)
                {
                    sb.AppendLine();
                    sb.AppendLine(
                        "  [forms.voice]   # 叫声:Wwise Pet_Vo_<拼音>.bnk(NPC 是 NPC_Vo_<名>)的事件 → ogg");
                    // 游戏里 -100~100 的 `voice` 属性喂给 Wwise 的 Pet_Vo_Pitch,由 RTPC 曲线变调;
                    // 这两端就是「粗嗓门」「婉转声」。Wwise 的 pitch 本身就是重采样(变调同时变速),
                    // 所以运行时按播放速率 2^(音分/1200) 放就是等价实现
                    sb.AppendLine($"  cents_low = {audio.CentsLow}    # voice = -100(粗嗓门)");
                    sb.AppendLine($"  cents_high = {audio.CentsHigh}   # voice = +100(婉转声)");
                    foreach (var clip in audio.Voice)
                        sb.AppendLine(
                            $"  {clip.Key} = {{ path = {Quote(clip.RelativePath)}, ms = {clip.Ms} }}");
                }

                if (audio.Sfx.Count > 0)
                {
                    sb.AppendLine();
                    sb.AppendLine(
                        "  [forms.sfx]   # 动作音效:Pet_Action_<拼音>.bnk,垫在叫声底下,**不变调**");
                    foreach (var clip in audio.Sfx)
                        sb.AppendLine(
                            $"  {clip.Key} = {{ path = {Quote(clip.RelativePath)}, ms = {clip.Ms} }}");
                }
            }

            if (report.Textures.Count > 0)
            {
                sb.AppendLine();
                sb.AppendLine("  [forms.textures]   # 材质槽(材质名后缀)→ 贴图,D=基色 M=遮罩 ID=分色");
                foreach (var tex in report.Textures)
                    sb.AppendLine(
                        $"  {ClipKey(tex.Name)} = {{ path = {Quote(tex.RelativePath)}, " +
                        $"slot = {Quote(tex.Slot)}, kind = {Quote(tex.Kind)}, " +
                        $"size = [{tex.Width}, {tex.Height}] }}");
            }

            if (report.Materials.Count > 0)
            {
                sb.AppendLine();
                sb.AppendLine("  [forms.materials]   # glb 里的材质名 → 原材质贴图、混合模式与专用着色参数");
                foreach (var mat in report.Materials)
                {
                    var parts = new List<string>();
                    if (mat.BaseColor is not null) parts.Add($"base_color = {Quote(mat.BaseColor)}");
                    parts.Add($"mask_alpha = {(mat.MaskAlpha ? "true" : "false")}");
                    parts.Add($"mask_clip = {Num(mat.MaskClip)}");
                    parts.Add($"blend = {Quote(mat.Blend)}");
                    if (mat.Translucent) parts.Add("translucent = true");
                    // **逐材质写**(不是「有才写」):运行时对旧包没有这个字段时得退回老行为,
                    // 只有明确写出来才敢按它开关描边。`outline` 是开关、`outline_width` 是宽度
                    // (米),同一个来源算出来的两面 —— 前者留着是因为旧包只有它。
                    parts.Add($"outline = {(mat.OutlineWidth > 0f ? "true" : "false")}");
                    parts.Add($"outline_width = {Num(mat.OutlineWidth)}");
                    if (mat.PaintOrder) parts.Add("paint_order = true");
                    // 星点/MatCap/边缘光对所有材质都可能有
                    if (mat.StarTexture is not null)
                    {
                        parts.Add($"star_tex = {Quote(mat.StarTexture)}");
                        parts.Add($"star_tiling = [{Num(mat.StarTiling[0])}, {Num(mat.StarTiling[1])}]");
                        parts.Add($"stick_intensity = {Num(mat.StickIntensity)}");
                        if (mat.StarFakeTrans) parts.Add("star_fake_trans = true");
                        // **不能只在 `StarFakeTrans` 时写。** 那个标记只有 `_Fx` 有,而身体是
                        // `_By` 画的 —— 只发给 `_Fx` 的话 `_By` 退回兜底值 [0,0,1,1],
                        // 于是仍走 UV0、强度 1.0:星点贴在身上、而且浓三十倍(踩过)。
                        // 星点层本来就是跨材质统一的,这套参数跟着一起发。
                        parts.Add($"noise_uv = [{string.Join(", ", mat.NoiseUv.Select(Num))}]");
                        if (mat.StarColor is { } sc)
                            parts.Add($"star_color = [{Num(sc[0])}, {Num(sc[1])}, {Num(sc[2])}]");
                    }
                    if (mat.MatcapTexture is not null && !mat.MaskIsMatcap)
                    {
                        parts.Add($"matcap_tex = {Quote(mat.MatcapTexture)}");
                        if (mat.MatcapColor is { } mc)
                            parts.Add($"matcap_color = [{Num(mc[0])}, {Num(mc[1])}, {Num(mc[2])}]");
                    }
                    // **只认 `Rim Intensity` 大于 1 的。** 这一族的强度普遍写着 1,那更像是
                    // 「没动过的默认值」而不是「开了边缘光」:曜星光那两颗球写着强度 1 + 绿色
                    // `Rim LightColor`,实机里它们是橙的和紫的,照着画怎么都不对。
                    // 全量 946 个带边缘光的材质里只有 3 个强度大于 1(暮星辰的裙子 = 3,青色边)。
                    if (mat.EmissiveColor is { } ec)
                    {
                        parts.Add($"emissive = [{Num(ec[0])}, {Num(ec[1])}, {Num(ec[2])}]");
                        parts.Add($"emissive_intensity = {Num(mat.EmissiveIntensity)}");
                    }
                    if (mat.RimIntensity > 1 && mat.RimColor is { } rc)
                    {
                        parts.Add($"rim_color = [{Num(rc[0])}, {Num(rc[1])}, {Num(rc[2])}]");
                        parts.Add($"rim_intensity = {Num(mat.RimIntensity)}");
                    }
                    // 半透族的输出覆盖率不是贴图 alpha 一项:实机的 ES3.1/Low shader
                    // 还会与高光取 max，再按场景深度差补一层 depth-fade。距离是 UE 厘米。
                    if (mat.Translucent)
                    {
                        parts.Add($"rim_power = {Num(mat.RimPower)}");
                        parts.Add($"rim_soft_edge = {Num(mat.RimSoftEdge)}");
                        parts.Add($"highlight_offset = [{string.Join(", ", mat.HighlightOffset.Select(Num))}]");
                        parts.Add($"highlight_color = [{string.Join(", ", mat.HighlightSpecColor.Select(Num))}]");
                        parts.Add($"highlight_power = {Num(mat.HighlightSpecPower)}");
                        parts.Add($"highlight_intensity = {Num(mat.HighlightSpecIntensity)}");
                        parts.Add($"force_default_opacity = {Num(mat.ForceUseDefaultOpacity)}");
                        parts.Add($"opacity_depth_distance = {Num(mat.OpacityDepthDistance)}");
                        parts.Add($"open_depth_distance = {Num(mat.OpenDepthDistance)}");
                    }
                    if (mat.ObjectTransLow)
                    {
                        parts.Add("object_trans_low = true");
                        if (mat.ObjectTransLightMaskTexture is not null)
                            parts.Add($"light_mask_tex = {Quote(mat.ObjectTransLightMaskTexture)}");
                        if (mat.ObjectTransRampTexture is not null)
                            parts.Add($"ramp_tex = {Quote(mat.ObjectTransRampTexture)}");
                        parts.Add($"object_trans_soft_edge = {Num(mat.ObjectTransSoftEdge)}");
                        parts.Add($"main_color = [{Num(mat.ObjectTransMainColor[0])}, " +
                                  $"{Num(mat.ObjectTransMainColor[1])}, {Num(mat.ObjectTransMainColor[2])}]");
                        parts.Add($"main_bright = {Num(mat.ObjectTransMainBright)}");
                    }
                    if (mat.WaterColor1 is { } w1)
                    {
                        parts.Add($"water_color1 = [{string.Join(", ", w1.Select(Num))}]");
                        if (mat.WaterColor2 is { } w2)
                            parts.Add($"water_color2 = [{string.Join(", ", w2.Select(Num))}]");
                        if (mat.WaterMain is { } wm)
                            parts.Add($"water_main = [{string.Join(", ", wm.Select(Num))}]");
                        parts.Add($"water_caustics = [{string.Join(", ", mat.WaterCaustics.Select(Num))}]");
                        parts.Add($"water_shape = [{string.Join(", ", mat.WaterShape.Select(Num))}]");
                        // caustics 走 `noise_tex` 那个槽(水体材质有基色,但没有色带,槽是空的)。
                        // **这一行必须在这儿,不能靠下面「BaseColor is null」那支** —— 水体有基色。
                        if (mat.NoiseTexture is not null)
                            parts.Add($"noise_tex = {Quote(mat.NoiseTexture)}");
                    }
                    if (mat.FlowTexture is not null)
                    {
                        parts.Add($"flow_tex = {Quote(mat.FlowTexture)}");
                        parts.Add($"flow_power = {Num(mat.FlowPower)}");
                        parts.Add($"flow = [{string.Join(", ", mat.Flow.Select(Num))}]");
                        if (mat.MaskIdTexture is not null)
                        {
                            parts.Add($"mask_id_tex = {Quote(mat.MaskIdTexture)}");
                            parts.Add($"mask_id_range = [{Num(mat.MaskIdRange[0])}, {Num(mat.MaskIdRange[1])}]");
                        }
                    }
                    if (mat.InteriorTexture is not null)
                    {
                        parts.Add($"interior_tex = {Quote(mat.InteriorTexture)}");
                        if (mat.InteriorColor is { } ic)
                            parts.Add($"interior_color = [{Num(ic[0])}, {Num(ic[1])}, {Num(ic[2])}]");
                        parts.Add($"refraction = {Num(mat.Refraction)}");
                        parts.Add($"refract_depth = {Num(mat.RefractDepth)}");
                        parts.Add($"flicker = [{Num(mat.FlickerSpeed)}, {Num(mat.FlickerPower)}]");
                    }
                    if (mat.GlassyInner)
                    {
                        parts.Add("glassy_inner = true");
                        parts.Add($"glassy_flow1 = [{string.Join(", ", mat.GlassyFlowColor01.Select(Num))}]");
                        parts.Add($"glassy_flow2 = [{string.Join(", ", mat.GlassyFlowColor02.Select(Num))}]");
                        parts.Add($"glassy_fresnel = [{string.Join(", ", mat.GlassyFresnelColor.Select(Num))}]");
                        parts.Add($"glassy_noise = [{string.Join(", ", mat.GlassyNoiseParams.Select(Num))}]");
                        parts.Add($"glassy_mask = [{string.Join(", ", mat.GlassyMaskParams.Select(Num))}]");
                    }
                    if (mat.XiaoYou)
                    {
                        // 目标 PS 的 t3。XiaoYou 有 MainTex 基色，不能落到下面仅限纯特效
                        // (`BaseColor is null`) 的 noise_tex 输出分支；漏掉时运行时会绑白图，
                        // flow 永远停在第二个（青色）端点。
                        if (mat.NoiseTexture is not null)
                            parts.Add($"noise_tex = {Quote(mat.NoiseTexture)}");
                        parts.Add("xiaoyou = true");
                        parts.Add($"xiaoyou_base1 = [{string.Join(", ", mat.XiaoYouBaseColor1.Select(Num))}]");
                        parts.Add($"xiaoyou_base2 = [{string.Join(", ", mat.XiaoYouBaseColor2.Select(Num))}]");
                        parts.Add($"xiaoyou_flow1 = [{string.Join(", ", mat.XiaoYouFlowColor1.Select(Num))}]");
                        parts.Add($"xiaoyou_flow2 = [{string.Join(", ", mat.XiaoYouFlowColor2.Select(Num))}]");
                        parts.Add($"xiaoyou_star_color = [{string.Join(", ", mat.XiaoYouStarColor.Select(Num))}]");
                        parts.Add($"xiaoyou_noise_flow = [{string.Join(", ", mat.XiaoYouNoiseFlow.Select(Num))}]");
                        parts.Add($"xiaoyou_shape = [{string.Join(", ", mat.XiaoYouShape.Select(Num))}]");
                        parts.Add($"xiaoyou_star_uv = [{string.Join(", ", mat.XiaoYouStarUv.Select(Num))}]");
                    }
                    if (mat.YutuEar is { } yutu)
                    {
                        parts.Add("yutu_ear = true");
                        if (yutu.BubbleTexture is not null)
                            parts.Add($"yutu_bubble_tex = {Quote(yutu.BubbleTexture)}");
                        if (yutu.DistortTexture is not null)
                            parts.Add($"yutu_distort_tex = {Quote(yutu.DistortTexture)}");
                        if (yutu.FlowTexture is not null)
                            parts.Add($"yutu_flow_tex = {Quote(yutu.FlowTexture)}");
                        parts.Add($"yutu_bubble_color = [{string.Join(", ", yutu.BubbleColor.Select(Num))}]");
                        parts.Add($"yutu_flow_color = [{string.Join(", ", yutu.FlowColor.Select(Num))}]");
                        parts.Add($"yutu_fresnel_color = [{string.Join(", ", yutu.FresnelColor.Select(Num))}]");
                        parts.Add($"yutu_inner_color = [{string.Join(", ", yutu.InnerColor.Select(Num))}]");
                        parts.Add($"yutu_overall_color = [{string.Join(", ", yutu.OverallColor.Select(Num))}]");
                        parts.Add($"yutu_ramp_color = [{string.Join(", ", yutu.RampColor.Select(Num))}]");
                        parts.Add($"yutu_top_color = [{string.Join(", ", yutu.TopColor.Select(Num))}]");
                        parts.Add($"yutu_bubble_shape = [{string.Join(", ", yutu.BubbleShape.Select(Num))}]");
                        parts.Add($"yutu_flow_shape = [{string.Join(", ", yutu.FlowShape.Select(Num))}]");
                        parts.Add($"yutu_light_shape = [{string.Join(", ", yutu.LightShape.Select(Num))}]");
                        parts.Add($"yutu_top_shape = [{string.Join(", ", yutu.TopShape.Select(Num))}]");
                    }
                    if (mat.FakeFluid is { } fluid)
                    {
                        parts.Add("fake_fluid = true");
                        parts.Add($"fluid_edge_color = [{string.Join(", ", fluid.EdgeColor.Select(Num))}]");
                        parts.Add($"fluid_fresnel_color = [{string.Join(", ", fluid.FresnelColor.Select(Num))}]");
                        parts.Add($"fluid_plane_color = [{string.Join(", ", fluid.PlaneColor.Select(Num))}]");
                        parts.Add($"fluid_gradient1 = [{string.Join(", ", fluid.Gradient1.Select(Num))}]");
                        parts.Add($"fluid_gradient2 = [{string.Join(", ", fluid.Gradient2.Select(Num))}]");
                        parts.Add($"fluid_height_tiling = [{string.Join(", ", fluid.HeightTiling.Select(Num))}]");
                        parts.Add($"fluid_plane_axis = [{string.Join(", ", fluid.PlaneAxis.Select(Num))}]");
                        parts.Add($"fluid_plane_center = [{string.Join(", ", fluid.PlaneCenter.Select(Num))}]");
                        parts.Add($"fluid_body_shape = [{string.Join(", ", fluid.BodyShape.Select(Num))}]");
                        parts.Add($"fluid_gradient_shape = [{string.Join(", ", fluid.GradientShape.Select(Num))}]");
                        parts.Add($"fluid_top_shape = [{string.Join(", ", fluid.TopShape.Select(Num))}]");
                    }
                    if (mat.MatcapMasked is { } masked)
                    {
                        parts.Add("matcap_masked = true");
                        parts.Add($"matcap_masked_base = [{string.Join(", ", masked.BaseColor.Select(Num))}]");
                        parts.Add($"matcap_masked_light_ramp = [{string.Join(", ", masked.LightRampColor.Select(Num))}]");
                        parts.Add($"matcap_masked_flat = [{string.Join(", ", masked.FlatEmissiveColor.Select(Num))}]");
                        parts.Add($"matcap_masked_main = [{string.Join(", ", masked.MainColor.Select(Num))}]");
                        parts.Add($"matcap_masked_selection = [{string.Join(", ", masked.SelectionColor.Select(Num))}]");
                        parts.Add($"matcap_masked_rim = [{string.Join(", ", masked.RimShape.Select(Num))}]");
                        parts.Add($"matcap_masked_surface = [{string.Join(", ", masked.SurfaceShape.Select(Num))}]");
                    }
                    if (mat.FairyBall is { } fairy)
                    {
                        parts.Add("fairy_ball = true");
                        if (fairy.Matcap is not null)
                            parts.Add($"fairy_matcap_tex = {Quote(fairy.Matcap)}");
                        parts.Add($"fairy_base = [{string.Join(", ", fairy.BaseColor.Select(Num))}]");
                        parts.Add($"fairy_matcap_color = [{string.Join(", ", fairy.MatcapColor.Select(Num))}]");
                        parts.Add($"fairy_rim_dark = [{string.Join(", ", fairy.RimDarkColor.Select(Num))}]");
                        parts.Add($"fairy_rim_light = [{string.Join(", ", fairy.RimLightColor.Select(Num))}]");
                        parts.Add($"fairy_main = [{string.Join(", ", fairy.MainColor.Select(Num))}]");
                        parts.Add($"fairy_shape = [{string.Join(", ", fairy.Shape.Select(Num))}]");
                    }
                    // 每个键只许出现一次:重复键 TOML 直接解析失败(opacity/flow 都踩过)
                    parts.Add($"opacity = {Num(mat.Opacity)}");
                    // 基色 alpha 就是不透明度(见 MaterialInfo.AlphaIsOpacity)
                    if (mat.AlphaIsOpacity) parts.Add("alpha_opacity = true");
                    // 父链对所有材质都记:它是「这一族该怎么画」的唯一线索
                    // (如 `M_FX_Fire_Mat` = 火焰、`..._Trans_XingGuang_WPO` = 需要顶点位移的纱)
                    parts.Add($"parents = [{string.Join(", ", mat.ParentChain.Select(Quote))}]");
                    if (mat.BaseColor is null)
                    {
                        // 特效层:主色 + 卷动 + 遮罩/噪声,运行时靠这些近似画出火焰/水壳/光晕
                        if (mat.Tint is { } t)
                            parts.Add($"tint = [{Num(t[0])}, {Num(t[1])}, {Num(t[2])}, {Num(t[3])}]");
                        parts.Add($"glow = {Num(mat.Glow)}");
                        if (mat.FlowTexture is null)
                            parts.Add($"flow = [{string.Join(", ", mat.Flow.Select(Num))}]");
                        if (mat.MaskTexture is not null)
                        {
                            parts.Add($"mask_tex = {Quote(mat.MaskTexture)}");
                            if (mat.MaskIsMatcap) parts.Add("mask_matcap = true");
                        }
                        if (mat.NoiseTexture is not null) parts.Add($"noise_tex = {Quote(mat.NoiseTexture)}");
                    }
                    sb.AppendLine($"  {ClipKey(mat.Name)} = {{ {string.Join(", ", parts)} }}");
                }
            }

            if (report.Warnings.Count > 0)
            {
                sb.AppendLine();
                sb.AppendLine("  [forms.report]   # 导出时的缺口,运行时按需降级而不是报错");
                sb.AppendLine($"  warnings = [{string.Join(", ", report.Warnings.Select(Quote))}]");
            }
        }

        return sb.ToString();
    }

    /// PETBASE_CONF.move_type 是中文(步行/浮游/…),转成运行时用的枚举。
    private static string Locomotion(string moveType) => moveType switch
    {
        "步行" => "ground",
        "浮游" => "hover",
        "游泳" or "游动" => "swim",
        "飞行" => "fly",
        _ => "ground",
    };

    /// TOML 裸键只允许 [A-Za-z0-9_-],其余情况加引号。
    private static string ClipKey(string name) =>
        name.All(c => char.IsAsciiLetterOrDigit(c) || c is '_' or '-') ? name : Quote(name);

    private static string Quote(string s) =>
        "\"" + s.Replace("\\", "\\\\").Replace("\"", "\\\"").Replace("\n", "\\n") + "\"";

    private static string Num(float v) => v.ToString("0.####", CultureInfo.InvariantCulture);
}
