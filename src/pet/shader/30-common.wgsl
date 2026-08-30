// 公共小工具:相机基、facing、半透族的高光 / 边缘光覆盖率、MatCap UV
//
// **这份文件不是独立的着色器** —— `src/pet/gpu.rs` 用 `concat!(include_str!(…))`
// 把 `src/pet/shader/*.wgsl` 按文件名顺序拼成一个模块再交给 wgpu。WGSL 的模块级条目
// 与顺序无关,所以拆分只是为了好读;**加新文件记得同步 gpu.rs 里那张 `concat!` 清单**。

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


/// `M_P_Object_Trans` 那一族的边缘光遮罩 —— **逐指令来自 PS 53987 第 195~223 行**
/// (莫比乌乌 `_By` / 幽星光 `_Fx1` 共用的 `quality=Num / lod=0 / dsid=0` 排列)。
///
/// ```text
/// ndv   = saturate(视线 · 法线)
/// 带宽  = 0.4 × (1 − |视线.z|)                 ← UE Z-up;我们是 Y-up,取 .y
/// gate  = 1 − smoothstep(saturate((ndv − 0.05) / 带宽))
/// base  = (1 − ndv) × gate
/// p     = pow(base, Rim Power) − 0.5           ← base ≤ 0 时汇编直接顶成 −0.5
/// cov   = smoothstep(saturate(p / Rim Soft Edge))
/// ```
///
/// **我们原来只写了 `saturate(pow(1 − |N·V|, Rim Power))`**,漏掉了 `gate`、
/// `− 0.5) / Rim Soft Edge` 那个重映射、以及最后那次 smoothstep。代价很具体:
/// 幽星光那两颗球的 `Rim Power = 0.35`,`pow(facing, 0.35)` 是一条**很平**的曲线
/// (facing = 0.1 就到 0.46),于是覆盖率在**整颗球**上都有 0.5~1 ——
/// 而这一族的 alpha 正是 `max(基色a, 高光, MatCap, 这个覆盖率)`,球因此变成不透明,
/// 把它背后那层**红色描边壳**整个挡住了。实机看到的红球就是那层壳。
/// 补上重映射之后 facing = 0.1 处直接归 0,只有轮廓一圈还留着。
fn trans_rim_coverage(n: vec3<f32>, power: f32, soft_edge: f32) -> f32 {
    let view = view_direction();
    let ndv = saturate(dot(view, n));
    let width = max(0.4 * (1.0 - abs(view.y)), 1.0e-4);
    let g = saturate((ndv - 0.05) / width);
    let gate = 1.0 - g * g * (3.0 - 2.0 * g);
    let base = (1.0 - ndv) * gate;
    let shaped = select(-0.5,
                        pow(max(base, 1.0e-6), max(power, 1.0e-4)) - 0.5,
                        base > 0.0);
    let c = saturate(shaped / max(soft_edge, 1.0e-4));
    return c * c * (3.0 - 2.0 * c);
}

/// MatCap 的采样坐标:视空间法线映射到 [0,1](球面查找表的标准做法)。
fn matcap_uv(n: vec3<f32>) -> vec2<f32> {
    let basis = camera_basis();
    return vec2<f32>(dot(n, basis[0]), -dot(n, basis[1])) * 0.5 + vec2<f32>(0.5, 0.5);
}
