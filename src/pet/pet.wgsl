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
    // [不透明度, 发光强度, 是否加色, 有没有噪声贴图]
    params: vec4<f32>,
    // [遮罩是否 matcap, 有基色, 有星点, 有 matcap]
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
};

@group(0) @binding(0) var<uniform> camera: Camera;
// 蒙皮矩阵:关节世界变换 × 逆绑定矩阵,每帧由 CPU 采样动画后上传
@group(0) @binding(1) var<storage, read> joints: array<mat4x4<f32>>;
@group(1) @binding(0) var base_color: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;
// 特效层的噪声贴图;普通材质这里是 1×1 白图
@group(1) @binding(2) var noise_tex: texture_2d<f32>;
@group(1) @binding(3) var<uniform> material: MaterialParams;
// 星点(身上的细碎星光)与 MatCap(球面反射查找表);没有就是 1×1 白图
@group(1) @binding(4) var star_tex: texture_2d<f32>;
@group(1) @binding(5) var matcap_tex: texture_2d<f32>;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) joint_ids: vec4<u32>,
    @location(4) weights: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) view_dir: vec3<f32>,
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
    // 正交投影下视线方向是常量,取 +Z(相机看向 -Z)
    out.view_dir = vec3<f32>(0.0, 0.0, 1.0);
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
    out.view_dir = vec3<f32>(0.0, 0.0, 1.0);
    return out;
}


/// 相机的右/上向量。正交投影没有透视错切,`view_proj` 的行向量归一化后就是它们,
/// 所以不必额外往 uniform 里塞。
fn camera_basis() -> mat2x3<f32> {
    let right = normalize(vec3<f32>(camera.view_proj[0][0], camera.view_proj[1][0], camera.view_proj[2][0]));
    let up = normalize(vec3<f32>(camera.view_proj[0][1], camera.view_proj[1][1], camera.view_proj[2][1]));
    return mat2x3<f32>(right, up);
}

/// MatCap 的采样坐标:视空间法线映射到 [0,1](球面查找表的标准做法)。
fn matcap_uv(n: vec3<f32>) -> vec2<f32> {
    let basis = camera_basis();
    return vec2<f32>(dot(n, basis[0]), -dot(n, basis[1])) * 0.5 + vec2<f32>(0.5, 0.5);
}

/// 身上的细碎星光。共享图 `Tex_PetGlassyStar_004` 一类,形状在 alpha 里,
/// `StarStickTiling` 控制密度(暮星辰 = 4×4)。按视空间贴,于是转身时星点像浮在表面。
fn star_light(n: vec3<f32>) -> vec3<f32> {
    if material.flags.z < 0.5 {
        return vec3<f32>(0.0);
    }
    let uv = matcap_uv(n) * material.star.xy;
    let star = textureSample(star_tex, base_sampler, uv);
    return material.star_color.rgb * star.rgb * star.a;
}

/// MatCap 高光。`MatCapColor` 可能是 HDR(暮星辰那两个球是 (3,3,3)),所以直接相乘。
fn matcap_light(n: vec3<f32>) -> vec3<f32> {
    if material.flags.w < 0.5 {
        return vec3<f32>(0.0);
    }
    let m = textureSample(matcap_tex, base_sampler, matcap_uv(n));
    return material.matcap_color.rgb * m.rgb * m.a;
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
    // 边缘光:让轮廓从桌面背景里浮出来,桌宠场景下比环境光更有用
    let rim = pow(1.0 - max(dot(n, normalize(in.view_dir)), 0.0), 3.0) * 0.25;

    // 纹路提亮:alpha 高的地方(线条)比底色亮一档。
    // 星点只轻轻加一层;**不透明层不叠 MatCap**——游戏那边靠遮罩通道选择性反射,
    // 无条件叠会把宠物冲白(试过,整只发白),而 toon 着色本身对着截图已经够像。
    let color = tex.rgb * shade * mix(1.0, material.params.y, line)
        + vec3<f32>(rim) + star_light(n) * 0.3;
    // 输出预乘 alpha:透明表面合成要求(见 render.rs)
    return vec4<f32>(color, 1.0);
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

// 特效层(火焰 / 水壳 / 光晕)。**不是复刻游戏 shader,是够用的近似**:
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
    // 相机基向量从 view_proj 里取:正交投影没有透视错切,行向量归一化后就是右/上。
    let n = normalize(in.normal);
    var mask_uv = in.uv;
    if material.flags.x > 0.5 {
        let right = normalize(vec3<f32>(camera.view_proj[0][0], camera.view_proj[1][0], camera.view_proj[2][0]));
        let up = normalize(vec3<f32>(camera.view_proj[0][1], camera.view_proj[1][1], camera.view_proj[2][1]));
        mask_uv = vec2<f32>(dot(n, right), -dot(n, up)) * 0.5 + vec2<f32>(0.5, 0.5);
    }
    let mask = textureSample(base_color, base_sampler, mask_uv);
    var flow_amount = 1.0;
    if has_noise {
        let uv = mask_uv * material.flow.zw + vec2<f32>(material.flow.x, material.flow.y) * camera.time;
        flow_amount = textureSample(noise_tex, base_sampler, uv).r;
    }

    // 边缘处更亮/更实:水壳的菲涅尔感与火焰的边缘都靠这个
    let facing = 1.0 - abs(dot(n, normalize(in.view_dir)));
    let rim = mix(0.35, 1.0, facing);

    // **有基色的半透材质**走另一条:暮星辰的裙子与那两个球都是 `BLEND_Translucent`,
    // 固有色来自贴图而不是 tint。当不透明画就是死板的实心块(球会变成纯色圆片)。
    if material.flags.y > 0.5 {
        let ndl = dot(n, normalize(camera.light_dir));
        let shade = mix(0.72, 1.0, smoothstep(-0.04, 0.04, ndl));
        let base = mask;   // 这里 base_color 绑的就是基色贴图
        // 边缘更实、中间更透:玻璃与薄纱都是这个观感
        let a = clamp(mix(material.star.w, 1.0, facing * facing), 0.0, 1.0);
        let rim_glow = material.rim_color.rgb * pow(facing, 3.0) * material.star.z;
        let lit = base.rgb * material.main_color.rgb * shade * mix(1.0, material.star_color.a, base.a)
            + star_light(n) * 0.45 + matcap_light(n) * 0.35 + rim_glow;
        return vec4<f32>(lit * a, a);
    }

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
