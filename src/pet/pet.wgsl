// 宠物着色:顶点里做蒙皮,片元里做卡通(分段)光照 + 边缘光;描边走第二遍法线外扩。
//
// 目标是「像」游戏那套自研 toon,而不是复刻(设计 §3.3):基色贴图 + 2 段明暗 + 轻边缘光
// + 描边,已经能抓住观感;RampTex/MatCap/StarStick 那几十个参数不追。

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
    // 裁剪空间的 xy(= NDC,正交投影下 w 恒为 1);星点层拿它当「屏幕上的位置」
    @location(2) ndc: vec2<f32>,
    // 玻璃内部层的采样起点(直接透传顶点属性)
    @location(3) interior_pos: vec3<f32>,
    // **物体空间**的法线与视线:玻璃内部层的折射必须在这个空间里算(见 `interior_star`)
    @location(4) local_normal: vec3<f32>,
    @location(5) local_view: vec3<f32>,
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
    out.ndc = out.clip.xy;
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
    out.ndc = out.clip.xy;
    out.interior_pos = input.interior_pos;
    out.local_normal = normalize(input.normal);
    out.local_view = vec3<f32>(0.0, 0.0, 1.0);
    return out;
}


/// 星点遮罩的平铺倍率。**现在是 1.0 —— 也就是这个手挑的倍率没了**,平铺纯粹来自材质的
/// `StarStickTiling`(幽星光 2.5、暮星辰 4)。
///
/// 原来写 3.0,理由是「星点比实机大一倍以上」。那个判断是错的:放大对照实机截图看,
/// 实机是**少而大**的四角星(整只身上三四颗),我原来是**多而小**(十几二十颗)——
/// 方向正好反了,3.0 让它更密更小。
const STAR_TILE_SCALE: f32 = 1.0;
/// 星点层的整体标定系数;与材质的 `Stick_Intensity`(根默认 1.5)相乘 ⇒ 净 0.9。
///
/// **`Stick_Intensity` 直接代进来会过强**,但原因不是强度 —— 是密度。放大对照实机才看清:
/// 实机少而大、我原来多而小,所以先把平铺改对(见 `STAR_TILE_SCALE`),强度才有意义。
/// `Stick_Intensity` 现在留作**材质间的相对权重**(全库 82 个材质都是 1.5,暂时不改变什么)。
///
/// **试过一个无效的指标,记下来免得再用**:拿「身体区域去掉 8×8 块均值后的高频 std」
/// 比我的渲图与实机截图 —— 那个数**由锯齿主导**(我们没有抗锯齿、还有描边,
/// 实机截图是抗锯齿+缩放过的),星点层开关前后比值只从 2.77 变到 2.73,分辨不出东西。
const STICK_GAIN: f32 = 0.6;
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
/// 玻璃族 MatCap 高光的叠加量。游戏那边这项还乘着遮罩通道选出来的高光区,我们没有那张遮罩的
/// 语义,只能整片叠,所以要压一档——满强度叠上去,幽星光那两颗球会泛成一团白。
const GLASS_MATCAP_GAIN: f32 = 0.35;
/// 玻璃族边缘光的叠加量。汇编里这一项除了 `RimIntensity` 还乘着一个 cb 标量(`cb5[56].w`,
/// 槽位没对上名字),所以系数只能标定。**两条独立测量给出同一个数**:
/// ① 实机暮星辰裙子中位 (71,91,232) 减去基色贴图在那块 UV 的 (66,64,197),残差正好是
///    0.144 × `Rim LightColor`(53,187,214),三通道同时吻合;
/// ② 把整只渲图合成到实机背景色上、按「有/无边缘光」两版对裙子区解线性方程,得 0.35×0.46。
const GLASS_RIM_GAIN: f32 = 0.16;

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

/// 星点遮罩。**一只宠物只有一份,盖在整只身上**(导出器统一好了,见 Program.cs):
/// 游戏里它像挂在镜头前的一层遮罩投到宠物身上,不随模型转动。那两颗球身上的星星也是它——
/// 球的基色在图集里是一片平色圆盘,星形完全来自这层(所以幽星光一颗球是星、另一颗是圆点)。
///
/// 采样坐标取 **NDC**:平铺数就是「横跨模型几格」,密度不随宠物大小变。取景用的正交视体
/// 是正方的(见 `orthographic_view`),格子天然不会被拉扁;用视空间世界坐标则是
/// 「每世界单位几格」,大宠物身上会密到糊成一片。
///
/// **形状不在 alpha 里,在 min(r,g,b) 里。** 两种星点图的底都是**饱和**的
/// (共享图 `Tex_PetGlassyStar_004` 是红橙黄色块 + 每块中间一颗浅蓝白小星、**alpha 恒为 255**;
/// 「假半透」族那张是纯黑底 + 粉白星点),至少一个通道贴近 0;而星芒是浅色/白的、三通道都高。
/// 取最小通道于是同时吃下两族、还不碰底。按 `rgb * a` 算过一版,等于把整张橙图糊到表面——
/// 暮星辰的裙子从饱和蓝被冲成彩虹糖就是那么来的。
fn star_light(ndc: vec2<f32>) -> vec3<f32> {
    if material.flags.z < 0.5 {
        return vec3<f32>(0.0);
    }
    // `ndc * 0.5` = 横跨画布一格,再乘材质给的平铺数。**还要再乘一个倍率**:
    // 光按 `StarStickTiling`(2.5~4)算出来是「整只宠物上 2~4 格」,星点比实机大一倍以上,
    // 看着像「一张图拉伸后投上去」;实机更像原图小尺寸密铺。倍率对着截图挑的。
    let uv = vec2<f32>(ndc.x, -ndc.y) * 0.5 * material.star.xy * STAR_TILE_SCALE;
    let star = textureSample(star_tex, base_sampler, uv);
    let glyph = min(star.r, min(star.g, star.b));
    // 强度用根材质里**有名字**的 `Stick_Intensity`(默认 1.5),不再是外面那个手挑的 0.3。
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
    //    `smoothstep(2a-1, 2b-1, x)`,所以这里照旧对 `ndl` 取阈值;
    // ② 实机的两端是**颜色对**而不是灰度系数(暗部会偏色)。那两个槽位(cb5[24]/[25])
    //    是四对同构槽 (24,25)/(28,29)/(32,33)/(36,37) 之一,**参数名还没解出来**
    //    (见 design.md「cb 槽位 ↔ 参数名」),所以暂时保留灰度对,不猜颜色。
    let lit = smoothstep(-0.04, 0.04, ndl);
    let shade = mix(0.72, 1.0, lit);
    let facing = facing_ratio(n);
    // 边缘光:让轮廓从桌面背景里浮出来,桌宠场景下比环境光更有用
    let rim = pow(facing, 3.0) * 0.25;

    // 固有色:卷动色带 → 两段明暗 → 纹路提亮(alpha 高的地方比底色亮一档)。
    //
    // **不再乘 `MainColor`。** 原来对半透族乘了一层 `MainColor`(暮星辰裙子 (0.39,0.4,0.63)),
    // 理由是「不乘裙子会偏白」—— 那也是在错法线上看到的。对着实机截图量:裙子实测
    // (71,91,232),而基色贴图在那块 UV 是 (66,64,197),**几乎就是基色原样**;乘上去只有
    // (26,26,124),暗了三倍。另外静态开关 `GlassySwitch` 全库一个没开,而 `MainColor`
    // 属于那条 glassy 通路 —— 两边都指向「这一乘是多余的」。
    // 纯特效层的主色仍走 `tint`(那些材质压根没有基色贴图),不受影响。
    var albedo = flow_band(in.uv, tex.rgb);
    // 加上去的光。星点只轻轻一层;**不透明层不叠 MatCap**——游戏那边靠遮罩通道选择性反射,
    // 无条件叠会把宠物冲白(试过,整只发白),而 toon 着色本身对着截图已经够像。
    //
    // 那层白色 `rim` 是我们自己加的(汇编里没有,桌宠场景下让轮廓从背景里浮出来)。
    // **玻璃族不加**:它有材质自己的边缘光(`RimColor`/`RimIntensity`/`RimPower`),
    // 两层叠起来轮廓会糊成一圈白 —— 暮星辰的裙子就是这么被冲成淡青的。
    let generic_rim = select(rim, 0.0, material.flags.y > 0.5);
    var glow = vec3<f32>(generic_rim) + star_light(in.ndc) * STICK_GAIN;
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
        // **HDR 的材质色要先转到显示空间再用。** 那个 shader 的尾巴是
        // `movc o0.xyz, (曝光 < 1), sqrt(色 × 曝光), 色` —— 输出前一次 gamma-0.5 编码;
        // 而我们整条链路本来就跑在显示空间(没做第 ④ 步的反色调映射),所以拿线性 HDR 值
        // 直接当显示值是错的。`StarColor` = (0.33, 0.67, **2.0**),sqrt 后 (0.58, 0.82, 1.0),
        // 亮度从 153 抬到 199 —— 星这才亮得起来(实机那颗是 near-white)。
        let star_color = sqrt(material.interior_color.rgb);
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
    // 输出预乘 alpha(见 render.rs)。**固有色乘 alpha、加上去的光不乘**:高光/星点/边缘光
    // 是打在表面上的光,半透表面照样该有,乘进去会随着变透明一起消失。
    return vec4<f32>(body * alpha + glow, alpha);
}

@fragment
fn fs_outline(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_color, base_sampler, in.uv);
    // 只有镂空遮罩才剔;本体的线条遮罩要是拿来剔,描边壳会跟着被啃掉
    if material.params.x > 0.5 && tex.a < 0.35 {
        discard;
    }
    // 描边取基色的暗版而不是纯黑,卡通渲染里这样更自然
    return vec4<f32>(tex.rgb * 0.25, 1.0);
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
