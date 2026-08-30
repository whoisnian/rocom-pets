// MatCap 取样与色彩空间:显示↔线性、游戏色调映射的逆、场景深度淡化
//
// **这份文件不是独立的着色器** —— `src/pet/gpu.rs` 用 `concat!(include_str!(…))`
// 把 `src/pet/shader/*.wgsl` 按文件名顺序拼成一个模块再交给 wgpu。WGSL 的模块级条目
// 与顺序无关,所以拆分只是为了好读;**加新文件记得同步 gpu.rs 里那张 `concat!` 清单**。

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
    // **MatCap 是 sRGB 资源,采样值要自己解码。** 探针对这一族每张图打的都是
    // `MatCap → …/matcapNN sRGB=1`,游戏由硬件采样器解;我们统一按 `Rgba8Unorm` 上传,
    // 没有那一步。`shade_fake_fluid` 里早就解了,玻璃族这条一直没解 —— 于是**暗部**强了
    // 约 8 倍(matcap26 圆内 R 中位 0.122,解码后 0.014),而**亮斑几乎不动**
    // (0.9 → 0.787)。也就是说漏掉这一步的代价不是「高光太亮」,是「整颗球糊了一层白雾」:
    // 三对球的 G 通道因此系统性偏高(暮星辰 128 vs 实机 94、曜星光 182 vs 120),
    // 看着就是用户说的「球颜色偏亮」。解码后暮星辰左球 G 128 → 80、两颗合计
    // 逐通道 |误差| 159 → 136。`GLASS_MATCAP_GAIN` 不用改:它当年标的是**亮斑**的强度,
    // 而亮斑在解码前后只差 13%。
    return srgb_to_linear(vec3<f32>(textureSample(matcap_tex, base_sampler, matcap_uv(n)).r)).r;
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
