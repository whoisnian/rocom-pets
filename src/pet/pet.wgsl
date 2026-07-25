// 宠物着色:顶点里做蒙皮,片元里做卡通(分段)光照 + 边缘光;描边走第二遍法线外扩。
//
// 目标是「像」游戏那套自研 toon,而不是复刻(设计 §3.3):基色贴图 + 2 段明暗 + 轻边缘光
// + 描边,已经能抓住观感;RampTex/MatCap/StarStick 那几十个参数不追。

struct Camera {
    view_proj: mat4x4<f32>,
    // 光照方向(指向光源)与描边参数打包进一个 vec4 省 binding
    light_dir: vec3<f32>,
    outline_width: f32,
};

@group(0) @binding(0) var<uniform> camera: Camera;
// 蒙皮矩阵:关节世界变换 × 逆绑定矩阵,每帧由 CPU 采样动画后上传
@group(0) @binding(1) var<storage, read> joints: array<mat4x4<f32>>;
@group(1) @binding(0) var base_color: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;

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
    // **不能拿 tex.a 当不透明度做 alpha 测试。** `_D` 贴图的 alpha 不是 opacity,
    // 是美术塞进去的遮罩通道(游戏那边自研 shader 另有用途)。实测 813 张 `_By_D`
    // 里 160 张 alpha 通过率 < 95%、60 张 < 5%:原来那句 `if tex.a < 0.35 { discard; }`
    // 把火花啃成只剩眼睛(通过率 4.8%)、迪莫整只消失(0.39%)。
    // 而喵喵的 By alpha 恰好全 255,开关都一样——它是当初唯一肉眼验过的宠物,
    // 所以这个 bug 一直没露头。**轮廓由几何决定,不由贴图 alpha 决定。**
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
    // 同 fs_main:不按贴图 alpha 剔像素,否则描边跟着身体一起被啃掉
    // 描边取基色的暗版而不是纯黑,卡通渲染里这样更自然
    return vec4<f32>(tex.rgb * 0.25, 1.0);
}
