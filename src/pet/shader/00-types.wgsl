// 数据结构与绑定:Camera / MaterialParams / 纹理槽 / VsIn / VsOut
//
// **这份文件不是独立的着色器** —— `src/pet/gpu.rs` 用 `concat!(include_str!(…))`
// 把 `src/pet/shader/*.wgsl` 按文件名顺序拼成一个模块再交给 wgpu。WGSL 的模块级条目
// 与顺序无关,所以拆分只是为了好读;**加新文件记得同步 gpu.rs 里那张 `concat!` 清单**。

// 宠物着色:顶点里做蒙皮,片元里做卡通(分段)光照 + 边缘光;描边走第二遍法线外扩。
//
// 目标是「像」游戏那套自研 toon,而不是复刻(设计 §3.3):基色贴图 + 2 段明暗 + 轻边缘光
// + 描边,已经能抓住观感。
//
// **MatCap / StarStick / 玻璃内部层这几层后来是照反汇编做的**(见 docs/shader.md),
// 不再是「不追」。但**基础 toon 那几个数仍然是猜的**,而且是在上游法线 bug 修好**之前**
// 调出来的、之后没复核过 —— 逐个标在下面各自的定义处:
//   `mix(0.72, 1.0, lit)` 的 0.72(已换成汇编的 0.5/1.5)、
//   `rim = pow(facing, 3.0) * 0.25`。旧的全局 `LINE_BOOST = 1.55` 已按原材质的
//   `Glow Color * Glow Intensity` 恢复为默认 0，避免给所有高 alpha 身体覆一层白膜。

struct Camera {
    view_proj: mat4x4<f32>,
    // 光照方向(指向光源)与描边参数打包进一个 vec4 省 binding
    light_dir: vec3<f32>,
    // 描边宽度的全局倍率(1 = 材质里读出来的实机宽度);宽度本身在 `material.outline.x`
    outline_scale: f32,
    // 秒;特效层的 UV 卷动靠它推进
    time: f32,
    // 是否选择高材质质量排列。目标实机为 Low；它实际绑定了 MobileDirectionalLight，
    // 但所选 `M_P_Object_Trans` shader map 仍没有 StarStick 采样块。
    high_material_quality: f32,
    // ⚠ `vec2<f32>` 按 8 字节对齐:这里落在 88。
    // 表情:脸那两个材质的 UV 偏移(整格)。**每只一份**,所以放在这儿而不是材质里 ——
    // 材质是按形态共享的,同一个形态的两只可以是两种表情。
    face_uv: vec2<f32>,
    // 当前蒙皮姿势的 PrimitiveSceneData bounds：[中心.xyz,最长边]。
    object_bounds: vec4<f32>,
    // 网格脸要画第几张卡(1–8);已在 CPU 侧退过档,这只一定有这张。
    face_card: f32,
};

/// 每材质一份。普通材质也有(tint 全 1、params.z=0),两条通道共用布局。
struct MaterialParams {
    tint: vec4<f32>,
    // [u 速度, v 速度, u 平铺, v 平铺]
    flow: vec4<f32>,
    // 纯特效层:[不透明度, 发光强度, 是否加色, 有没有噪声贴图]
    // 有基色的:  [alpha 是否镂空遮罩, 线条提亮倍数, alpha 是否不透明度, -]
    params: vec4<f32>,
    // 纯特效层:[遮罩是否 matcap, -, 有星点, 有 matcap]
    // 有基色的:  [-, 是否玻璃/纱(半透族), 有星点, 有 matcap]
    flags: vec4<f32>,
    // [星点 u 平铺, v 平铺, 边缘光强度, 不透明度]
    star: vec4<f32>,
    // 星点着色(rgb)+ **星点层强度**(a,根材质 `Stick_Intensity` = 1.5)
    star_color: vec4<f32>,
    // MatCap 着色(rgb,可能是 HDR)
    matcap_color: vec4<f32>,
    // 自发光:`Emitter Color`(rgb,线性)+ `Emitter Intensity`(a);a = 0 时整层不画
    emissive: vec4<f32>,
    rim_color: vec4<f32>,
    // HighLight Offset(xyz,已换成 glTF Y-up)+ HighLightSpecPow
    highlight: vec4<f32>,
    // HighLight SpecCol(rgb)+ HighLight SpecInt
    highlight_color: vec4<f32>,
    // [Rim Power, 色带混入强度, Rim Soft Edge, 有没有色带]
    extra: vec4<f32>,
    // 玻璃内部那层:[折射率, GlobalDepth, 闪烁速度, 有没有内部层]
    interior: vec4<f32>,
    // 内部星光的着色(rgb,HDR)+ 闪烁次数(a)
    interior_color: vec4<f32>,
    // 模型包围盒:最小角(xyz)与尺寸(w 存最长边),内部层要拿它把位置归一化
    bounds_min: vec4<f32>,
    bounds_size: vec4<f32>,
    // 色带的 ID 遮罩:[区间下限, 区间上限, 有没有遮罩, -]
    mask_id: vec4<f32>,
    /// 假半透族星点层:[速度X, 速度Y, 强度, 是否用 UV0]
    noise_uv: vec4<f32>,
    // `M_ShuiMu_ByIn` 专用参数；顺序与 Rust 的 MaterialUniform 完全一致。
    glassy_flow1: vec4<f32>,
    glassy_flow2: vec4<f32>,
    glassy_fresnel: vec4<f32>,
    // [GlassyNoiseSpeed, UVScale, GlassyNoiseRefract 原参数, Depth]
    glassy_noise: vec4<f32>,
    // [FresnelMaskPow, Offset, Smooth, TriPlannarBlendInt]
    glassy_mask: vec4<f32>,
    // `M_P_Object_Trans`:[场景深度距离(米),开启强度,走目标 Low 局部链,SoftEdge]
    depth_fade: vec4<f32>,
    // [MI_P_Object_XiaoYou, M_Gra_Yutu_Ear_Lighting, MI_P_FakeFulid, M_P_MatCap_Masked]
    // 第五族 `M_FairyBall_BallFront` 的开关在 `family11.w`(这一行满了,见 gpu.rs)。
    family_flags: vec4<f32>,
    xiaoyou_base1: vec4<f32>,
    xiaoyou_base2: vec4<f32>,
    xiaoyou_flow1: vec4<f32>,
    xiaoyou_flow2: vec4<f32>,
    xiaoyou_star_color: vec4<f32>,
    // [USpeedTex01,VSpeedTex01,USpeedTex02,VSpeedTex02]
    xiaoyou_noise_flow: vec4<f32>,
    // [FlowNoseInt1,FlowNoiseInt2,Star_RG_Int,Star_RG_TwinkleSpeed]
    xiaoyou_shape: vec4<f32>,
    xiaoyou_star_uv: vec4<f32>,
    // 第二层星点(`Star_BA_*`):UV 控制 + [RG阈值, BA阈值, BA强度, BA闪烁速度]。
    xiaoyou_star_uv2: vec4<f32>,
    xiaoyou_star2: vec4<f32>,
    // YutuEar / FakeFulid / MatCapMasked / FairyBall 互斥复用的原始参数区。
    family0: vec4<f32>,
    family1: vec4<f32>,
    family2: vec4<f32>,
    family3: vec4<f32>,
    family4: vec4<f32>,
    family5: vec4<f32>,
    family6: vec4<f32>,
    family7: vec4<f32>,
    family8: vec4<f32>,
    family9: vec4<f32>,
    family10: vec4<f32>,
    family11: vec4<f32>,
    // 描边:[沿法线外扩多少米, 有五档颜色(0/1), -, -]
    outline: vec4<f32>,
    // 描边的五档颜色(线性 RGB × `Outline Intensity`),按 `outline_id_tex` 的 alpha 挑;
    // `outline.y = 0`(旧包)时整份不看。见 `fs_outline`。
    outline_ramp: array<vec4<f32>, 5>,
    // ── 炫彩(`GlassySwitch` 那条分支)。整套推导见 src/pet/glassy.rs。
    // `RedChannel`(rgb)+ 开关:0 = 这个槽不刷炫彩
    glassy_red: vec4<f32>,
    // `GreenChannel`(rgb)+ `StarIntensity`
    glassy_green: vec4<f32>,
    // [GlobalRefraction, GlobalDepth(米), MainTexTiling, NormalEffectAmount]
    glassy_p0: vec4<f32>,
    // [MainTexFlowSpeedX, MainTexFlowSpeedY, StarStickTiling, BaseColorDetail]
    glassy_p1: vec4<f32>,
    // 通用炫彩:星贴层四段渐变的四个色标(`StickRandomColor01..04`)。
    // 赛季那一族(`glassy_red.w = 2`)复用前三个:BlueChannel / MetalColor / MetalColor02,
    // 各自的 `.w` 依次是 FlowMaskInt / FlowMaskPow / MainTexFlowSpeedY。
    glassy_stick0: vec4<f32>,
    glassy_stick1: vec4<f32>,
    glassy_stick2: vec4<f32>,
    glassy_stick3: vec4<f32>,
    // 闪点层:[StarTiling, StarDensity, StarIntensity, -]。见 `glassy_sparkle`。
    glassy_sparkle: vec4<f32>,
    // 玻璃层那圈边缘光:[RimColor.rgb, RimIntensity]。
    glassy_rim: vec4<f32>,
    // 逐 `MatID` 的高光:[SpecColor.rgb, 开着(0/1)];四档 [SpecPow, SpecIntensity,
    // SpecRadius, -](挡位 2~5)。见 `matid_specular`。
    spec_color: vec4<f32>,
    spec_slots: array<vec4<f32>, 4>,
    // 法线图:[有(0/1), 强度, -, -]。贴图就是 `glassy_id_tex` 那张 `MaskTex` 的 RG。
    // 见 `mapped_normal`。
    normal_map: vec4<f32>,
    /// `M_P_Object` 公共链上的加性流动层与那圈菲涅尔发光。见 `uv_flow_layer` / `fresnel_layer`。
    uv_flow_color: vec4<f32>,
    uv_flow_shape: vec4<f32>,
    uv_flow_radial: vec4<f32>,
    fresnel: vec4<f32>,
    fresnel_shape: vec4<f32>,
    fresnel_hard: vec4<f32>,
    /// 火系族在同一个发光累加器上多的两层。见 `fire_layers`。
    fire1: vec4<f32>,
    fire2: vec4<f32>,
    fire3: vec4<f32>,
    fire4: vec4<f32>,
    fire_shape: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
// 蒙皮矩阵:关节世界变换 × 逆绑定矩阵,每帧由 CPU 采样动画后上传
@group(0) @binding(1) var<storage, read> joints: array<mat4x4<f32>>;
@group(1) @binding(0) var base_color: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;
// 第二张贴图,两种用途共用(一个材质只会是其中一种):
// 纯特效层 = 噪声(火焰的流动);有基色的 = 卷动色带(暮星辰环带的渐变)。没有就是 1×1 白图
@group(1) @binding(2) var noise_tex: texture_2d<f32>;
@group(1) @binding(3) var<uniform> material: MaterialParams;
// 星点(身上的细碎星光)与 MatCap(球面反射查找表);没有就是 1×1 白图
@group(1) @binding(4) var star_tex: texture_2d<f32>;
@group(1) @binding(5) var matcap_tex: texture_2d<f32>;
// 玻璃内部那颗星的四角星场(`StarTex` = `T_EMeng003`);没有就是 1×1 白图
@group(1) @binding(6) var interior_tex: texture_2d<f32>;
// 色带的 ID 遮罩(`MaskTex`,ID 在 alpha 里);没有就是 1×1 白图
@group(1) @binding(7) var mask_id_tex: texture_2d<f32>;
// 目标 ES3.1/Low `MI_P_Object_Trans` 的 t3/t4。RampTex 在 cooked uniform
// expression 中明确使用 clamp sampler，不能复用其它贴图的 repeat sampler。
@group(1) @binding(8) var light_mask_tex: texture_2d<f32>;
@group(1) @binding(9) var ramp_tex: texture_2d<f32>;
@group(1) @binding(10) var ramp_sampler: sampler;
// 炫彩的两张**共享**贴图:`MainTex`(花纹)与 `StarStickTex`(粒子)。
// 不在宠物包里,由导出器的 `--glassy` 单独导;没选炫彩时是 1×1 白图。
@group(1) @binding(11) var glassy_main_tex: texture_2d<f32>;
@group(1) @binding(12) var glassy_star_tex: texture_2d<f32>;
// 炫彩的**区域门**:那张 `_M` 的 alpha 是离散 ID 台阶,只有 `>= GLASSY_MIN_ID` 的地方刷玻璃。
// 没导这张(旧包)时绑 1×1 白图 = 门恒开。
@group(1) @binding(13) var glassy_id_tex: texture_2d<f32>;
// `SeasonMutation` 那族的区域遮罩(`MixMask`):`.b` 走幂曲线混向 `glassy_stick0`,
// `.a >= 0.79` 的地方整片换成 `glassy_stick1`(那片金属色)。没有就是 1×1 白图。
@group(1) @binding(14) var season_mask_tex: texture_2d<f32>;
// 金属区那层**金属光泽**的 matcap(`Mutation_MatCap`)。**这一条是近似,不是从汇编读的**
// —— 那份排列里金属区就是平涂。只有 `MetalSpecInt > 0` 的材质导得到它,导不到就是白图
// (乘 1 = 平涂):机幕方舟有(实机是带高光的银),龙息帕尔没有(实机是平白)。
@group(1) @binding(15) var season_matcap_tex: texture_2d<f32>;
// **描边那一遍的炫彩花纹图。** 描边材质有自己的 `MainTex` 槽,而 lua 只往它身上写
// `GlassySwitch` + 两个 Channel 色(`processAdditionalMaterial`),贴图一概不动 ——
// 所以隐藏款/赛季款在描边上用的**仍是共享的那张 `Tex_PetGlassy_007_D`**,
// 不是本体那张。两张不能共用一个绑定,故单开一条;没选炫彩时是 1×1 白图。
@group(1) @binding(16) var glassy_outline_tex: texture_2d<f32>;
// **描边挑档用的 `MatID` 遮罩**(读 alpha)。和上面那张区域门 `glassy_id_tex` 不是一路:
// 854 份 `_Ol` 里 732 份指着同一张 `_M`,但 38 份指着别的、81 份本体压根没有 `MaskTex`;
// 而且区域门只给炫彩槽上传,描边每个材质都要。没这份数据时是 1×1 白图。
@group(1) @binding(17) var outline_id_tex: texture_2d<f32>;
// 第一遍不透明材质留下的场景深度；半透明材质按原 shader 的
// `OpacityDepthDistance` 计算与后方实体/背景的距离。
@group(2) @binding(0) var scene_depth: texture_depth_2d;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) joint_ids: vec4<u32>,
    @location(4) weights: vec4<f32>,
    // 预蒙皮局部位置。原 VS 21175/31053 明确把它传给折射材质。
    @location(5) local_pos: vec3<f32>,
    // glTF `COLOR_0`。XiaoYou / YutuEar / FakeFluid 的目标 Low PS 都直接读取。
    @location(6) color: vec4<f32>,
    // glTF `TEXCOORD_1`。`M_P_Object` 的加性流动层按 `UV Number` 在它与 UV0 之间选。
    @location(7) uv1: vec2<f32>,
    // glTF `TEXCOORD_2`。背板族的 `UVNumber` 选的是**它**(VS 54079 的 `o5 = ATTRIBUTE7`,
    // 签名查实)。全库 123 个骨骼网格真的有第 3 套 UV,见 `model::Vertex::uv2`。
    @location(8) uv2: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    // 折射材质的预蒙皮局部起点。
    @location(2) local_pos: vec3<f32>,
    // **物体空间**的法线与视线:玻璃内部层的折射必须在这个空间里算(见 `interior_star`)
    @location(3) local_normal: vec3<f32>,
    @location(4) local_view: vec3<f32>,
    /// **裁剪空间 NDC**。假半透族的星点层在这个空间里采 —— 材质图 `UseNoiseUV0 = 0`
    /// 明写了"不走网格 UV0",实机观感正是"蒙在镜头前、拖动旋转时星点不随着转"。
    /// 用 NDC 而非 `@builtin(position)`,是为了不依赖视口尺寸。
    @location(5) ndc: vec2<f32>,
    @location(6) color: vec4<f32>,
    /// 蒙皮后世界位置。FakeFulid 的目标 PS 42877 从 v7 读取 AbsoluteWorldPosition；
    /// 未蒙皮 local_pos 只用于它自己的局部纹理坐标，不能拿来切液面。
    @location(7) world_pos: vec3<f32>,
    /// 第二套 UV。`M_P_Object` 的加性流动层按 `UV Number` 在它与 `uv` 之间选。
    @location(8) uv1: vec2<f32>,
    /// 第三套 UV。背板族的 `UVNumber` 选它。
    @location(9) uv2: vec2<f32>,
};
