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
    outline_width: f32,
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
    // YutuEar / FakeFulid / MatCapMasked 互斥复用的原始参数区。
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
};

// 线性混合蒙皮:权重和不为 1 的顶点(导出误差)按权重和归一化,否则会缩水
fn skin_matrix(ids: vec4<u32>, weights: vec4<f32>) -> mat4x4<f32> {
    let total = weights.x + weights.y + weights.z + weights.w;
    let w = select(weights / total, weights, total <= 0.0001);
    return joints[ids.x] * w.x + joints[ids.y] * w.y + joints[ids.z] * w.z + joints[ids.w] * w.w;
}

fn skin(input: VsIn) -> VsOut {
    let m = skin_matrix(input.joint_ids, input.weights);
    let world = m * vec4<f32>(input.pos, 1.0);
    // 均匀缩放假设下法线可以直接用左上 3x3 变换;宠物骨骼没有非均匀缩放动画
    let normal = normalize((m * vec4<f32>(input.normal, 0.0)).xyz);

    var out: VsOut;
    out.clip = camera.view_proj * world;
    out.ndc = out.clip.xy / max(out.clip.w, 1e-6);
    out.uv = input.uv;
    out.normal = normal;
    out.local_pos = input.local_pos;
    // **物体空间**:法线取**未蒙皮**的顶点法线(它是烘死在网格里的,不随动画变),
    // 视线取模型空间的那份。
    //
    // 视线**不要**再用骨骼矩阵的逆转一次 —— 汇编里用的是 `cb2[6..8]`,那是 `Primitive`
    // 即**组件**的 world→local,不是逐骨骼的。我们的宠物没有额外的模型变换
    // (yaw 烘在 `view_proj` 里、模型在原点),所以模型空间 == 世界空间,直接用世界视线。
    //
    // 这两条合起来才是「星画在球上、跟着球刚体转」:烘死的法线让每个顶点的折射方向恒定,
    // 于是采样位置钉在表面上;球一转,图案跟着转。用**蒙皮后**的世界法线则相反 ——
    // 球面的世界法线分布本身是旋转不变的,图案会钉在屏幕上不动(那就是「像屏幕投影」)。
    out.local_normal = normalize(input.normal);
    out.local_view = normalize(vec3<f32>(camera.view_proj[0][2], camera.view_proj[1][2], camera.view_proj[2][2]));
    out.color = input.color;
    out.world_pos = world.xyz;
    return out;
}

@vertex
fn vs_main(input: VsIn) -> VsOut {
    return skin(input);
}

// 描边:同一份网格沿法线外扩一点,只画背面,颜色压暗
@vertex
fn vs_outline(input: VsIn) -> VsOut {
    let m = skin_matrix(input.joint_ids, input.weights);
    let normal = normalize((m * vec4<f32>(input.normal, 0.0)).xyz);
    let world = m * vec4<f32>(input.pos, 1.0);
    var out: VsOut;
    out.clip = camera.view_proj * (world + vec4<f32>(normal * camera.outline_width, 0.0));
    out.ndc = out.clip.xy / max(out.clip.w, 1e-6);
    out.uv = input.uv;
    out.normal = normal;
    out.local_pos = input.local_pos;
    out.local_normal = normalize(input.normal);
    out.local_view = vec3<f32>(0.0, 0.0, 1.0);
    out.color = input.color;
    out.world_pos = world.xyz;
    return out;
}


/// 星贴层的四段渐变色:**`StickRandomColor01 → 02 → 03 → 04`**,每段 ⅓ 宽。
/// 全库没有实例覆盖过它们,所以写成常量。
///
/// **这里一度记成「02 → 03 → 04 → 白,01 不在渐变里」,那是解析 bug 的假象** ——
/// 冻结块里的向量参数数组被截断了两条(见 rocom-capture 的 `uniexpr.param_pair`
/// 与「短串前进步长」那两处修复),于是每个槽位的名字都**错位一格**。
/// 修完重解,四个槽正好是 01/02/03/04,比原来那个「三个浓色 + 白」自洽得多。
const STICK_RAMP_0: vec3<f32> = vec3<f32>(0.9462, 0.0636, 0.0214);
const STICK_RAMP_1: vec3<f32> = vec3<f32>(0.9601, 0.1603, 0.9074);
const STICK_RAMP_2: vec3<f32> = vec3<f32>(0.0489, 0.1545, 0.9774);
const STICK_RAMP_3: vec3<f32> = vec3<f32>(0.9253, 0.7416, 0.0273);
/// 星贴层的强度与混合下限:汇编 `cb6[96].w` = `Stick_Intensity` = 1.5、
/// `cb6[97].x` = `GlassyMainColorOpacity` = 0。两个都没有实例覆盖过。
///
/// 这里换掉的是原先那个手挑的 `STICK_GAIN = 0.083`(由「旧 0.2² / EXPOSURE」折算而来)——
/// 那是为「相加式叠一层白光」标的,而汇编里这层是 **lerp 替换固有色**,两者不可比。
///
/// **另记一个试过的无效指标**:拿「身体区域去掉 8×8 块均值后的高频 std」比渲图与实机截图 ——
/// 那个数**由锯齿主导**(我们没抗锯齿、还有描边,实机截图是抗锯齿+缩放过的),
/// 星点层开关前后比值只从 2.77 变到 2.73,分辨不出东西。
const STICK_INTENSITY: f32 = 1.5;
const STICK_BLEND_FLOOR: f32 = 0.0;
/// 星点闪烁的相位速度。汇编里是 `frac(View 时间 × 0.25)` —— **0.25 是硬写在材质图里的
/// 字面量**(和它并列的 `frac(时间 × 0.0056)` 喂另一层),不是可覆盖的参数,所以照抄。
const STAR_PHASE_SPEED: f32 = 0.25;
/// 见 `stick_layer` 里 `base_uv` 那段:离屏画布 ↔ 实机屏幕的尺度比。
///
/// **这条线上唯一一个标定值**,其余(坐标系/滚动速度/浓度/平铺)全部读自解包数据。
/// 它取决于实机里宠物占屏幕多大 —— **越大越密**(屏幕上铺的格子越多)。
/// 2.0 是对着实机截图目视定的;1.0 星点偏大、2.5 起就偏密。
const SCREEN_REF: f32 = 2.0;
/// 球内星点的整体强度:汇编 `mul_sat r0.y, r0.y, cb5[62].z`,而 `cb5[62].z` 是
/// **`CrossStarColor.w` = 1.0**。
///
/// **这里一度写成 0(「实机不画这一层」),那是解析 bug 的假象** —— 参数数组被截断,
/// 槽位名整体错位一格,把 `CrossStarColor.w` 读成了 `FragmentsColor.w`(= 0)。
/// 修完重解是 1.0,这一层**是画的**。
/// (更早还按语义猜成 `StarIntensity` = 1 —— 值碰巧对,理由是错的。)
const INTERIOR_GAIN: f32 = 1.0;
/// 球内星场的采样平铺:汇编 `cb5[61].y`,**名字读出来是 `StarUVScale` = 3.0**。
///
/// 原来这里写 0.4,理由是「根材质有个语义对得上的 `StarTiling` = 0.4」—— **猜错了参数**:
/// 两个都存在,而接在这个槽上的是 `StarUVScale`。差 7.5 倍。
/// 定名靠把 shader 34529(`V=54`、`dcl cb5[70]`)配到 `MI_Ill_XingGuang1_001_Fx1` 的块 10
/// (`54 + ⌈60/4⌉ + 1 = 70`,精确相等、12 个块里唯一),同块的 `FlickerSpeed` = 0.3、
/// `FlickerPower` = 5、`MatCapColor` 与宠物包里的值逐字对上 —— 配对是可信的。
/// 全库没有实例覆盖过它,所以写成常量。
const INTERIOR_UV_SCALE: f32 = 3.0;
/// march 距离的倍率:汇编 `marchDist = halfExtent × 0.01 × cb5[61].x`,而
/// **`cb5[61].x` 的名字是 `StarColorDepth` = 15**,不是 `GlobalDepth`。
///
/// 原来代的是 `GlobalDepth` = 100,依据是「代 100 进去正好让 marchDist = halfExtent,
/// 那个 0.01 的配合就是强证据」—— 那只是个自洽性论证,**名字读出来就否掉了**。
/// 真值 15 ⇒ marchDist = 0.15 × halfExtent。同样全库零覆盖。
const INTERIOR_DEPTH: f32 = 15.0;
const STAR_TRIPLANAR_BLEND: f32 = 2.0;
/// 玻璃族 MatCap 高光的叠加量。**已经撤成 1.0 —— 这一层不再有手挑参数。**
///
/// 搬进线性后,材质的 HDR `MatCapColor`(幽星光的球 (2, 1.76, 1.45)、暮星辰的球 (3,3,3))
/// 可以原样用了:`sqrt(2.0 × EXPOSURE)` ≈ 0.98,正好是一块接近白的高光 —— 而原来在显示空间
/// 里它被当成显示值、还要乘 0.35 压一档,出来只有很淡的一层。
///
/// 为什么敢撤:那个系数存在的唯一理由是「游戏那边这项还乘着遮罩通道选出的高光区,我们没有
/// 那张遮罩」。但**遮罩限制的是面积、不是亮度** —— 对着实机那颗球看,它的高光比我们压过的
/// 那版**更亮**(实机是左上一小块明确的白),压亮度是修错了地方。全库回归也证实没有代价:
/// 0.254 / 0.6 / 1.0 三挡的 `过曝` 都是 2。
///
/// 仍然对不上的是**面积**:缺遮罩,我们的高光铺得比实机宽。那要等遮罩通道的语义。
const GLASS_MATCAP_GAIN: f32 = 1.0;
/// 玻璃族边缘光的叠加量。汇编里这一项除了 `RimIntensity` 还乘着一个 cb 标量(`cb5[56].w`,
/// 槽位没对上名字),所以系数只能标定。**两条独立测量给出同一个数**:
/// ① 实机暮星辰裙子中位 (71,91,232) 减去基色贴图在那块 UV 的 (66,64,197),残差正好是
///    0.144 × `Rim LightColor`(53,187,214),三通道同时吻合;
/// ② 把整只渲图合成到实机背景色上、按「有/无边缘光」两版对裙子区解线性方程,得 0.35×0.46。
///
/// **那两条测量都是在显示空间做的**(量的是截图像素),搬进线性后要换算:
/// `旧² / EXPOSURE` = 0.16² / 0.4816 ≈ 0.0532。换算保持观感等值,原来那两条测量的效力也保留。
const GLASS_RIM_GAIN: f32 = 0.0532;
/// 输出前软肩的白点(extended Reinhard);<= 0 关闭,退回硬削顶。见 `fs_main` 末尾。
///
/// **它解开了一个卡了很久的死结。** 之前两次都撞到同一堵墙:想把亮度比从 0.83 拉到 1.0,
/// 无论提 `EXPOSURE`(0.70)还是提 `AMBIENT`(1.0),全库过曝都会从 1 暴涨到 34~98 ——
/// 因为我们的贴图是 LDR、削顶是硬的。而游戏那条链在 HDR 里算完再由曝光压回来,削顶发生在
/// 有余量的地方。软肩就是在补这份余量:低值几乎不动,高值平滑压向白点。
///
/// 加上它之后 `AMBIENT` 才提得动(0.5765 → 1.5),17 只对照:
///
/// | | 亮度 | 调色板 | 描边 | 对比 | 全库过曝 |
/// |---|---|---|---|---|---|
/// | 无软肩 A=0.58 | 0.83 | 0.088 | 1.02 | 1.11 | 1 |
/// | 无软肩 A=1.0 | 0.92 | 0.088 | 0.93 | 1.10 | **34** |
/// | **W=1.5 A=1.5** | **0.91** | 0.090 | 0.98 | **0.97** | **1** |
///
/// 注意 **W = 1 是恒等**(extended Reinhard 在白点 1 时不压缩),别拿它当「开启」。
const SHOULDER_WHITE: f32 = 1.5;


/// **曝光**:整条固有色链路改到线性空间后唯一的自由标量。
///
/// 汇编尾部是 `min(色, 100)` → `× View 预曝光(cb0[145].y)` → `× 材质整体色(cb6[84])`
/// → `× 曝光(cb1[79].w)` → `sqrt` → 输出(见 docs/shader.md)。也就是说游戏整条链跑在 **HDR**,
/// 靠曝光压回来、再用 `sqrt` 编码到显示空间。**这正是我们缺的那一环** —— 缺了它,材质里所有
/// 大于 1 的值(两段明暗的亮端 1.5、`MatCapColor` 的 (2,1.76,1.45)/(3,3,3)…)代进来只会硬顶到白。
///
/// 编码用 `sqrt` 就意味着解码是**平方**(gamma 2.0)——这是游戏自己的约定,不是 sRGB,
/// 所以这里照抄:基色贴图平方进线性、末尾 `sqrt(色 × 曝光)` 出来。**贴图格式不用改**
/// (仍是 `Rgba8Unorm`),平方在 shader 里做,避免动整条纹理上传路径。
///
/// 那两个曝光值是 View 常量、每帧由引擎给,离线读不出来,所以这里合成一个常量并标定。
/// **它替掉的是原来一堆手挑系数**(每个都在替某个未解名槽位),自由度从五六个降到一个。
/// 基色贴图的解码次数。**编解码是不对称的**:编码那头是 `sqrt`(gamma 2.0,汇编尾部写死),
/// 而贴图是 UE 交给硬件按 **sRGB**(≈2.2)解的 —— 净效果是 `色^1.1`,中间调略暗、饱和度略升。
///
/// 原来这里对称地用了平方(2.0)。17 只实机对照说 2.2 更对:
///
/// | | 亮度 | 调色板 | 描边 | 对比 | 全库过曝 |
/// |---|---|---|---|---|---|
/// | 2.0 | 0.88 | 0.098 | 0.99 | 1.07 | 3 |
/// | **2.2** | 0.83 | **0.088** | 1.02 | 1.12 | **1** |
/// | 2.4 | 0.81 | 0.107 | 1.03 | 1.18 | — |
///
/// **是个有得有失的取舍**:色度(调色板)与过曝更好,亮度与对比更差。取 2.2 的理由是它是
/// 引擎真实的解码方式,不是拟合出来的数;而变差的那两项**本来就补不回来** ——
/// 抬曝光能把亮度拉到 0.98(曝光 0.70),但全库过曝会从 3 暴涨到 **98**:
/// 游戏在 HDR 里有余量、削顶很少,我们的贴图是 LDR,抬曝光只会削顶。
/// 明暗过渡的上下界。**读出来的**:汇编里这一步是
/// `smoothstep(BlackMagicSoftMin, BlackMagicSoftMax, (N·L + 1) / 2)`
/// (`MI_Ill_XingGuang1_001_Fx1` 块 10 的 `cb5[59].x` / `cb5[58].w`,值 0.50 / 0.52,
/// 全库零覆盖),换算到 `N·L` 空间就是 `smoothstep(2×0.50 − 1, 2×0.52 − 1)` = **(0.00, 0.04)**。
///
/// 原来写的是 `(-0.04, 0.04)` —— 宽一倍且偏低,是当初扫参数扫出来的。
const SHADE_TERM_LO: f32 = 0.0;
const SHADE_TERM_HI: f32 = 0.04;
/// 特效层边缘系数的下限(`rim = mix(下限, 1, facing)`)。**当年在显示空间对着截图标的。**
///
/// **把 `fs_effect` 搬进线性的尝试到此为止,记下别再走**:三种编码 × 四档下限全测过,
/// 按受影响的两只(波波拉 + 水灵,它们才有特效层;中位对这两只不敏感)看 ——
///
/// | 版本 | 波波拉 | 水灵 | 合计 |
/// |---|---|---|---|
/// | **现状(显示空间)** | 0.337 | **0.097** | **0.434** |
/// | 线性 + 下限 0.35 | 0.315 | 0.145 | 0.460 |
/// | 线性 + 下限 0.6 | 0.311 | 0.224 | 0.535 |
/// | 线性 + 下限 0.8 | 0.369 | 0.271 | 0.640 |
/// | 线性 + 下限 1.0 | 0.361 | **抠图不可比** | — |
///
/// 每一档都更差,下限 1.0 时水灵的颜色甚至跑到贴近背景、触发了抠图丢块检测。
/// ⇒ 这层的显示空间标定是**自洽**的,只调「编码 + 这一个下限」搬不过去。
/// 真要做还得动 `glow`(来自 `Glow Intensity`,而它的根默认其实是 **0** —— 导出器兜底成 1,
/// 说明这一项对特效层根本不是 `Glow Intensity`)与 alpha 的耦合方式。
const EFFECT_RIM_FLOOR: f32 = 0.35;
const DECODE_GAMMA: f32 = 2.2;
const EXPOSURE: f32 = 0.4816;
/// 环境 / 间接光。实机由 mobile base pass 的天光那批 View 常量给,离线读不出来,所以标定
/// (见 `fs_main` 里两段明暗那段的推导)。**它是这条链路上仅剩的一个自由标量。**
const AMBIENT: f32 = 1.5;

/// 相机的右/上向量。正交投影没有透视错切,`view_proj` 的行向量归一化后就是它们,
/// 所以不必额外往 uniform 里塞。
fn camera_basis() -> mat2x3<f32> {
    let right = normalize(vec3<f32>(camera.view_proj[0][0], camera.view_proj[1][0], camera.view_proj[2][0]));
    let up = normalize(vec3<f32>(camera.view_proj[0][1], camera.view_proj[1][1], camera.view_proj[2][1]));
    return mat2x3<f32>(right, up);
}

/// 「这个面有多侧对着镜头」,0 = 正对、1 = 与视线平行(轮廓)。边缘光/菲涅尔都用它。
///
/// **视线方向必须从 `view_proj` 取,不能写死世界 +Z。** 相机是绕着宠物转的(yaw),
/// 写死 +Z 时凡是背对世界 +Z 的面都会被判成「完全侧对」→ 平白吃一层 0.25 的白,
/// 幽星光整只被冲淡成粉白就是这么来的。取第三行(深度行)归一化即得视线轴,
/// 用 `abs` 所以不必关心它的正负号。
/// 相机射向场景的入射方向。正交投影下每个像素相同；`local_view` 的折射用这一支。
fn camera_forward_direction() -> vec3<f32> {
    return normalize(vec3<f32>(camera.view_proj[0][2], camera.view_proj[1][2], camera.view_proj[2][2]));
}

/// 表面指向相机的观察方向。原材质的 N·V、Fresnel 与半角高光用这一支。
fn view_direction() -> vec3<f32> {
    return -camera_forward_direction();
}

fn facing_ratio(n: vec3<f32>) -> f32 {
    return 1.0 - abs(dot(n, view_direction()));
}

/// 目标实机 ES3.1/Low、LOD0 `M_P_Object_Trans` 的原始高光覆盖项：
/// `smoothstep(.4,.5,pow(max(N·H,0),HighLightSpecPow)) × HighLight SpecInt`。
/// 这个选中排列的 alpha 链没有 Fresnel/rim 覆盖，不能拿别的排列补进来。
fn trans_spec_coverage(n: vec3<f32>) -> f32 {
    let view = view_direction();
    let half_dir = normalize(view + normalize(camera.light_dir) + material.highlight.xyz);
    let spec_base = pow(max(dot(n, half_dir), 0.0), max(material.highlight.w, 1e-4));
    let spec_t = saturate((spec_base - 0.4) * 10.0);
    return spec_t * spec_t * (3.0 - 2.0 * spec_t) * material.highlight_color.w;
}

/// MatCap 的采样坐标:视空间法线映射到 [0,1](球面查找表的标准做法)。
fn matcap_uv(n: vec3<f32>) -> vec2<f32> {
    let basis = camera_basis();
    return vec2<f32>(dot(n, basis[0]), -dot(n, basis[1])) * 0.5 + vec2<f32>(0.5, 0.5);
}

/// 星点层。**一只宠物只有一份,盖在整只身上**(导出器统一好了,见 Program.cs):
/// 那两颗球身上的星星也是它 —— 球的基色在图集里是一片平色圆盘,星形完全来自这层
/// (所以幽星光一颗球是星、另一颗是圆点)。
///
/// **这里是照反汇编原样搬的**,来源:`MI_P_Object_XingGuang_FakeTrans01` 的世界 base pass
/// (shader 27803,7 个 uniform buffer ⇒ 材质 cb 是 cb6),第 375~403 行:
///
/// ```text
/// r12.w = frac(cb0[153].z * 0.25)                  ← View 时间 × 硬写的 0.25(4 秒一周)
/// θ     = r12.w * 2π
/// k     = 1.1 * lerp(|sin θ|, |cos θ|, tex.g)
/// uv    = v2.xy * cb6[130].w                       ← v2 = **网格 UV0**
/// x     = saturate((tex.b * (k - tex.r) - 0.01) * 25)
/// m     = x²(3 - 2x)                               ← smoothstep,×25 造出很硬很细的边
/// c     = 4 段渐变 cb68 →(t=⅓) cb67 →(⅔) cb70 →(1) cb72,t = saturate(k)
/// 出    = lerp(底色, cb6[131].x * m * c, saturate(m + cb6[131].y))
/// ```
///
/// 读这段有个坑:`sample` 写的是 `r11.xyz`,**把上面 `sincos` 存进 r11.x 的 sin 覆盖掉了**,
/// 所以后面 `add r3.z, -r11.x, r3.z` 减的是 `tex.r` 而不是 sin。
///
/// 由此贴图三通道的分工是 **r = 每颗星的阈值、g = 相位混合、b = 幅度**,星形完全烘在贴图里
/// (实测那张 512² 图:三通道基本共位、都是连续的 0..1、alpha 恒 1 未用)。
///
/// **这段汇编推翻了一条写进过文档的旧结论**:「采样坐标取 NDC、星点贴在镜头上不随模型转」
/// 是错的 —— 这个 shader 里 `v8`(SV_Position)只在 View 的抖动那条出现过。
///
/// ---
///
/// **这一层现在是照汇编实现的,四个渐变色是读出来的、不是猜的。** 定名的办法是把同一段代码
/// 在 `MI_P_Object_Masked` 的 shader 27931 里配到冻结块 9,再经 uexp 里 shader map 自带的
/// 名字表把 `paramId` 翻成名字(整条链见 rocom-capture 的 docs/shader.md「最后一步:名字」)。
/// 槽位对应:
///
/// | 汇编 | 名字 | 值 |
/// |---|---|---|
/// | `cb6[96].z` | `StarStickTiling` | 4(逐材质,走 `material.star.xy`) |
/// | `cb6[96].w` | `Stick_Intensity` | 1.5 |
/// | `cb6[97].x` | `GlassyMainColorOpacity` | 0 |
/// | `cb6[38]` | `StickRandomColor02` | (0.960, 0.160, 0.907) 洋红 |
/// | `cb6[37]` | `StickRandomColor03` | (0.049, 0.155, 0.977) 蓝 |
/// | `cb6[40]` | `StickRandomColor04` | (0.925, 0.742, 0.027) 黄 |
/// | `cb6[42]` | `00FX_BaseColor` | (1, 1, 1) 白 |
///
/// **`StickRandomColor01` 不在这条渐变里** —— 名字有 `01..04` 四个,用到的是 `02/03/04` + 白。
/// 除 `StarStickTiling`(2 处)与 `00FX_BaseColor`(2 处)外,这些参数**全库没有任何实例覆盖过**
/// (拿探针的 `--probe-material ALL` 那 395 条实例覆盖清单查的),所以写成常量是安全的。
///
/// **推翻的旧结论**:我曾写下「那 4 个色不要去套 `StickRandomColor01..04`,实机星点是淡白粉、
/// 和 HDR 的 `Color02` 才对得上」。汇编说了它就是这四个浓色 —— 之前那个观察站不住,
/// 因为浓色经 `× m`(m 多数时候很小)、HDR 曝光再 `sqrt` 编码之后本来就会往白里跑。
/// 旧的 `min(r, g, b)` 近似同时废弃:它连遮罩形状都和汇编不是一回事。
/// 星贴层的一次求值:`color` 已含 `Stick_Intensity`,`cover` 是 lerp 的混合系数。
struct StickLayer {
    color: vec3<f32>,
    cover: f32,
}

fn stick_layer(uv0: vec2<f32>, ndc: vec2<f32>) -> StickLayer {
    if material.flags.z < 0.5 {
        return StickLayer(vec3<f32>(0.0), 0.0);
    }
    // 这是原 shader-map 的质量排列差异,不是宠物特判。目标实机的 ES3.1/Low
    // `M_P_Object_Trans` 资源只声明 4 张纹理、没有 StarStick；它即使绑定了
    // MobileDirectionalLight 也一样。假半透族走另一张材质图,不受这个分支影响。
    let trans_star_stick = material.flags.y > 0.5 && material.noise_uv.w > 0.5;
    if trans_star_stick && camera.high_material_quality < 0.5 {
        return StickLayer(vec3<f32>(0.0), 0.0);
    }
    let theta = fract(camera.time * STAR_PHASE_SPEED) * 6.2831855;
    // **坐标系由 `UseNoiseUV0` 定。** 假半透族(幽星光一家)根默认是 0 ⇒ 走**相机空间**
    // (这里用 NDC),再按 `Mat_NoiseSpeedX/Y` 随时间滚动。`SpeedY = -0.1` 为负 ⇒
    // 采样坐标下移 ⇒ 图案**上浮**,与实机一致。
    // **屏幕参考尺度。** 实机里宠物只占屏幕一小块,`Mat_NoiseTiling`(5 / 2.5)铺的是
    // **整个屏幕**;而我们离屏渲染时宠物**填满画布**,同样的平铺落到宠物身上只剩一两次
    // —— 星点偏大、滚动相对星点也偏快(用户实测两条)。乘一个参考尺度同时校正两者:
    // 平铺变密 ⇒ 星点变小,而滚动是 UV 单位、相对格子就慢了同样的倍数。
    // **这个数是标定的**,不是读出来的:它取决于实机截图里宠物占屏幕多大。
    let base_uv = select((ndc * 0.5 + vec2<f32>(0.5)) * SCREEN_REF, uv0,
                         material.noise_uv.w > 0.5);
    let scroll = vec2<f32>(material.noise_uv.x, material.noise_uv.y) * camera.time;
    let tex = textureSample(star_tex, base_sampler, base_uv * material.star.xy + scroll);
    // `k = 1.1 * lerp(|sin θ|, |cos θ|, tex.g)`,每颗星按 g 通道拿到自己的相位
    let k = 1.1 * mix(abs(sin(theta)), abs(cos(theta)), tex.g);
    let ks = saturate(k);
    // 4 段渐变,每段 ⅓ 宽。第三段汇编用的是 `max(3k-2, 0)` 不是 saturate ——
    // k ≤ 1 时两者等价,照抄以免以后 k 的上界改了还对
    // **两族的着色不一样,不能共用四段渐变。** `StickRandomColor01..04` 属于
    // `StarStickTex` 那一族;而幽星光一族走的是**「假半透」**族(`NoiseTex` + `Color02`),
    // 它的颜色就是 `Color02` —— 导出器早就把它归一化后写成 `star_color` 了
    // (曜星光 `Color02` = (10, 8.07, 9.04) ⇒ `star_color` = (1, 0.807, 0.904))。
    //
    // **踩过**:我按汇编把渐变读出来之后,不分族地套到所有材质上,而退步的三只
    // (幽星光 0.086→0.115、曜星光 0.078→0.129、暮星辰 0.082→0.094)**正好全是假半透族**。
    // 「公式读对了」不等于「这条公式属于这个材质」—— 先确认材质属于哪一族。
    var c = material.star_color.rgb;
    if material.params.w < 0.5 {
        c = mix(STICK_RAMP_0, STICK_RAMP_1, min(ks * 3.0, 1.0));
        c = mix(c, STICK_RAMP_2, saturate(ks * 3.0 - 1.0));
        c = mix(c, STICK_RAMP_3, max(ks * 3.0 - 2.0, 0.0));
    }
    // 遮罩:`× 25` 造出很硬很细的边。**减的是 tex.r 不是 sin θ** ——
    // 汇编里 sample 的目标寄存器把 sincos 的结果覆盖掉了,踩过一次
    //
    // **两族的遮罩读法也不一样,和着色一样得分开** —— 分不开的代价见下面 `StarStickTex` 那支。
    var m: f32;
    if material.noise_uv.w > 0.5 {
        // **`StarStickTex` 族:照汇编算。** 这一族的贴图(全库 31 个材质都是
        // `Tex_PetGlassyStar_004`)**不是成品星场,是张噪声图** —— 三通道按汇编分工
        // (r = 每颗星的阈值、g = 相位、b = 幅度),星形是这条公式**算出来**的。
        // 实测那张 512²:r 均值 0.855(92% 亮过一半)、b 均值 0.121。
        let x = saturate((tex.b * (k - tex.r) - 0.01) * 25.0);
        m = x * x * (3.0 - 2.0 * x);
    } else {
        // **`NoiseTex`(假半透)族:贴图本身就是成品星场,不该再去切。**
        // 拿上面那条公式切它必出**空心环**:`tex.r < k` 是个随时间移动的阈值,
        // 会把亮的星芯排除在外、只留外圈辉光(实机报的"星点周围一圈光晕闪烁")。
        // **遮罩要收到只剩星芯。** 颜色是 `c·m·gain`(幽星光 15 × 0.05 = 0.75·m),
        // 而 cover 用裸 m —— 若把星芒外围那圈暗辉光(m ≈ 0.3)也算进 cover,
        // 就会往身体上混一层比底色更暗的**灰**:星芯亮不起来、暗区反而发灰(用户实测)。
        // 只保留 `c·m·gain` 能压过底色的那一段。
        m = smoothstep(0.5, 1.0, max(tex.r, max(tex.g, tex.b)));
    }
    // **增益也分族**:假半透族是 `Mat_NoiseIntensity`(0.05,与 HDR 的 `Color02` 配对,
    // 用 `Stick_Intensity` 会差三十倍);`StarStickTex` 族的汇编里乘的就是
    // `Stick_Intensity`(1.5),而它那条 `noise_uv.z` 是默认值 1、不是读出来的。
    let gain = select(material.star_color.w, material.noise_uv.z, material.noise_uv.w < 0.5);
    // **强度只进颜色,不进混合系数。** 汇编那条是
    //     lerp(底, Stick_Intensity × m × c, saturate(m + GlassyMainColorOpacity))
    // —— cover 用的是**裸的 m**。把 gain 也乘进 cover(0.05·m)会让这一层几乎不参与
    // 混合,星点整个看不见(踩过)。
    // **颜色取贴图自身。** `_Fx_D` 里那些星芒本来就是**品红 / 白 / 青**的成品色
    // (实机看是"浅青粉"),用一个平的 `Color02` 白色会把这层色相抹平(用户实测)。
    // `Color02` 只作为 HDR 增益。
    let lit_c = select(c, tex.rgb * c, material.noise_uv.w < 0.5);
    return StickLayer(lit_c * m * gain, saturate(m + STICK_BLEND_FLOOR));
}

/// 卷动色带:一张渐变图沿 UV 滚过表面,乘在固有色上。暮星辰的环带靠它出青↔粉渐变
/// (`FlowTexture` = 青↔粉竖条纹 + `Flow_U_Speed` = 0.25;基色贴图里环带那一条是纯粉的)。
fn flow_band(uv: vec2<f32>, albedo: vec3<f32>) -> vec3<f32> {
    if material.extra.w < 0.5 {
        return albedo;
    }
    // **只在 ID 遮罩选中的地方卷。** `MaskTex` 的 alpha 是离散的材质 ID 台阶,材质给的
    // `MaskID Min/Max` 划出该卷动的那一档:暮星辰环带是 0.72、额头与身体中央的黄装饰是 0.50,
    // 阈值 0.6~0.8 只选中环带。不门控就是黄装饰跟着在黄绿之间来回变(实机里它们是固定黄)。
    if material.mask_id.z > 0.5 {
        let id = textureSample(mask_id_tex, base_sampler, uv).a;
        if id < material.mask_id.x || id > material.mask_id.y {
            return albedo;
        }
    }
    let scrolled = uv * material.flow.zw + vec2<f32>(material.flow.x, material.flow.y) * camera.time;
    let band = textureSample(noise_tex, base_sampler, scrolled).rgb;
    // **色带是黑的地方不混。** `FlowTexture` 这个槽位装的东西并不统一:暮星辰给的是一张
    // 青↔粉的**渐变色带**(`_Fx_D`),而水蓝蓝给的是一张 85% 全黑的**遮罩**(`_Fx_M`)——
    // 后者配上 `FlowPower = 1`,`mix(固有色, 色带, 1)` 直接把整只身体换成了黑,
    // 水蓝蓝/波波拉的触手就是这么黑掉的(实测暗像素占不透明区 14.5%、alpha 全是 1)。
    //
    // 汇编里那条链**从不替换固有色**:`MI_P_Object_UVFlow_*` 的流动贴图一路喂的是
    // 法线扰动与双层 UV 合成(`cb6[73]` 平铺 / `[74]` 偏移 / `[75]` 速度,两次采样再
    // `r4*(r7-1)`),不是拿来当颜色混的。完整复刻是另一件事(已记入待办),
    // 这里先只堵住「变黑」这一条 —— 它在任何读法下都是错的。
    let band_lit = max(band.r, max(band.g, band.b));
    // **是混色不是相乘。** 色带图本身就是成品颜色(青↔粉竖条纹),而基色图里环带那条是纯粉;
    // 相乘等于「粉 × 青」→ 出来是蓝,实机是真青。`FlowPower`(暮星辰 0.8)就是混色权重。
    return mix(albedo, band, material.extra.y * step(0.05, band_lit));
}

/// glTF 导出把 UE `(X,Y,Z)` 换成运行时 `(X,Z,Y)`；材质里的三平面采样仍须按 UE 轴序。
fn runtime_to_ue(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(v.x, v.z, v.y);
}

/// HLSL/WGSL `refract(I,N,eta)` 展开式。显式写出是为了和两条 DXBC 的 k<0 清零分支一致。
fn refract_direction(incident: vec3<f32>, n: vec3<f32>, eta: f32) -> vec3<f32> {
    let ni = dot(n, incident);
    let k = 1.0 - eta * eta * (1.0 - ni * ni);
    if k < 0.0 {
        return vec3<f32>(0.0);
    }
    return eta * incident - (eta * ni + sqrt(k)) * n;
}

/// **玻璃内部那颗星。** 实机是这么做的(读 `MI_P_Object_Trans_MatCap` 的 pixel shader 汇编,
/// 见 docs/design.md §1):把视线按 `GlobalRefraction`(=1.3)折射进物体内部,沿折射光线
/// march 一段(`GlobalDepth`),在**模型空间**按三向投影采 `StarTex`(= `T_EMeng003`,
/// 一张四角星场、alpha 是干净的稀疏星形遮罩),采样坐标再叠上时间卷动。
///
/// 于是球看着像「里面飘着一颗星」,而且那颗星**自己在动、与球的自转无关** —— 正是实机观感。
/// 这一层只给玻璃族(静态开关 `是否使用MatCap` 开着的那 17 个材质)。
///
/// **返回强度(标量),不带颜色。** 汇编里星场的采样结果是个标量,颜色是另外几个 cb 槽
/// 给的(`星点底色 + 强度 × 星点亮色`,再与按高度 lerp 的那对颜色混)。分开才对得上。
///
/// **是近似不是复刻**:游戏那边还有第二张三向投影贴图、两段 `pow` 相位曲线、以及那对按
/// 高度做的渐变色;这里只取「折射 + 三向投影星场 + 时间」这条主干。
/// 卷动速度实机是个 cb 里的向量参数,而 cb 槽位与参数名的对应还没解出来(§1),
/// 所以先用一个定值。
fn interior_star(start: vec3<f32>, n: vec3<f32>, forward: vec3<f32>) -> f32 {
    if material.interior.w < 0.5 {
        return 0.0;
    }
    // 顶点 shader 的配套写入已经查实：start 是预蒙皮局部位置，不是 UV1/UV2。
    // 换回 UE 轴序后再执行原材质的折射与三平面采样。
    //
    // **这一层离对上实机还很远**:实机那两颗球是「红球 + 居中的大号黄色四角星/圆点」,
    // 我们画出来只有几点很淡的紫斑(换成 UV1/UV2 起点那版连斑都没有)。
    // 起点之外,着色那一路缺的槽位更多 —— 见 docs/design.md 的待办表。
    let start_ue = runtime_to_ue(start);
    let n_ue = normalize(runtime_to_ue(n));
    let forward_ue = normalize(runtime_to_ue(forward));
    // eta 取 1/折射率(空气 → 介质)
    let eta = 1.0 / max(material.interior.x, 0.001);
    let dir = refract_direction(forward_ue, n_ue, eta);

    // **march 距离与平铺照汇编算,不再手挑。** 汇编(fx1/34529.asm 63..78):
    //   halfExtent = 0.5 * |包围盒尺寸|
    //   marchDist  = halfExtent * 0.01 * GlobalDepth        ← 代 100 进去正好 = halfExtent
    //   tiling     = <一个 cb 标量> / halfExtent            ← 那个标量取 1(中性),名字未解出
    //   p = (start + 折射方向 * marchDist) * tiling
    // 于是 p = start/halfExtent + 折射方向 —— 折射方向是单位向量,所以每颗球看到的是
    // 星场里以某点为心、约一格大小的一块,这正是「每颗球稳定居中一颗星」的机制。
    let half_extent = 0.5 * length(material.bounds_size.xyz);
    let march = half_extent * 0.01 * INTERIOR_DEPTH;
    let p = (start_ue + dir * march) * (INTERIOR_UV_SCALE / max(half_extent, 0.0001));

    // **三向投影不是「归一化权重加权和」,是两次嵌套 lerp。** 汇编(34529,83..88):
    //   k    = saturate(|n| * (2*StarTriPlannarBlendInt + 1) - StarTriPlannarBlendInt)
    //   s    = lerp(sample(p.xz), sample(p.yz), k.y)
    //   s    = lerp(s,            sample(p.xy), k.w)
    // 原来那版是 `pow(|n|, B)` 再归一化 —— 结构就不对(而且更早还写死次数 8,
    // 那让权重几乎完全偏向单一轴)。
    let blend = saturate(abs(n_ue) * (2.0 * STAR_TRIPLANAR_BLEND + 1.0) - STAR_TRIPLANAR_BLEND);
    let s0 = textureSample(interior_tex, base_sampler, p.xz);
    let s1 = textureSample(interior_tex, base_sampler, p.yz);
    let s2 = textureSample(interior_tex, base_sampler, p.xy);
    let s = mix(mix(s0, s1, blend.y), s2, blend.z);

    // **星点不是在移动、是在闪。** 汇编(同上 89..106):
    //   phase = frac(FlickerSpeed * 时间 + 星场.G)      ← 每颗星的相位来自 G 通道
    //   闪    = -1.2 * |sin(2π * phase)|^FlickerPower   ← 注意是**减**
    //   形状  = pow(星场.B, q)                          ← q 是未解出的 cb 标量,取 1
    //   强度  = saturate((形状 + 闪) * 星场.A * 强度)
    // 通道语义与贴图实测一致(T_EMeng003:G 均 0.328 且分散 = 相位;B 87% 为零 + 稀疏亮核
    // = 形状;A 是星形遮罩)。所以星是一明一暗地闪,而不是整片飘 —— 原来那版按时间卷动 UV
    // 是**猜的**,那会让星在球里滑动。
    let phase = fract(material.interior.z * camera.time + s.g);
    let twinkle = -1.2 * pow(abs(sin(phase * 6.28318548)), material.interior_color.w);
    return saturate((s.b + twinkle) * s.a * INTERIOR_GAIN);
}

/// MatCap 高光。`MatCapColor` 可能是 HDR(暮星辰那两个球是 (3,3,3)),所以直接相乘。
///
/// **实机只取一张单通道当标量**,不是 rgb 查表:汇编里是
/// `sample r2.w, (u, 1-v), t3.yzwx, s3` —— 目标只写 .w、资源 swizzle 第 4 位是 x,即取 **R**,
/// 紧接着 `mul r4.xyz, r2.w, cb5[4]`(cb5[4] = `MatCapColor`)。两张 matcap 图实测都是灰度
/// (三通道与亮度的相关系数 ≈ 1.000),所以 R 就是它的亮度,取单通道与取 rgb 数值上等价。
/// UV 也对得上:实机 `r4.z = 1 - r4.y`,与这里的 `-dot(n, up) * 0.5 + 0.5` 同一个式子。
///
/// 之前那版「减掉 0.35 的底再归一化」是**猜的**,把整张图的暗区削成 0 →
/// 球大部分时间不吃 matcap、高光块扫过来时又猛地一亮,反而放大了闪烁。
fn matcap_light(n: vec3<f32>) -> vec3<f32> {
    return material.matcap_color.rgb * matcap_strength(n);
}

/// MatCap 那一路的**标量**强度(汇编里它就是单通道 × `MatCapColor`,见 docs/shader.md
/// 「采样取了哪个通道」)。颜色与不透明度两处都要它,所以单拎出来。
fn matcap_strength(n: vec3<f32>) -> f32 {
    if material.flags.w < 0.5 {
        return 0.0;
    }
    return textureSample(matcap_tex, base_sampler, matcap_uv(n)).r;
}

/// 已逐指令还原材质的原始输出尾段。两份目标 PS 都是
/// `clamp(color, 0, 100) → ViewPreExposure × MobileExposure → sqrt`；没有通用 toon
/// 分支为弥补 LDR 余量而加的 extended-Reinhard 软肩。`EXPOSURE` 是两个运行时场景量的
/// 合并值（资产里不存在、离线不能分别取得），运算结构仍与原 shader 一致。
fn encode_linear_color(color: vec3<f32>) -> vec3<f32> {
    let clamped = clamp(color, vec3<f32>(0.0), vec3<f32>(100.0));
    return sqrt(clamped * EXPOSURE);
}

/// UE 颜色贴图由硬件做 sRGB→线性；本项目为了让数据遮罩与颜色贴图共用上传格式，
/// 纹理本身是 Rgba8Unorm，所以在需要逐指令对齐的 Low 分支显式补回同一转换。
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(lo, hi, c > vec3<f32>(0.04045));
}

/// 游戏材质函数 `MF_ToneMapInverse`。常数与 Low PS 2109/55790 第 103–114 行逐项相同：
/// 贴图的 filmic 显示值先反解回 HDR 线性值，再参与材质光照。
fn game_tonemap_inverse(c: vec3<f32>) -> vec3<f32> {
    let root = sqrt(max(c * c * -0.2072 + c * 0.70896 + vec3<f32>(0.002209),
                        vec3<f32>(0.0)));
    let numerator = c * -0.56 + vec3<f32>(0.047) - root;
    let denominator = (c * 0.93 - vec3<f32>(1.36)) * 2.0;
    return numerator / denominator;
}

/// 目标设备选中的 ES3.1/Low `MI_P_Object_Trans` 基色链。
///
/// 对应 PS 2109/55790 第 36–126 行：MaskTex.r 与 N·L 合成 ramp 横坐标，
/// SoftEdge 控制阈值上方的过渡；RampTex 固定采第一行；再按 `MPC_S_Global` 的资产默认
/// `C_CharacterEnvSkyInt=.8`、`C_CharacterMainLightInfInt=1` 和
/// `C_ML_Sat_For_SS.y=0` 合成环境/直射项。桌宠场景没有 UE SkyLight，故 View.SkyLightColor
/// 这一**场景输入**为 0；不是为某只宠物补的颜色。
fn object_trans_low_light(uv: vec2<f32>, n: vec3<f32>, base: vec3<f32>) -> vec3<f32> {
    let mask = saturate(textureSample(light_mask_tex, base_sampler, uv).r);
    var ramp_coord = dot(n, normalize(camera.light_dir)) * 0.25 + mask * 0.5 + 0.25;
    if mask > 0.95 {
        ramp_coord = 1.0;
    }
    if mask < 0.05 {
        ramp_coord = 0.0;
    }

    // cb6[26] = (0.4, SoftEdge, 0, 0)。保留汇编里阈值下方那条
    // `1-saturate(.4-q)`，而不是擅自改成普通 smoothstep。
    let below = 1.0 - saturate(0.4 - ramp_coord);
    let upper = saturate((ramp_coord - 0.4) /
                         max(0.1 * material.depth_fade.w, 1e-6));
    let ramp_u = mix(below, 1.0, upper);

    var ramp = srgb_to_linear(textureSample(
        ramp_tex, ramp_sampler, vec2<f32>(ramp_u, 1.0 / 256.0)).rgb);
    // Env_GameTime 的资产默认是 0，原式因而是 ramp * (.85 + .15 * 0)。
    ramp *= 0.85;
    ramp = mix(ramp, vec3<f32>(1.0), upper);

    // C_EnvColor.a=0 ⇒ 使用 View.SkyLightColor；本渲染器没有 UE 天光，取 0。
    let environment = ramp * 0.8;
    // C_ML_Sat_For_SS 的原始浮点是 (1,0,.1,.8)，所以 `.y=0`，汇编中的
    // `lerp(base, luma(base), .y)` 保留完整基色。不能用属性树的颜色十六进制显示去猜分量。
    // C_CharacterMainLightInfInt=1，且无 UE 动态阴影纹理时直射权重就是 upper。
    return base * (environment + vec3<f32>(upper));
}

/// 把硬件深度差换算成正交相机下的世界距离，再照原材质的
/// `OpenDepthDistance * saturate(gap / OpacityDepthDistance)` 求 alpha 增量。
fn trans_depth_coverage(in: VsOut) -> f32 {
    let fake_fluid = material.family_flags.z > 0.5;
    if !fake_fluid && (material.depth_fade.y <= 0.0 || material.depth_fade.x <= 0.0) {
        return 0.0;
    }
    let dimensions = vec2<i32>(textureDimensions(scene_depth));
    let pixel = clamp(vec2<i32>(in.clip.xy), vec2<i32>(0), dimensions - vec2<i32>(1));
    let opaque_depth = textureLoad(scene_depth, pixel, 0);
    // clip.z = dot(VP 的深度行, world)+常量；正交投影下除以该行长度就是米。
    let depth_row = vec3<f32>(
        camera.view_proj[0][2], camera.view_proj[1][2], camera.view_proj[2][2]
    );
    let gap_m = max(opaque_depth - in.clip.z, 0.0) / max(length(depth_row), 1e-6);
    if fake_fluid {
        // PS 42877 的 cb3[27].x 是 FadeDistance，资产单位为厘米。
        return saturate(gap_m / max(material.family10.w * 0.01, 1e-5));
    }
    return material.depth_fade.y * saturate(gap_m / material.depth_fade.x);
}

/// `MI_P_Object_XiaoYou` 的目标 ES3.1/Low PS 32511 主干。
///
/// 这条材质过去被误判为“没有 BaseTex 的纯特效”，导致整个 By1 身体进入半透明近似通道。
/// 原 PS 实际声明 MainTex(t2)、NoiseTex(t3)、StarTex(t4)，并且 `o0.w = 1`。其核心合成是：
///
/// - NoiseTex 在 FlowNoiseColor1/2 间混色；
/// - StarTex.r 提供闪烁相位、g 提供星形强度；
/// - `COLOR_0.r * COLOR_0.g * (1-COLOR_0.a)` 是逐顶点覆盖遮罩；
/// - `lerp(MF_ToneMapInverse(MainTex), lerp(BaseColor1,BaseColor2,flow.g), flow.r)`
///   与 flow/星光按上述遮罩合成。
///
/// 这里的 0.1、0.02 与 2π 都是 32511 第 70–75 行的字面量；其余数值来自材质参数，
/// 没有按宠物名称调色或手写星点。
fn shade_xiaoyou(in: VsOut) -> vec4<f32> {
    let main_sample = textureSample(base_color, base_sampler, in.uv);
    // 原资产元数据：MainTex sRGB=0、NoiseTex sRGB=1、StarTex sRGB=0。
    // PNG 不携带 UE 的采样色彩空间，故在专用链里显式还原硬件视图转换。
    let main_linear = game_tonemap_inverse(main_sample.rgb);

    let flow_uv = in.uv
        + material.xiaoyou_noise_flow.xy * camera.time
        + vec2<f32>(0.0, sin(fract(camera.time * 0.1) * 6.28318548) * 0.02);
    // `sample ... r2.z, ..., t3.xzyw`：目标寄存器写 z，而资源 z 被 swizzle 到 y，
    // 因而这里取的是 NoiseTex.g，不是视觉上最显眼的红通道。
    let noise = srgb_to_linear(textureSample(noise_tex, base_sampler, flow_uv).rgb).g;
    let flow1 = material.xiaoyou_flow1.rgb * material.xiaoyou_shape.x;
    let flow2 = material.xiaoyou_flow2.rgb * material.xiaoyou_shape.y;
    let flow_color = mix(flow1, flow2, noise);

    // preshader 把 UV_Control 的 Y/W 与 TwinkleSpeed 都除以 100 后再送入 cb。
    let star_uv = in.uv * material.xiaoyou_star_uv.xz
        + material.xiaoyou_star_uv.yw * (camera.time * 0.01);
    let star = textureSample(star_tex, base_sampler, star_uv);
    let phase = fract(camera.time * material.xiaoyou_shape.w * 0.01 + star.r) * 6.28318548;
    let wave = sin(phase) * 0.5 + 0.5;
    // cb6[52].x/y 分别是 `Star_RG_DarkTime` / `Star_RG_Int`。前者在
    // ML_P_Flow_XiaoYou 的冻结默认值中为 0，且三只实例均未覆盖；不是由 Int 反推阈值。
    let star_gain = max(material.xiaoyou_shape.z, 1.0);
    let star_pulse = saturate(wave) * star_gain;
    let star_amount = star.g * star_pulse;

    // 32511 第 92~97 行:噪声与星光都要算进覆盖遮罩,再乘顶点色 R·G·(1−A)。
    let vertex_mask = saturate(
        (1.0 + noise + star_amount) * in.color.r * in.color.g * (1.0 - in.color.a)
    );
    // **固有色不读 MainTex,读的是材质自己的 BaseColor1/BaseColor2** —— 32511 第 140~143 行:
    //     r4 = lerp(cb6[28], cb6[27], 1 − 顶点色 G)
    //     r3 = lerp(MF_ToneMapInverse(BaseTex), r4, 1 − 顶点色 A)
    // 这一族三只的顶点色 A 恒为 0 ⇒ 基色贴图整支权重为 0,固有色**全部**来自那对颜色;
    // 顶点色 G 在身体上是 1(取 BaseColor1 的暗紫)、在手臂上降到 0.23~0.71
    // (混向 BaseColor2 的青)——实机手臂那道紫→青的渐变就是它。
    //
    // 原来这里写的是 `mix(MainTex, mix(base1, base2, flow.g), flow.r)`:`flow.r` 只到 0.19,
    // 于是手臂几乎整块显示 `Tex_PetGlassy_007_D` 的原样。那是一张**全库共享的红/绿平铺图案图**
    // (design.md §1.1 早已查实「不是本体固有色」),照搬出来就是实机没有的橙绿斑。
    let custom_base = mix(material.xiaoyou_base1.rgb,
                          material.xiaoyou_base2.rgb,
                          saturate(1.0 - in.color.g));
    let base = mix(main_linear, custom_base, saturate(1.0 - in.color.a));
    let effect = flow_color + material.xiaoyou_star_color.rgb * star_amount;
    // r5(flow + star) 在原 PS 中加到 emissive 分支，绕过 mobile 直接/间接光；
    // 只有 `(1-mask) * base` 进入受光照的 base-color 分支。
    let ndl = dot(normalize(in.normal), normalize(camera.light_dir));
    let lit = smoothstep(SHADE_TERM_LO, SHADE_TERM_HI, ndl);
    let shade = mix(0.5, 1.5, lit) + AMBIENT;
    let surface = base * (1.0 - vertex_mask) * shade + effect * vertex_mask;
    return vec4<f32>(encode_linear_color(max(surface, vec3<f32>(0.0))), 1.0);
}

/// `M_Gra_Yutu_Ear_Lighting` 的目标 Low PS 6037 材质主干。原 PS 的 t2/t3/t4
/// 分别是 Bubble Texture、DistortTex、FlowTex；局部坐标先乘 -0.01，是因为 UE 顶点
/// 插值量以厘米传入，而 glTF 顶点已经换成米，所以这里直接取负的 local_pos。
fn shade_yutu_ear(in: VsOut) -> vec4<f32> {
    let n = normalize(in.normal);
    let ndv = max(dot(n, view_direction()), 0.0);

    // 6037 用两组正交局部坐标、两档速度采同一张泡泡图，再取均值。
    let bubble_scale = material.family7.z;
    let bubble_uv1 = -vec2<f32>(in.local_pos.x, in.local_pos.y) * bubble_scale
        + vec2<f32>(0.0, camera.time * material.family7.x);
    let bubble_uv2 = -vec2<f32>(in.local_pos.z, in.local_pos.y) * bubble_scale
        + vec2<f32>(0.0, camera.time * material.family7.y);
    // `sample r0.w, ..., t2.xzwy`：目标 w 对应资源 swizzle 的 w 项，即源绿色通道。
    let bubble1 = srgb_to_linear(textureSample(base_color, base_sampler, bubble_uv1).rgb).g;
    let bubble2 = srgb_to_linear(textureSample(base_color, base_sampler, bubble_uv2).rgb).g;
    let bubble = (bubble1 + bubble2) * 0.5;

    // `sample t3` 的 xy 扰动随后以 0.5×FlowDistort 加到 t4 的 panner 坐标。
    let planar = -vec2<f32>(in.local_pos.x, in.local_pos.y);
    let distort = textureSample(noise_tex, base_sampler, planar).rg;
    let flow_uv = planar * material.family8.zw
        + camera.time * material.family8.xy
        + distort * (0.5 * material.family7.w);
    let flow_sample = textureSample(star_tex, base_sampler, flow_uv).rgb;
    let flow = flow_sample * material.family1.rgb * material.family9.x;

    let fresnel = pow(max(1.0 - ndv, 1e-4), max(material.family9.y, 1e-4))
        * material.family9.z;

    // **液面是顶点色 R 的阈值门,不是法线朝向。** 目标 Low PS 70474 第 147~152 行:
    //     ge r2.w, v2.x, <随时间摆动的液面高度>     ← v2 = COLOR0(ISGN 查实)
    //     ge r3.w, v2.x, l(0.5)
    //     mul r2.w, r2.w, r3.w                      ← 两道门都过才算「液面之上」
    //     mad r7.xyz, …, r2.w, …                    ← 在液面色与内胆色之间选
    // 春兔耳膜里那泡液体的顶点色实测 R = 0(238 个顶点)/ 0.78(10 个),G 才是 1 ——
    // 也就是**几乎整泡都在液面以下**,取 InColor 那支。
    let top = select(0.0, 1.0, in.color.r >= 0.5);
    let ramped_inner = mix(material.family3.rgb * material.family5.rgb,
                           material.family6.rgb,
                           top);
    let bubbles = material.family0.rgb * bubble;
    let surface = (ramped_inner + bubbles + flow) * material.family4.rgb
        + material.family2.rgb * fresnel;

    // 目标材质最终仍进入 mobile 光照；颜色参数中的 Flow/Fresnel 是材质内的加光项。
    let ndl = dot(n, normalize(camera.light_dir));
    let lit = smoothstep(SHADE_TERM_LO, SHADE_TERM_HI, ndl);
    let shade = mix(0.5, 1.5, lit) + AMBIENT;
    let color = ramped_inner * material.family4.rgb * shade
        + (surface - ramped_inner * material.family4.rgb);
    // **整层不再乘顶点色 R。** 70474 里 `v2.x` 只出现在两处:第 88 行给
    // `lerp(cb5[6], cb5[5], …) × 菲涅尔` 那一层调强度、第 147~152 行当液面门 ——
    // 没有一处是乘在最终颜色上的。原来那一乘把整泡液体乘成了黑的(R 实测 0)。
    return vec4<f32>(encode_linear_color(max(color, vec3<f32>(0.0))), 1.0);
}

/// `M_P_FakeFulid` 的目标 Low PS 42877 局部链。原 shader 从 PrimitiveSceneData 读取
/// 当前蒙皮盒中心/尺度，以 AbsoluteWorldPosition 对水平虚拟平面求交，再合成场景深度、
/// FuildMask、MatCap、边缘/渐变色；最终覆盖率明确为
/// `saturate(matcap_luma + fluid_coverage) * COLOR_0.g`。
fn shade_fake_fluid(in: VsOut, depth_coverage: f32) -> vec4<f32> {
    let n = normalize(in.normal);
    let view = view_direction();
    let ndv = saturate(dot(n, view));

    // UE Z-up 参数换到 glTF Y-up，与导出器处理法线/位置的轴变换一致。
    let axis = normalize(vec3<f32>(material.family6.x, material.family6.z, material.family6.y));
    let plane_offset = vec3<f32>(material.family7.x, material.family7.z, material.family7.y) * 0.01;
    let center = camera.object_bounds.xyz + plane_offset;

    // 42877 第 63–70 行以像素世界位置相对 ObjectWorldPosition 的平面坐标采样，
    // 不是网格 UV。UE 的水平 XY 平面换到 glTF 后是 XZ；除最长边保持原组件缩放语义。
    let plane_uv = (in.world_pos.xz - center.xz) / max(camera.object_bounds.w, 1e-5);
    let height_uv = plane_uv * material.family5.xy
        + camera.time * material.family5.zw;
    let mask_sample = srgb_to_linear(textureSample(base_color, base_sampler, height_uv).rgb);
    let height_noise = (mask_sample.r * 2.0 - 1.0) * material.family8.w * 0.01;
    let signed_height = dot(in.world_pos - center, axis) - height_noise;
    let plane_soft = max(material.family10.y * 0.01, 1e-4);
    // 原 PS 的 r0.z 是世界位置位于噪声液面下方的比较结果；场景深度差随后按
    // FadeDistance 衰减成 r0.w。用 smoothstep 只复现 TopEdgeSmooth 的连续边界。
    let below_plane = 1.0 - smoothstep(-plane_soft, plane_soft, signed_height);
    let fluid = below_plane * depth_coverage;

    let depth = saturate(-signed_height / max(material.family9.x, 1e-4));
    let gradient_t = smoothstep(
        material.family9.x - material.family9.y,
        material.family9.x + material.family9.y,
        depth
    );
    let gradient = mix(material.family4.rgb, material.family3.rgb, gradient_t);
    let top_edge = depth_coverage * (1.0 - smoothstep(
        material.family10.x * 0.01,
        material.family10.x * 0.01 + plane_soft,
        abs(signed_height)
    ));
    var fluid_color = gradient;

    let matcap = srgb_to_linear(textureSample(matcap_tex, base_sampler, matcap_uv(n)).rgb)
        * material.matcap_color.rgb;
    let fresnel = smoothstep(material.family9.z,
                             material.family9.z + max(material.family9.w, 1e-4),
                             ndv);
    // 42877 第 178–185 行用“液面以下的世界距离 / BodyEdgeArea”构造边缘色，
    // 不是普通的视角 Fresnel。实例参数的 5/0.8/0.1 分别就是厘米范围与重映射阈值。
    let body_distance = max(-signed_height, 0.0);
    let body_proximity = 1.0 - saturate(
        body_distance / max(material.family8.x * 0.01, 1e-5)
    );
    let body_edge = smoothstep(material.family8.y,
                               material.family8.y + max(material.family8.z, 1e-4),
                               below_plane * body_proximity);
    fluid_color = mix(fluid_color, material.family2.rgb, saturate(top_edge));
    fluid_color = mix(fluid_color, material.family0.rgb, body_edge);
    let glass = matcap + material.family1.rgb * fresnel;
    let linear = glass + fluid_color * fluid;

    let matcap_luma = dot(matcap, vec3<f32>(0.3, 0.59, 0.11));
    let alpha = saturate(matcap_luma + fresnel + fluid) * in.color.g;
    let encoded = encode_linear_color(max(linear, vec3<f32>(0.0)));
    return vec4<f32>(encoded * alpha, alpha);
}

/// `M_P_MatCap_Masked` 的目标 ES3.1/Low PS 19654 材质局部链。
///
/// Color PS 最终 `o0.w` 明确写 1；基础遮罩在同 resource 的 Early-Z depth PS 15293
/// 中执行（函数末尾合并回来）。它也不是加色 VFX：19654 的 55–56 行是
/// `BaseColor * LightRamp + MatCap`，115–121 行依次接 Rim、Flat_Emissive、
/// MainColor/MainBright 与 SelectionColor。把 BaseColor=1.5 误当成“HDR tint ⇒ additive”
/// 会让这一层最后画且不写深度，正好把 FakeFulid 液面盖掉。
fn shade_matcap_masked(in: VsOut) -> vec4<f32> {
    let n = normalize(in.normal);
    let view = view_direction();
    let ndl = dot(n, normalize(camera.light_dir));

    // PS 19654 第 33–38 行：saturate((((N·L)+1)*.5-.3)*10)，再从
    // LightRampColor 混到白。MatCapTex 的资产标记 sRGB=1，显式恢复硬件解码。
    let ramp_t = saturate(((ndl + 1.0) * 0.5 - 0.3) * 10.0);
    let light_ramp = mix(material.family1.rgb, vec3<f32>(1.0), ramp_t);
    let matcap = srgb_to_linear(
        textureSample(base_color, base_sampler, matcap_uv(n)).rgb
    );
    var surface = material.family0.rgb * light_ramp + matcap;

    // cb3[5].xy / cb3[13].z：Rim Power、Rim Soft Edge、Rim Intensity。
    // 98–114 行先按观察方向 Z 缩窄轮廓区，再 pow、减 .5、除 SoftEdge。
    let ndv = saturate(dot(n, view));
    let rim_width = max((1.0 - abs(view.y)) * 0.4, 1e-4); // UE Z-up → glTF Y-up
    let rim_gate = 1.0 - saturate((ndv - 0.05) / rim_width);
    let rim_base = max((1.0 - ndv) * rim_gate, 1e-6);
    let rim = saturate(
        (pow(rim_base, max(material.family5.x, 1e-4)) - 0.5)
        / max(material.family5.y, 1e-4)
    ) * material.family5.z;
    // 原 r5 是 MobileBasePass 提供的中性环境/主光颜色；本渲染器的灯色同样为白。
    surface = mix(surface, vec3<f32>(1.0), saturate(rim));

    // 117–121 行。family6 = [FlatIntensity, FlatRatio, MainBright, XrayGate]。
    surface = mix(surface,
                  material.family2.rgb * material.family6.x,
                  saturate(material.family6.y));
    surface *= material.family3.rgb * material.family6.z;
    surface = mix(surface,
                  material.family4.rgb,
                  saturate(material.family4.a));

    // 基础 OpacityMask 不在上面的 color PS 里重复算：目标 resource 开着 masked
    // Early-Z，它由同资源的 depth PS 15293 第 30–45 行先写深度。那条链无条件采
    // MatCap、取亮度，与 pow(1-N·V,FresnelPow) 取 max，再减 cooked clip 0.3333。
    // 本渲染器是单遍，必须在这里合并同一条 depth-PS discard；只照抄 color PS 会让
    // 深度预通过滤凭空消失，整个闭合外壳写满深度并挡住内部 FakeFulid。
    let matcap_luma = dot(matcap, vec3<f32>(0.3, 0.59, 0.11));
    let fresnel = pow(max(1.0 - ndv, 1e-4), max(material.family5.w, 1e-4));
    if max(matcap_luma, fresnel) < 0.3333 {
        discard;
    }

    return vec4<f32>(encode_linear_color(max(surface, vec3<f32>(0.0))), 1.0);
}

fn shade_main(in: VsOut, depth_coverage: f32) -> vec4<f32> {
    if material.family_flags.x > 0.5 {
        return shade_xiaoyou(in);
    }
    if material.family_flags.y > 0.5 {
        return shade_yutu_ear(in);
    }
    if material.family_flags.z > 0.5 {
        return shade_fake_fluid(in, depth_coverage);
    }
    if material.family_flags.w > 0.5 {
        return shade_matcap_masked(in);
    }
    // 表情:脸那两个槽的贴图是 2×4 的图集,网格 UV 落在左上那一格,
    // 换表情就是整格地偏一下(flags.x = 这是脸)。其余材质偏移量恒为 0。
    let uv = in.uv + camera.face_uv * step(0.5, material.flags.x);
    let tex = textureSample(base_color, base_sampler, uv);
    // **alpha 有三种含义,由材质决定**(params.x / params.z):
    // - 镂空遮罩(眼/嘴的表情图集,params.x):按阈值剔,不剔就是一块方糊;
    // - **不透明度**(params.z,静态开关 `Opacity or OpacityMask` 点名的 11 个材质);
    // - 线条遮罩(其余本体):RGB 是完整固有色,alpha 里画着身上的纹路(水灵的竖条纹就在这儿)。
    //   这种**绝对不能拿来剔像素**——本体贴图的 alpha 覆盖率普遍很低(813 张里 60 张 <5%),
    //   剔了就只剩眼睛(火花)甚至整只消失(迪莫)。要做的是照着它提亮。
    let cutout = material.params.x > 0.5;
    if cutout && tex.a < 0.35 {
        discard;
    }
    let alpha_is_opacity = material.params.z > 0.5;
    let exact_object_trans = material.depth_fade.z > 0.5;
    let line = select(select(tex.a, 0.0, alpha_is_opacity), 0.0, cutout);

    let n = normalize(in.normal);
    let ndl = dot(n, normalize(camera.light_dir));
    // 两段明暗:亮部原色,暗部压到 0.72,过渡带 0.08 宽度避免锯齿。
    //
    // 实机是 `smoothstep(thr, hi, (N·L + 1) * 0.5)` 再 `lerp(暗色, 亮色, 结果)`
    // (汇编:`mad r0.x, N·L+1, 0.5, -cb5[59].x` → `div/mul_sat` 归一 → `t*t*(3-2t)`
    //  → `mad r4.xyz, r0.x, cb5[24]-cb5[25], cb5[25]` → `mul r3.xyz, 基色, r4.xyz`)。
    // 两处差别:
    // ① **半兰伯特只是换参数,不是结构差异** —— `smoothstep(a, b, (x+1)/2)` 恒等于
    //    `smoothstep(2a-1, 2b-1, x)`,所以这里照旧对 `ndl` 取阈值。不过实机那个偏置
    //    **也是个参数**(`mad r0.w, N·L, 0.5, cb6[104].y`),不是写死的 0.5;
    //    阈值上下界是另外两个槽(`cb6[104].w` / `.z`)。三个都没解出名字。
    // ② **「实机两端是颜色对」这条是错的,已更正:它是灰度对。** 重解冻结块查实(见
    //    rocom-capture/scripts/uniexpr.py 的「cb 布局」):那两个槽装的是 `Parameter(下标)`,
    //    下标落在**标量**参数段 —— 一个标量广播成 float4。所以 `mix(暗, 亮, lit)` 这个
    //    **灰度结构本身是对的**。
    //
    // **那对值已经读出来并代进来了:亮 = 1.5、暗 = 0.5**(`MI_P_Object_Trans_MatCap` 的
    // shader 20284,`cb6[60]`/`cb6[61]`,标量 #29 / #32)。它被 `r6 * r11 + r13` 消费
    // (r11 是那对高度渐变色),确认是乘在颜色上的明暗因子。
    //
    // 代进来之前踩过三条,记下来免得再走:
    // ① 直接换成 `mix(0.5, 1.5)`(还在显示空间时):幽星光圆顶中位 (238,143,200) →
    //    (255,209,255),对实机 (255,197,242) 的误差和 111 → 25,但**冲白 30.4%**(实机 0%),
    //    全库 `过曝` 9 → **109**。原因是亮端 1.5 是靠曝光压回来的,少了曝光就硬顶到白。
    // ② 只取暗部 `mix(0.5, 1.0)`:全库更干净(过曝 7),但**对比比实机更强** ——
    //    幽星光的裙子、暮星辰的翅膀都明显发暗,而实机那两处更亮更均匀。缺的是环境光。
    // ③ 只取比值 `mix(0.333, 1.0)`:误差 115,比原来的 111 **还差** —— 差距不在暗部在亮面。
    //
    // 另外那个「实机圆顶更亮更不饱和」的差距**不全是材质**:按加性白项拟合,线性下加 0.283
    // 白能让三个通道**同时**吻合 (1.0,0.773,0.948) vs 实机 (1.0,0.773,0.949) —— 而汇编尾部
    // 正好有 `mad r0.xyz, r0, v5.w, v5.xyz`(**高度雾的 inscatter**,加性)。也就是说参考截图里
    // 那层淡白是**场景的雾**,桌宠不该有,所以圆顶颜色存在一个不可消的偏差,别去追。
    let lit = smoothstep(SHADE_TERM_LO, SHADE_TERM_HI, ndl);
    // **直接光项照汇编:暗 0.5 / 亮 1.5**(见上面 ② 那段),再加一层环境光。
    //
    // **环境那一项是必须的,不是补丁。** 汇编那对只乘在**直接光**上,而实机的 mobile base pass
    // 还叠着天光/间接光(`cb0` 那批 View 常量,离线读不出来)。只代 0.5 不加环境,暗部就比实机
    // **更深** —— 实测幽星光的裙子、暮星辰的翅膀都明显发暗,而实机那两处更亮更均匀。
    // 所以这里的自由度只剩**一个** `AMBIENT`(替掉原来凭空的 0.72,它本来是「直接+间接」
    // 揉成一个数),取值让亮/暗两端与标定过的观感等值:
    //   亮 `sqrt((1.5 + A) · E) = 1`、暗 `sqrt((0.5 + A) · E) = 0.72` ⇒ A = 0.5765、E = 0.4816。
    let shade = mix(0.5, 1.5, lit) + AMBIENT;
    let facing = facing_ratio(n);
    // 边缘光:**汇编里没有这一层,是我们自己加的**(桌宠场景下让轮廓从背景里浮出来)。
    // 系数 0.25 调于 `466326f`,那时 `facing_ratio` 的视线还写死世界 +Z(修于 `ba49e56`)、
    // 法线还是切线(修于 `1daa75e`)—— 两个前提都变了,这个数从没重新标过。
    // 系数按 `旧² / EXPOSURE` = 0.25² / 0.4816 ≈ 0.13 换算到线性(**指数不动**:
    // 平方会把 `pow(facing,3)` 变成 `pow(facing,6)`,那是改形状不是改强度)。
    let rim = pow(facing, 3.0) * 0.13;

    // 固有色:卷动色带 → 两段明暗 → 纹路提亮(alpha 高的地方比底色亮一档)。
    //
    // **不再乘 `MainColor`。** 原来对半透族乘了一层 `MainColor`(暮星辰裙子 (0.39,0.4,0.63)),
    // 理由是「不乘裙子会偏白」—— 那也是在错法线上看到的。对着实机截图量:裙子实测
    // (71,91,232),而基色贴图在那块 UV 是 (66,64,197),**几乎就是基色原样**;乘上去只有
    // (26,26,124),暗了三倍。另外静态开关 `GlassySwitch` 全库一个没开,而 `MainColor`
    // 属于那条 glassy 通路 —— 两边都指向「这一乘是多余的」。
    // 纯特效层的主色仍走 `tint`(那些材质压根没有基色贴图),不受影响。
    // **平方 = gamma 2.0 解码**,把基色贴图从显示空间搬进线性(见 `EXPOSURE`)。
    // 卷动色带那张也是显示空间的成品颜色,所以在 `flow_band` 里混完再一起平方。
    var albedo = flow_band(in.uv, tex.rgb);
    if exact_object_trans {
        albedo = game_tonemap_inverse(srgb_to_linear(albedo));
    } else {
        albedo = pow(albedo, vec3<f32>(DECODE_GAMMA));
    }
    // 基色 alpha 的那个重映射:`Glow Color × Glow Intensity` 与 `Emitter Color ×
    // Emitter Intensity` 两层共用它当遮罩(水灵 Low PS 68952 第 60~65 行与第 145 行,
    // 两条都是 `颜色 × 这个遮罩`,而且**都进发光累加器 r1**、在光照之后才相加)。
    let detail_mask = saturate((line - 0.04) * 1.1111);
    // 加上去的光。**不透明层不叠 MatCap**——游戏那边靠遮罩通道选择性反射,
    // 无条件叠会把宠物冲白(试过,整只发白),而 toon 着色本身对着截图已经够像。
    //
    // 那层白色 `rim` 是我们自己加的(汇编里没有,桌宠场景下让轮廓从背景里浮出来)。
    // **玻璃族不加**:它有材质自己的边缘光(`RimColor`/`RimIntensity`/`RimPower`),
    // 两层叠起来轮廓会糊成一圈白 —— 暮星辰的裙子就是这么被冲成淡青的。
    let generic_rim = select(rim, 0.0, material.flags.y > 0.5);
    // `params.y` = `Glow Color × Glow Intensity`(实测全库根默认 0、只有 2 处实例覆盖),
    // 位置按汇编放进发光层;原来它加在固有色上,那是从另一条 shader 读串了。
    var glow = vec3<f32>(generic_rim + material.params.y * detail_mask);
    // **自发光**:`Emitter Color × Emitter Intensity × 基色 alpha 的那个重映射`,
    // 线性空间里加性叠加在光照**之后**。遮罩不再是猜的了 —— 水灵本体那条
    // ES3.1/Low/LOD0 的 `M_P_Object` 像素着色器(资源 `BF0167AE…`,PS 68952)写着:
    //
    //     add     r2.z, r3.w, l(-0.04)          ← r3 = BaseTex 采样
    //     mul_sat r2.z, r2.z, l(1.1111)         ← 和不透明度用的是同一个重映射
    //     mul     r4.xyz, r2.z, cb6[2].xyzx     ← 颜色 × 那个遮罩
    //     mul     r5.xyz, r4.xyzx, cb6[39].z    ← × 强度标量
    //     …
    //     add     r0.xyz, r0.xyzx, r1.xyzx      ← 第 268 行:发光累加器 + 已着色的颜色
    //
    // 也就是「**基色 alpha 画在哪儿就发光在哪儿**」,整条链里没有任何 Fresnel/N·V 项。
    // 两张贴图都能对上眼:水灵 `_By_D` 的 alpha 就是身上那几道竖向条纹(>200 占 21%),
    // 火神 `_By_D` 的 alpha 只圈住火焰爪/角上的高光块(>200 占 1.1%),黑翅膀是 0。
    //
    // 原来这里拿 `facing` 当遮罩、把整层糊在所有像素上:水灵的条纹一根都看不见(遮罩没参与),
    // 火神那对黑翅膀反被橙色自发光染成褐黄 —— 实机报的「多了一层黄色遮罩」就是它。
    if material.emissive.w > 0.0 {
        glow += material.emissive.rgb * material.emissive.w * detail_mask;
    }
    // **不透明度**:`alpha_is_opacity` 的材质取基色 alpha,并照汇编做那个重映射
    // (`add r1.z, a, -0.04` → `mul_sat r1.z, r1.z, 1.1111`,即把 0.04..0.94 拉到 0..1)。
    // 暮星辰裙子那块 UV 的 alpha 中位 0.537 → 0.55,与从实机截图水印衰减反推的 0.50 对得上。
    var alpha = select(1.0, saturate((tex.a - 0.04) * 1.1111), alpha_is_opacity);

    // **玻璃 / 薄纱**(`MI_P_Object_Trans_*` 族:幽星光那两个球、暮星辰的裙子与球)。
    // 只有这一族叠 MatCap 高光与材质自己的边缘光。
    //
    // 材质里的边缘光是**加在边上的一层光**,不能拿去染固有色:球的颜色就是基色图集里
    // 那片平色圆盘。导出器只把「`Rim Intensity` 真的大于 1」的边缘光写进来(见 Manifest.cs)——
    // 曜星光那两颗球写着强度 1 + 绿色 `Rim LightColor`,而实机里它们是橙的和紫的。
    if material.flags.y > 0.5 {
        let spec_coverage = trans_spec_coverage(n);
        // **加上去的几层光是 `max` 合的,不是相加。** 汇编里连着两条:
        // `max r2.yzw, matcap*MatCapColor, spec*SpecColor` 再 `max r2.xyz, 上一步, rim`。
        // 相加会让高光与边缘光在轮廓处叠成一圈白边;取 max 则是「哪层亮听哪层」。
        let rim_strength = saturate(pow(facing, material.extra.x) * material.star.z);
        // 这组 SpecCol 属于 `M_P_Object_Trans` 的 alpha-opacity 排列；MatCap 族是另一张
        // 材质图，继续只走自己的查找表，不能被这一组根默认高光改色。
        let spec_light = select(vec3<f32>(0.0),
                                material.highlight_color.rgb * max(spec_coverage, 0.0),
                                alpha_is_opacity);
        glow += max(spec_light,
                    max(matcap_light(n) * GLASS_MATCAP_GAIN,
                        material.rim_color.rgb * rim_strength * GLASS_RIM_GAIN));
        // 目标实机实际选中的 Low 排列先做
        // `lerp(max(基色 alpha, spec), 基色 alpha, ForceUseDefOpacity)`，再加场景深度淡化。
        // 这里没有别的排列里的 rim/Fresnel alpha，也没有 MatCap 采样值。
        if alpha_is_opacity {
            let covered = max(alpha, spec_coverage);
            alpha = mix(covered, alpha, saturate(material.rim_color.w));
            alpha = saturate(alpha + depth_coverage);
        }
        // **rim 与 MatCap 那两层也顶不透明度,这一条不能省。** 汇编
        // (`MI_P_Object_Trans_MatCap` 37998)里输出 alpha 是 `max(基色a, 高光, 菲涅尔)` ——
        // 上面那段只补了「高光」那一路(而且只在 alpha 是不透明度时),菲涅尔/MatCap 那一路
        // 仍要在这儿取 max。**不接的代价**:幽星光那两颗球的基色 alpha 中位 0.000、p90 也是
        // 0.000(形状压根不在基色里),球会整个消失;暮星辰的裙子会薄掉一档
        // (量过:去掉这行 0.074 → 0.091,再叠上 Low 分支是 0.084)。
        alpha = max(alpha, max(rim_strength, saturate(matcap_strength(n))));
        // **球内那颗星是「混进固有色」,不是加在上面。** 汇编最后一步是
        // `out = lerp(基色 × 明暗色, 发光层色, 混合系数)`(fx1/34529.asm ⑥,见 design.md §1),
        // 而发光层色 = `星点底色 + 星点强度 × 星点亮色`,再与「按物体空间高度 lerp 的那对颜色」混。
        // 我们只拿到其中的 `StarColor`(根默认 (0.33,0.67,2) 的 HDR 蓝),底色那两对与混合
        // 系数都是还没解出名字的 cb 槽位,所以这里退化成「按星点强度往 StarColor 混」——
        // 结构照汇编(lerp 而不是相加),缺的那几项当成中性。
        // **HDR 的材质色现在直接用,不再预先 sqrt。** 原来那个 `sqrt` 是在「整条链跑在
        // 显示空间」时代的补偿(把线性 HDR 值硬编码成显示值);现在固有色链路本来就在线性里、
        // 末尾统一 `sqrt(色 × 曝光)`,再单独 sqrt 一次就是编码两次了。
        let star_color = material.interior_color.rgb;
        // **折射必须在物体空间算**(见 `interior_star`),所以这里传物体空间的法线与视线,
        // 不是世界空间的 `n`。
        albedo = mix(albedo, star_color,
                     saturate(interior_star(in.local_pos,
                                            normalize(in.local_normal),
                                            normalize(in.local_view))));
        // 玻璃族自身的整体不透明度;`alpha_is_opacity` 的材质已经从基色 alpha 拿到了,别覆盖
        if !alpha_is_opacity {
            alpha = clamp(material.star.w, 0.0, 1.0);
        }
        // **玻璃也吃两段明暗。** 这一族走的是同一条固有色链路:同一个 pixel shader 里
        // `mul r3.xyz, 基色, lerp(暗色, 亮色, smoothstep(N·L))` 就在折射/matcap 那些
        // 分支的下游,没有任何开关把玻璃排除掉。原来这儿硬写 `lambert = 1.0`,理由是
        // 「开口薄壳自转时会在 0.72↔1.0 之间跳」—— 那个跳动是法线被写成切线造成的
        // (见 design.md 法线那条),法线修好后不复存在,所以这个特例撤掉。
    }
    // **线条遮罩是「加一个颜色」,不是「乘一个亮度倍数」** —— 查实于罗隐(阿米亚特)的 body
    // shader 51377 第 99~103 行:
    //     r1.w = saturate((基色.a − 0.04) × 1.1111)      ← 和不透明度用的是同一个重映射
    //     mad r6.xyz, cb6[7].xyzx, r1.w, r6.xyzx          ← 加上 cb6[7] × 那个遮罩
    // 原来这里是 `× mix(1.0, LINE_BOOST, alpha)`(乘法),形状就不对。
    // `cb6[7]` 那个颜色的名字还没解出来(这条 shader 的 V=112,全库没有材质带这个块),
    // 所以先取中性白 × 一个标定强度 —— 但**形状按汇编改对了**。
    // **星贴层是 lerp 替换已着色的颜色,不是加一层光、也不是染固有色。** 汇编:
    //     mad r7.xyw, Stick_Intensity, r7.xyxw, -r0.xyxz    ← 强度 × (m × 渐变色) − 底
    //     mad r0.xyz, r9.w, r7.xywx, r0.xyzx                ← 底 + 混合系数 × 上面那个差
    // 合起来 `lerp(底, Stick_Intensity × m × c, saturate(m + GlassyMainColorOpacity))`。
    //
    // **位置很要紧**:那一步作用在 `r0`(效果累加器)上,而固有色累加器是 `r6` ——
    // 两者到第 487 行才合并。所以渐变色**不该再乘 `shade`**。先放在 `albedo * shade`
    // 之前试过:`shade` 最高 3.0(两段明暗 1.5 + AMBIENT 1.5),把浓色直接顶到过曝,
    // 全库过曝 11 → 14,多出来的正好是开着这层的星光族三只。
    //
    // 渐变色是材质参数(本来就在线性空间),所以**不过 `DECODE_GAMMA`** —— 只有贴图要解码。
    let stick = stick_layer(in.uv, in.ndc);
    // **假半透族那层是加光,不是 lerp 替换。** 上面那条 lerp 是 `StarStickTex` 那一族的
    // (汇编查实);两族公式不同,合并成一套是已知的简化。这一族的星点色是
    // `Color02 × Mat_NoiseIntensity`(幽星光 15 × 0.05 = 0.75),**比被照亮的身体还暗** ——
    // 替上去就是一片深色麻点(用户实测「星点的黑灰色明显不对」)。加上去才是星芒。
    // **判据要跟着坐标系走,不能用 `params.w`。** `star_fake_trans` 那个标记只有 `_Fx` 有,
    // 而身体是 `_By` 画的 —— 按 `params.w` 判会让 `_By` 走 lerp 替换那一支,
    // 拿一个比底色暗的星点色替上去 ⇒ **身上一片黑斑**(用户实测)。
    // `noise_uv.w < 0.5` 表示"这个材质用相机空间的噪声坐标",与加光是同一族。
    let fake_trans = material.noise_uv.w < 0.5;
    // 高质量 `M_P_Object_Trans` 排列还会在 ForceUseDefOpacity 之后
    // `alpha = max(alpha, starMask)`；目标实机的 Low 排列在 `stick_layer` 已返回空层。
    if alpha_is_opacity && material.flags.y > 0.5 && !fake_trans
        && camera.high_material_quality > 0.5 {
        alpha = max(alpha, stick.cover);
    }
    // **星贴层混的是固有色,不是已经着色的颜色。** 汇编(`M_P_Object_Trans` 51670)里
    // 那条 lerp 作用在 `r0` 上,而 `r0` 一路攒到第 690 行才 `mul r1.xzw, r0.xxyz, r1.xxzw`
    // **乘上光照** —— 也就是星点色要跟着这一点的明暗一起变。原来写在 `albedo * shade`
    // **之后**,等于把一块不受光的平色贴上去:亮处不亮、暗处不暗,实机看不见的这一层
    // 在我们这儿成了一身灰紫斑(实机报的「果冻…看不到星点」)。
    // **`else` 那支的位置本来就是对的**:`mov r0.xyz, r9.xyzx`(第 511 行)—— 不开这层时
    // `r0` 就是干净的固有色,同样在乘光照之前。
    let surface_albedo = select(mix(albedo, stick.color, stick.cover), albedo, fake_trans);
    var body = surface_albedo * shade;
    if exact_object_trans {
        body = object_trans_low_light(in.uv, n, surface_albedo);
    }
    let stick_add = select(vec3<f32>(0.0), stick.color, fake_trans);
    // **末尾统一编码到显示空间**:`sqrt(色 × 曝光)`,照汇编尾部那条
    // `movc o0.xyz, (曝光 < 1), sqrt(色 × 曝光), 色`。
    //
    // **`glow` 也在线性里了**,所以几层光**先在线性里相加、再一起编码一次**——
    // 这才是对的:加性光就该在线性里加。原来是各自在显示空间加,等于把小值各编码一次
    // (`sqrt` 会放大小值:0.25 → 0.418),几层叠起来偏亮。
    // 四个系数按 `旧² / EXPOSURE` 换算过,保持观感等值(见各自的定义处)。
    //
    // UE 的 BLEND_Translucent 在 pixel shader **完成颜色输出以后**才由固定功能混合器执行
    // `SrcColor * SrcAlpha + DstColor * (1-SrcAlpha)`。我们的管线使用等价的预乘混合，
    // 因而必须先把整条直色链（包括高光/星点/边缘光）编码到输出空间，最后才预乘 alpha。
    //
    // 旧代码在这里先在线性空间做 `body * alpha`、随后再 sqrt：半透明贡献实际变成
    // `sqrt(alpha)`，例如 alpha=.2 会按约 .45 的强度盖住内层；同时 glow 又完全没乘
    // alpha。两处都与游戏的 BLEND_Translucent 顺序相反，会系统性冲亮所有透明壳。
    var combined = body + glow + stick_add;
    if exact_object_trans {
        // PS 尾部 cb6[29].xyz，覆盖基色与高光在内的整条材质颜色。
        combined *= material.tint.rgb;
    }
    var encoded: vec3<f32>;
    if exact_object_trans {
        // 2109/55790 的真实尾段；精确分支不能再经过项目为通用 toon 增补的软肩。
        encoded = encode_linear_color(combined);
    } else {
        var lin = max(combined, vec3<f32>(0.0)) * EXPOSURE;
        // **软肩**:尚未逐材质还原的通用 toon 分支仍用它补 LDR 余量。目标 Low
        // ObjectTrans 与 M_ShuiMu_ByIn 已有原 PS，均不走这里。
        if SHOULDER_WHITE > 0.0 {
            let w2 = SHOULDER_WHITE * SHOULDER_WHITE;
            lin = lin * (1.0 + lin / w2) / (1.0 + lin);
        }
        encoded = sqrt(lin);
    }
    return vec4<f32>(encoded * alpha, alpha);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return shade_main(in, 0.0);
}

@fragment
fn fs_glass(in: VsOut) -> @location(0) vec4<f32> {
    return shade_main(in, trans_depth_coverage(in));
}

@fragment
fn fs_outline(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_color, base_sampler, in.uv);
    // 只有镂空遮罩才剔;本体的线条遮罩要是拿来剔,描边壳会跟着被啃掉
    if material.params.x > 0.5 && tex.a < 0.35 {
        discard;
    }
    // 描边取基色的暗版而不是纯黑,卡通渲染里这样更自然。
    //
    // **0.25 → 0.55 是量出来的**(2026-07-28,17 只有实机截图的宠物全覆盖)。做法:把两边的
    // 不透明遮罩各腐蚀 2 像素,分成「描边环」与「主体」,比 `中位(环)/中位(主体)`。
    // 原来我们的描边环只有实机的 **0.50** 倍亮(即深得多);0.70 抬到 0.98,整只的
    // 跨度比也从 1.14 降到 1.03。**先取过 0.55(留一档辨识度),后来取景守卫修好后重标,0.70 才落在 1.00**
    // —— 桌宠要在任意背景上认得出轮廓,所以留一档。
    //
    // **这条是从一次差点走偏的排查里捞出来的**,过程值得记:先量到「我们整体比实机更花」
    // (跨度比 1.29),差点去调 `AMBIENT`;做了两步分解才找对地方 ——
    // ① 抗锯齿对照(渲 4 倍再缩)只把比值从 1.54 拉到 1.47,**不是走样造成的**;
    // ② 换成对边缘不敏感的统计量,`p75−p25` 的比值只有 **1.05** —— 身体主体的对比其实是对的,
    //    超出的全在暗尾;腐蚀 2 像素后暗端比从 0.75 跳到 0.95,**暗尾就是这条描边**。
    // 所以调 `AMBIENT` 会是错的:它会为了掩盖描边而把整只的明暗压平。
    //
    // **实机侧的抠图也踩过一次**:按「与角落背景色的距离」判,好几张截图里宠物很小、
    // 背景是带花纹的卡片,会把大片背景算成宠物(菊花梨的「色偏」因此虚高到 1.46,
    // 修正后只有 0.16)。判据要加两道:**取最大连通块**、面积占比 > 55% 视为抠图失败。
    // **不是每个材质绑在 `base_color` 上的都是固有色贴图。** 四个专用族里绑的分别是
    // MatCap 查找表 / 泡泡图 / 液面遮罩 / 全库共享的图案图 —— 按网格 UV 采出来是一块
    // 与外观无关的乱色。其中走不透明通道、因而真的会画描边的是下面两族;
    // 耳膜(special_opaque)与液面(混合通道)本来就不进这一遍。
    var albedo = tex.rgb;
    if material.family_flags.w > 0.5 {
        // `M_P_MatCap_Masked`:克莱因龙的玻璃壳。整只泡泡原来罩着一层黑,就是这一遍 ——
        // 外扩的背面壳用 `MatCap34` 按网格 UV 采出近黑色,而正面壳按遮罩 discard 掉了,
        // 于是背面壳直接露出来铺满整个泡泡。这里补上同一条遮罩(与 depth PS 15293 一致),
        // 描边底色改用材质自己的 BaseColor。
        let n = normalize(in.normal);
        let ndv = saturate(dot(n, view_direction()));
        let matcap = srgb_to_linear(
            textureSample(base_color, base_sampler, matcap_uv(n)).rgb);
        let matcap_luma = dot(matcap, vec3<f32>(0.3, 0.59, 0.11));
        let fresnel = pow(max(1.0 - ndv, 1e-4), max(material.family5.w, 1e-4));
        if max(matcap_luma, fresnel) < 0.3333 {
            discard;
        }
        albedo = encode_linear_color(material.family0.rgb);
    } else if material.family_flags.x > 0.5 {
        // `MI_P_Object_XiaoYou`:绑的是 `Tex_PetGlassy_007_D`(红/绿平铺图案),
        // 描边因此在小灵面身上镶了一圈橙绿。改用与 `shade_xiaoyou` 同一条固有色。
        albedo = encode_linear_color(mix(material.xiaoyou_base1.rgb,
                                         material.xiaoyou_base2.rgb,
                                         saturate(1.0 - in.color.g)));
    }
    return vec4<f32>(albedo * 0.80, 1.0);
}

/// `M_ShuiMu_ByIn` 的原始材质局部链（pixel shader 71636）。这不是通用特效近似：
///
/// 1. 用预蒙皮局部位置/法线和 `GlassyNoiseRefract` 求折射方向；
/// 2. 按组件包围盒、Depth、UVScale 与 Speed 构造三平面坐标；
/// 3. 三次采 `GlassyNoiseTex` 的 R/A，并按原来的两次 lerp 合成 `saturate(R*A)`；
/// 4. 在 FlowColor02→01 之间混色，再按 Schlick 修正后的 Fresnel mask 混向 FresnelColor。
///
/// 原 shader 是 BLEND_Opaque 且 `o0.w = 1`；GPU 侧因此给这一入口独立的写深度管线。
/// 71636 第 104–387 行还有 UE clustered local-light 循环，但它的最终贡献全乘在
/// `lerp(Flat_EmissiveColor * FlatRatio, SelectionColor, SelectionColor.a)` 上。该实例的
/// `FlatRatio=0`、`SelectionColor=(0,0,0,0)`，uniform preshader 因而把这项精确求成 0；
/// 这里不凭空补一层 N·L 或环境光。
@fragment
fn fs_glassy_inner(in: VsOut) -> @location(0) vec4<f32> {
    let start = runtime_to_ue(in.local_pos);
    let local_n = normalize(runtime_to_ue(in.local_normal));
    let incident = normalize(runtime_to_ue(in.local_view));
    // 71636 读取的不是原参数本身，而是 uniform preshader 的 scalar-slot[5]：
    //     Constant(1), Constant(1), GlassyNoiseRefract, Add, Div
    // 即 eta = 1 / (1 + GlassyNoiseRefract)。果冻的 0.2 因而得到 0.833333；旧实现把
    // 0.2 直接交给 refract，会把三平面噪声投射到完全不同的位置。
    let eta = 1.0 / (1.0 + material.glassy_noise.z);
    let refracted = refract_direction(incident, local_n, eta);

    // 71636: halfExtent = length(.5*(boundsMax-boundsMin));
    // p = (start + refracted*(halfExtent*.01*Depth))*(UVScale/halfExtent)
    //     + frac(Time*Speed)。
    let half_extent = 0.5 * length(material.bounds_size.xyz);
    let march = half_extent * 0.01 * material.glassy_noise.w;
    let scale = material.glassy_noise.y / max(half_extent, 1e-4);
    let scroll = fract(camera.time * material.glassy_noise.x);
    let p = (start + refracted * march) * scale + vec3<f32>(scroll);

    let sample_xz = textureSample(noise_tex, base_sampler, p.xz);
    let sample_yz = textureSample(noise_tex, base_sampler, p.yz);
    let sample_xy = textureSample(noise_tex, base_sampler, p.xy);
    let tri = material.glassy_mask.w;
    let weight = saturate(abs(local_n) * (1.0 + 2.0 * tri) - tri);
    // 汇编先按 localNormal.x 混 xz→yz，再按 localNormal.z 混向 xy；每次只保留 R/A。
    let ra_xz = vec2<f32>(sample_xz.r, sample_xz.a);
    let ra_yz = vec2<f32>(sample_yz.r, sample_yz.a);
    let ra_xy = vec2<f32>(sample_xy.r, sample_xy.a);
    let ra = mix(mix(ra_xz, ra_yz, weight.x), ra_xy, weight.z);
    let noise = saturate(ra.x * ra.y);
    let flow_color = mix(material.glassy_flow2.rgb, material.glassy_flow1.rgb, noise);

    let n = normalize(in.normal);
    let ndv = max(dot(n, view_direction()), 0.0);
    var fresnel = pow(max(abs(1.0 - ndv), 1e-4), material.glassy_mask.x);
    fresnel = 0.96 * fresnel + 0.04;
    let mask_t = saturate(
        (fresnel - material.glassy_mask.y) / max(material.glassy_mask.z, 1e-6)
    );
    let fresnel_mask = mask_t * mask_t * (3.0 - 2.0 * mask_t);
    let color = mix(flow_color, material.glassy_fresnel.rgb, fresnel_mask);
    return vec4<f32>(encode_linear_color(color), 1.0);
}

// 纯特效层(火焰 / 水壳 / 光晕):材质里没有 BaseTex/EyeTex,固有色是 shader 算的。
// **有基色的半透材质不走这里**——暮星辰的裙子、那两个球都有基色贴图,和不透明本体共用
// `fs_main`,只是多一个 alpha,少一次代码分叉。
//
// **不是复刻游戏 shader,是够用的近似**:
// 主色 × 遮罩 × 卷动噪声,加色或半透二选一。参数全部来自游戏材质实例:
// 火花 `M_FX_Fire_Mat` 给 Color01=(6,0.8,0)(R>1 的 HDR 橙,说明是加色)+ Mask/Noise + 流速;
// 水蓝蓝 `M_Wat_ShuiLanLan_PP` 给 MainColor 浅蓝 + Opacity=0.8 + MatCap(当遮罩用)。
//
// 输出**预乘 alpha**,于是一条混合状态覆盖两种模式:
// - 加色:alpha 输出 0 → dst + rgb,黑色不加东西,正好是加色的语义;
// - 半透:alpha 输出不透明度 → 常规 src + dst*(1-a)。
@fragment
fn fs_effect(in: VsOut) -> @location(0) vec4<f32> {
    let opacity = material.params.x;
    let glow = material.params.y;
    let additive = material.params.z > 0.5;
    let has_noise = material.params.w > 0.5;

    // 遮罩决定形状。**matcap 要按视空间法线采样**(它是球面反射查找表),
    // 拿网格 UV 采会糊成一块块的斑——水灵的水膜踩过这个坑。
    let n = normalize(in.normal);
    let mask_uv = select(in.uv, matcap_uv(n), material.flags.x > 0.5);
    let mask = textureSample(base_color, base_sampler, mask_uv);
    var flow_amount = 1.0;
    if has_noise {
        let uv = mask_uv * material.flow.zw + vec2<f32>(material.flow.x, material.flow.y) * camera.time;
        flow_amount = textureSample(noise_tex, base_sampler, uv).r;
    }

    // 边缘处更亮/更实:水壳的菲涅尔感与火焰的边缘都靠这个
    let facing = facing_ratio(n);
    let rim = mix(EFFECT_RIM_FLOOR, 1.0, facing);

    let strength = mask.a * flow_amount * rim;
    // **这一层至今整个留在显示空间**(主通道早就搬进线性了)。搬过来试过三种编码 ×
    // 四档 `EFFECT_RIM_FLOOR`,**每一档都比现状差** —— 见下面常量的注释。
    let color = material.tint.rgb * glow * strength;
    if additive {
        // 加色:alpha=0,只往目标上加光
        return vec4<f32>(color, 0.0);
    }
    // **不透明度不含 `tint.a`。** `tint` 来自 `Color01` / `MainColor` / `Emitter Color`
    // 这类**颜色**参数,它们的 alpha 是 UE `FLinearColor` 里美术随手留下的分量,不是不透明度
    // —— 真正的不透明度是 `Opacity` 那个标量,已经在 `opacity` 里了。
    // **判据**:全库 65 个纯特效层里 `tint.a` 只取 0(17 个)与 1(31 个)两种值,
    // **一个中间值都没有**;真是不透明度不会长这样。
    // 代价实测:那 17 个里非加色的会被整层乘成 0 —— 克莱因龙那只半透明的气泡身体
    // (`MI_P_FakeFulid`,tint = (0.22, 0.22, 0.375, **0**))就是这么**整个消失**的。
    // 指标对它是盲的(调色板一位不动),验收靠裁图对着实机看。
    let alpha = clamp(strength * opacity, 0.0, 1.0);
    // 预乘
    return vec4<f32>(material.tint.rgb * alpha, alpha);
}
