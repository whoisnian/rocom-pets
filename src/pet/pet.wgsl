// 宠物着色:顶点里做蒙皮,片元里做卡通(分段)光照 + 边缘光;描边走第二遍法线外扩。
//
// 目标是「像」游戏那套自研 toon,而不是复刻(设计 §3.3):基色贴图 + 2 段明暗 + 轻边缘光
// + 描边,已经能抓住观感。
//
// **MatCap / StarStick / 玻璃内部层这几层后来是照反汇编做的**(见 docs/shader.md),
// 不再是「不追」。但**基础 toon 那几个数仍然是猜的**,而且是在上游法线 bug 修好**之前**
// 调出来的、之后没复核过 —— 逐个标在下面各自的定义处:
//   `mix(0.72, 1.0, lit)` 的 0.72、`smoothstep(-0.04, 0.04, ndl)` 的阈值与过渡宽度、
//   `rim = pow(facing, 3.0) * 0.25`、`gpu.rs` 的 `LINE_BOOST = 1.55`。

struct Camera {
    view_proj: mat4x4<f32>,
    // 光照方向(指向光源)与描边参数打包进一个 vec4 省 binding
    light_dir: vec3<f32>,
    outline_width: f32,
    // 秒;特效层的 UV 卷动靠它推进
    time: f32,
    // 不要在这儿补 vec3 占位:WGSL 里 vec3 要 16 字节对齐,会把结构体从 96 撑到 112,
    // 和 Rust 侧的 96 对不上(wgpu 会报 "bound with size 96 where the shader expects 112")。
    // mat4x4 已经让整个结构按 16 对齐,尾部的 12 字节填充由规则自动补上。
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
    rim_color: vec4<f32>,
    // [边缘光衰减次数, 色带混入强度, -, 有没有色带]
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

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) joint_ids: vec4<u32>,
    @location(4) weights: vec4<f32>,
    // 玻璃内部层的采样起点 (UV1.x, UV1.y, UV2.x),见 model.rs 的 `interior_pos`
    @location(5) interior_pos: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    // 玻璃内部层的采样起点(直接透传顶点属性)
    @location(2) interior_pos: vec3<f32>,
    // **物体空间**的法线与视线:玻璃内部层的折射必须在这个空间里算(见 `interior_star`)
    @location(3) local_normal: vec3<f32>,
    @location(4) local_view: vec3<f32>,
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
    out.uv = input.uv;
    out.normal = normal;
    out.interior_pos = input.interior_pos;
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
    out.uv = input.uv;
    out.normal = normal;
    out.interior_pos = input.interior_pos;
    out.local_normal = normalize(input.normal);
    out.local_view = vec3<f32>(0.0, 0.0, 1.0);
    return out;
}


/// 星点层的额外平铺倍率。**这个手挑的倍率已经撤掉了(=1)**,平铺纯粹来自材质。
///
/// 原来是 3.0,理由是「材质给的 2.5/1.8 算出来星点偏大一倍」。真正的原因不是倍率,是
/// **导出器读错了参数**:汇编里星点的采样是 `mul rX.zw, v2.xxxy, cb6[130].w` —— 网格 UV0
/// 乘**一个标量**,而 `StarStickTiling` 在材质图里**同名存在标量与向量两份**,导出器只查了
/// 向量表,于是幽星光一族全掉进 `NoiseTilingSpeed` 兜底拿到 1.8/2.5。改成标量优先后三只是
/// **4 / 5.3 / 4**,而 5.3 × 1.0 ≈ 1.8 × 3.0 —— 这个倍率一直就是在替它。
///
/// **中途还改过一次 1.0 又改回 3.0 再撤掉**,那次的依据(「实机是少而大的四角星」)是错的:
/// 它来自**两张放大倍率不同的裁图**(420px 渲图裁 28% 去比 1440px 截图裁 60%)。
///
/// 教训:观感比对**必须先把两边的宠物在屏幕上的尺寸对齐**,否则裁图尺度会直接翻转结论。
/// 按 bbox 高度归一也不够 —— 实机那张的 bbox 含环绕的粉环,同比例裁框还是落偏。
/// 最后用的是**按躯干宽度归一**(宽度剖面的 97 分位),裁框以最宽那一行为中心,与放大倍率无关。
const STAR_TILE_SCALE: f32 = 1.0;
/// 星点层的整体标定系数;折的是汇编里的 `cb6[131].x`(名字未解),再与材质的
/// `Stick_Intensity`(根默认 1.5)相乘 ⇒ 净 0.3。
///
/// **试过一个无效的指标,记下来免得再用**:拿「身体区域去掉 8×8 块均值后的高频 std」
/// 比我的渲图与实机截图 —— 那个数**由锯齿主导**(我们没有抗锯齿、还有描边,
/// 实机截图是抗锯齿+缩放过的),星点层开关前后比值只从 2.77 变到 2.73,分辨不出东西。
///
/// 搬进线性后按 `旧² / EXPOSURE` = 0.2² / 0.4816 ≈ 0.083 换算(保持观感等值)。
const STICK_GAIN: f32 = 0.083;
/// 星点闪烁的相位速度。汇编里是 `frac(View 时间 × 0.25)` —— **0.25 是硬写在材质图里的
/// 字面量**(和它并列的 `frac(时间 × 0.0056)` 喂另一层),不是可覆盖的参数,所以照抄。
const STAR_PHASE_SPEED: f32 = 0.25;
/// 球内星点的整体强度。汇编里这项是 `cb5[62].z`(未解出名字);根材质有个语义对得上的
/// `StarIntensity` = 1,所以取 1。
const INTERIOR_GAIN: f32 = 1.0;
/// 星场的平铺标量(汇编里的 `cb5[61].y`)。根材质默认 `StarTiling` = 0.4 ——
/// 值越小采样范围越窄、星点看着越大。**名字现在是查实的**:根材质 `CachedExpressionData`
/// 的 `NameHashes` 可以用 `CityHash64WithSeed(名字大写, 0)` 反查(见 RootDefaults.cs),
/// 139 个标量默认值全部有名字,不再靠语义猜。
const STAR_FIELD_TILING: f32 = 0.4;
/// 三向投影权重的次数:根材质 `StarTriPlannarBlendInt` = 2。
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
const EXPOSURE: f32 = 0.4816;
/// 环境 / 间接光。实机由 mobile base pass 的天光那批 View 常量给,离线读不出来,所以标定
/// (见 `fs_main` 里两段明暗那段的推导)。**它是这条链路上仅剩的一个自由标量。**
const AMBIENT: f32 = 0.5765;

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
fn facing_ratio(n: vec3<f32>) -> f32 {
    let forward = normalize(vec3<f32>(camera.view_proj[0][2], camera.view_proj[1][2], camera.view_proj[2][2]));
    return 1.0 - abs(dot(n, forward));
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
/// **下面这段代码不是上面那个公式。** 上面是 ground truth,下面是能跑的近似,差别如下:
///
/// | | 汇编 | 这里 |
/// |---|---|---|
/// | 采样坐标 | 网格 UV0 × 标量 | **同**(这条是与名字无关的修正,已改对) |
/// | 遮罩 | `smoothstep(saturate((b·(k−r)−0.01)×25))` | `min(r,g,b)` ← **和汇编不是一回事** |
/// | 颜色 | 4 段渐变 `c(t)` | `star_color`(材质的 `Color02` 归一化后) |
/// | 叠加 | `lerp(底色, 强度·m·c, saturate(m+下限))` | 相加 |
///
/// **为什么不照抄:照抄过一版,整只糊成一片白。** 那个遮罩 `m` 在 k=1 时覆盖贴图的 **29.6%**
/// (k=0.4 时 10%)—— 这一层压根不是「细碎星点」,而是**一层脉动的大面积柔光**,靠 4 段渐变色
/// 着色才成立。缺了那 4 个色槽(`cb6[67]/[68]/[70]/[72]`)与 `cb6[131].x/.y` 两个强度标量的
/// 名字,公式就落不了地。**这一层卡在 cb 名字上,不是标定能救的。**
///
/// 那 4 个色**不要**去套 `StickRandomColor01..04`:数量正好对得上,但那 4 个是红/品红/蓝/黄的
/// 浓色,而实机星点是淡白粉、和宠物 MI 上 HDR 的 `Color02`(曜星光 (10, 8.07, 9.04))才对得上
/// —— 更像「黑 → Color02」那条亮度渐变。查过一次省掉一次错误改动,记在这儿。
///
/// 眼下这套 `min(r,g,b)` 是**碰巧**能看(这张图 g 通道最小、均值 0.028,取最小通道刚好筛出
/// 稀疏亮点),而且是对着实机截图标过的,所以留着 —— 但它**是猜的**,别当成读出来的。
fn star_light(uv0: vec2<f32>) -> vec3<f32> {
    if material.flags.z < 0.5 {
        return vec3<f32>(0.0);
    }
    let uv = uv0 * material.star.xy * STAR_TILE_SCALE;
    let star = textureSample(star_tex, base_sampler, uv);
    let glyph = min(star.r, min(star.g, star.b));
    return material.star_color.rgb * star.rgb * glyph * material.star_color.w;
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
    // **是混色不是相乘。** 色带图本身就是成品颜色(青↔粉竖条纹),而基色图里环带那条是纯粉;
    // 相乘等于「粉 × 青」→ 出来是蓝,实机是真青。`FlowPower`(暮星辰 0.8)就是混色权重。
    return mix(albedo, band, material.extra.y);
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
    // refract():WGSL 没有内建,照 Snell 写。eta 取 1/折射率(空气 → 介质)
    let eta = 1.0 / max(material.interior.x, 0.001);
    let cosi = dot(n, forward);
    let k = 1.0 - eta * eta * (1.0 - cosi * cosi);
    if k < 0.0 {
        return 0.0;   // 全内反射
    }
    let dir = eta * forward - (eta * cosi + sqrt(k)) * n;

    // **march 距离与平铺照汇编算,不再手挑。** 汇编(fx1/34529.asm 63..78):
    //   halfExtent = 0.5 * |包围盒尺寸|
    //   marchDist  = halfExtent * 0.01 * GlobalDepth        ← 代 100 进去正好 = halfExtent
    //   tiling     = <一个 cb 标量> / halfExtent            ← 那个标量取 1(中性),名字未解出
    //   p = (start + 折射方向 * marchDist) * tiling
    // 于是 p = start/halfExtent + 折射方向 —— 折射方向是单位向量,所以每颗球看到的是
    // 星场里以某点为心、约一格大小的一块,这正是「每颗球稳定居中一颗星」的机制。
    let half_extent = 0.5 * length(material.bounds_size.xyz);
    let march = half_extent * 0.01 * material.interior.y;
    // `tiling = <cb 标量> / halfExtent`。那个标量没解出名字,取根材质里语义对得上的
    // `StarTiling` = 0.4:它把采样范围缩到 0.4 倍,于是星场被放大 2.5 倍 ——
    // 取 1 时星点比实机小得多(用户实测「太小」)。
    let p = (start + dir * march) * (STAR_FIELD_TILING / max(half_extent, 0.0001));

    // 三向投影:权重取 |法线| 的高次,归一化。次数用根材质里那个**有名字**的
    // `StarTriPlannarBlendInt` = 2(汇编里对应 `pow(|v3.yzw|, cb5[63].y)` 再归一化)。
    // 原来写死 8 是猜的 —— 8 让权重过于偏向单一轴,三个面几乎不混。
    let w = pow(abs(n), vec3<f32>(STAR_TRIPLANAR_BLEND));
    let wn = w / max(w.x + w.y + w.z, 0.001);
    let s = textureSample(interior_tex, base_sampler, p.yz) * wn.x
        + textureSample(interior_tex, base_sampler, p.xz) * wn.y
        + textureSample(interior_tex, base_sampler, p.xy) * wn.z;

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
    if material.flags.w < 0.5 {
        return vec3<f32>(0.0);
    }
    return material.matcap_color.rgb * textureSample(matcap_tex, base_sampler, matcap_uv(n)).r;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_color, base_sampler, in.uv);
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
    let lit = smoothstep(-0.04, 0.04, ndl);
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
    albedo = albedo * albedo;
    // 加上去的光。星点只轻轻一层;**不透明层不叠 MatCap**——游戏那边靠遮罩通道选择性反射,
    // 无条件叠会把宠物冲白(试过,整只发白),而 toon 着色本身对着截图已经够像。
    //
    // 那层白色 `rim` 是我们自己加的(汇编里没有,桌宠场景下让轮廓从背景里浮出来)。
    // **玻璃族不加**:它有材质自己的边缘光(`RimColor`/`RimIntensity`/`RimPower`),
    // 两层叠起来轮廓会糊成一圈白 —— 暮星辰的裙子就是这么被冲成淡青的。
    let generic_rim = select(rim, 0.0, material.flags.y > 0.5);
    var glow = vec3<f32>(generic_rim) + star_light(in.uv) * STICK_GAIN;
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
        // **加上去的几层光是 `max` 合的,不是相加。** 汇编里连着两条:
        // `max r2.yzw, matcap*MatCapColor, spec*SpecColor` 再 `max r2.xyz, 上一步, rim`。
        // 相加会让高光与边缘光在轮廓处叠成一圈白边;取 max 则是「哪层亮听哪层」。
        glow += max(matcap_light(n) * GLASS_MATCAP_GAIN,
                    material.rim_color.rgb * pow(facing, material.extra.x) * material.star.z
                        * GLASS_RIM_GAIN);
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
                     saturate(interior_star(in.interior_pos,
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
    let body = albedo * shade * mix(1.0, material.params.y, line);
    // **末尾统一编码到显示空间**:`sqrt(色 × 曝光)`,照汇编尾部那条
    // `movc o0.xyz, (曝光 < 1), sqrt(色 × 曝光), 色`。
    //
    // **`glow` 也在线性里了**,所以几层光**先在线性里相加、再一起编码一次**——
    // 这才是对的:加性光就该在线性里加。原来是各自在显示空间加,等于把小值各编码一次
    // (`sqrt` 会放大小值:0.25 → 0.418),几层叠起来偏亮。
    // 四个系数按 `旧² / EXPOSURE` 换算过,保持观感等值(见各自的定义处)。
    //
    // 输出预乘 alpha(见 render.rs)。**固有色乘 alpha、加上去的光不乘**:高光/星点/边缘光
    // 是打在表面上的光,半透表面照样该有,乘进去会随着变透明一起消失。
    let lin = max(body * alpha + glow, vec3<f32>(0.0));
    return vec4<f32>(sqrt(lin * EXPOSURE), alpha);
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
    // 原来我们的描边环只有实机的 **0.59** 倍亮(即深得多);0.55 抬到 0.81,整只的
    // `p90−p10` 跨度比也从 1.21 降到 1.04。取 0.7 能到 0.92/1.02 更准,但那时描边几乎看不见了
    // —— 桌宠要在任意背景上认得出轮廓,所以留一档。
    //
    // **这条是从一次差点走偏的排查里捞出来的**,过程值得记:先量到「我们整体比实机更花」
    // (跨度比 1.29),差点去调 `AMBIENT`;做了两步分解才找对地方 ——
    // ① 抗锯齿对照(渲 4 倍再缩)只把比值从 1.54 拉到 1.47,**不是走样造成的**;
    // ② 换成对边缘不敏感的统计量,`p75−p25` 的比值只有 **1.05** —— 身体主体的对比其实是对的,
    //    超出的全在暗尾;腐蚀 2 像素后暗端比从 0.75 跳到 0.95,**暗尾就是这条描边**。
    // 所以调 `AMBIENT` 会是错的:它会为了掩盖描边而把整只的明暗压平。
    return vec4<f32>(tex.rgb * 0.55, 1.0);
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
    let rim = mix(0.35, 1.0, facing);

    let strength = mask.a * flow_amount * rim;
    let color = material.tint.rgb * glow * strength;
    if additive {
        // 加色:alpha=0,只往目标上加光
        return vec4<f32>(color, 0.0);
    }
    let alpha = clamp(strength * opacity * material.tint.a, 0.0, 1.0);
    // 预乘
    return vec4<f32>(material.tint.rgb * alpha, alpha);
}
