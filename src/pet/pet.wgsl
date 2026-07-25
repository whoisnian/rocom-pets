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
};

@group(0) @binding(0) var<uniform> camera: Camera;
// 蒙皮矩阵:关节世界变换 × 逆绑定矩阵,每帧由 CPU 采样动画后上传
@group(0) @binding(1) var<storage, read> joints: array<mat4x4<f32>>;
@group(1) @binding(0) var base_color: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;
// 特效层的噪声贴图;普通材质这里是 1×1 白图
@group(1) @binding(2) var noise_tex: texture_2d<f32>;
@group(1) @binding(3) var<uniform> material: MaterialParams;

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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_color, base_sampler, in.uv);
    // alpha 测试**只对叠加片有意义**(眼/嘴那种贴在脸上的小面片,贴图是带透明背景的
    // 表情图集,不剔就是一块方糊)。**本体贴图的 alpha 不是不透明度**,是美术塞的遮罩通道,
    // 拿来剔像素会把身体啃掉(火花通过率 4.8% → 只剩眼睛,迪莫 0.39% → 整只消失)。
    // 两者靠同一个测试共存的办法:载入时**把本体槽的 alpha 强制刷成 255**
    // (见 model.rs 的 `find_base_color`),于是这里一个分支就够。
    if tex.a < 0.35 {
        discard;
    }

    let n = normalize(in.normal);
    let ndl = dot(n, normalize(camera.light_dir));
    // 两段明暗:亮部原色,暗部压到 0.72,过渡带 0.08 宽度避免锯齿
    let lit = smoothstep(-0.04, 0.04, ndl);
    let shade = mix(0.72, 1.0, lit);
    // 边缘光:让轮廓从桌面背景里浮出来,桌宠场景下比环境光更有用
    let rim = pow(1.0 - max(dot(n, normalize(in.view_dir)), 0.0), 3.0) * 0.25;

    let color = tex.rgb * shade + vec3<f32>(rim);
    // 输出预乘 alpha:透明表面合成要求(见 render.rs)
    return vec4<f32>(color, 1.0);
}

@fragment
fn fs_outline(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_color, base_sampler, in.uv);
    if tex.a < 0.35 {
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

    // 遮罩决定形状;卷动只作用在噪声上(火焰的动感来自噪声流过固定遮罩)
    let mask = textureSample(base_color, base_sampler, in.uv);
    var flow_amount = 1.0;
    if has_noise {
        let uv = in.uv * material.flow.zw + vec2<f32>(material.flow.x, material.flow.y) * camera.time;
        flow_amount = textureSample(noise_tex, base_sampler, uv).r;
    }

    // 边缘处更亮/更实:水壳的菲涅尔感与火焰的边缘都靠这个
    let n = normalize(in.normal);
    let facing = 1.0 - abs(dot(n, normalize(in.view_dir)));
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
