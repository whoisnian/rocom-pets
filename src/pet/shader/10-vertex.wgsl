// 顶点:线性混合蒙皮,以及本体 / 描边外扩两个顶点入口
//
// **这份文件不是独立的着色器** —— `src/pet/gpu.rs` 用 `concat!(include_str!(…))`
// 把 `src/pet/shader/*.wgsl` 按文件名顺序拼成一个模块再交给 wgpu。WGSL 的模块级条目
// 与顺序无关,所以拆分只是为了好读;**加新文件记得同步 gpu.rs 里那张 `concat!` 清单**。

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
    out.uv1 = input.uv1;
    out.uv2 = input.uv2;
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
    // 宽度逐材质,来自那份 `_Ol` 描边材质(`0.01 × OutlineWidthPC × MaxWidthScale` 厘米);
    // 推导与「有一条看着像宽度、其实是死设定的参数」见 exporter/Materials.cs 的 `OutlineWidthOf`。
    let width = material.outline.x * camera.outline_scale;
    out.clip = camera.view_proj * (world + vec4<f32>(normal * width, 0.0));
    out.ndc = out.clip.xy / max(out.clip.w, 1e-6);
    out.uv = input.uv;
    out.normal = normal;
    out.local_pos = input.local_pos;
    out.local_normal = normalize(input.normal);
    out.local_view = vec3<f32>(0.0, 0.0, 1.0);
    out.color = input.color;
    out.world_pos = world.xyz;
    out.uv1 = input.uv1;
    out.uv2 = input.uv2;
    return out;
}

