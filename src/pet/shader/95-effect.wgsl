// 其余片元入口:背板族 / 果冻内胆 / 纯特效层
//
// **这份文件不是独立的着色器** —— `src/pet/gpu.rs` 用 `concat!(include_str!(…))`
// 把 `src/pet/shader/*.wgsl` 按文件名顺序拼成一个模块再交给 wgpu。WGSL 的模块级条目
// 与顺序无关,所以拆分只是为了好读;**加新文件记得同步 gpu.rs 里那张 `concat!` 清单**。

/// **`M_P_BackRenderEmissive`:只画一侧的不透明背板(unlit)。**
///
/// 逐指令来源:莫比乌乌 `_Fx` 的 `quality=Num / lod=0 / dsid=0` 排列
/// (resource `C2685A88…`,PS 48913)。整条链只有 130 行,而且**没有任何光照** ——
/// 它不吃 `shade`、不吃 AMBIENT,输出 alpha 恒 1(`mov o0.w, l(1.0)`)。
///
/// 用户描述的「一侧透明,可以看到身体内形状和粉色液体;另一侧是白色基底,避免透出背景」
/// 里的**白色基底**就是它:外壳 `_By` 的基色 alpha 在中段 z∈[−0.3,+0.3] 上中位 0.000
/// (那是一块透明窗口),窗口后面就是这块背板。
///
/// 参数名与 `M_P_Object` 那条加性流动层高度重合(`FlowInt` / `FlowPower` /
/// `Flow_U_Speed` / `OpenRadialUV` / `RadialCenterOffset*`),但**合成方式相反**:
/// 那边是加进发光累加器,这边是 `lerp` **替换**固有色。
///
/// 链上被 0 乘掉的两层已经查过(这本子里同一个坑踩过三次):`FresnelIntensity` 与
/// `Glow Intensity` 的根默认都是 0,而全库 16 份覆盖过 `FresnelIntensity` 的材质
/// (小火苗 / 水蓝蓝 / 落大蟹)没有一份在这一族里 ⇒ 菲涅尔层与 Glow 层恒为 0,不实现。
/// `Flat_EmissiveRatio` = 0、`SelectionColor.a` = 0,那两条 lerp 也是恒等。
@fragment
fn fs_back_render(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let level = material.family0;
    let saturation = material.family1;
    let flow_color = material.family2;
    let flow_uv = material.family3;
    let radial = material.family4;
    let main = material.family5;

    // **剔面。** 材质的 `BasePropertyOverrides` 写着 `TwoSided = True`,光栅器两面都出;
    // PS 自己按 `SV_IsFrontFace` 丢掉一面(汇编 `if_nz` 那段,`BackFaceOnly` 根默认 1):
    //     a   = saturate(场景淡出 × (正面 ? +1 : −1))     ← 背面恒 0,正面取那个淡出量(常 1)
    //     cov = select(a, 1 − a, UseBackFace)
    //     按屏幕 4×4 抖动阈值 discard
    // 抖动那一步是 UE 的 LOD 淡入淡出,离线没有那两个场景标量(恒 1)⇒ 化简成一个硬判据。
    // 全库 3393 份材质里只有**莫比乌乌**把 `UseBackFace` 设成 1(只画背面),其余 11 份画正面。
    let want_front = level.w < 0.5;
    if front != want_front {
        discard;
    }

    // ① 电平重映射 + 逐通道去饱和。基色贴图是 sRGB 资源(`main.w`),而运行时统一按
    //    `Rgba8Unorm` 上传、没有硬件解码那一步,所以自己解。
    let sampled = textureSample(base_color, base_sampler, in.uv).rgb;
    let tex = select(sampled, srgb_to_linear(sampled), main.w >= 0.5);
    var base = mix(vec3<f32>(level.x), vec3<f32>(level.y), tex);
    // `饱和度变化` 是**逐通道**的去饱和量(根默认正好是亮度权重 (0.3, 0.59, 0.11));
    // 实例可以给负值(莫比乌乌 (−0.12, 0, −0.292))—— 那是**加**饱和。
    let lum = dot(base, vec3<f32>(0.3, 0.59, 0.11));
    base += saturation.rgb * (vec3<f32>(lum) - base);

    // ② 流动层的 UV。`UVNumber` 选的是 **TEXCOORD2**(VS 的 `o5 = ATTRIBUTE7`,签名查实),
    //    不是 UV1。**「源网格最多两套 UV」那条旧结论是错的**(见 `model::Vertex::uv2`):
    //    全库 123 个骨骼网格有第 3 套,柴渣虫的 UV2 有 99.4% 非零。两套的网格 UV2 全零,
    //    与游戏顶点工厂的行为一致,所以这里直接用它、不必特判。
    var uv = mix(in.uv, in.uv2, saturate(level.z));
    if radial.z > 0.0 {
        let d = uv - radial.xy;
        uv = mix(uv, vec2<f32>(fract(atan2(d.y, d.x) * 0.15915494), length(d)), radial.z);
    }
    let scrolled = uv * flow_uv.zw + fract(camera.time * flow_uv.xy);
    let flow_raw = textureSample(noise_tex, base_sampler, scrolled).rgb;
    let flow_lin = select(flow_raw, srgb_to_linear(flow_raw), radial.w >= 0.5);
    // 汇编是 `movc(raw <= 0, 0, exp(log(raw) × FlowPower))` —— ≤0 的通道直接取 0。
    let shaped = select(pow(max(flow_lin, vec3<f32>(1.0e-6)), vec3<f32>(saturation.w)),
                        vec3<f32>(0.0), flow_lin <= vec3<f32>(0.0));
    // ③ **替换**,不是相加(汇编 `mad r1, flow, (UVFlowColor×FlowInt − base), base`)。
    //    权重是顶点色 B —— 和 `M_P_Object` 那条加性流动层用的是同一个通道。
    let weight = shaped * in.color.b;
    var color = mix(base, flow_color.rgb * flow_color.w, weight);
    color *= main.rgb;
    return vec4<f32>(encode_linear_color(color), 1.0);
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
