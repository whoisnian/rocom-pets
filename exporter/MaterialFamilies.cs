// **七个原生材质族各自的参数**,从 `MaterialInfo` 拆出来的一半。
//
// 这些属性只做一件事:把「这份材质属不属于某个族」和「这个族要哪几个参数」翻译成
// manifest 里的字段。族的判据一律看**父链**(`ParentChain`),不看名字;每个参数都写清
// 它在目标排列的汇编里是哪一条、以及**全库有几份材质真的设过它**(设了 0 的另算)——
// 「代码在字节码里 ≠ 这一层可见」是这本子里踩得最多的坑。
//
// 另一半(基色 / 不透明度 / 描边 / 炫彩与赛季 / 查参数的四层回退)在 Materials.cs。
// 两份是同一个 `partial record`。

using CUE4Parse.UE4.Assets.Exports.Material;

namespace RocomPets.Export;

public partial record MaterialInfo
{
    /// 遮罩是不是 MatCap。**这决定采样方式**:matcap 要按视空间法线采(球面反射查找表),
    /// 拿网格 UV 采会变成一块块的斑,水灵的水膜就是这么糊掉的。
    public bool MaskIsMatcap =>
        !Textures.ContainsKey("FuildMask")
        && !Textures.ContainsKey("Mask") && !Textures.ContainsKey("MaskTex")
        && !Textures.ContainsKey("BaseMap") && !Textures.ContainsKey("Base Color")
        && (Textures.ContainsKey("MatCap") || Textures.ContainsKey("MatCapTex"));

    /// 果冻内胆使用的独立材质图。它不是通用“纯特效层”:原 shader 71636 输出 alpha=1,
    /// 用物体空间折射坐标三向采 `GlassyNoiseTex`,再做 flow 两色与 Fresnel 色的两次 lerp。
    public bool IsGlassyInner =>
        ParentChain.Any(p => p.Equals("M_ShuiMu_ByIn", StringComparison.OrdinalIgnoreCase));

    /// 小灵面家族专用的 `MI_P_Object_XiaoYou`。目标 Low PS 32511 输出 alpha=1，
    /// 并用 MainTex/NoiseTex/StarTex 与 COLOR_0 合成，不是通用半透或 VFX。
    public bool IsXiaoYou =>
        ParentChain.Any(p => p.Equals("MI_P_Object_XiaoYou", StringComparison.OrdinalIgnoreCase));

    /// 莫比乌乌内层使用的独立不透明材质。目标 Low PS 6037 的四张材质贴图依次是
    /// Bubble Texture / DistortTex / FlowTex / BaseColor；后三张只存在于根材质默认值。
    public bool IsYutuEar =>
        ParentChain.Any(p => p.Equals("M_Gra_Yutu_Ear_Lighting", StringComparison.OrdinalIgnoreCase));

    /// 克莱因龙的玻璃液体材质。游戏资产里的 `Fulid/Fuild` 就是这个拼写，不能按正确的
    /// Fluid 去匹配；目标 Low PS 42877 直接以 COLOR_0.g 乘最终覆盖率。
    public bool IsFakeFluid =>
        ParentChain.Any(p => p.Contains("FakeFulid", StringComparison.OrdinalIgnoreCase));

    /// **幻星族那两颗球**:`MI_P_Object_Trans_XingGuang_Fresnel`。
    /// 全库 3393 份材质里**只有暮星辰 `_Fx2` 一份**用它(`--probe-material FIND:` 查过)。
    ///
    /// 用户报的是「暮星辰两颗球颜色差距最大,实机一个偏紫黑、一个偏粉紫」,而我们两颗都是黑的。
    /// 目标 PS **53466**(`Num/lod=0/dsid=0`,resource `6CCB83FD…`)第 151~209 行给出全部:
    ///
    /// ```text
    /// fres = pow(max(1 − max(N顶点·V, 0), 1e-4), Range) × 0.96 − 0.46
    /// t    = smoothstep(saturate(fres / (Soft × 0.1))) × Int
    /// col  = UseVertexColorG ≥ 0.5 ? lerp(Color02, Color, 顶点色G) : Color
    /// w    = lerp(max(基色a, 高光, MatCap, 边缘光), saturate(t), BottomLayer/TopLayer Opacity)
    /// 发光 = lerp(发光, t × col, OpenEmissiveBlend × w)      ← **替换**,不是相加
    /// 不透明度 += OpenOpacityAdd × w
    /// ```
    ///
    /// **两颗球的差别全在顶点色 G**:它们绑在两根不同的骨骼上
    /// (`Bone_Qhuan_M_00` 的那颗 G=0、`Bone_Qhuan_M_03` 的那颗 G=1),UV / 遮罩 / 基色**完全一样**。
    /// 所以一颗取 `Color`(0.148, 0.059, 0.22 深紫)、另一颗取 `Color02`(0, 0.562, 1.5 青)。
    ///
    /// `cb6[12]`/`cb6[13]` ↔ `Color`/`Color02` 是按 `vector-slot` 字节码定的
    /// (`vector-slot[12] = 04 06 00 …` ⇒ vector-param[6] = `Color`),不是猜的;
    /// `v2 = COLOR0` 由 `dxbcsig.py` 的 ISGN 查实。
    public bool IsXingGuangFresnel =>
        ParentChain.Any(p => p.Contains("XingGuang_Fresnel", StringComparison.OrdinalIgnoreCase));

    /// `[Color.rgb, Int]`
    public float[] XingGuangColor =>
    [
        ..(FirstVector("Color") ?? RootVector("Color") ?? [1f, 1f, 1f, 1f])[..3],
        RootScalar("Int", 1f),
    ];

    /// `[Color02.rgb, OpenEmissiveBlend]`
    public float[] XingGuangColor02 =>
    [
        ..(FirstVector("Color02") ?? RootVector("Color02") ?? [1f, 1f, 1f, 1f])[..3],
        RootScalar("OpenEmissiveBlend", 0f),
    ];

    /// `[OpenOpacityAdd, UseOpacityMask, InversionMask, ForceUseDefOpacity]` —— 这一层
    /// 影响的是**不透明度**那一路(汇编第 197~207、304 行)。暮星辰只有第一格非零(0.15),
    /// 其余三格是根默认 0;四格都导出来,将来有别的材质设了值时能在 manifest 里直接看见。
    public float[] XingGuangAlpha =>
    [
        RootScalar("OpenOpacityAdd", 0f), RootScalar("UseOpacityMask", 0f),
        RootScalar("InversionMask", 0f), RootScalar("ForceUseDefOpacity", 0f),
    ];

    /// `[Range, Soft, UseVertexColorG, BottomLayer/TopLayer Opacity]`。
    /// **后两项别省**:`UseVertexColorG` = 0 时两颗球同色(那才是「差距最小」那一档),
    /// 而最后那格 = 1 时不透明度整个由这条菲涅尔接管、`max` 链只剩加性的一份。
    public float[] XingGuangShape =>
    [
        RootScalar("Range", 15f), RootScalar("Soft", 0.5f),
        RootScalar("UseVertexColorG", 0f),
        RootScalar("BottomLayer Opaciy or TopLayer Opacity", 1f),
    ];

    /// **`M_P_BackRenderEmissive`:只画一侧的不透明背板。**
    ///
    /// 目标排列(莫比乌乌 `_Fx` 的 `quality=Num / lod=0 / dsid=0`,resource `C2685A88…`,
    /// PS 48913)整条链很短,而且**没有任何光照**——它是 unlit 的:
    ///
    /// ```text
    /// base  = lerp(RGB强度(Dark), RGB强度(Light), BaseMap)      ← 电平重映射
    /// base += 饱和度变化 × (luminance(base) − base)              ← 逐通道去饱和
    /// uv    = lerp(TEXCOORD0, TEXCOORD2, saturate(UVNumber))
    /// uv    = lerp(uv, 极坐标(uv − RadialCenter), OpenRadialUV)
    /// flow  = pow(FlowTexture(uv × FlowTiling + frac(时间 × FlowSpeed)), FlowPower) × 顶点色.b
    /// color = lerp(base, UVFlowColor × FlowInt, flow)            ← **替换**,不是相加
    /// color = color × MainColor × MainBright
    /// out   = sqrt(color × 曝光),  alpha = 1
    /// ```
    ///
    /// **链上被 0 乘掉的两层已经查过了**(这本子里同一个坑踩过三次,读出公式必查每个因子):
    /// `FresnelIntensity` 与 `Glow Intensity` 的根默认都是 **0**,而全库 16 份覆盖过
    /// `FresnelIntensity` 的材质(小火苗 / 水蓝蓝 / 落大蟹)**没有一份在这一族里** ⇒
    /// 菲涅尔层与 Glow 层恒为 0,不实现。`Flat_EmissiveRatio` = 0、`SelectionColor.a` = 0,
    /// 那两条 lerp 也是恒等。
    ///
    /// **哪一面**:材质的 `BasePropertyOverrides` 写着 `TwoSided = True`(光栅器两面都出),
    /// 而 PS 自己按 `SV_IsFrontFace` 丢掉一面:
    ///
    /// ```text
    /// a   = saturate(场景淡出 × (正面 ? +1 : −1))     ← 背面恒 0
    /// b   = BackFaceOnly × (a − 1) + 1                 ← 根默认 1 ⇒ b = a
    /// cov = saturate(UseBackFace × ((1 − a) − b) + b)  ← 0 ⇒ a(只留正面);1 ⇒ 1 − a(只留背面)
    /// 按屏幕 4×4 抖动阈值 discard
    /// ```
    ///
    /// 也就是「两面 + 着色器自己剔一面」= 直接剔另一面。全库只有**莫比乌乌**把
    /// `UseBackFace` 设成 1(普查 3393 份,只此一份)⇒ 只有它画背面,其余 11 份画正面。
    /// 画背面正是用户描述的那块「白色基底,避免透出背景」:壳中段是透明窗口
    /// (`_By` 基色 alpha 在 z∈[−0.3,+0.3] 上中位 0.000),窗口后面就是这块背板。
    ///
    /// **没实现的一项**:`SwingIntensity`/`Swing Direction`/`SwingNum`/`SpeedR` 是顶点着色器里的
    /// 摆动(WPO),我们没有 WPO 这一路。莫比乌乌的 `SwingIntensity` = (0,0,0) ⇒ 对它无影响;
    /// 电环(0.5,0.5,2)与柴渣虫(4.5)那几只会缺这段摆动。
    public bool IsBackRender =>
        ParentChain.Any(p => p.Equals("M_P_BackRenderEmissive", StringComparison.OrdinalIgnoreCase));

    /// `[RGB强度(Dark), RGB强度(Light), saturate(UVNumber), UseBackFace]`。
    ///
    /// `UVNumber` 在汇编里选的是 **TEXCOORD2**(VS 的 `o5 = ATTRIBUTE7`,签名查实),
    /// 不是 UV1。源网格最多两套 UV(见 docs/findings.md「缺逐顶点烘焙项」那条),
    /// 所以那一支采到的是 `(0,0)` —— 运行时照这个做,别拿 UV1 顶替。
    public float[] BackRenderLevel =>
    [
        RootScalar("RGB强度(Dark)", 0f),
        RootScalar("RGB强度(Light)", 1f),
        Math.Clamp(RootScalar("UVNumber", 0f), 0f, 1f),
        RootScalar("UseBackFace", 0f),
    ];

    /// `[饱和度变化.rgb, FlowPower]`。这个参数名有点误导:它是**逐通道**的去饱和量,
    /// 根默认 (0.3, 0.59, 0.11) 正好是亮度权重,而实例会给负值
    /// (莫比乌乌 (−0.12, 0, −0.292) ⇒ 反而加饱和)。
    public float[] BackRenderSaturation =>
    [
        ..(FirstVector("饱和度变化") ?? RootVector("饱和度变化")
           ?? [0f, 0f, 0f, 0f])[..3],
        RootScalar("FlowPower", 1f),
    ];

    /// `[UVFlowColor.rgb, FlowInt]` —— 流动层要**替换**成的颜色。
    public float[] BackRenderFlowColor =>
    [
        ..(FirstVector("UVFlowColor") ?? RootVector("UVFlowColor")
           ?? [1f, 1f, 1f, 0f])[..3],
        RootScalar("FlowInt", 1f),
    ];

    /// `[Flow_U_Speed, Flow_V_Speed, Flow_U_Tiling, Flow_V_Tiling]`。
    public float[] BackRenderFlow =>
    [
        RootScalar("Flow_U_Speed", 0f), RootScalar("Flow_V_Speed", 0f),
        RootScalar("Flow_U_Tiling", 1f), RootScalar("Flow_V_Tiling", 1f),
    ];

    /// `[RadialCenterOffsetX, RadialCenterOffsetY, OpenRadialUV, -]`;
    /// 第四位留给流动贴图的 sRGB 旗标(见 Program.cs 的 `BackRenderRadialWithSrgb`)。
    public float[] BackRenderRadial =>
    [
        RootScalar("RadialCenterOffsetX", 0.5f), RootScalar("RadialCenterOffsetY", 0.5f),
        RootScalar("OpenRadialUV", 0f), 0f,
    ];

    /// `[MainColor.rgb × MainBright, -]`;第四位留给**基色贴图**的 sRGB 旗标。
    public float[] BackRenderMain
    {
        get
        {
            var c = FirstVector("MainColor") ?? RootVector("MainColor")
                    ?? [1f, 1f, 1f, 1f];
            var k = RootScalar("MainBright", 1f);
            return [c[0] * k, c[1] * k, c[2] * k, 0f];
        }
    }

    /// 这一族自己的流动贴图。根默认那张 `TestResBlack` 是纯黑(⇒ 流动层恒 0),
    /// 所以只认实例链上显式设过的那份 —— 莫比乌乌没设,它只出基色。
    public string? BackRenderFlowTexture => IsBackRender ? FirstTexture("FlowTexture") : null;

    /// 克莱因龙外壳使用的 MatCap 遮罩材质。目标 Low color PS 19654 先算
    /// `BaseColor * LightRamp + MatCap`，再接 Rim/FlatEmissive/Main/Selection，
    /// 最终 alpha 恒为 1；基础 OpacityMask 由同 resource 的 Early-Z depth PS 15293
    /// 以 `max(MatCap亮度,Fresnel) >= 0.3333` 写深度。过去把它当“无基色纯特效”并按
    /// HDR tint 判成加色层，会绕过遮罩且在所有内层液体之后盖上一整层白膜。
    public bool IsMatcapMasked =>
        ParentChain.Any(p => p.Equals("M_P_MatCap_Masked", StringComparison.OrdinalIgnoreCase));

    public float[] MatcapMaskedBaseColor =>
        FirstVector("BaseColor")
        ?? RootVector("BaseColor")
        ?? [1f, 1f, 1f, 0f];

    public float[] MatcapMaskedLightRamp =>
        FirstVector("LightRampColor")
        ?? RootVector("LightRampColor")
        ?? [1f, 1f, 1f, 0f];

    public float[] MatcapMaskedFlatEmissive =>
        FirstVector("Flat_EmissiveColor")
        ?? RootVector("Flat_EmissiveColor")
        ?? [1f, 1f, 1f, 1f];

    public float[] MatcapMaskedMainColor =>
        FirstVector("MainColor")
        ?? RootVector("MainColor")
        ?? [1f, 1f, 1f, 1f];

    public float[] MatcapMaskedSelectionColor =>
        FirstVector("SelectionColor")
        ?? RootVector("SelectionColor")
        ?? [0f, 0f, 0f, 0f];

    /// PS 19654 中 cb3[5].xy / cb3[13].z / cb3[14].w 的确切参数映射。
    public float[] MatcapMaskedRimShape =>
    [
        Scalar("Rim Power", RootScalar("Rim Power", 0.4f)),
        Scalar("Rim Soft Edge", RootScalar("Rim Soft Edge", 0.3f)),
        Scalar("Rim Intensity", RootScalar("Rim Intensity", 0f)),
        Scalar("FresnelPow", RootScalar("FresnelPow", 3f)),
    ];

    /// PS 19654 的 Flat/Main 与 Xray 门。最后一项是 Xray/Common_Xray 的 max，
    /// 对应 uniform preshader scalar-slot[11] 的原式。
    public float[] MatcapMaskedSurfaceShape =>
    [
        RootScalar("Flat_EmissiveIntensity", 1f),
        RootScalar("Flat_EmissiveRatio", 0f),
        RootScalar("MainBright", 1f),
        Math.Max(RootScalar("Xray", 0f), RootScalar("Common_Xray", 0f)),
    ];

    /// 沙漏 / 水晶球外面那层玻璃壳(等一等鸭、落陨星兔、逗逗、白发懒人,全库 5 个材质)。
    ///
    /// **它不是 `M_P_Object_Trans` 的一支**:自己一张材质图、自己一套参数名
    /// (`RimArea`/`RimSmoothness`/`MatCapColor`/`RimDarkColor`),而且**整条链上没有固有色、
    /// 也不吃光照** —— 目标 PS 52626(整族只有一个 cooked resource,没有静态开关)只采一张
    /// 贴图,就是 MatCap。原来它落在通用半透那条路上,`Opacity` 一项就决定了画成实心还是全透,
    /// 两头都不对:兜 1 是一坨白(挡住沙子),取根默认 0 是整层看不见。
    public bool IsFairyBall =>
        ParentChain.Any(p => p.Equals("M_FairyBall_BallFront", StringComparison.OrdinalIgnoreCase));

    /// MatCap 查找表。这一族**不看 `是否使用MatCap` 开关**(它压根没有静态开关),
    /// MatCap 是它唯一的贴图,无条件要。
    public string? FairyBallMatcap => IsFairyBall ? FirstTexture("MatCap") : null;

    /// PS 52626 第 81 行 `MatCapColor.rgb × MatCap + BaseColor.rgb`;两个 alpha 也都有用:
    /// `MatCapColor.a` 是 MatCap 亮度换算成覆盖率的增益,`BaseColor.a` 与 `Opacity` 相加是底。
    public float[] FairyBallBaseColor =>
        FirstVector("BaseColor")
        ?? RootVector("BaseColor")
        ?? [1f, 1f, 1f, 0f];

    public float[] FairyBallMatcapColor =>
        FirstVector("MatCapColor")
        ?? RootVector("MatCapColor")
        ?? [1f, 1f, 1f, 0.1f];

    /// 边缘光的暗/亮两色,第 83–84 行按 `N·L` 在两者之间取;alpha 是这一层自己的覆盖率。
    public float[] FairyBallRimDark =>
        FirstVector("RimDarkColor")
        ?? RootVector("RimDarkColor")
        ?? [1f, 1f, 1f, 1f];

    public float[] FairyBallRimLight =>
        FirstVector("RimLightColor")
        ?? RootVector("RimLightColor")
        ?? [1f, 1f, 1f, 1f];

    /// 第 94 行的整体色,`MainColor.rgb`(xyz)+ `MainBright`(w)。五个实例都是白 × 1。
    public float[] FairyBallMainColor
    {
        get
        {
            var main = FirstVector("MainColor")
                       ?? RootVector("MainColor")
                       ?? [1f, 1f, 1f, 1f];
            return [main[0], main[1], main[2], RootScalar("MainBright", 1f)];
        }
    }

    /// 边缘光的形状 + 覆盖率的底:`[RimArea, RimSmoothness, Opacity, 1]`。
    /// 最后一位是这一族的开关(运行时放在 `family11.w`,见 gpu.rs)。
    ///
    /// `[RimArea, RimSmoothness, Opacity, -]`。汇编第 45–56 行是
    /// **`smoothstep(0.5 − RimSmoothness, 0.5 + RimSmoothness, pow(1 − N·V, RimArea))`**,
    /// 见 pet/shader/60-families.wgsl 的 `shade_fairy_ball`。
    ///
    /// **这两格以前是配错的**,原注释写着「cooked 参数表的名字对不上,按实机截图定」——
    /// 真因是 CUE4Parse 读 `UniformScalarParameters` 的步长差 4 字节,那张表**只有第 0 条
    /// 名字是对的**(见 docs/findings.md「标量参数名那条终于通了」)。补丁修掉之后
    /// `cb3[19]` 四格逐个读出来是 `RimSmoothness` / `RimArea` / `0.5 + RimSmoothness` /
    /// `0.5 − RimSmoothness` —— 一点都不用猜,两个名字也到这儿才讲得通。
    public float[] FairyBallShape =>
    [
        RootScalar("RimArea", 2f),
        RootScalar("RimSmoothness", 0.05f),
        Opacity,
        1f,
    ];

    public string? YutuBubbleTexture => IsYutuEar ? FirstTexture("Bubble Texture") : null;
    public string? YutuDistortTexture => IsYutuEar
        ? RootDefaults?.Textures.GetValueOrDefault("DistortTex") : null;
    public string? YutuFlowTexture => IsYutuEar
        ? RootDefaults?.Textures.GetValueOrDefault("FlowTex") : null;

    public float[] YutuBubbleColor => FirstVector("Bubble Color") ?? [0f, 0.508735f, 1f, 1f];
    public float[] YutuFlowColor => FirstVector("FlowColor") ?? [1f, 1f, 1f, 0f];
    public float[] YutuFresnelColor => FirstVector("FresnelCol") ?? [1f, 1f, 1f, 0f];
    public float[] YutuInnerColor => FirstVector("InColor") ?? [1f, 1f, 1f, 1f];
    public float[] YutuOverallColor => FirstVector("OverAllColor") ?? [1f, 1f, 1f, 0f];
    public float[] YutuRampColor => FirstVector("RampColor") ?? [1f, 1f, 1f, 0f];
    public float[] YutuTopColor => FirstVector("TopColor2") ?? [0f, 0f, 0f, 0f];
    public float[] YutuBubbleShape =>
    [
        Scalar("Bubble Speed 1", 0.05f), Scalar("Bubble Speed 2", 0.05f),
        Scalar("Bubbles Scale", 5f), Scalar("FlowDistort", 0.2f),
    ];
    public float[] YutuFlowShape =>
    [
        Scalar("U_Speed1", 0.1f), Scalar("V_Speed1", -0.5f),
        Scalar("U_Tiling1", 1f), Scalar("V_Tiling1", 0.8f),
    ];
    public float[] YutuLightShape =>
    [
        Scalar("Flow Int", 0.3f), Scalar("Fres ExponentIn", 1f),
        Scalar("Fres Int", 1f), Scalar("InColor Size", 0f),
    ];
    public float[] YutuTopShape =>
    [
        Scalar("TopColor Offset", 0f), Scalar("TopColor Size", 0f),
        Scalar("TopColor Size2", 1f), Scalar("Contrast Soft 软硬", 0f),
    ];

    public float[] FluidEdgeColor => FirstVector("EdgeColor") ?? [1f, 1f, 1f, 1f];
    public float[] FluidFresnelColor => FirstVector("FresnelColor") ?? [1f, 1f, 1f, 0f];
    public float[] FluidPlaneColor => FirstVector("FulidPlaneColor") ?? [1f, 1f, 1f, 1f];
    public float[] FluidGradient1 => FirstVector("GradientColor01") ?? [1f, 1f, 1f, 1f];
    public float[] FluidGradient2 => FirstVector("GradientColor02") ?? [1f, 1f, 1f, 1f];
    public float[] FluidHeightTiling => FirstVector("HeightNoiseTiling") ?? [1f, 1f, 0f, 0f];
    public float[] FluidPlaneAxis => FirstVector("PlaneAxis") ?? [0f, 0f, 1f, 1f];
    public float[] FluidPlaneCenter => FirstVector("PlaneCenter") ?? [0f, 0f, 0f, 0f];
    public float[] FluidBodyShape =>
    [
        Scalar("BodyEdgeArea", 5f), Scalar("BodyEdgeOffset", 0.8f),
        Scalar("BodyEdgeSmooth", 0.1f), Scalar("HeightNoiseIntensity", 5f),
    ];
    public float[] FluidGradientShape =>
    [
        Scalar("GradientOffset", 0.5f), Scalar("GradientSmooth", 0.01f),
        Scalar("FresnelOffset", 0.3f), Scalar("FresnelSmooth", 0.2f),
    ];
    public float[] FluidTopShape =>
    [
        Scalar("TopEdgeOffset", 0.3f), Scalar("TopEdgeSmooth", 0.05f),
        RootScalar("RippleOpacity", 1f), Scalar("FadeDistance", 30f),
    ];

    public float[] XiaoYouBaseColor1 =>
        FirstVector("BaseColor1") ?? [0f, 0f, 0f, 1f];

    public float[] XiaoYouBaseColor2 =>
        FirstVector("BaseColor2") ?? [0f, 0f, 0f, 1f];

    public float[] XiaoYouFlowColor1 =>
        FirstVector("FlowNoiseColor1") ?? [0f, 0f, 0f, 1f];

    public float[] XiaoYouFlowColor2 =>
        FirstVector("FlowNoiseColor2") ?? [0f, 0f, 0f, 1f];

    public float[] XiaoYouStarColor =>
        FirstVector("StarColor") ?? [0f, 0f, 0f, 0f];

    /// 原材质两组 panner 速度，参数名与目标 PS 使用的两条时间坐标一一对应。
    public float[] XiaoYouNoiseFlow =>
    [
        Scalar("USpeedTex01"), Scalar("VSpeedTex01"),
        Scalar("USpeedTex02"), Scalar("VSpeedTex02"),
    ];

    /// [两层 flow 强度, RG 星点阈值强度, 闪烁速度]。
    public float[] XiaoYouShape =>
    [
        Scalar("FlowNoseInt1", 1f), Scalar("FlowNoiseInt2", 1f),
        Scalar("Star_RG_Int", 1f), Scalar("Star_RG_TwinkleSpeed", 0f),
    ];

    /// `Star_RG_UV_Control` = **(平铺U, 速度U, 平铺V, 速度V)**。
    /// 两个速度在 preshader 里**除以 100** 才进 cb(`scalar-slot[15]/[17]` 的字节码尾部
    /// 是 `02 <100> 08`),运行时按同一条换算。
    public float[] XiaoYouStarUv =>
        FirstVector("Star_RG_UV_Control") ?? [1f, 0f, 1f, 0f];

    /// 同上,第二层(`Star_BA_*`)。星点是**两层**:RG 那层用 `StarTex` 的 R(相位)与 G(遮罩)、
    /// BA 那层用 B(相位)与 A(遮罩),各有自己的 UV 控制、强度、闪烁速度与阈值。
    /// 见 pet/shader/60-families.wgsl 的 `shade_xiaoyou`(PS 41540 第 91~117 行)。
    public float[] XiaoYouStarUv2 =>
        FirstVector("Star_BA_UV_Control") ?? [1f, 0f, 1f, 0f];

    /// 两层星点各自的 **[阈值, 强度]**:`[Star_RG_DarkTime, Star_BA_DarkTime,
    /// Star_BA_Int, Star_BA_TwinkleSpeed]`(RG 那层的强度与速度在 `XiaoYouShape` 的后两位)。
    /// `DarkTime`(参数名里带着「数值越大, 黑的时间越长」)全库没人覆盖,根默认 0。
    public float[] XiaoYouStar2 =>
    [
        Scalar("Star_RG_DarkTime", 0f), Scalar("Star_BA_DarkTime", 0f),
        Scalar("Star_BA_Int", 1f), Scalar("Star_BA_TwinkleSpeed", 0f),
    ];

    public string? NoiseTexture =>
        (IsYutuEar ? YutuDistortTexture : null)
        ?? (IsFakeFluid ? FirstTexture("BubbleColorLutTex") : null)
        ?? FirstTexture("Noise", "NoiseTex", "FlowTexture", "GlassyNoiseTex")
        ?? (IsGlassyInner ? RootDefaults?.Textures.GetValueOrDefault("GlassyNoiseTex") : null);

    public float[] GlassyFlowColor01 =>
        FirstVector("GlassyFlowColor01")
        ?? RootVector("GlassyFlowColor01")
        ?? [1f, 1f, 1f, 1f];

    public float[] GlassyFlowColor02 =>
        FirstVector("GlassyFlowColor02")
        ?? RootVector("GlassyFlowColor02")
        ?? [1f, 1f, 1f, 1f];

    public float[] GlassyFresnelColor =>
        FirstVector("GlassyFresnelColor")
        ?? RootVector("GlassyFresnelColor")
        ?? [1f, 1f, 1f, 1f];

    /// [速度, UV 尺度, GlassyNoiseRefract 原参数, 深度]。四个槽逐一对应 71636 的
    /// cb4[7]/[18]；其中 shader 实际读取的折射 eta 是 preshader 求出的
    /// `1 / (1 + GlassyNoiseRefract)`，运行时保留原参数是为了兼容已经导出的包。
    public float[] GlassyNoiseParams =>
    [
        RootScalar("GlassyNoiseSpeed", -0.1f),
        RootScalar("GlassyNoiseUVScale", 1f),
        RootScalar("GlassyNoiseRefract", 0.2f),
        RootScalar("GlassyNoiseDepth", 30f),
    ];

    /// [Fresnel 次数,阈值起点,过渡宽度,三向混合强度]。
    public float[] GlassyMaskParams =>
    [
        RootScalar("GlassyNoiseFresnelMaskPow", 1f),
        RootScalar("GlassyNoiseFresnelMaskOffset", 0.7f),
        RootScalar("GlassyNoiseFresnelMaskSmooth", 0.1f),
        RootScalar("GlassyNoiseTriPlannarBlendInt", 0f),
    ];
}
