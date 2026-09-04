// 解析宠物材质实例:拿到**每个材质槽真正用哪张贴图**、以及混合模式/遮罩阈值。
//
// 为什么需要:原来贴图是按命名约定硬接的(材质名后缀 `_By/_Es/_Mh` ↔ `T_<Asset>_<槽>_D`),
// 于是两类东西接不上——
//   ① 指向**共享贴图**的槽(眼睛用 CommonTexture 里的图集),按约定找不到,只能退用本体贴图;
//   ② 半透/加色材质(水蓝蓝的水体、幽星光的发光壳),压根没法判该怎么混合。
// 材质实例里这些信息是全的:`TextureParameterValues` 给「参数名 → 贴图」,
// `BasePropertyOverrides.BlendMode` 给混合模式。
//
// **`UMaterialInstance.Deserialize` 抛 OverflowException 这条旧结论是错的**(见 git 历史里
// docs/findings.md §1 的旧表述):实测本作的材质实例能正常强类型加载,参数一条不少。
// 之所以一直以为不行,是 CUE4Parse 会为**别的**资产刷 OverflowException 日志,当时误当成材质的。
//
// 参数是**继承**的:材质实例只存自己覆盖的部分,其余要顺 `Parent` 链往上找,
// 一直找到根材质。所以这里逐级合并,子覆盖父。

using CUE4Parse.FileProvider;
using CUE4Parse.FileProvider.Vfs;
using CUE4Parse.UE4.Assets.Exports.Material;
using CUE4Parse.UE4.Assets.Exports.SkeletalMesh;
using CUE4Parse.UE4.Assets.Exports.Texture;
using CUE4Parse.UE4.Assets.Objects;
using CUE4Parse.UE4.Objects.Core.Misc;
using CUE4Parse.UE4.Objects.UObject;

namespace RocomPets.Export;

/// 一个材质槽解析出来的结果。
public partial record MaterialInfo(
    string Name,
    /// 参数名 → 贴图对象路径(已顺父链合并,子覆盖父)。
    Dictionary<string, string> Textures,
    /// 参数名 → 线性色(RGBA)。特效层的颜色就在这儿:火焰的 Color、光晕的 EmissColor 之类。
    Dictionary<string, float[]> Vectors,
    /// **只有这个材质自己写的**那批向量参数(不含父链继承)。
    ///
    /// 为什么要和 `Vectors` 分开:UE 的参数按**关联**(Global / Layer)分别解析,而
    /// 赛季那一族的 Red/Green/Blue/Metal 都是 `LayerParameter`。机幕方舟**没写**
    /// `BlueChannel`,可祖先上有一个同名的 `GlobalParameter` 白色 —— 合并视图里它是
    /// (1,1,1),而那条排列实际用的是图默认 **(0,0,0)**。拿合并值去喂,头顶那道竖条
    /// 就从「普通红 + 炫彩红」变成「红 + 惨白」。
    Dictionary<string, float[]> OwnVectors,
    /// 参数名 → 标量。强度/流速/菲涅尔次数一类。
    Dictionary<string, float> Scalars,
    /// 参数名 → 该参数的 `ExpressionGUID`。**这是通往根材质默认值的桥**:根材质
    /// (`UMaterial`)的 `CachedExpressionData` 里参数**名字被剥了、只剩哈希**,但有一份
    /// 与值数组同序的 `ExpressionGuids`;而实例这边每条参数都同时带名字和 GUID。
    /// 两边按 GUID 一对,就能给根材质那 149 个标量 / 43 个向量 / 13 张贴图的默认值配上名字。
    ///
    /// 为什么非要读根默认值:顺父链只能合并到根**之前**(根不是 `UMaterialInstance`),
    /// 所以只在根上给了默认、实例没覆盖的参数,平时完全看不见 —— 而那两颗球的固有色
    /// 恰恰就在那儿(根默认里有 F94728 红橙、FFC635 琥珀、64358B 紫、FF1BE7 品红)。
    Dictionary<string, string> ParameterGuids,
    /// **静态开关**:参数名 → 开/关。这是「这个特性到底开没开」的**明写答案**,
    /// 名字多半是中文(`是否使用MatCap`、`开启黑魔法效果`、`使用顶点色`)。
    /// 在拿到它之前只能靠「美术有没有显式写某个参数」间接推断,那是猜。
    Dictionary<string, bool> Switches,
    EBlendMode BlendMode,
    float OpacityMaskClipValue,
    /// 父链上所有材质的名字,由近及远;排查用。
    List<string> ParentChain,
    /// **根材质的参数默认值**(参数名 → 值)。顺父链走不到根,所以只在根上给了默认、
    /// 没有任何实例覆盖过的参数,只能从这儿拿(见 RootDefaults.cs)。
    /// **刻意与上面几张表分开**:现有判据看的是「美术显式设了没有」,混进根默认会整片翻转。
    RootDefaults? RootDefaults = null,
    /// **实机那条 cooked 排列自带的参数默认表**(见 ShaderDefaults.cs)。与上面那张
    /// `RootDefaults` 是两个来源、都自称「没人覆盖时的默认值」,而全库有 10 处对不上
    /// (`--probe-material DEFAULTDIFF`:`FlowColor` / `StarColor` / `StarTiling` /
    /// `FlickerSpeed` / `MainColor`)—— 对不上时**这一张才是 GPU 真拿到的**,
    /// 因为根材质允许有两个同名参数而 `CachedExpressionData` 只留一条。
    ShaderDefaults? ShaderDefaults = null,
    /// 材质资产是不是真的读到了。`false` = 网格引用的材质包在 pak 里根本不存在(悬空引用),
    /// 参数全空,导出器会退回按贴图命名约定给基色,见 Program.cs。
    bool Resolved = true,
    /// 同目录那份 `_Ol` 描边材质算出来的**描边宽度(米)**;没有 `_Ol` 就是 null(不画描边)。
    ///
    /// 「画不画」不是启发式,是资产表本身:`Mat/` 目录里除了 `MI_<形态>_<槽>` 还并排放着
    /// `MI_<形态>_<槽>_Ol`,而那份材质的参数表是描边专用的(`Outline Intensity`、
    /// `OutlineWidthPC`、`OutLineOtherColor1..5`、`MinID`/`MatID`…)。
    /// 实测:小灵面 `_By`/`_By1` 有、幽火 `_Fx` **没有**;水灵只有 `_By`;
    /// 幽星光 `_By` 与**那两颗玻璃球 `_Fx1`** 都有;克莱因龙 `_By`/`_Fx` 有、液面 `_Fx1` 没有。
    ///
    /// 宽度怎么来的见 `OutlineOf`。
    float? OutlineWidth = null,
    /// 不为 null 时描边宽度改按「占这个形态包围盒高度的比例」算(乘 `height_cm` 得米)。
    /// 全库 851/854 走这一支,见 `Materials.OutlineOf` 里「两个区间」那段。
    float? OutlineHeightRatio = null,
    /// 描边的**五档颜色**(线性 RGB,已乘过 `Outline Intensity`),按 `MatID` 遮罩挑。
    /// 没有 `_Ol`、或那份 `_Ol` 挂在别的根材质上(`M_FairyBall_BallBack`,3 份)⇒ null,
    /// 运行时退回「固有色压暗」那条老路。见 `OutlineOf`。
    float[][]? OutlineColors = null,
    /// 挑档用的那张遮罩(`MatID`,读 **alpha**)。854 份里 732 份就是本体的 `MaskTex`
    /// (= 炫彩那张 `_M`),但**不能直接复用** —— 38 份指着另一张、81 份本体压根没有 `MaskTex`。
    string? OutlineIdTexture = null)
{
    /// 这个材质画不画描边。
    public bool HasOutline => OutlineWidth is > 0f;

    /// 承载基色的参数名。`BaseTex` = 本体一类,`EyeTex` = 眼/嘴那种贴脸的小面片。
    /// `M_P_Object_XiaoYou` 是一套独立的不透明材质，固有色入口明确叫 `MainTex`；
    /// 过去只认前两项会把它误分成纯特效层，正是小灵面身体缺失的直接原因。
    /// 没有这两个参数 = 这个材质**不画固有色**(纯 VFX:火焰、水壳、光晕),桌宠该整片跳过。
    public string? BaseColorParam =>
        (IsXiaoYou
            ? Textures.Keys.FirstOrDefault(k => k.Equals("MainTex", StringComparison.OrdinalIgnoreCase))
            : null)
        ?? Textures.Keys.FirstOrDefault(k => k.Equals("BaseTex", StringComparison.OrdinalIgnoreCase))
        ?? Textures.Keys.FirstOrDefault(k => k.Equals("EyeTex", StringComparison.OrdinalIgnoreCase))
        // **`Base Color`(带空格)也是基色。** 一只一份的定制材质用的是这个名字,而不是
        // 通用的 `BaseTex` —— 小灵面一族身旁那两团幽火(`M_Gho_XiaoYou_GhostFire`)就是:
        // 它的 `Base Color` 指着一张画好的青色渐变图,整团幽火的颜色全在那儿。
        // 认不出来就落进「纯特效层」那条路,拿它当形状遮罩、颜色走 `Tint`(这个材质没有)
        // ⇒ 渲成一团**没有颜色的白**,正是实机反馈里的「幽火缺少颜色」。
        // 排在 `BaseTex`/`EyeTex` 之后:两者都有时仍以通用名为准。
        ?? Textures.Keys.FirstOrDefault(k => k.Equals("Base Color", StringComparison.OrdinalIgnoreCase))
        // **`BaseMap` 也是基色。** `M_P_BackRenderEmissive` 那一族用的是这个名字,
        // 认不出来就整族落进「纯特效层」—— 而它们在原资产里是 `BLEND_Opaque`、
        // 输出 alpha 恒 1 的**不透明背板**,当半透画会直接透出背景
        // (莫比乌乌那条面条实机是白的,我们透出了红卡)。
        // 全库普查:`BaseMap` 只出现在 **12 份**材质上,正好就是这一族,不会误伤别人。
        ?? Textures.Keys.FirstOrDefault(k => k.Equals("BaseMap", StringComparison.OrdinalIgnoreCase));

    /// 基色贴图的对象路径;没有就是纯特效材质。
    public string? BaseColorTexture => BaseColorParam is { } p ? Textures[p] : null;

    /// 是不是贴脸的小面片(眼/嘴)。它的贴图是**带透明背景的眼神图集**,alpha 是真遮罩,
    /// 渲的时候要按阈值剔;本体贴图的 alpha 是美术塞的遮罩通道,不能拿来剔(会把身体啃掉)。
    public bool IsFacePatch =>
        BaseColorParam?.Equals("EyeTex", StringComparison.OrdinalIgnoreCase) == true;

    /// 特效层的主色。这些材质没有基色贴图,固有色写在颜色参数里:
    /// 火焰是 `Color01`(火花实测 (6, 0.8, 0) —— R>1 的 HDR 橙,说明是加色发光),
    /// 水壳是 `MainColor`(水蓝蓝 (0.19, 0.65, 1)),其余族用 BaseColor 一类。
    /// **纯白的那个不算。** 这些名字里靠前的往往是父材质留下的中性默认值:果冻的内胆
    /// (`M_ShuiMu_ByIn`)同时有 `MainColor = (1,1,1,1)` 与 `BaseColor = (0.117,0.283,0.054)`,
    /// 按名字顺序取会拿到白色 —— 内胆于是渲成一颗白球(外壳不透明时看不见,一做成半透就露出来)。
    /// 所以先挑非白的,全白才退回白色。
    ///
    /// **`OverAllColor`/`InColor` 是「一只一份」的定制材质留的口子。** 春兔耳朵里那泡粉色
    /// 液体走的是 `M_Gra_Yutu_Ear_Lighting` —— 一个只给这只宠物写的材质,整套参数名
    /// (`Bubble Color` / `InColor` / `OverAllColor` / `TopColor` / 「小球大小」…)都不在
    /// 上面那批通用名里,于是 `Tint` 拿到 null、耳朵渲成一泡白的(实机报的第二条)。
    /// 这两个名字**语义明确**(「整体颜色」/「内部颜色」,实测都是 (1, 0.343, 0.733) 粉),
    /// 补进来就够把颜色接对;泡泡、液面高度、折射那些还没做,见 docs/findings.md §1.1。
    ///
    /// `M_ShuiMu_ByIn` 不读这里挑出的通用主色；它由下方 `Glassy*` 字段把原 shader 的
    /// 两个 flow 端点、噪声与 Fresnel 完整传给专用管线。这里不再用两色均值冒充流动结果。
    public float[]? Tint => FirstColor("Color01", "MainColor", "OverAllColor", "InColor",
        "BaseColor", "BaseColor1", "Emitter Color", "FresnelColor", "PatternColor",
        "BackColor");

    /// 半透强度。实例没写就问**根材质的默认值**,根上也没有才当全不透明。
    ///
    /// **不能直接兜 1。** `M_FairyBall_BallFront`(沙漏/水晶球那层玻璃壳)的根默认就是 0 ——
    /// 「这层壳自己不出颜色,亮的只有 MatCap 与边缘光」。实例普遍不覆盖它,于是兜 1 就把
    /// 玻璃壳画成了实心:等一等鸭手里那个沙漏成了一坨白,把里面的紫沙整个挡住(实机是透的)。
    /// 同一个根材质的落陨星兔在实例上显式写了 `Opacity = 0`,渲出来是对的 —— 一个根材质
    /// 两种结果,差别只在「实例写没写」,这正是该问根默认的信号。
    public float Opacity => RootScalar("Opacity", 1f);

    /// **基色贴图的 alpha 是不透明度还是纹路遮罩,由这个静态开关决定。**
    ///
    /// 本体贴图的 alpha 平时是美术塞的纹路遮罩(绝不能拿来剔像素);但 `Opacity or OpacityMask`
    /// 开着的那 11 个材质(蜜蜂/小甲虫的翅膀、果冻、暮星辰的裙子…)里,它就是不透明度。
    /// 两处独立测量对上了:暮星辰裙子那块 UV 的基色 alpha 中位 0.537,经汇编里那个重映射
    /// `saturate((a - 0.04) * 1.1111)` → 0.55;而拿实机截图的**水印对比度衰减**反推出来的
    /// 单层区 α ≈ 0.50。
    ///
    /// **但只看那个开关是不够的 —— `M_P_Object_Trans` 族无条件就这么干**(2026-08-04,
    /// 实机报「春花兔的耳朵也是半透明」)。春花兔的 `_Fx` 没设这个开关,于是被我们当不透明画,
    /// 耳朵渲成一坨实心白;实机是透的。汇编说了话:
    ///
    /// - `M_P_Object_Trans` 的三个排列(51670 / 8752 / 21938)**每一条**都有
    ///   `add r1.z, r8.w, l(-0.04)` + `mul_sat r1.z, r1.z, l(1.1111)` 接在基色采样之后 ——
    ///   **没有「alpha 当纹路遮罩」的那条分支**;
    /// - `MI_P_Object_Trans_MatCap`(幽星光的玻璃球)那三条(37998 / 20284 / 70710)也都有;
    /// - 春兔 `_Fx`(开关**开**)与春花兔 `_Fx`(开关**没设**)**命中的是同一批 24 个
    ///   shader map、同一批 shader**(51670 打头)—— 静态排列一模一样,也就是说这个开关
    ///   的根默认本来就是开的,那 11 个只是把它又写了一遍。
    ///
    /// 数据也对得上:春花兔 `_Fx` 那块 UV 的基色 alpha 中位 **0.378**(它的 `_By` 是 1.000),
    /// 是张画出来的不透明度图。所以判据改成「开关开着 **或** 父链走 `M_P_Object_Trans`」。
    ///
    /// `Trans_MatCap` 也必须包含在内：它的目标 PS 最终是
    /// `lerp(max(重映射 alpha, 高光, 菲涅尔), 重映射 alpha, ForceUseDefOpacity)`。
    /// 先前为了让两颗星光球在不完整的 MatCap 近似下保持实心而排除了这一支，副作用是
    /// 莫比乌乌的整个玻璃外壳 alpha 恒为 1，原生不透明内层无论怎么画都会被挡住。
    /// 现在高光/菲涅尔覆盖已进入运行时，按 cooked shader 恢复整族语义。
    public bool AlphaIsOpacity =>
        Switch("Opacity or OpacityMask")
        || ParentChain.Any(p => p.Contains("Object_Trans", StringComparison.OrdinalIgnoreCase));

    /// **幽火那一族要按画家序画(不写深度),不是普通不透明。**
    ///
    /// 小灵面一家身旁那两团幽火,每团在网格里是**两层闭合壳**:外壳(123 顶点、UV 落在
    /// 基色图左半的青色区)套着一层小的内壳(147 顶点、UV 在右半的浅色区),
    /// 而索引缓冲里的三角顺序是**每团「外壳 → 内壳」**。这个顺序只在「后画的盖住先画的」
    /// 时才有意义 —— 也就是这一族在实机走的是**不写深度**的那一遍:外壳不写深度,
    /// 随后的内壳照样通过深度测试、直接盖上去。
    ///
    /// 三条证据合到一起才敢这么判:
    /// ① 目标 Low PS(资源 `0141823D…`)`o0.w = 1`,唯一那处 discard 是
    ///    `1 − 1.01 × max(Xray, Common_Xray)`(两个都是 0)⇒ **完全不透明**,不是 alpha 混合;
    /// ② 顶点着色器里没有任何沿法线的位置偏移,两层壳都是闭合的(边界边 0 条),
    ///    所以外壳在不透明通道里必然把内壳整个挡住 —— 我们原来就是这样,实机不是;
    /// ③ 按上面那条改成不写深度之后,渲图与实机逐一对上:外层青、内层浅、层次与位置都对,
    ///    而且**背景一点都不透出来**(实机也不透)。
    ///
    /// 按材质根名认,与 `M_Gra_Yutu_Ear_Lighting` / `M_P_FakeFulid` / `M_P_MatCap_Masked`
    /// 那几族的做法一致 —— 这些都是「一只(或一家)一份」的定制材质。
    public bool IsPaintOrder =>
        ParentChain.Any(p => p.Equals("M_Gho_XiaoYou_GhostFire", StringComparison.OrdinalIgnoreCase));

    /// 遮罩/噪声贴图:特效的形状与流动来源。没有就当常量 1。
    public string? MaskTexture =>
        (IsYutuEar ? YutuBubbleTexture : null)
        ?? FirstTexture("FuildMask", "Mask", "MaskTex", "BaseMap", "Base Color", "MatCap", "MatCapTex");

    /// **炫彩的区域门**:`MaskTex` 的 alpha 是离散 ID 台阶,`GlassySwitch=true` 那条排列
    /// 整段玻璃层包在 `if (MaskTex.a >= MinID)` 里(反汇编 `ge r3.x, r3.w, l(0.4)` +
    /// `else { 输出原着色 }`),`MinID` 根默认 0.4。
    ///
    /// 这就是「游戏里只给部位上色」的机制:鸭吉吉那张 By_M 的 alpha 身体 1.0、**喙与脚 0**,
    /// 白金独角兽是鬃毛/尾/腿毛 0.5、**身体 0** —— 和实机截图里哪块变色一一对上。
    /// 不接这道门,整只(连喙带脚)都会被刷上玻璃色。
    ///
    /// 和 `MaskIdTexture`(色带那道门)是**同一张图、两道不同的门**,各带各的阈值。
    /// 只给炫彩会刷到的槽导(判据与 `pack.rs` 的 `is_glassy_target` 必须一致)。
    public string? GlassyIdTexture => IsGlassyTarget ? FirstTexture("MaskTex", "Mask") : null;


    /// 炫彩刷在哪些材质槽上。**判据照抄客户端** `PetMutationUtils.SetGlassyDiffMutation`:
    /// 只取后缀 `by` / `by0..by9` 的材质,再加一道父链闸(`M_P_Object` 一族才带
    /// `GlassySwitch`)。**与 `src/pack.rs` 的 `is_glassy_target` 是同一条判据,改一处要改两处。**
    public bool IsGlassyTarget
    {
        get
        {
            // 大小写不能较真:同一只宠物的材质名在资产文件名与对象名之间会漂
            // (`MiaoMiao`/`Miaomiao`)。
            var lower = Name.ToLowerInvariant();
            var suffixOk = lower.EndsWith("_by", StringComparison.Ordinal)
                || (lower.Length >= 4 && char.IsAsciiDigit(lower[^1])
                    && lower.AsSpan(lower.Length - 4, 3).SequenceEqual("_by"));
            return suffixOk
                && ParentChain.Any(p => p.Contains("M_P_Object", StringComparison.OrdinalIgnoreCase));
        }
    }

    /// **赛季传说精灵的专属贴图**(「铅绘」那种)。
    ///
    /// 机制比看上去简单:`BaseTex` 与 `BaseTexSketch` 在编译产物里**共用同一个绑定槽**
    /// (探针 `tex[0] BaseTex: index=3` / `tex[0] BaseTexSketch: index=3`),
    /// 动态开关 `MutationSwitch` 一开就换成后面那张 —— **整套「特殊效果」就是换基色贴图**。
    ///
    /// 客户端只对 `HIDDEN_GLASS_CONF.season_pet` 里那几只走这条路
    /// (`PetMutationUtils` 的赛季分支;注意它**不开** `GlassySwitch`,所以这几只
    /// 上赛季炫彩时根本没有玻璃层,只是换了张图)。三种观感都由这张图自己决定:
    /// 加尔/黑化加尔整张图都是铅绘 ⇒ 全身;龙息帕尔只有翅膀那块不一样 ⇒ 只翅膀变;
    /// 机幕方舟多画了银色扑克花纹 ⇒ 身体与肩顶多出花纹。
    public string? SeasonBaseTexture => FirstTexture("BaseTexSketch");

    /// **赛季传说精灵的另一族做法**(`MI_P_Object_SeasonMutation*`)。
    ///
    /// 铅字幻梦那家(加灵)只是换基色贴图;暗夜拾光的龙息帕尔与狂欢怪谈的机幕方舟走的是
    /// 这一族 —— 客户端同样只开 `MutationSwitch`,但效果整套烘在材质里,配置表那边
    /// **没有** `season_pet_tex`。
    ///
    /// `MutationSwitch=true` 那份排列(`DynamicSwitchId = 2`)读下来,**骨架就是玻璃层**:
    /// 同一条折射 + 相对包围盒中心的屏幕 UV、同一个 `1.62` 增益(汇编里是折进去的常量,
    /// 正好印证 `(BaseColorDetail+1) × FlowColorIntensity`)、同一道 `MaskTex.a` 区域门、
    /// 同一条 `pow(mean(基色), 0.35)` 亮度门。换掉的只有输入:
    ///
    /// - 花纹图用材质自己的 `FlowNoise`,不是全库共享的 `MainTex`;
    /// - 两个 Channel 色用材质自己的(玩家选不了);
    /// - 多两个区域:`MixMask.b` 按 `pow(x × FlowMaskInt, FlowMaskPow)` 混向 `BlueChannel`,
    ///   `MixMask.a ≥ 0.79` 的地方直接换成 `MetalColor`。
    ///
    /// 机幕方舟的 `MetalColor = (1.5, 1.5, 1.5)` —— 「身体与肩顶那圈**银色**扑克花纹」就是它;
    /// 而 `MixMask` 是**每宠物一张**,「只在翅膀」「只在身体与肩顶」全由它划定。
    public bool IsSeasonMutation =>
        ParentChain.Any(p => p.StartsWith("MI_P_Object_SeasonMutation", StringComparison.OrdinalIgnoreCase));

    /// 花纹图。**可以没有** —— 机幕方舟的 `_By1`(胳膊/肩膀/小尖塔)就没写,
    /// 那时用全库共享的 `MainTex`(运行时取烘进来的那张)。
    /// 按 `FlowNoise` 存不存在来判定这一族会把 `_By1` 整块丢掉,判据要用 `MixMask`。
    public string? SeasonFlowNoise => IsSeasonMutation ? FirstTexture("FlowNoise") : null;
    public string? SeasonMixMask => IsSeasonMutation ? FirstTexture("MixMask") : null;


    /// `[RedChannel.rgb, GlobalRefraction]` —— 这一族的两个 Channel 色是**材质自己的**,
    /// 玩家选不了(客户端在这条分支上一个颜色都不设)。
    public float[] SeasonRed =>
        [.. OwnColor("RedChannel", [0f, 0.932292f, 0.829095f]), Scalar("GlobalRefraction", 2f)];

    /// `[GreenChannel.rgb, GlobalDepth]`。
    public float[] SeasonGreen =>
        [.. OwnColor("GreenChannel", [0.05486f, 0.420539f, 1f]), Scalar("GlobalDepth", 30f)];

    /// `[BlueChannel.rgb, FlowMaskInt]` —— `MixMask.b` 那条幂曲线混向的颜色 + 曲线强度。
    public float[] SeasonBlue => [.. OwnColor("BlueChannel", [0f, 0f, 0f]), Scalar("FlowMaskInt", 1f)];

    /// `[MetalColor.rgb, FlowMaskPow]` —— 高遮罩区那片金属色 + 幂曲线指数。
    public float[] SeasonMetal =>
        [.. OwnColor("MetalColor", [0.99132f, 0.669075f, 0.184151f]), Scalar("FlowMaskPow", 1f)];

    /// `[MetalColor02.rgb, 0]`。
    /// 金属区那层**金属光泽**的 matcap(`Mutation_MatCap`)。
    ///
    /// **这一条是近似,不是从这条排列读出来的** —— `MutationSwitch=true` 那份汇编里
    /// 金属区就是平涂的 `mix(r0, MetalColor, zone)`,没有 matcap;实机那点明暗有一部分
    /// 来自后面统一走的 toon 光照(我们用 `shaded/base` 的亮度比近似回去了),
    /// 但光靠它出不来「银」的那种金属高光。
    ///
    /// 判据用材质自己的 **`MetalSpecInt`**:机幕方舟是 **1**(实机就是带高光的银),
    /// 龙息帕尔是 **0**(实机翅膀上那圈花纹是平白的)。两只正好各归其位 ——
    /// 曾经不分青红皂白一律乘 matcap,把龙息帕尔的白星月染成了紫(`Matcap29` 是紫的)。
    public string? SeasonMatCap =>
        IsSeasonMutation && Scalar("MetalSpecInt", 0f) > 0f ? FirstTexture("Mutation_MatCap") : null;

    public float[] SeasonMetal02 =>
        [.. OwnColor("MetalColor02", [0.99132f, 0.669075f, 0.184151f]), 0f];

    /// `[MainTexFlowSpeedX, MainTexFlowSpeedY, MainTexTiling, NormalEffectAmount]`。
    ///
    /// **两个轴都要导**:机幕方舟给的是 Y(0.4)、龙息帕尔给的是 **X**(0.1)。
    /// 只导 Y 的话龙息帕尔那层花纹是静止的 —— 而实机里它在动(翅膀上淡金那块
    /// 时隐时现就是这个)。
    public float[] SeasonFlow =>
    [
        Scalar("MainTexFlowSpeedX", 0f), Scalar("MainTexFlowSpeedY", 0f),
        Scalar("MainTexTiling", 1.5f), Scalar("NormalEffectAmount", 0.1f),
    ];

    /// UV 卷动:速度与平铺。火焰靠它动起来。
    public float[] Flow =>
    [
        Scalar("Flow_U_Speed"), Scalar("Flow_V_Speed"),
        Scalar("Flow_U_Tiling", 1f), Scalar("Flow_V_Tiling", 1f),
    ];

    /// 静态开关查询。**开关只在美术真的打开时才写进实例**(全量统计里这些开关是「N 个开 / 0 个关」),
    /// 所以「查不到这一条」= 用父材质的默认值,而这批开关的默认基本都是关。
    public bool Switch(string name) => Switches.TryGetValue(name, out var v) && v;

    /// **卷动色带**:一张渐变图沿 UV 滚过表面,给固有色叠上流动的颜色。
    /// 暮星辰的环带就是它——`MI_P_Object_XingGuang_UVFlow_Morph` 给 `FlowTexture`
    /// = `T_..._Fx_D`(青↔粉竖条纹渐变)+ `Flow_U_Speed` = 0.25,于是青粉渐变绕着环跑;
    /// 基色贴图里环带那一条是**纯粉的**,渐变完全来自这张图。
    ///
    /// **判据是「明确的 `XingGuang_UVFlow` 色带族,或静态开关
    /// `是否需要BaseColor流动` 打开」。** 不能把所有名字含 `UVFlow` 的父材质都算进来:
    /// `MI_P_Object_UVFlow_WPO_NoMetal` 的 `FlowTexture` 在目标 Low shader 中接的是
    /// **法线扰动**,不是 BaseColor。水灵的蝴蝶结与身体走这支；把那张蓝黑遮罩当颜色混入，
    /// 就会把本来干净的红蝴蝶结染蓝。`XingGuang_UVFlow` 才是把贴图接到颜色的那一支。
    /// 原来只看「美术给了流速」,那会多出 17 个火焰族材质(火花/迪莫/守夜烛):它们的
    /// `Flow_U_Speed` 是给**特效层自己的噪声卷动**用的,不是给固有色叠色带。
    public string? FlowTexture =>
        (ParentChain.Any(p => p.Contains("XingGuang_UVFlow", StringComparison.OrdinalIgnoreCase))
         || Switch("是否需要BaseColor流动"))
        && (Scalar("Flow_U_Speed") != 0f || Scalar("Flow_V_Speed") != 0f)
            ? FirstTexture("FlowTexture")
            : null;

    /// 色带的混入强度(暮星辰环带 0.8)—— 是**混色权重**,不是乘法强度。
    public float FlowPower => Scalar("FlowPower", 1f);

    /// **`M_P_Object` 公共链上的加性流动层**(不是「色带」那一支,别和 `FlowTexture` 混)。
    ///
    /// 读自波波拉 `_By` 的 quality=**Num** 排列(resource `0F1003EB…`、LOD0、**DSId=1**,
    /// PS 49966 第 110~123 行。DSId=0 那条(`16F07608…`)**根本没有世界 base pass 那组
    /// shader**,`matshader.py` 直接报「那组是空的」—— 所以这里不能按「DSId=0」的老习惯挑);
    /// 火系那条(PS 41058 第 160~177 行)是**逐指令相同**的一段,只是 cb 下标不同 ——
    /// 所以这不是某一族的专属层,是根图 `M_P_Object` 的公共件:
    ///
    /// ```text
    /// uv  = uv × (Flow_U_Tiling, Flow_V_Tiling) + frac(time × (Flow_U_Speed, Flow_V_Speed))
    /// F   = pow(FlowTexture(uv).rgb, FlowPower) × FlowColor × FlowInt
    /// vb  = 顶点色B + InverVertexColor × (1 − 2 × 顶点色B)
    /// w   = m + Inv Or Not × (1 − 2m)          m = saturate((基色a − 0.04) × 1.1111)
    /// 发光 += w × vb × F
    /// ```
    ///
    /// **过去把这一层读成「法线扰动」是从 Low 排列读的**(见 pet/shader/40-layers.wgsl 的 `flow_band` 注释),
    /// 实机跑 Num —— 同一个坑第 N 次。
    ///
    /// 判据:**实例链自己给了 `FlowTexture`**(根默认那张 `TestResMaskTex` 不算)、
    /// 而且不是暮星辰那条「卷动色带」支(那一支已由 `FlowTexture`/`flow_band` 处理)。
    public string? UvFlowTexture =>
        IsObjectRoot && FlowTexture is null ? FirstTexture("FlowTexture") : null;

    /// 这两层都长在根图 **`M_P_Object`** 上,不是某一族的。**必须按根名字挡**:
    /// 水蓝蓝的 `_Fx`(`MI_ShuiLanLan_PP ← M_Wat_ShuiLanLan_PP`)也有个叫
    /// `FresnelIntensity` 的参数,那是**另一张图里同名的另一个参数** —— 不挡就会把
    /// 一层根本不存在的边缘光加到那片外壳上。同名不同图,这本子里已经栽过好几次。
    private bool IsObjectRoot =>
        ParentChain.Any(p => p.Equals("M_P_Object", StringComparison.OrdinalIgnoreCase));

    /// `[FlowColor.rgb, FlowInt]`。**16 份材质自己覆盖了 `FlowColor`**(小火苗一族是橙、
    /// 多多一族是暗紫、幻星一族是粉),其余用根默认(波波拉那条根链 (0.5, 0, 0.6))。
    ///
    /// 注:全库普查最初报的是「3393 份一个都没覆盖」—— 那是因为当时的 `PARAM:` 普查
    /// **只查标量与静态开关**,向量参数一律落进「没设」那一档。普查器已经补上向量与贴图。
    public float[] UvFlowColor =>
    [
        ..(FirstVector("FlowColor") ?? RootVector("FlowColor")
           ?? [1f, 1f, 1f, 0f])[..3],
        RootScalar("FlowInt", 1f),
    ];

    /// `[FlowPower, InverVertexColor, Inv Or Not, OpenRadialUV]`。
    ///
    /// **`EmissContrast` 不在这儿**:全库只有一份材质设过它、值还是 0
    /// (`saturate(x × (2k+1) − k)` 在 k=0 时就是 `saturate(x)`),所以运行时按定值 0 做,
    /// 只保留那一步 `saturate`。火系那条排列里连这步都没编进去。
    public float[] UvFlowShape =>
    [
        RootScalar("FlowPower", 1f),
        RootScalar("InverVertexColor", 0f),
        RootScalar("Inv Or Not", 0f),
        RootScalar("OpenRadialUV", 0f),
    ];

    /// 极坐标卷动的中心。`OpenRadialUV` 打开时,采样 UV 先换成
    /// `(atan2(d.y, d.x) / 2π 的小数部分, |d|)`(`d = uv − 中心`),再按上面那组平铺/卷动走。
    /// 全库 **10 份**材质开了这个开关(小火苗一族在内),波波拉没开。
    public float[] UvFlowRadial =>
    [
        RootScalar("RadialCenterOffsetX", 0.5f), RootScalar("RadialCenterOffsetY", 0.5f),
        // `.z` = UV 集选择器 `saturate(UV Number)`:0 取 UV0、1 取 UV1。
        Math.Clamp(RootScalar("UV Number", 0f), 0f, 1f), 0f,
    ];

    /// **`M_P_Object` 公共链上那圈菲涅尔发光**(PS 49966 第 128~149 行,火系 41058 第 178~197 行):
    ///
    /// ```text
    /// f    = pow(1 − saturate(N·V), FresnelExponent) × FresnelBoost   ← N 是**顶点法线**
    /// c    = f × FresnelColor × FresnelIntensity
    /// g    = FresnelIntensity × (FresnelBaseMin − 1) + 1
    /// 硬边 = smoothstep(0.99, 1, c.r × g) × HardLineCol × HardLineColMul
    /// 发光 += lerp(硬边, c × g, FresnelSoftTohard)
    /// ```
    ///
    /// 全库只有 **16 份**材质设过 `FresnelIntensity`(其中 8 份设成 0),所以这一层
    /// 用「强度 > 0」当门就够,不必再挑族。
    public float[]? Fresnel =>
        !IsObjectRoot || Scalar("FresnelIntensity") <= 0f ? null
        : [..(FirstVector("FresnelColor")
              ?? RootVector("FresnelColor") ?? [1f, 1f, 1f, 0f])[..3],
           Scalar("FresnelIntensity")];

    /// `[FresnelExponent, FresnelBoost, FresnelBaseMin, FresnelSoftTohard]`。
    public float[] FresnelShape =>
    [
        RootScalar("FresnelExponent", 8f), RootScalar("FresnelBoost", 20f),
        RootScalar("FresnelBaseMin", 0.4f), RootScalar("FresnelSoftTohard", 1f),
    ];

    /// `[HardLineCol.rgb, HardLineColMul]` —— 菲涅尔超过 0.99 之后接管的那一档硬边色。
    public float[] FresnelHard =>
    [
        ..(FirstVector("HardLineCol")
           ?? RootVector("HardLineCol") ?? [1f, 1f, 1f, 1f])[..3],
        RootScalar("HardLineColMul", 1f),
    ];

    /// 火系族(`MI_P_Object_Fire*`)。它在**同一个发光累加器**上比通用链多两层,
    /// 读自火神 `_By` 的 quality=Num 排列(resource `041D1E47…`,PS 41058 第 68~122 行,
    /// `V=64 / S=75`,cb 槽位逐格读出)。
    ///
    /// ```text
    /// base = toneInv(BaseTex.rgb)            ← 和通用链同一条反色调映射
    /// 层1  = base × lerp(Color1, Color2, pow(max(N·V,0), FresnelPower)) × FresnelInt
    /// 带   = smoothstep 形状,见 FireShape;色 = UseVertexColorG ? lerp(Color02, Color, 顶点色.g) : Color
    /// 层2  = base × 色 × 带 × Int
    /// 两层各自再 lerp(层, m × 层, `Use Opacity as Mask`)      m = saturate((基色a−0.04)×1.1111)
    /// 发光 += 层1 + 层2 (+ 通用链那层 m × Emitter Color × Emitter Intensity)
    /// ```
    ///
    /// **这两层是加性发光,不是固有色**(汇编第 199 行把它们并进 `r6` 那个累加器,
    /// 而基色 `r5` 另走一路)。火神代进去:`FresnelInt = 0` ⇒ 层1 整个为零;
    /// `Range = 0` ⇒ 那条带恒为 1 ⇒ 层2 = `toneInv(基色) × (1.2, 0.825, 0) × 0.4`,
    /// 一层均匀的橙色自发光。这正是它 0.160 里缺的那块。
    public bool IsFireFamily =>
        ParentChain.Any(p => p.Contains("Object_Fire", StringComparison.OrdinalIgnoreCase));

    /// **`Color1`/`Color2` 那条菲涅尔带火系与水体两族共用**,但水体那一族**没有落地**
    /// —— 量下来更差,撤回了,见 docs/findings.md「水体那层:公式这次是对的,量下来还是更差」。
    /// `[Color1.rgb, FresnelPower]`。
    public float[] Fire1 =>
    [
        ..(FirstVector("Color1") ?? RootVector("Color1")
           ?? [1f, 1f, 1f, 1f])[..3],
        RootScalar("FresnelPower", 1f),
    ];

    /// `[Color2.rgb, FresnelInt]` —— `.w = 0` 就是层1 不画。
    public float[] Fire2 =>
    [
        ..(FirstVector("Color2") ?? RootVector("Color2")
           ?? [1f, 1f, 1f, 1f])[..3],
        RootScalar("FresnelInt", 1f),
    ];

    /// `[Color.rgb, Int]` —— `.w = 0` 就是层2 不画。
    public float[] Fire3 =>
    [
        ..(FirstVector("Color") ?? RootVector("Color")
           ?? [1f, 1f, 1f, 1f])[..3],
        RootScalar("Int", 0f),
    ];

    /// `[Color02.rgb, UseVertexColorG]`。`UseVertexColorG >= 0.5` 时层2 的颜色是
    /// `lerp(Color02, Color, 顶点色.g)`,否则就是 `Color`。
    public float[] Fire4 =>
    [
        ..(FirstVector("Color02") ?? RootVector("Color02")
           ?? [1f, 1f, 1f, 1f])[..3],
        RootScalar("UseVertexColorG", 0f),
    ];

    /// `[Range, Soft, Use Opacity as Mask, 这一族(0/1)]`。带的形状(汇编第 100~113 行):
    ///
    /// ```text
    /// t   = saturate((pow(max(1 − max(N·V,0), 1e-4), Range) × 0.96 − 0.46) / (Soft × 0.1))
    /// 带  = t²(3 − 2t)
    /// ```
    ///
    /// 那个 `Soft × 0.1` 来自 preshader:`cb6[66].y = 0.5 + Soft × 0.1`,汇编再减 0.5。
    /// **`Use Opacity as Mask`(带空格)和 `UseOpacityAsMask`(不带)是两个参数**,
    /// 火神分别是 0 与 1;这里要的是带空格那个。
    public float[] FireShape =>
    [
        RootScalar("Range", 0f), RootScalar("Soft", 0.5f),
        RootScalar("Use Opacity as Mask", 0f), IsFireFamily ? 1f : 0f,
    ];

    /// **色带的 ID 遮罩**:只在 `MaskTex` 的 **alpha** 落在 [`MaskID Min`, `MaskID Max`] 的地方生效。
    ///
    /// 实测暮星辰(阈值 0.6~0.8):那张 By_M 的 alpha 是**离散 ID 台阶**(0.0 / 0.27 / 0.50 /
    /// 0.72 / 1.0),环带那片是 0.72(68.5% 落在区间内)、额头与身体中央的黄色装饰是 0.502
    /// (0% 落在区间内)。不按这个门控,色带会连黄装饰一起卷,装饰就在黄绿之间来回变 ——
    /// 而实机里那些装饰是固定黄色。
    /// 卷动色带那支与 `UvFlowTexture` 那支**共用这道门**(汇编里也是同一个
    /// `MaskID Min/Max` 对 `MaskTex.a` 的区间判断)。
    public string? MaskIdTexture =>
        FlowTexture is null && UvFlowTexture is null ? null : FirstTexture("MaskTex", "Mask");

    public float[] MaskIdRange => [Scalar("MaskID Min", 0f), Scalar("MaskID Max", 1f)];

    /// **「假半透」族也是一层星点**:`..._FakeTrans*` 家族给 `NoiseTex`(黑底 + 粉白星点)
    /// + `NoiseTilingSpeed` + HDR 的 `Color02`,幽星光一族的身体看着半透、身上有星星靠它。
    ///
    /// 实机里这层**不流动**,运行时和 `StarStickTex` 走同一条路,只是贴图与着色换成这一族
    /// 自己的。全量只有 3 个材质是这一族(幽星光的身体)。
    ///
    /// **注意两族的公式其实不一样**(反汇编查实的,见 pet/shader/*.wgsl `star_light`):`StarStickTex`
    /// 那张是彩色星形色块图集,这一族的 `NoiseTex` 是纯黑底、r/g/b 分别是阈值/相位/幅度。
    /// 运行时把两层并成一份时公式也并成了一套,那是已知的简化 —— 不是「同一份遮罩」。
    public bool IsFakeTrans =>
        ParentChain.Any(p => p.Contains("FakeTrans", StringComparison.OrdinalIgnoreCase));

    /// 发光强度(火焰族有);没有就 1。
    public float Glow => Scalar("Glow Intensity", 1f);

    /// 自发光:`Emitter Color` × `Emitter Intensity`。**根默认强度是 0**,也就是这一层
    /// 默认关闭、要用的宠物自己开 —— 所以只对开了的那些生效,风险有界。
    ///
    /// 汇编里它是 `材质颜色 × 一个遮罩` 加进结果(水蓝蓝 body 的 shader 33729:
    /// `mad r5.xyz, cb6[94].xyzx, r2.y, r5.xyzx`,r5 随后加进颜色);那个遮罩由若干标量
    /// 拼出的 ramp 给,**输入还没追到**,运行时先用菲涅尔当代理(见 pet/shader/*.wgsl)。
    ///
    /// 证据:全库唯二开着这一项的(波波拉 0.3/0.4 蓝、火神 0.5 橙)正好是 17 只实机对照里
    /// **唯二的非构图色差离群项**(调色板 0.329 / 0.162),而关着的那些都在 0.02~0.11。
    /// **水体预设里 `Emitter Intensity` 不是自发光强度,是 `Color1` 那层的增益。**
    /// 查实于水蓝蓝 `_Fx`(父 `MI_P_Object_Water_NoMetal`)的 shader 35663 —— 配到该材质
    /// 块 15(`V=83`、`dcl cb5[106]`、`83 + ⌈90/4⌉ + 1 = 107`),那一步是
    /// `mad r4.xyz, r4.xyzx, cb5[83].x, r5.xyzx`,而 `cb5[83].x` 的名字就是 `Emitter Intensity`,
    /// `r4` 是 `mask × Color1`。完整公式见 rocom-capture/docs/shader.md「水体预设」。
    ///
    /// 所以这一族不能输出自发光层 —— 原来当通用自发光加了「白 × 0.4 × 菲涅尔」。
    /// **实测也支持**:关掉自发光后波波拉的调色板距离一动不动(0.337),
    /// 而火神从 0.090 恶化到 0.178 —— 火神那边它确实是自发光,所以只排除水体。
    /// (火神的图里也有 `Color1`/`Color2`/`FresnelInt`,可能是同一个共享图层,
    /// 但它的 shader 还没读,没有证据前不动。)
    public bool IsWater =>
        ParentChain.Any(p => p.Contains("Water", StringComparison.OrdinalIgnoreCase));

    /// **水体预设里这一层不乘 `Emitter Intensity`。** 目标 Low PS 48738 第 59 行是
    /// `mul r4.xyz, mask, cb6[2].xyzx`,随后第 111 行直接 `add` 进发光累加器 —— 中间没有
    /// 任何标量;而 `cb6[2]` 的 preshader 字节码是 `04 0100 23 03 000102FF`
    /// = `vector-param[1].xyz` = **`Emitter Color`** 本身。通用 `M_P_Object`
    /// (PS 68952 第 64~65 行)才有那一步 `× cb6[39].z`。
    ///
    /// `Emitter Intensity` 在水体链里是**另一层**(`Color1`)的增益,见 `IsWater`;
    /// 所以这一族既不能拿它当强度、也不能拿它当开关(shader 里根本没有开关)。
    /// 实测:水灵身上那道条纹的峰值提升,乘 0.5 时 +39、不乘时 +89,实机是 +96。
    public float EmissiveIntensity => IsWater ? 1f : Scalar("Emitter Intensity", 0f);


    /// 水体预设(`ML_P_StylizedWater` 图层)。整条链是从 shader 35663 读出来的,
    /// 公式见 rocom-capture/docs/shader.md「水体预设」;这里只把参数搬出来。
    ///
    /// `Color1` 的**增益就是 `Emitter Intensity`**(见上面),所以合成一个 rgb + a 传出去;
    /// `Main Color` 的 **a 是末尾那步 lerp 的混合系数**(波波拉是 0 ⇒ 空操作)——
    /// 这个「rgb 存颜色、a 存混合量」的套路在这套材质里反复出现,别只取 rgb。
    public float[]? WaterColor1 => !IsWater ? null
        : Vectors.TryGetValue("Color1", out var c)
            ? [c[0], c[1], c[2], Scalar("Emitter Intensity", 0f)]
            : null;

    public float[]? WaterColor2 =>
        !IsWater ? null : Vectors.TryGetValue("Color2", out var c) ? [c[0], c[1], c[2], 0f] : null;

    /// `[Main Color.rgb, caustics 贴图是不是 sRGB]`。
    ///
    /// **第四位不是 `Main Color.a`**(汇编只用 `.xyz`,见 pet/shader/40-layers.wgsl 的 `water_layer`),
    /// 让给 sRGB 旗标 —— 这一族的 caustics 贴图(`Noise` 槽)实测 **sRGB = 1**,
    /// 而运行时统一按 `Rgba8Unorm` 上传、没有硬件解码那一步。不解码的代价是
    /// **整层强 9 倍**(G 中位 0.224 → 线性 0.041),实测会把水灵的亮度比从 0.85 顶到 1.15。
    /// 同一个坑这本子里踩过第二次(上一次是 `M_P_Object` 的流动贴图)。
    public float[]? WaterMain =>
        !IsWater ? null : Vectors.TryGetValue("Main Color", out var c) ? [c[0], c[1], c[2], 0f] : null;

    /// caustics 的 `[u 平铺, v 平铺, u 速度, v 速度]`。
    public float[] WaterCaustics =>
    [
        RootScalar("U_Tiling_Caustics", 1f), RootScalar("V_Tiling_Caustics", 0.8f),
        RootScalar("U_Speed_Caustics", 0.1f), RootScalar("V_Speed_Caustics", -0.5f),
    ];

    /// 流动扰动那一路的 `[u 平铺, v 平铺, u 速度, v 速度]`(与 caustics 那组**不是同一组**:
    /// 汇编 PS 16335 第 72~79 行分别用 `cb6[57].xy/.zw`(caustics)与 `cb6[58].yz`+`cb6[58].w`/
    /// `cb6[59].x`(flow))。
    public float[] WaterFlow =>
    [
        // **兜底值取自 cooked shader map 自带的参数默认表**(`PROBE_SHADER_DETAILS`),
        // 不是随手填的 0/1:这一族的参数来自材质**图层**,`RootDefaults`(读根 UMaterial 的
        // `CachedExpressionData`)里根本没有它们,而实例又只覆盖了一部分。
        RootScalar("U_Tiling_Flow", 1f), RootScalar("V_Tiling_Flow", 0.8f),
        RootScalar("U_Speed_Flow", 0.1f), RootScalar("V_Speed_Flow", -0.5f),
    ];

    /// `[CausticsInt, FlowDistort, FresnelInt, FresnelPower]`。
    public float[] WaterShape =>
    [
        RootScalar("CausticsInt", 1f), RootScalar("FlowDistort", 0.2f),
        RootScalar("FresnelInt", 1f), RootScalar("FresnelPower", 1.771117f),
    ];

    /// **实例没写 `Emitter Color` 时要退到根默认(通常是白),不能当成「没有自发光」。**
    /// 水灵/波波拉的水体层就只写了 `Emitter Intensity`,颜色留在根上 —— 而实机身上那几道
    /// 竖向浅色条纹正是「白 × 强度 × 基色 alpha」:把线上提升逐通道量出来,
    /// 白 × 0.5 是 (43, 15.6, 9.0)、实机是 (96, 23, 9)(同一比例),
    /// 而拿 `Color1` 那个蓝色去算是 (21, 11, 10) —— 色相与强度都不对。
    public float[]? EmissiveColor =>
        EmissiveIntensity <= 0f ? null
        : Vectors.TryGetValue("Emitter Color", out var c) ? c
        : RootVector("Emitter Color") ?? [1f, 1f, 1f, 1f];

    /// 是不是半透材质。**有基色的材质也可能是半透**——暮星辰的裙子(`Fx1`)与那两个球(`Fx2`)
    /// 都是 `MI_P_Object_Trans_*` 家族、`BLEND_Translucent`,当成不透明画就是死板的实心块。
    public bool IsTranslucent =>
        BlendMode is EBlendMode.BLEND_Translucent or EBlendMode.BLEND_AlphaComposite;

    /// 半透族(`M_P_Object_Trans`)的星点层门:**`RampID >= 0.4`,这条是从汇编读出来的**。
    ///
    /// shader 51670(`M_P_Object_Trans` 的世界 base pass、`Opacity or OpacityMask` 那个排列,
    /// 7 个 uniform buffer ⇒ 材质 cb6):
    ///
    /// ```text
    /// ge  r2.y, cb6[84].z, l(0.4)          ← 门
    /// and r5.w, r2.y, l(1065353216)        ← 门 → 1.0f / 0.0f
    /// mad r4.xyw, r5.w, r9.xyxz, r4.xyxw   ← 高光层按门混
    /// …
    /// max r1.z, r1.z, r8.w                 ← r8.w = 星点遮罩 m
    /// mad r1.z, r5.w, r1.z, r1.w           ← **星点对不透明度的贡献也按同一个门混**
    /// ```
    ///
    /// `cb6[84].z` 的名字是查出来的不是猜的:按 `uniexpr.py` 的两条判据(V = 83、
    /// V + ⌈S/4⌉ + 1 = 120 ≥ 声明的 cb6[119])这条 shader 唯一配到冻结块 9,块 9 里
    /// `cb[84].z = 标量 6 = RampID`(根默认 0)。**交叉验证**:同块 `cb[88].z = StarStickTiling = 4`,
    /// 而汇编里星点的 UV 正是 `mul r3.yw, v2.xxxy, cb6[88].z` —— 槽位对得上。
    ///
    /// 实机两张截图也对得上:春兔 `_Fx`(耳膜,`RampID` = 0)看不到星点,
    /// 果冻 `_By`(`RampID` = **0.5**,实例显式开的)那层是开着的 —— 而它在这一族里
    /// **只改不透明度、不画彩色星星**(见上面汇编:`m` 只进 `r1.z` 那条 alpha 链),
    /// 所以实机看着也只是「果冻更实了一点」,不是一身四角星。
    /// **这条「只改 alpha」运行时还没实现**(见 docs/findings.md §1.1),现在只做门。
    private bool TransStarGate =>
        !ParentChain.Any(p => p.Contains("Object_Trans", StringComparison.OrdinalIgnoreCase))
        || RootScalar("RampID", 0f) >= 0.4f;

    /// 星点贴图:游戏里身上那些细碎星光。共享图 `Tex_PetGlassyStar_004` 一类。
    ///
    /// **几乎每个宠物材质都挂着这张图,但绝大多数并没有真的启用它**——游戏靠静态开关
    /// 与遮罩通道决定要不要叠。半透族那道门现在**读出来了**(见 `TransStarGate`);
    /// 不透明族(`M_P_Object`)那道是**逐像素**的 —— shader 51377 里整段星点包在
    /// `if (法线贴图.a >= 0.4)` 里(`sample_l r3.xyzw, v2.xyxx, t3` 之后 `mad r3.xy, r3.xy, 2, -1`
    /// 再 `nz = sqrt(1-x²-y²)`,是标准切空间法线,所以 t3 就是法线图、`.w` 是塞在里面的遮罩),
    /// 运行时还没实现,所以那一族仍然退回下面这条启发式:
    /// 「美术是否显式设了向量 `StarStickTiling`」——设了(暮星辰的裙子 = 4×4)才当启用。
    /// 一开始无条件叠,结果整只宠物被星点冲白。
    ///
    /// 这里**故意只查向量**那份 `StarStickTiling`:平铺该读标量(见 `StarTiling`),但把标量
    /// 也算进这个门会让更多材质新启用这一层 —— 那是未经验证的行为改动,没做。
    public string? StarTexture =>
        !TransStarGate ? null
        : IsXiaoYou ? FirstTexture("StarTex")
        : Vectors.ContainsKey("StarStickTiling") ? FirstTexture("StarStickTex", "ShinyStarTex", "StarTex")
        : IsFakeTrans ? FirstTexture("NoiseTex", "Noise")
        : null;

    /// **这个材质的图里到底有没有星贴层。** 判据是**读出来的**:参数名表(uexp 里 shader map
    /// 自带那张,见 rocom-capture 的 `scripts/matparams.py`)就是「这个图实际用到哪些参数」,
    /// 而眼睛/嘴走的 `M_P_Eyes` 整张表只有 42 条、**一个 `Star*`/`Stick*` 都没有** ——
    /// 所以那两个槽压根不可能有这一层。
    ///
    /// 之所以要这道门:下面「一个形态只有一份星点遮罩」那段统一会把星点盖到**所有**材质上,
    /// 连眼睛和嘴一起刷。星光族三只实测就是这样(包里 `_Es`/`_Mh` 也带着 `star_tex`)。
    public bool GraphHasStickLayer =>
        RootDefaults is { } rd
        && (rd.Scalars.ContainsKey("Stick_Intensity") || rd.Scalars.ContainsKey("StarStickTiling"));

    /// 星点平铺(前两位是 uv 平铺)。
    ///
    /// **`StarStickTiling` 在材质图里同名存在标量与向量两份**,根默认是**标量** 4。汇编里星点
    /// 的采样是 `mul rX.zw, v2.xxxy, cb6[130].w` —— 网格 UV0 乘**一个标量**,u/v 同一个数,
    /// 所以标量那份才是它,向量那份是同名的另一个参数。
    ///
    /// 原来只查向量表,于是幽星光一族(标量覆盖 5.3 / 向量 (4,4) / 无覆盖)全掉进
    /// `NoiseTilingSpeed` 兜底,拿到 1.8/2.5 —— 偏小一半，运行时靠一个手挑的 ×3 补回来。
    /// 现在按标量优先,三只得到 4 / 5.3 / 4(与用户目视「三只星点大小间距差不多」一致),
    /// 运行时那个 ×3 也就撤掉了。
    public float[] StarTiling =>
        Scalars.TryGetValue("StarStickTiling", out var s) && s > 0 ? [s, s]
        : Vectors.TryGetValue("StarStickTiling", out var v) && v[0] > 0 ? [v[0], v[1]]
        : RootDefaults?.Scalars.TryGetValue("StarStickTiling", out var r) == true && r > 0 ? [r, r]
        : IsFakeTrans && Vectors.TryGetValue("NoiseTilingSpeed", out var n) && n[0] > 0 ? [n[0], n[1]]
        : [1f, 1f];

    /// **炫彩玻璃层的逐材质标量。** 顺序与 `pack::MaterialSpec::glassy_params` 一致:
    /// `GlobalRefraction / GlobalDepth / MainTexTiling / MainTexFlowSpeedX / MainTexFlowSpeedY /
    /// NormalEffectAmount / BaseColorDetail / FlowColorIntensity / StarTiling / StarDensity /
    /// StarIntensity`。
    ///
    /// **这几条不是全库一份根默认,是逐材质调过的。** lua 给常规炫彩只覆盖两个 Channel 色、
    /// `StarIntensity` 与 `StarStickTex`(见 `processMaterial`),这几个标量一个都不碰 ——
    /// 所以实机用的就是材质实例自己那份。实测差得很远:加油海葵 `_By` 的
    /// `MainTexTiling = 0.2`(根默认 1.5,差 7.5 倍),而鸭吉吉/火神/白金独角兽都是根默认;
    /// `GlobalRefraction = 1.3` / `GlobalDepth = 100` 则是四只全都覆盖过(根默认 2.0 / 30)。
    ///
    /// 运行时原来一律按根默认画,于是小宠物的花纹细得多 —— 实机加油海葵整只是一个颜色
    /// 在绿↔蓝之间慢慢扫,我们那版身上横着一道绿蓝分界。
    ///
    /// 图里没有这一层的材质(眼睛/嘴那族)返回空数组,不写进 manifest。
    public float[] GlassyScalars => !GraphHasStickLayer ? [] :
    [
        RootScalar("GlobalRefraction", 2f),
        RootScalar("GlobalDepth", 30f),
        RootScalar("MainTexTiling", 1.5f),
        RootScalar("MainTexFlowSpeedX", 0f),
        RootScalar("MainTexFlowSpeedY", 0.1f),
        RootScalar("NormalEffectAmount", 0.1f),
        RootScalar("BaseColorDetail", 0.35f),
        RootScalar("FlowColorIntensity", 1.2f),
        // 闪点层(高质量那条排列独有,见 pet/shader/70-glassy.wgsl 的 `glassy_sparkle`):
        // `StarTiling` 定格子大小、`StarDensity` 定密度、`StarIntensity` 定亮度。
        // 全库暂时没见到覆盖过的(鸭吉吉/加油海葵都是根默认),照样逐材质导,免得又踩
        // 「以为是全库一份」那个坑。
        RootScalar("StarTiling", 0.4f),
        RootScalar("StarDensity", 8f),
        RootScalar("StarIntensity", 1f),
    ];

    /// **炫彩那圈边缘光**:`[RimColor.rgb, RimIntensity]`。
    ///
    /// 注意和上面那个 [`RimColor`](= `Rim LightColor`)/[`RimIntensity`](= 带空格的
    /// `Rim Intensity`)**不是同一组参数** —— 玻璃层读的是不带空格的 `RimColor` 与
    /// `RimIntensity`(鸭吉吉两个都没覆盖,用根默认 (0.844, 0.961, 1) 与 1.5)。
    ///
    /// **lua 写的那个 `MutationRimColor` 不在这条排列里**(向量参数表逐条查过),
    /// 和当年的 `StarIntensity` 一个处境:设了,但这份编译产物根本不读。
    /// 所以运行时原来那句「`MutationRimColor` 是 lua 里写死的 (0.6,0.6,0.6)」是错的。
    public float[] GlassyRim => !GraphHasStickLayer ? [] :
    [
        .. (Vectors.TryGetValue("RimColor", out var rc) ? rc[..3]
            : RootVector("RimColor")?[..3] ?? [0.84375f, 0.961117f, 1f]),
        RootScalar("RimIntensity", 1.5f),
    ];

    /// **炫彩星贴层的平铺** —— 就是这个材质自己的标量 `StarStickTiling`([`StarTiling`] 的第一位)。
    ///
    /// 单开一条是因为 `StarTiling` 那份在 Program.cs 里会被**跨材质统一**(一只宠物只留一份
    /// 星点层),而且只写给真开了星点层的材质;炫彩这一层是运行时另外打开的,要的是
    /// **每个材质自己那份**,哪怕它平时不画星点。
    ///
    /// **不能拿 `PARTICLE_RANDOM_CONF.StarStickTiling`(2.2 / 1.0)去顶**:lua 里那句
    /// `starStickTiling = particleConf.StarStickTiling` 包在 `IsGlassyRandomEgg` 里 ——
    /// 只有随机蛋会写这个标量,宠物身上材质里那份原封不动。鸭吉吉 `_By` 写着 4.11,
    /// 接成 2.2 的话粒子会大近一倍、密度只剩四分之一。
    ///
    /// 图里没有星贴层的材质(眼睛/嘴那族)返回 0 —— 那种材质不写这一条。
    public float GlassyStarTiling => GraphHasStickLayer ? StarTiling[0] : 0f;

    /// **逐 `MatID` 的高光**:四档 `(SpecPow, SpecIntensity, SpecRadius)`,加上 `SpecColor`。
    /// 四档强度**全是 0** ⇒ 返回空(这一层不出场,全库 2539 份里 2326 份如此)。
    ///
    /// ## 这一层只在 quality=Num 那条排列里
    ///
    /// 鸭吉吉 `_By` 的 Low 排列(PS 17314,220 行)与 Num 排列(PS 8409,333 行)**参数级
    /// diff 只多一个 `SpecColor`**,多出来的 113 条指令就是这一块。实机跑 Num,所以要接。
    ///
    /// ## 公式(PS 8409 第 192~221 行)
    ///
    ///     挡位 = floor(min((1 − MaskTex.a) × 5 + 1, 5))          // 与描边同一张图同一套刻度
    ///     (Pow, Int, R) = 挡位 1 ? (0.35, 0.001, 0.5)            // 第 1 档是**硬写的立即数**
    ///                            : (SpecPow_n, SpecIntensity_n, SpecRadius_n)   // n = 2..5
    ///     α    = Pow²
    ///     D    = min((α / ((N·H)²(α² − 1) + 1))², 2048)          // 就是 GGX,少了 1/π
    ///     k    = 0.25 × Pow + 0.25
    ///     v    = lerp(saturate(k·D), k·D, R²)
    ///     edge = saturate((saturate((k·D + 0.5) × 0.5) − (0.49 − R)) / (2R))
    ///     out  = 基色 × 光照 × (1 + Int × SpecColor × v × edge)   ← **乘性加亮**,不是加一层白
    ///
    /// `R = 0` 时 `edge` 退化成 `k·D > 0.48` 的**硬阶跃** —— 那就是点点身上那种硬边高光块;
    /// `R = 1` 时是一片宽而柔的加亮(蛋煲蛋)。四个槽位的顺序**是按用法定的、不是猜的**:
    /// `.x` 进 `α = x²` 与 `k`、`.y` 是纯倍数、`.z` 进 `0.49 ± z` 的软边宽度、
    /// `.w` 是 `RampID`(去查那张色带图,**我们没有那条链,所以不导**)。
    ///
    /// 第 1 档那个 `0.001` 让「没设过的部位」实际等于关闭(实测加亮量 0.006)。
    public float[][] SpecSlots
    {
        get
        {
            float[] Slot(int n) =>
            [
                RootScalar($"SpecPow{n}", 1f),
                RootScalar($"SpecIntensity{n}", 0f),
                RootScalar($"SpecRadius{n}", 0.5f),
            ];
            var slots = new[] { Slot(2), Slot(3), Slot(4), Slot(5) };
            return slots.Any(s => s[1] != 0f) ? slots : [];
        }
    }

    /// 上面那一层的染色。根默认白;`SpecSlots` 为空时不写。
    public float[] SpecColor =>
        Vectors.TryGetValue("SpecColor", out var c) ? [c[0], c[1], c[2]]
            : RootVector("SpecColor")?[..3] ?? [1f, 1f, 1f];

    /// `MaskTex` —— 这一张同时装着三样东西,而我们原来只用了 alpha:
    ///
    /// | 通道 | 是什么 | 用在哪 |
    /// | --- | --- | --- |
    /// | **RG** | **切线空间法线**(`xy`,`z` 由 `sqrt(1 − x² − y²)` 补出) | PS 8409 第 44~52 行,喂主光照的 `N·L` |
    /// | B | 明暗覆写(`> 0.95` 强制全亮、`< 0.05` 强制全暗;平时进 `0.5·N·L + b` 当偏置) | 全库实测恒为 **0.298**,当覆写用是死的 |
    /// | A | `MatID`,炫彩区域门与描边/高光的五档都读它 | 见 `OutlineOf` 与 `SpecSlots` |
    ///
    /// 抽 14 张 `_By_M` 量过:**14 张全都带真实的法线扰动**(nx/ny 的 p2~p98 到 ±0.3~±0.7),
    /// 不是一张平图。
    ///
    /// **只在有基色贴图的材质上写**:纯特效层(火焰/水壳/光晕)与几个专用族的 `MaskTex`
    /// 装的不是法线(`M_P_MatCap_Masked` 那族的 `base_color` 槽绑的就是查找表),
    /// 拿去当法线会把整块面翻掉。
    public string? MatIdTexture =>
        BaseColorTexture is null ? null : FirstTexture("MaskTex", "Mask");

    /// 星点层的强度。**根材质里叫 `Stick_Intensity`(默认 1.5)** —— 运行时原来写死 0.3,
    /// 那是手挑的。名字现在查实了(参数名哈希,见 RootDefaults.cs)。
    ///
    /// 顺带一条**查实后决定「不改」**的:那 4 个 `StickRandomColor01..04`
    /// (红橙/品红/蓝/金黄)不是给 shader 用来重新上色的 —— 共享星点图
    /// `Tex_PetGlassyStar_004` 里的颜色块本来就是红/橙/黄(R≈0.94、B≈0.06),
    /// 其中两种正好等于 `StickRandomColor01`(0.946,0.064,0.021)与
    /// `04`(0.925,0.742,0.027),而 `02`(品红)`03`(蓝)在贴图里根本不出现。
    /// 所以颜色是烘在贴图里的,运行时「用贴图 rgb」是对的,别去按那 4 个色重上。
    public float StickIntensity => RootScalar("Stick_Intensity", 1.5f);

    /// 星点着色。「假半透」族用 `Color02`,但那是**配着别处的衰减用的 HDR**
    /// (幽星光 = (15,15,15)、暮星辰 = (14.8,11,15)),直接乘会糊成一片白;
    /// 只取它的色相(按最大通道归一化),亮度交给运行时那一档固定系数。
    public float[]? StarColor
    {
        get
        {
            if (FirstVector("StarColor", "StarStickColor") is { } c) return c;
            // **HDR 量级要保留,不能归一化。** 原来除掉了峰值(曜星光 `Color02` =
            // (10, 8.07, 9.04) ⇒ (1, 0.807, 0.904)),那是因为当时运行时的强度写死 1.5、
            // 不除会糊白。现在强度是**读出来的** `Mat_NoiseIntensity` = 0.05,
            // 而它本就是配着原始 HDR 值用的:
            //     实机 (10, 8.07, 9.04) × 0.05 = (0.50, 0.40, 0.45)
            //     归一化后          × 0.05 = (0.05, 0.04, 0.045)   ← 暗十倍
            // 用户实测「幽星光身上的星点不明显」就是这十倍。
            if (!IsFakeTrans || FirstVector("Color02") is not { } hdr) return null;
            return [hdr[0], hdr[1], hdr[2], 1f];
        }
    }

    /// MatCap:球面反射查找表。暮星辰那两个球的玻璃感就是它 + `MatCapColor=(3,3,3)` 的 HDR 白。
    ///
    /// **判据直接用静态开关 `是否使用MatCap`**(全量 17 个材质开着)。很多材质的 MatCap 槽绑的
    /// 压根不是反射图(幽星光的 `By` 绑的是 `Fx_ID` 描边图),无条件当高光叠会把宠物冲成一片白。
    /// 原来拿「美术有没有显式设 `MatCapColor`」当判据,数目正好也是 17 个但对错各有两处
    /// (多算了果冻与翡翠水母、漏了莫比乌乌与风铃鲨三阶)—— 开关是明写的答案,不必再推断。
    public string? MatcapTexture =>
        IsFakeFluid
            ? FirstTexture("MatCapTex")
            : Switch("是否使用MatCap") ? FirstTexture("MatCap", "MatCapTex") : null;

    public float[]? MatcapColor => FirstVector("MatCapColor");

    /// 边缘光颜色/强度。暮星辰的球有一圈紫边(`Rim LightColor` = (0.67, 0.11, 1))。
    public float[]? RimColor => FirstVector("Rim LightColor", "RimLightColor", "FresnelColor");

    public float RimIntensity => Scalar("Rim Intensity", Scalar("RimIntensity", 0f));

    /// **玻璃内部那颗星**:`StarTex`(根默认 = `T_EMeng003`,一张四角星场、alpha 是干净的
    /// 稀疏星形遮罩)沿**折射光线**在物体空间 march、按三向投影采样,坐标再叠时间卷动。
    /// 读 shader 汇编读出来的(见 docs/findings.md §1):`refract()` 的教科书实现 + triplanar
    /// + `View` 的时间项 —— 这就是实机里「球内有颗星、自己在动、和球自转无关」的来源。
    ///
    /// 贴图取自根材质默认值:没有任何实例覆盖 `StarTex`,顺父链是看不见它的。
    ///
    /// **采样起点是 `(UV1.x, UV1.y, UV2.x)`,从 shader 里查出来的**:片元着色器里那句
    /// `r4.xy = v2.zw; r4.z = v3.x`,配 DXBC `ISGN` 签名段(`v2` = TEXCOORD0、`v3` = TEXCOORD1)
    /// 与 UE 的 UV 打包规则(TEXCOORD0 = UV0.xy + UV1.xy、TEXCOORD1 = UV2.xy + UV3.xy)。
    /// 顶点侧见 model.rs 的 `Vertex::interior_pos`。
    ///
    /// **判据是「直接父就是 `MI_P_Object_Trans_MatCap`」,不能用「父链里有」。**
    /// 暮星辰那两颗球的直接父是 `..._Trans_XingGuang_Fresnel`(它自己的父才是 Trans_MatCap),
    /// 而它的 shader 反汇编下来是**完全另一套**:223 行、4 张贴图、`N·L` + 遮罩 →
    /// 一维 `RampTex` 行查(采样 v 是常数 1/256),既没有折射也没有三向投影。
    /// 拿同一套画法套上去是错的 —— 按「父链里有」判会把它也算进来(踩过)。
    /// ~~曾因「区域白闪」关掉过~~ **已重新开启**:那个白闪的根因是上游把切线写进了 NORMAL
    /// (见 docs/design.md 法线那条),不是这一层的问题 —— 法线修好后它就稳了。
    public string? InteriorTexture =>
        ParentChain.FirstOrDefault() == "MI_P_Object_Trans_MatCap"
            ? RootDefaults?.Textures.GetValueOrDefault("StarTex")
            : null;

    /// 内部星光的着色:**`CrossStarColor`**(根默认 (0.5, 0.1, 0.8) 紫)。
    ///
    /// **原来取的是 `StarColor`(0.33, 0.67, 2) —— 错的。** 汇编里球内星层那一步是
    /// `lerp(底, cb5[41], 星点强度)`,而 `cb5[41]` 在 `MI_Ill_XingGuang1_001_Fx1` 块 10 里
    /// 解出来是 `CrossStarColor`;`StarColor` 根本不在这条链上。
    /// 早先一版把 `cb5[36]` 读成 `StarColor` 并据此说「运行时用 StarColor 是对的」——
    /// 那是解析 bug 造成的**槽位名整体错位一格**(见 rocom-capture 的 `uniexpr.param_pair`),
    /// 修完 `cb5[36]` 是 `BlackMagicRimColor`。全库没有实例覆盖过 `CrossStarColor`。
    public float[]? InteriorColor =>
        FirstVector("CrossStarColor") ?? RootVector("CrossStarColor")
        ?? FirstVector("StarColor") ?? RootVector("StarColor");

    /// 折射率与 march 深度。这两个每个宠物材质都写着(1.3 / 100)。
    ///
    /// **`GlobalDepth` 的量纲是从汇编定出来的**:`marchDist = |半包围盒| × 0.01 × GlobalDepth`,
    /// 代 100 进去正好等于 `|半包围盒|` —— 那个 0.01 的配合是这个槽位就是 `GlobalDepth` 的强证据。
    public float Refraction => Scalar("GlobalRefraction", 1f);

    public float RefractDepth => Scalar("GlobalDepth", 0f);

    /// **球内那颗星的闪烁**:汇编里是 `pow(星场.B, q) - 1.2 × |sin(2π × frac(速度×时间 + 星场.G))|^p`,
    /// 再乘星场的 A(星形遮罩)。也就是说**星点不是在移动、是在一明一暗地闪**,
    /// 而每颗星的相位来自贴图的 G 通道(实测 T_EMeng003 的 G 均值 0.328、取值分散,正合此用)。
    ///
    /// 速度与次数取根材质里**语义对得上的命名默认值**:`FlickerSpeed` = 0.3、`FlickerPower` = 5。
    /// 这是语义匹配,不是靠 cb 槽位证实的(槽位↔参数名还没打通,见 docs/design.md)。
    public float FlickerSpeed => RootScalar("FlickerSpeed", 0.3f);

    public float FlickerPower => RootScalar("FlickerPower", 5f);

    /// 查一个标量:**实例链 → 实机排列的编译期默认 → 根材质 `CachedExpressionData` → 兜底**。
    /// 中间那一层是后加的,理由见 `ShaderDefaults` 那一格的注释。
    private float RootScalar(string name, float fallback) =>
        Scalars.TryGetValue(name, out var v) ? v
        : ShaderDefaults?.Scalars.TryGetValue(name, out var s) == true ? s
        : RootDefaults?.Scalars.TryGetValue(name, out var r) == true ? r
        : fallback;

    /// 查一个向量的**默认值**(不含实例链 —— 调用方那边先 `FirstVector` 再落到这儿)。
    /// 层序与 `RootScalar` 同。
    private float[]? RootVector(string name) =>
        ShaderDefaults?.Vectors.GetValueOrDefault(name)
        ?? RootDefaults?.Vectors.GetValueOrDefault(name);

    /// 半透族的颜色边缘参数。根材质默认是 0.4 / 0.3,实例会逐只覆盖
    /// (果冻 = 1.4 / 0.2)。目标实机选中的 Low alpha 链不读取这层；它只用于颜色。
    public float RimPower => RootScalar("Rim Power", RootScalar("RimPower", 0.4f));

    public float RimSoftEdge => RootScalar("Rim Soft Edge", 0.3f);

    /// 半透族高光覆盖率。公式来自 `M_P_Object_Trans` 的有/无方向光两个排列:
    /// `smoothstep(.4, .5, pow(max(N·H, 0), HighLightSpecPow)) × HighLight SpecInt`。
    /// Offset 是 UE 的 Z-up 向量,导出时换成运行时 glTF 的 Y-up `(x,z,y)`。
    public float[] HighlightOffset
    {
        get
        {
            var v = FirstVector("HighLight Offset")
                    ?? RootVector("HighLight Offset")
                    ?? [0f, 0f, 0f, 1f];
            return [v[0], v[2], v[1]];
        }
    }

    public float[] HighlightSpecColor
    {
        get
        {
            var v = FirstVector("HighLight SpecCol")
                    ?? RootVector("HighLight SpecCol")
                    ?? [1f, 1f, 1f, 0f];
            return [v[0], v[1], v[2]];
        }
    }

    public float HighlightSpecPower => RootScalar("HighLightSpecPow", 10f);

    public float HighlightSpecIntensity => RootScalar("HighLight SpecInt", 1f);

    /// 0 = 使用 max(基础 alpha,高光覆盖),1 = 强制退回基础 alpha。
    public float ForceUseDefaultOpacity => RootScalar("ForceUseDefOpacity", 0f);

    /// `M_P_Object_Trans` 场景深度淡化的距离(UE 厘米)。实机 ES3.1/Low 的 LOD0
    /// shader 在基础 alpha / 高光覆盖之后计算
    /// `saturate((sceneDepth - pixelDepth) / OpacityDepthDistance)`。
    public float OpacityDepthDistance => RootScalar("OpacityDepthDistance", 40f);

    /// 是否把上面的深度淡化加进 alpha；果冻实例明确覆盖为 1。
    public float OpenDepthDistance => RootScalar("OpenDepthDistance", 0f);

    /// 目标设备的 ES3.1/Low 基础半透明排列。只认**经由 `MI_P_Object_Trans` 这个共同中间父**
    /// 的那一支:果冻外壳、春兔耳膜与同类 `_WPO` 变体。
    ///
    /// **`_XingGuang_*` 与 `_Trans_MatCap` 必须挡掉。** 它们的父链里**也有**
    /// `MI_P_Object_Trans`(例如 `MI_Ill_XingGuang3_001_Fx1` 是
    /// `[_Trans_XingGuang_WPO, _Trans_WPO, MI_P_Object_Trans, M_P_Object_Trans]`),
    /// 所以只写 `ParentChain.Any(== "MI_P_Object_Trans")` 拦不住 —— 得按名字排除。
    /// 而「只认直接父」也不行:**果冻自己的直接父就是 `_WPO`**。
    ///
    /// 挡不住的代价量过:暮星辰的裙子会走上这条为果冻反汇编出来的短链,
    /// 调色板距离 0.068 → 0.082。这条 shader map(`ACB16DBC…`)只对果冻外壳验过,
    /// 别往没验过的分支上摊。
    public bool IsObjectTransLow =>
        IsTranslucent
        && BaseColorTexture is not null
        && ParentChain.Any(p => p.Equals("MI_P_Object_Trans", StringComparison.OrdinalIgnoreCase))
        && !ParentChain.Any(p => p.Contains("XingGuang", StringComparison.OrdinalIgnoreCase)
                                 || p.Contains("MatCap", StringComparison.OrdinalIgnoreCase));

    /// Low shader 2109/55790 的 t3/t4；绑定顺序由 cooked
    /// `UniformTextureParameters` 明确给出，而不是按文件名猜。
    public string? ObjectTransLightMaskTexture =>
        IsObjectTransLow ? FirstTexture("MaskTex") : null;

    public string? ObjectTransRampTexture =>
        IsObjectTransLow
            ? FirstTexture("RampTex") ?? RootDefaults?.Textures.GetValueOrDefault("RampTex")
            : null;

    /// `cb6[26].y`，原图中以 `0.1 * SoftEdge` 作为明暗过渡宽度。
    public float ObjectTransSoftEdge => RootScalar("SoftEdge", 0.5f);

    /// 原 shader 尾部 `cb6[29].xyz = MainColor.rgb * MainBright`。
    public float[] ObjectTransMainColor =>
        FirstVector("MainColor")
        ?? RootVector("MainColor")
        ?? [1f, 1f, 1f, 1f];

    public float ObjectTransMainBright => RootScalar("MainBright", 1f);

    /// **假半透族那层星点不走网格 UV0。** 材质图里有个明写的开关 `UseNoiseUV0`,根默认 **0**;
    /// 配套 `Mat_NoiseTilingX/Y = 5 / 2.5`、`Mat_NoiseSpeedX/Y = 0.1 / -0.1`、
    /// `Mat_NoiseIntensity = 0.05`。四条实机观察逐条对上:很淡、像蒙在镜头前、
    /// 拖动旋转时星点不随着转、略微上浮(`SpeedY` 为负 ⇒ 采样坐标下移 ⇒ 图案上浮)。
    ///
    /// **必须用 `RootScalar` 取。** 这几个参数全库没有任何实例覆盖过,只存在于根默认里,
    /// 而 `Scalar()` **不查根默认**(那是刻意的,见 `RootDefaults` 那条注释)。
    /// 踩过两次:两轮都拿到兜底值 `[0,0,1,1]`,还一度误判成"解包数据里没有"。
    public float[] NoiseUv =>
    [
        RootScalar("Mat_NoiseSpeedX", 0f), RootScalar("Mat_NoiseSpeedY", 0f),
        RootScalar("Mat_NoiseIntensity", 1f), RootScalar("UseNoiseUV0", 1f),
    ];

    /// 同上那套里的平铺;0 = 这个材质没有这套参数。
    public float[] NoiseTiling => [RootScalar("Mat_NoiseTilingX", 0f), RootScalar("Mat_NoiseTilingY", 0f)];

    private float Scalar(string name, float fallback = 0f) =>
        Scalars.TryGetValue(name, out var v) ? v : fallback;

    /// 取一个向量参数的 rgb;没有就用给的兜底(**不跳过纯白**,这里纯白是有意义的值)。
    private float[] Color(string name, float[] fallback) =>
        Vectors.TryGetValue(name, out var v) ? [v[0], v[1], v[2]] : fallback;

    /// 同上,但**只看材质自己写的**(见 `OwnVectors` 的说明);没写就用兜底,
    /// 而兜底要填**那条编译排列的默认值**,不是随手写个白。
    private float[] OwnColor(string name, float[] fallback) =>
        OwnVectors.TryGetValue(name, out var v) ? [v[0], v[1], v[2]] : fallback;

    private float[]? FirstVector(params string[] names) =>
        names.Select(n => Vectors.TryGetValue(n, out var v) ? v : null).FirstOrDefault(v => v is not null);

    /// 同 `FirstVector`,但**优先跳过纯白**(纯白是颜色参数的中性默认值,不带信息)。
    private float[]? FirstColor(params string[] names)
    {
        var present = names.Select(n => Vectors.TryGetValue(n, out var v) ? v : null)
            .Where(v => v is not null)
            .ToList();
        return present.FirstOrDefault(v => v![0] < 0.99f || v[1] < 0.99f || v[2] < 0.99f)
               ?? present.FirstOrDefault();
    }

    private string? FirstTexture(params string[] names) =>
        names.Select(n => Textures.TryGetValue(n, out var v) ? v : null).FirstOrDefault(v => v is not null);
}

public static class Materials
{
    /// UE 默认的遮罩阈值;材质没覆盖时用它。
    private const float DefaultMaskClip = 0.3333f;

    /// 解析**网格自己声明的**材质槽。键是材质对象名,与 glb 里的材质名一致。
    ///
    /// 为什么不去列 `<资产>/Mat/` 目录:那是个不成立的假设。小浣蛋(`Dem_XiaoHuanDan1_001`)
    /// 的 `Mat/` 里只有描边材质,本体材质根本不在那儿;还有些资产把材质放在 `Yise/Mat/`
    /// (异色变体)之类的子目录。网格的 `Materials` 数组是权威来源:它按槽序给出材质对象,
    /// 不管对象存在哪个包里。实测这一改把 13 个「材质表为空」的形态全救回来了。
    /// 解析一个材质实例(顺父链合并 + 补根默认 + 描边宽度)。
    ///
    /// **`key` 与 `material.Name` 可以不同**:异色走的是另一套材质,但要按**默认槽**的
    /// 对象名登记 —— glb 里的材质名是默认那套(见 `Shiny`)。
    public static MaterialInfo ResolveInstance(string key, UMaterialInstance material)
    {
        var roots = RootMaterial.Of(material);
        var outline = OutlineOf(material);
        var info = Resolve(key, material) with
        {
            RootDefaults = roots,
            // 只有 `--probe-material` 与导出主流程会打开 `ReadShaderMaps`,别处拿到的是
            // `Empty` —— 那时整条链退回 `RootDefaults`,和加这一层之前一模一样。
            ShaderDefaults = ShaderMapDefaults.Of(material),
            OutlineWidth = outline?.Width,
            OutlineHeightRatio = outline?.HeightRatio,
            OutlineColors = outline?.Colors,
            OutlineIdTexture = outline?.IdTexture,
        };
        // **实例没覆盖混合模式时,用根材质自己的。** 实例侧的 `BLEND_Opaque` 是 0,
        // 与「没写」不可区分(见 `Resolve` 里那条注释),所以直接挂在根材质上的
        // 材质会被一律当成不透明 —— 幽火的 `M_Gho_XiaoYou_GhostFire` 就是这样,
        // 于是外层壳把里面那层小水滴整个盖住(实机是两层)。
        if (info.BlendMode == EBlendMode.BLEND_Opaque
            && roots.BlendMode != EBlendMode.BLEND_Opaque)
            info = info with { BlendMode = roots.BlendMode, OpacityMaskClipValue = roots.MaskClip };
        return info;
    }

    public static Dictionary<string, MaterialInfo> Load(
        USkeletalMesh mesh,
        List<string> warnings)
    {
        var result = new Dictionary<string, MaterialInfo>(StringComparer.OrdinalIgnoreCase);
        foreach (var slot in mesh.Materials)
        {
            if (slot is null) continue;
            try
            {
                if (slot.Load() is not UMaterialInstance material)
                {
                    // 悬空引用:网格声明了这个材质,但 pak 里没有对应资产(实测 13 个形态如此,
                    // 如小浣蛋的 `MI_Dem_XiaoHuanDan1_001_By`)。仍然登记一条空的,
                    // 让导出器能按名字去凑基色贴图,免得整只宠物画不出来。
                    warnings.Add($"材质 {slot.Name} 在 pak 里没有资产(悬空引用),退回按贴图名接基色");
                    result[slot.Name] = new MaterialInfo(slot.Name, [], [], [], [], [], [],
                        EBlendMode.BLEND_Opaque, DefaultMaskClip, [], Resolved: false);
                    continue;
                }
                // **键用对象名。** 本作的 pak 里对象名与资产文件名的大小写能对不上,方向还不一致:
                // 喵呜的文件是 `MI_Gra_MiaoMiao2_001_By`、对象名是 `…Miaomiao2…`,魔力猫正好反过来。
                // glb 里的材质名取的是对象名,键不一致运行时就查不到 → 整只宠物一片都画不出来。
                var key = material.Name;
                if (!string.IsNullOrEmpty(key)) result[key] = ResolveInstance(key, material);
            }
            catch (Exception e)
            {
                warnings.Add($"材质槽 {slot.Name} 解析失败: {e.Message}");
            }
        }
        return result;
    }

    /// 配套 `_Ol` 描边材质算出来的描边宽度(**米**);没有 `_Ol` 返回 null。
    ///
    /// ## 宽度是从描边 VS 读出来的,不是调出来的
    ///
    /// `M_P_Outline` 的顶点着色器(垂头鸟 `_Ol` 的 ES3.1/Low/LOD0/Switch0 排列,
    /// resource `984D6A92…`,shader 69972)最后把顶点在**裁剪空间**沿投影后的法线推开:
    ///
    ///     clip.xy += 0.01 × OutlineWidthPC × max(|LocalToWorld 三行|)
    ///                     × lerp(1, 顶点色, IgnoreVertexColor)
    ///                     × lerp(1, atan(ClipToView[0][0]) × 1.283426, DistanceUniform)
    ///                     × clamp(clip.w, MinWidthScale, MaxWidthScale) × (ViewProj·N).xy
    ///
    /// 三项化简:
    ///
    /// - `max(|LocalToWorld|)` 是**物体缩放**,而我们的网格坐标就是局部坐标 ⇒ 这一项正好抵消,
    ///   宽度与 `model_scale` 无关;
    /// - `lerp(1, 顶点色, IgnoreVertexColor)`:`IgnoreVertexColor` 全库 845 份是 0 ⇒ 这一项 = 1;
    /// - `DistanceUniform` 那一项是 FOV 补偿。它**和投影自己的 `P00` 会抵掉**:
    ///   `(ViewProj·N).xy` 里带一个 `P00 = 1/tan(半水平FOV)`,而补偿项是
    ///   `atan(tan(半水平FOV)) × 1.283426`,两者相乘 = `1.283426 × atan(t)/t`,
    ///   在 t ∈ (0, 1] 上只有 **1.00~1.28** —— 这条公式本来就设计成与 FOV 无关。
    ///
    /// ## 两个区间:`clamp(clip.w, …)` 决定这圈是屏幕空间常数还是世界空间常数
    ///
    /// 剩下的 `clamp(clip.w, Min, Max)` 除以透视除法的那个 `w`:
    ///
    /// - **`Min < Max`(851/854)**:相机落在区间内时 `clamp(w)/w = 1` ⇒ NDC 偏移与距离无关
    ///   ⇒ **屏幕空间常数**。
    /// - **`Max ≤ Min`(4 份)**:`clamp` 退化成常数,NDC 偏移 ∝ 1/w ⇒ **世界空间常数**。
    ///   火源那 3 份 `Min = Max = 200` 就是刻意做成这样的;呜呜 `_Fx` 的 `Max = 0`
    ///   ⇒ 常数 0 ⇒ 不画描边。**「Min=Max 是个特例」本身就是「默认那支不是世界空间常数」的旁证。**
    ///
    /// ⚠ **原来只推了后一支,把全库都当成世界空间常数的 0.0039 米,那是错的。**
    /// 错法很隐蔽:大宠物上只差 1.3 倍,小宠物上差 5 倍 —— 莫比乌乌(体高 27.6cm)的面条身子
    /// 会被一圈明显的黑边裹住,而实机那张几乎看不到描边。以前看不出来是因为描边取的是
    /// 「固有色 × 0.80」,在浅色身体上本来就近乎隐形;**把颜色按汇编改对之后才露出来**。
    ///
    /// ## 屏幕空间常数怎么落到我们的正交相机上
    ///
    /// 我们的取景是「把包围盒最长边铺满画布」(见 `pet::gpu::framing_radius`),所以
    /// **世界宽度 ÷ 宠物世界高度 = 描边像素 ÷ 宠物像素高**,与画布大小、取景余量都无关。
    /// 于是屏幕空间常数在我们这儿的等价形式就是「占这个形态包围盒高度的固定比例」,
    /// 由 `OutlineRatioPerPc × OutlineWidthPC` 给出,调用方乘上 `height_cm` 得米。
    ///
    /// 12 只有实机截图的宠物量下来(暗环面积反推宽度,4×SSAA + 同尺度),
    /// 三条候选律「该是常数」的那一列的离散度:
    ///
    /// | 律 | 中位 | max/min | 变异系数 |
    /// | --- | --- | --- | --- |
    /// | 世界空间常数(原实现) | 0.227 厘米 | 7.06 | 0.46 |
    /// | 屏幕空间常数(同一张 1440 画面里的像素) | 1.62 px | 4.05 | 0.37 |
    /// | 占宠物自身高度(= 我们这儿的等价形式) | **0.255%** | **3.42** | 0.38 |
    ///
    /// 原实现那条明显最差。后两条在统计上分不开(我们的取景下它们本来就是同一件事),
    /// 而汇编支持的是它们,所以照它改。估计量本身不干净(姿势、投影、特效层都会掺进来),
    /// 别拿这三个数去推更细的结论。
    ///
    /// ## 有一条看着像宽度、其实是死设定的参数
    ///
    /// 854 份 `_Ol` 里有 851 份写着 `OutLine Offset = 0`(父级 `MI_P_Outline` 写 0.7)。
    /// **它不生效**:`M_P_Outline` 的参数表里根本没有这个名字(哈希对不上,
    /// `--probe-material OUTLINES` 会把这类「死设定」单独列出来),UE 是按参数名查的,
    /// 查不到就退回根默认。照着它做会得出「全库都没有描边」的错误结论。
    ///
    /// 全库分布(`--probe-material OUTLINES`):`OutlineWidthPC` 0.13 × 848、
    /// `MaxWidthScale` 300 × 847;例外只有火源的两份(0.4/0.7 × 200)、呜呜 `_Fx`
    /// (`MaxWidthScale = 0` ⇒ 不画)、以及 3 份挂在 `M_FairyBall_BallBack` 上的(没有这套参数)。
    ///
    /// ## 颜色是五档的,按 `MatID` 遮罩挑
    ///
    /// 描边 PS 58499(鸭吉吉 `_By_Ol` 的 quality=Num / LOD0 / DSId=0 排列,resource `984D6A92…`)
    /// 第 32~41 行:
    ///
    ///     挡位  = floor(min((1 − MatID.a) × 5 + 1, 5))        // 1..5
    ///     color = OutLineOtherColor[挡位] × `Outline Intensity`
    ///     color = lerp(color, Flat_EmissiveColor × Flat_EmissiveIntensity, Flat_EmissiveRatio)
    ///     color = lerp(color, SelectionColor.rgb, SelectionColor.a)
    ///     out   = lerp(color, color × saturate(2 × 灯色亮度), OutlineIgnoreEnvColor)
    ///
    /// 后三行**全库都是恒等的**,所以只导第一行的结果:`Flat_EmissiveRatio` = 0 × 851、
    /// `SelectionColor` 全库无值(引擎的编辑器选中色,打包后是 0)、`Outline Intensity` = 1 × 851。
    /// 最后那个环境项 `OutlineIgnoreEnvColor` = 1 × 850(唯一例外呜呜 `_Fx` 是 0.35,而它
    /// `MaxWidthScale = 0` 本来就不画)—— 名字有点反直觉,它的意思是「只吃环境光的**亮度**、
    /// 不吃它的颜色」;白光下 `saturate(2 × 亮度)` = 1,我们没有等价量,取 1。
    ///
    /// **五档不是摆设**:854 份里 4 档不同的 334、5 档不同的 281、3 档不同的 198,
    /// 真·全同只有 5 份。挡位与炫彩那道门是同一张图同一套刻度 —— `MinID` = 0.4 对应
    /// 挡位 1~3(炫彩区),挡位 4/5 就是喙、脚这些非炫彩部位,所以第 5 档最常被美术改
    /// (597 种不同的值)。颜色都很暗(线性 0.0x 量级),这与实机截图量到的一致:
    /// 加益边界处有 1~2 像素明显低于背景与身体的暗环(见 docs/design.md)。
    ///
    /// `MatID` 那张图 732 份就是本体的 `MaskTex`,但**不能直接复用本体那张** ——
    /// 38 份指着另一张、81 份本体压根没有 `MaskTex`。
    /// 配套 `_Ol` 里读出来的那几件事。
    ///
    /// `Width` 是**世界空间**宽度(米);`HeightRatio` 不为 null 时改按它走 ——
    /// 「占这个形态包围盒高度的比例」,由调用方乘上 `height_cm` 得出最终宽度。
    /// 哪个生效见 `OutlineOf` 里「两个区间」那段。
    private record OutlineRead(float Width, float? HeightRatio, float[][]? Colors, string? IdTexture);

    /// 屏幕空间那一档:`OutlineWidthPC` = 1 时描边占宠物自身高度的比例。
    ///
    /// **这一个数是标定出来的,不是读出来的**,原因见 `OutlineOf`:汇编给的是
    /// 「NDC 偏移 = 0.01 × OutlineWidthPC × F」,而 NDC 要换算成「占宠物多少」还差一项
    /// **游戏那边的取景**(同一张 1440 画面里宠物占 264~1110 像素不等)—— 那是 UI 的事,
    /// 不在 shader 里。所以拿 12 只有实机截图的宠物量:
    /// 用暗环面积反推实机的描边像素宽,再除以宠物像素高,**中位 0.255%**(`OutlineWidthPC` = 0.13)。
    /// ⇒ 每单位 PC = 0.0196。
    ///
    /// 这一档下 1024 画布上典型宠物的描边约 1.4 像素,和实机同一量级。
    private const float OutlineRatioPerPc = 0.0196f;

    private static OutlineRead? OutlineOf(UMaterialInstance material)
    {
        var provider = material.Owner?.Provider;
        var package = material.Owner?.Name;
        if (provider is null || string.IsNullOrEmpty(package)) return null;
        var dir = package[..(package.LastIndexOf('/') + 1)];
        // 名字大小写在本作的 pak 里对不齐,所以两种拼法都试:
        // 对象名(`material.Name`)与包名(`material.Owner.Name` 的最后一段)。
        UMaterialInstance? outline = null;
        foreach (var stem in new[] { material.Name, package[(package.LastIndexOf('/') + 1)..] })
        {
            if (string.IsNullOrEmpty(stem)) continue;
            if (!provider.Files.ContainsKey($"{dir}{stem}_Ol.uasset")) continue;
            try { outline = provider.LoadPackageObject($"{dir}{stem}_Ol") as UMaterialInstance; }
            catch { /* 读不出来就退回全库模态值,下面兜底 */ }
            break;
        }
        if (outline is null) return null;

        var chain = Resolve(outline.Name, outline);
        var roots = RootMaterial.Of(outline);
        // 实例侧的覆盖**只有在根材质里有同名参数时才生效**(见上面「死设定」那段)。
        float Value(string name, float fallback) =>
            roots.Scalars.ContainsKey(name) && chain.Scalars.TryGetValue(name, out var v) ? v
                : roots.Scalars.GetValueOrDefault(name, fallback);
        float[]? Color(string name) =>
            roots.Vectors.ContainsKey(name) && chain.Vectors.TryGetValue(name, out var v) ? v
                : roots.Vectors.GetValueOrDefault(name);

        // 兜底用全库模态值:`M_FairyBall_BallBack` 那 3 份没有这套参数。
        var pc = Value("OutlineWidthPC", 0.13f);
        var minScale = Value("MinWidthScale", 20f);
        var maxScale = Value("MaxWidthScale", 300f);
        var width = 0.0001f * pc * maxScale;
        // `clamp(clip.w, Min, Max)` = `min(max(w, Min), Max)`。**`Max <= Min` 时它是个常数**,
        // 这一支才是世界空间常数;`Min < Max` 时相机在区间内 ⇒ `clamp(w)/w = 1` ⇒ 屏幕空间常数。
        // 全库只有 4 份走前一支:火源那 3 份 `Min = Max = 200`(美术刻意做成与距离无关)、
        // 呜呜 `_Fx` 的 `Max = 0`(⇒ 恒 0,不画描边)。
        var ratio = maxScale > minScale ? OutlineRatioPerPc * pc : (float?)null;

        // 五档颜色。第 1 档参数名**不带序号**(`OutLineOtherColor`),后四档才带 2..5。
        var intensity = Value("Outline Intensity", 1f);
        var ramp = new[] { "OutLineOtherColor", "OutLineOtherColor2", "OutLineOtherColor3",
                           "OutLineOtherColor4", "OutLineOtherColor5" }
            .Select(Color).ToArray();
        // 有一档读不到就整份不给 —— 半份颜色比没有更糟(会把某个部位刷成黑)。
        var colors = ramp.Any(c => c is null)
            ? null
            : ramp.Select(c => new[] { c![0] * intensity, c[1] * intensity, c[2] * intensity }).ToArray();

        return new OutlineRead(width, ratio, colors, chain.Textures.GetValueOrDefault("MatID"));
    }

    /// 全零 GUID 是「没记」(材质层参数就是这样),不收。
    private static void Remember(Dictionary<string, string> into, string? name, FGuid guid)
    {
        if (!string.IsNullOrEmpty(name) && guid != default) into.TryAdd(name, guid.ToString());
    }

    /// 顺父链合并参数。**从最远的祖先开始写**,近的覆盖远的,于是子实例的覆盖最终生效。
    private static MaterialInfo Resolve(string name, UMaterialInstance material)
    {
        var ownVectors = new Dictionary<string, float[]>(StringComparer.OrdinalIgnoreCase);
        var chain = new List<UMaterialInstance>();
        var parents = new List<string>();
        var current = material;
        // 防环:材质链正常只有两三层,超过 8 层就是数据坏了
        while (current is not null && chain.Count < 8)
        {
            chain.Add(current);
            var parent = current.Parent;
            if (parent is not null) parents.Add(parent.Name);
            current = parent as UMaterialInstance;
        }

        var textures = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        var vectors = new Dictionary<string, float[]>(StringComparer.OrdinalIgnoreCase);
        var scalars = new Dictionary<string, float>(StringComparer.OrdinalIgnoreCase);
        var switches = new Dictionary<string, bool>(StringComparer.OrdinalIgnoreCase);
        var guids = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        var blend = EBlendMode.BLEND_Opaque;
        var maskClip = DefaultMaskClip;
        // chain 是「自己 → 父 → 祖父」,倒着遍历 = 从祖先到自己
        for (var i = chain.Count - 1; i >= 0; i--)
        {
            var mi = chain[i];
            foreach (var param in mi.GetOrDefault<FTextureParameterValue[]>("TextureParameterValues", []))
            {
                var texture = param.ParameterValue.ResolvedObject?.Object?.Value as UTexture;
                var path = texture?.GetPathName();
                if (!string.IsNullOrEmpty(param.Name) && !string.IsNullOrEmpty(path))
                    textures[param.Name] = path;
                Remember(guids, param.Name, param.ExpressionGUID);
            }
            foreach (var param in mi.GetOrDefault<FVectorParameterValue[]>("VectorParameterValues", []))
            {
                var c = param.ParameterValue;
                if (!string.IsNullOrEmpty(param.Name) && c is not null)
                {
                    vectors[param.Name] = [c.Value.R, c.Value.G, c.Value.B, c.Value.A];
                    if (i == 0) ownVectors[param.Name] = vectors[param.Name];
                }
                Remember(guids, param.Name, param.ExpressionGUID);
            }
            foreach (var param in mi.GetOrDefault<FScalarParameterValue[]>("ScalarParameterValues", []))
            {
                if (!string.IsNullOrEmpty(param.Name))
                    scalars[param.Name] = param.ParameterValue;
                Remember(guids, param.Name, param.ExpressionGUID);
            }
            // 静态开关。**`bOverride` 一律是 false 而 `Value` 却各不相同**(实测幽星光一族
            // 100 条里没有一条 bOverride=true,但 `是否使用MatCap` 是 true、`GlassySwitch` 是
            // false),说明本作存的是**合并后的有效值**而不是「我覆盖了什么」——
            // 和 BasePropertyOverrides 那边一个套路。所以照样「有值就用、近的覆盖远的」。
            var staticSet = mi.GetOrDefault<FStructFallback>("StaticParameters");
            foreach (var entry in staticSet?.GetOrDefault<FStructFallback[]>("StaticSwitchParameters", [])
                                 ?? [])
            {
                var pname = entry.GetOrDefault<FStructFallback>("ParameterInfo")
                    ?.GetOrDefault<FName>("Name").Text;
                if (!string.IsNullOrEmpty(pname)) switches[pname] = entry.GetOrDefault<bool>("Value");
            }
            // BasePropertyOverrides 只在「勾了 override」时才有意义,但本作的实例普遍不写
            // bOverride_* 标记,所以按「有值就用」处理:BLEND_Opaque 是 0,等于没覆盖。
            var overrides = mi.BasePropertyOverrides;
            if (overrides is not null)
            {
                if (overrides.BlendMode != EBlendMode.BLEND_Opaque) blend = overrides.BlendMode;
                if (overrides.OpacityMaskClipValue > 0) maskClip = overrides.OpacityMaskClipValue;
            }
        }
        return new MaterialInfo(
            name, textures, vectors, ownVectors, scalars, guids, switches, blend, maskClip, parents);
    }
}
