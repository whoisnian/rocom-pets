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
    // 有基色的:  [alpha 是否镂空遮罩, 线条提亮倍数, -, -]
    params: vec4<f32>,
    // 纯特效层:[遮罩是否 matcap, -, 有星点, 有 matcap]
    // 有基色的:  [-, 是否玻璃/纱(半透族), 有星点, 有 matcap]
    flags: vec4<f32>,
    // [星点 u 平铺, v 平铺, 边缘光强度, 不透明度]
    star: vec4<f32>,
    // 星点着色(rgb)+ 线条提亮(a)
    star_color: vec4<f32>,
    // MatCap 着色(rgb,可能是 HDR)
    matcap_color: vec4<f32>,
    rim_color: vec4<f32>,
    // 半透材质的整体着色
    main_color: vec4<f32>,
    // [边缘光衰减次数, 色带混入强度, -, 有没有色带]
    extra: vec4<f32>,
    // 玻璃内部那层:[折射率, march 深度, -, 有没有内部层]
    interior: vec4<f32>,
    // 内部星光的着色(rgb,HDR)
    interior_color: vec4<f32>,
    // 模型包围盒:最小角(xyz)与尺寸(w 存最长边),内部层要拿它把位置归一化
    bounds_min: vec4<f32>,
    bounds_size: vec4<f32>,
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
    return out;
}


/// 星点遮罩的额外平铺倍率(见 `star_light`)。
const STAR_TILE_SCALE: f32 = 3.0;
/// matcap 图里「算高光」的亮度门槛(见 `matcap_light`)。
const SPEC_FLOOR: f32 = 0.35;
/// 内部星层的卷动速度与叠加量。速度实机来自一个 cb 向量参数(槽位还没对上名字),
/// 亮度那边 `StarColor` 是 HDR 的 (0.33, 0.67, 2),直接乘会过曝。
const INTERIOR_SPEED: f32 = 0.03;
const INTERIOR_GAIN: f32 = 1.6;
/// 内部星场的平铺。**必须远大于 1**:位置是按整只宠物的包围盒归一化的,而那两颗球的直径
/// 只有包围盒最长边的 0.2 上下 —— 平铺 1 时一整颗球只摊到星场的一个格子上,
/// 出来就是「一颗被拉伸的星贴在表面」而不是「球里有颗星」。
const INTERIOR_TILING: f32 = 1.0;
/// 玻璃族高光的叠加量。游戏那边这项还乘着遮罩通道选出来的高光区,我们没有那张遮罩的语义,
/// 只能整片叠,所以要压一档——满强度叠上去,幽星光那两颗球会泛成一团白。
const GLASS_GAIN: f32 = 0.35;

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
    return material.star_color.rgb * star.rgb * glyph;
}

/// 卷动色带:一张渐变图沿 UV 滚过表面,乘在固有色上。暮星辰的环带靠它出青↔粉渐变
/// (`FlowTexture` = 青↔粉竖条纹 + `Flow_U_Speed` = 0.25;基色贴图里环带那一条是纯粉的)。
fn flow_band(uv: vec2<f32>, albedo: vec3<f32>) -> vec3<f32> {
    if material.extra.w < 0.5 {
        return albedo;
    }
    let scrolled = uv * material.flow.zw + vec2<f32>(material.flow.x, material.flow.y) * camera.time;
    let band = textureSample(noise_tex, base_sampler, scrolled).rgb;
    // 色带整体偏亮(均值 ~0.7),直接乘会压暗固有色,所以按亮度归一化后再按强度混入
    let normalized = band / max(max(band.r, max(band.g, band.b)), 0.001);
    return mix(albedo, albedo * normalized, material.extra.y);
}

/// **玻璃内部那颗星。** 实机是这么做的(读 `MI_P_Object_Trans_MatCap` 的 pixel shader 汇编,
/// 见 docs/design.md §1):把视线按 `GlobalRefraction`(=1.3)折射进物体内部,沿折射光线
/// march 一段(`GlobalDepth`),在**模型空间**按三向投影采 `StarTex`(= `T_EMeng003`,
/// 一张四角星场、alpha 是干净的稀疏星形遮罩),采样坐标再叠上时间卷动。
///
/// 于是球看着像「里面飘着一颗星」,而且那颗星**自己在动、与球的自转无关** —— 正是实机观感。
/// 这一层只给玻璃族(静态开关 `是否使用MatCap` 开着的那 17 个材质)。
///
/// **是近似不是复刻**:游戏那边还有第二张三向投影贴图、两段 `pow` 相位曲线、以及一个按
/// 高度做的两色渐变当固有色;这里只取「折射 + 三向投影星场 + 时间」这条主干。
/// 卷动速度实机是个 cb 里的向量参数,而 cb 槽位与参数名的对应还没解出来(§1),
/// 所以先用一个定值。
fn interior_star(start: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    if material.interior.w < 0.5 {
        return vec3<f32>(0.0);
    }
    let forward = normalize(vec3<f32>(camera.view_proj[0][2], camera.view_proj[1][2], camera.view_proj[2][2]));
    // refract():WGSL 没有内建,照 Snell 写。eta 取 1/折射率(空气 → 介质)
    let eta = 1.0 / max(material.interior.x, 0.001);
    let cosi = dot(n, forward);
    let k = 1.0 - eta * eta * (1.0 - cosi * cosi);
    if k < 0.0 {
        return vec3<f32>(0.0);   // 全内反射
    }
    let dir = eta * forward - (eta * cosi + sqrt(k)) * n;

    // 起点是顶点里带的 (UV1.xy, UV2.x),沿折射线走一段
    let p = (start + dir * material.interior.y) * INTERIOR_TILING;
    // 三向投影:权重取 |法线| 的高次,归一化
    let w = pow(abs(n), vec3<f32>(8.0));
    let wn = w / max(w.x + w.y + w.z, 0.001);
    let drift = camera.time * INTERIOR_SPEED;
    let a = textureSample(interior_tex, base_sampler, p.yz + drift).a * wn.x
        + textureSample(interior_tex, base_sampler, p.xz + drift).a * wn.y
        + textureSample(interior_tex, base_sampler, p.xy + drift).a * wn.z;
    return material.interior_color.rgb * a * INTERIOR_GAIN;
}

/// MatCap 高光。`MatCapColor` 可能是 HDR(暮星辰那两个球是 (3,3,3)),所以直接相乘。
///
/// **只取亮的那部分。** matcap 图是整颗球的反射查找表(实测 matcap26/Matcap35 均值 0.2、
/// 只有 8% 的像素亮过 0.5),暗区是球体自己的暗面——连暗区一起加等于给整片抬一层灰,
/// 再乘上 HDR 的 MatCapColor 就把球冲成一团白。这里减掉底再归一化,留下的就是那几块高光。
fn matcap_light(n: vec3<f32>) -> vec3<f32> {
    if material.flags.w < 0.5 {
        return vec3<f32>(0.0);
    }
    let m = textureSample(matcap_tex, base_sampler, matcap_uv(n));
    let spec = max(m.rgb - vec3<f32>(SPEC_FLOOR), vec3<f32>(0.0)) / (1.0 - SPEC_FLOOR);
    return material.matcap_color.rgb * spec;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_color, base_sampler, in.uv);
    // **alpha 有两种含义,由材质决定**(params.x):
    // - 镂空遮罩(眼/嘴的表情图集):按阈值剔,不剔就是一块方糊;
    // - 线条遮罩(本体):RGB 是完整固有色,alpha 里画着身上的纹路(水灵的竖条纹就在这儿)。
    //   这种**绝对不能拿来剔像素**——本体贴图的 alpha 覆盖率普遍很低(813 张里 60 张 <5%),
    //   剔了就只剩眼睛(火花)甚至整只消失(迪莫)。要做的是照着它提亮。
    let cutout = material.params.x > 0.5;
    if cutout && tex.a < 0.35 {
        discard;
    }
    let line = select(tex.a, 0.0, cutout);

    let n = normalize(in.normal);
    let ndl = dot(n, normalize(camera.light_dir));
    // 两段明暗:亮部原色,暗部压到 0.72,过渡带 0.08 宽度避免锯齿
    let lit = smoothstep(-0.04, 0.04, ndl);
    let shade = mix(0.72, 1.0, lit);
    let facing = facing_ratio(n);
    // 边缘光:让轮廓从桌面背景里浮出来,桌宠场景下比环境光更有用
    let rim = pow(facing, 3.0) * 0.25;

    // 固有色:卷动色带 → 整体着色 → 两段明暗 → 纹路提亮(alpha 高的地方比底色亮一档)
    let albedo = flow_band(in.uv, tex.rgb * material.main_color.rgb);
    var lambert = shade;
    // 加上去的光。星点只轻轻一层;**不透明层不叠 MatCap**——游戏那边靠遮罩通道选择性反射,
    // 无条件叠会把宠物冲白(试过,整只发白),而 toon 着色本身对着截图已经够像。
    var glow = vec3<f32>(rim) + star_light(in.ndc) * 0.3;
    var alpha = 1.0;

    // **玻璃 / 薄纱**(`MI_P_Object_Trans_*` 族:幽星光那两个球、暮星辰的裙子与球)。
    // 只有这一族叠 MatCap 高光与材质自己的边缘光。
    //
    // 材质里的边缘光是**加在边上的一层光**,不能拿去染固有色:球的颜色就是基色图集里
    // 那片平色圆盘。导出器只把「`Rim Intensity` 真的大于 1」的边缘光写进来(见 Manifest.cs)——
    // 曜星光那两颗球写着强度 1 + 绿色 `Rim LightColor`,而实机里它们是橙的和紫的。
    if material.flags.y > 0.5 {
        glow += (matcap_light(n)
            + material.rim_color.rgb * pow(facing, material.extra.x) * material.star.z)
            * GLASS_GAIN
            + interior_star(in.interior_pos, n);
        alpha = clamp(material.star.w, 0.0, 1.0);
        // **玻璃不吃两段明暗。** 它的明暗来自反射(MatCap + 边缘光),不是漫反射;
        // 而那两颗球是**开口薄壳**(129 顶点、边界 30 条边),自转时露出来的面一直在换,
        // 硬分成亮/暗两段就让整颗球在 0.72 与 1.0 之间来回跳 —— 那就是「转起来在闪」。
        lambert = 1.0;
    }
    let body = albedo * lambert * mix(1.0, material.params.y, line);
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
