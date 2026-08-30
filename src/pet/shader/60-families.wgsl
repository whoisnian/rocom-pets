// 五个原生材质族各自的着色:小灵面 / 玉兔耳 / 假液体 / MatCap 遮罩 / 沙漏玻璃
//
// **这份文件不是独立的着色器** —— `src/pet/gpu.rs` 用 `concat!(include_str!(…))`
// 把 `src/pet/shader/*.wgsl` 按文件名顺序拼成一个模块再交给 wgpu。WGSL 的模块级条目
// 与顺序无关,所以拆分只是为了好读;**加新文件记得同步 gpu.rs 里那张 `concat!` 清单**。

/// `MI_P_Object_XiaoYou` 的目标 ES3.1/Low PS 32511 主干。
///
/// 这条材质过去被误判为“没有 BaseTex 的纯特效”，导致整个 By1 身体进入半透明近似通道。
/// 原 PS 实际声明 MainTex(t2)、NoiseTex(t3)、StarTex(t4)，并且 `o0.w = 1`。其核心合成是：
///
/// - NoiseTex 在 FlowNoiseColor1/2 间混色；
/// - StarTex.r 提供闪烁相位、g 提供星形强度；
/// - `COLOR_0.r * COLOR_0.g * (1-COLOR_0.a)` 是逐顶点覆盖遮罩；
/// - `lerp(MF_ToneMapInverse(MainTex), lerp(BaseColor1,BaseColor2,flow.g), flow.r)`
///   与 flow/星光按上述遮罩合成。
///
/// 这里的 0.1、0.02 与 2π 都是 32511 第 70–75 行的字面量；其余数值来自材质参数，
/// 没有按宠物名称调色或手写星点。
fn shade_xiaoyou(in: VsOut) -> vec4<f32> {
    let main_sample = textureSample(base_color, base_sampler, in.uv);
    // 原资产元数据：MainTex sRGB=0、NoiseTex sRGB=1、StarTex sRGB=0。
    // PNG 不携带 UE 的采样色彩空间，故在专用链里显式还原硬件视图转换。
    let main_linear = game_tonemap_inverse(main_sample.rgb);

    // **卷动层要采两次,不是一次。** 实机排列(quality=Num / LOD0 / DSId=0,resource
    // `069E8956…`,PS 41540 第 70~90 行)拿同一张 `NoiseTex` 按**两组不同的速度**各采一次,
    // 取**不同的通道**,再**相乘**当 lerp 系数:
    //
    //     wob = sin(frac(time × 0.1) × 2π) × 0.02          // 只加在 V 上,两次共用
    //     a   = NoiseTex(uv + frac(time × 速度1) + wob).g   // 第 82 行 t3.yxzw → dest .x → g
    //     b   = NoiseTex(uv + frac(time × 速度2) + wob).b   // 第 87 行 t3.xywz → dest .w → b
    //     r8  = lerp(FlowNoiseColor1 × Int1, FlowNoiseColor2 × Int2, a × b)
    //
    // 原来只采一次(速度1 的 `.g`),`USpeedTex02`/`VSpeedTex02` 那一组导出器早就写进包了、
    // 运行时压根没读。相乘的效果是**把两层慢速卷动打散成更细的斑**,而单层是一整片。
    let wob = vec2<f32>(0.0, sin(fract(camera.time * 0.1) * 6.28318548) * 0.02);
    let flow_uv1 = in.uv + fract(camera.time * material.xiaoyou_noise_flow.xy) + wob;
    let flow_uv2 = in.uv + fract(camera.time * material.xiaoyou_noise_flow.zw) + wob;
    let noise_a = srgb_to_linear(textureSample(noise_tex, base_sampler, flow_uv1).rgb).g;
    let noise_b = srgb_to_linear(textureSample(noise_tex, base_sampler, flow_uv2).rgb).b;
    let flow1 = material.xiaoyou_flow1.rgb * material.xiaoyou_shape.x;
    let flow2 = material.xiaoyou_flow2.rgb * material.xiaoyou_shape.y;
    let flow_color = mix(flow1, flow2, noise_a * noise_b);

    // **星点是两层,不是一层**(PS 41540 第 91~117 行):`StarTex` 的 **R/G** 给一层、
    // **B/A** 给另一层 —— 正是参数名里那两族 `Star_RG_*` / `Star_BA_*`:
    //
    // ```text
    // uvN  = uv × UV_Control.xz + frac(time × UV_Control.yw / 100)   // 速度 preshader 除过 100
    // 相位  = frac(time × TwinkleSpeed / 100) + StarTex.相位通道
    // A    = saturate((sin(2π·相位) × 0.5 + 0.5) − DarkTime) × Int    // RG 那层
    // B    = StarTex.a × saturate(|sin(2π·相位)| − DarkTime) × Int    // BA 那层,取**绝对值**
    // 星点  = A × StarTex.g + B                                       // 第 117 行
    // ```
    //
    // 槽位都是从 preshader 字节码定的:`scalar-slot[12]/[13]` = `UV_Control` 的 x/z(平铺)、
    // `[15]/[17]` = 它的 y/w **除以 100**(速度)、`[19]`/`[18]` = RG 的 Int/DarkTime、
    // `[29]`/`[28]` = BA 的 Int/DarkTime、`[11]`/`[27]` = 两个 `TwinkleSpeed` 除以 100。
    //
    // 两层的差别不只是参数:**RG 那层是 `sin×0.5+0.5`(单向脉冲),BA 那层是 `|sin|`
    // (一个周期闪两次)**,而且 RG 那层的结果还要再乘一次 `StarTex.g`。
    // 原来只实现了 RG 那一层,`Star_BA_*` 那几个参数导出器压根没导。
    //
    // `DarkTime`(参数名后面跟着「数值越大, 黑的时间越长」)全库没人覆盖,取根默认 0;
    // 旧包没有 `xiaoyou_star2` ⇒ BA 那层的 Int 是 0 ⇒ 整层不出场,退回原来的单层。
    let star_t = camera.time * 0.01;
    let star_uv_a = in.uv * material.xiaoyou_star_uv.xz
        + fract(star_t * material.xiaoyou_star_uv.yw);
    let star_uv_b = in.uv * material.xiaoyou_star_uv2.xz
        + fract(star_t * material.xiaoyou_star_uv2.yw);
    let star_a = textureSample(star_tex, base_sampler, star_uv_a);
    let star_b = textureSample(star_tex, base_sampler, star_uv_b);
    let wave_a = sin(fract(star_t * material.xiaoyou_shape.w + star_a.r) * 6.28318548) * 0.5 + 0.5;
    let layer_a = saturate(wave_a - material.xiaoyou_star2.x) * material.xiaoyou_shape.z;
    let wave_b = sin(fract(star_t * material.xiaoyou_star2.w + star_b.b) * 6.28318548);
    let layer_b = star_b.a
        * saturate(abs(wave_b) - material.xiaoyou_star2.y)
        * material.xiaoyou_star2.z;
    let star_amount = layer_a * star_a.g + layer_b;

    // 覆盖遮罩(PS 41540 第 119~124 行):`saturate((a + b + 星光) × 顶点色 R·G·(1−A))`。
    //
    // **原来这儿多了一个 `1.0 +`**(从 Low 排列 32511 读的)。那一项让遮罩几乎恒等于
    // `R·G·(1−A)` 的饱和值 ⇒ 卷动层整片盖住本体,实机报的「小灵面身体像一层薄雾」
    // 就是它。Num 那条没有这个 1,遮罩跟着噪声走 —— 噪声暗的地方露出受光的固有色。
    let vertex_mask = saturate(
        (noise_a + noise_b + star_amount) * in.color.r * in.color.g * (1.0 - in.color.a)
    );
    // **固有色不读 MainTex,读的是材质自己的 BaseColor1/BaseColor2** —— 这条**在 Num 排列里
    // 一模一样**(PS 41540 第 167~170 行,`cb6[31]` = `BaseColor1`、`cb6[30]` = `BaseColor2`,
    // 槽位由 preshader 字节码 `040B00…` / `040C00…` 定死),下面这段 32511 的记录照旧成立:
    //     r4 = lerp(cb6[28], cb6[27], 1 − 顶点色 G)
    //     r3 = lerp(MF_ToneMapInverse(BaseTex), r4, 1 − 顶点色 A)
    // 这一族三只的顶点色 A 恒为 0 ⇒ 基色贴图整支权重为 0,固有色**全部**来自那对颜色;
    // 顶点色 G 在身体上是 1(取 BaseColor1 的暗紫)、在手臂上降到 0.23~0.71
    // (混向 BaseColor2 的青)——实机手臂那道紫→青的渐变就是它。
    //
    // 原来这里写的是 `mix(MainTex, mix(base1, base2, flow.g), flow.r)`:`flow.r` 只到 0.19,
    // 于是手臂几乎整块显示 `Tex_PetGlassy_007_D` 的原样。那是一张**全库共享的红/绿平铺图案图**
    // (findings.md §1.1 早已查实「不是本体固有色」),照搬出来就是实机没有的橙绿斑。
    let custom_base = mix(material.xiaoyou_base1.rgb,
                          material.xiaoyou_base2.rgb,
                          saturate(1.0 - in.color.g));
    let base = mix(main_linear, custom_base, saturate(1.0 - in.color.a));
    let effect = flow_color + material.xiaoyou_star_color.rgb * star_amount;
    // r5(flow + star) 在原 PS 中加到 emissive 分支，绕过 mobile 直接/间接光；
    // 只有 `(1-mask) * base` 进入受光照的 base-color 分支。
    let ndl = dot(normalize(in.normal), normalize(camera.light_dir));
    let lit = smoothstep(SHADE_TERM_LO, SHADE_TERM_HI, ndl);
    let shade = mix(0.5, 1.5, lit) + AMBIENT;
    let surface = base * (1.0 - vertex_mask) * shade + effect * vertex_mask;
    return vec4<f32>(encode_linear_color(max(surface, vec3<f32>(0.0))), 1.0);
}

/// `M_Gra_Yutu_Ear_Lighting` 的目标 Low PS 6037 材质主干。原 PS 的 t2/t3/t4
/// 分别是 Bubble Texture、DistortTex、FlowTex；局部坐标先乘 -0.01，是因为 UE 顶点
/// 插值量以厘米传入，而 glTF 顶点已经换成米，所以这里直接取负的 local_pos。
fn shade_yutu_ear(in: VsOut) -> vec4<f32> {
    let n = normalize(in.normal);
    let ndv = max(dot(n, view_direction()), 0.0);

    // 6037 用两组正交局部坐标、两档速度采同一张泡泡图，再取均值。
    let bubble_scale = material.family7.z;
    let bubble_uv1 = -vec2<f32>(in.local_pos.x, in.local_pos.y) * bubble_scale
        + vec2<f32>(0.0, camera.time * material.family7.x);
    let bubble_uv2 = -vec2<f32>(in.local_pos.z, in.local_pos.y) * bubble_scale
        + vec2<f32>(0.0, camera.time * material.family7.y);
    // `sample r0.w, ..., t2.xzwy`：目标 w 对应资源 swizzle 的 w 项，即源绿色通道。
    let bubble1 = srgb_to_linear(textureSample(base_color, base_sampler, bubble_uv1).rgb).g;
    let bubble2 = srgb_to_linear(textureSample(base_color, base_sampler, bubble_uv2).rgb).g;
    let bubble = (bubble1 + bubble2) * 0.5;

    // `sample t3` 的 xy 扰动随后以 0.5×FlowDistort 加到 t4 的 panner 坐标。
    let planar = -vec2<f32>(in.local_pos.x, in.local_pos.y);
    let distort = textureSample(noise_tex, base_sampler, planar).rg;
    let flow_uv = planar * material.family8.zw
        + camera.time * material.family8.xy
        + distort * (0.5 * material.family7.w);
    let flow_sample = textureSample(star_tex, base_sampler, flow_uv).rgb;
    let flow = flow_sample * material.family1.rgb * material.family9.x;

    let fresnel = pow(max(1.0 - ndv, 1e-4), max(material.family9.y, 1e-4))
        * material.family9.z;

    // **液面是顶点色 R 的阈值门,不是法线朝向。** 目标 Low PS 70474 第 147~152 行:
    //     ge r2.w, v2.x, <随时间摆动的液面高度>     ← v2 = COLOR0(ISGN 查实)
    //     ge r3.w, v2.x, l(0.5)
    //     mul r2.w, r2.w, r3.w                      ← 两道门都过才算「液面之上」
    //     mad r7.xyz, …, r2.w, …                    ← 在液面色与内胆色之间选
    // 春兔耳膜里那泡液体的顶点色实测 R = 0(238 个顶点)/ 0.78(10 个),G 才是 1 ——
    // 也就是**几乎整泡都在液面以下**,取 InColor 那支。
    let top = select(0.0, 1.0, in.color.r >= 0.5);
    let ramped_inner = mix(material.family3.rgb * material.family5.rgb,
                           material.family6.rgb,
                           top);
    let bubbles = material.family0.rgb * bubble;
    let surface = (ramped_inner + bubbles + flow) * material.family4.rgb
        + material.family2.rgb * fresnel;

    // 目标材质最终仍进入 mobile 光照；颜色参数中的 Flow/Fresnel 是材质内的加光项。
    let ndl = dot(n, normalize(camera.light_dir));
    let lit = smoothstep(SHADE_TERM_LO, SHADE_TERM_HI, ndl);
    let shade = mix(0.5, 1.5, lit) + AMBIENT;
    let color = ramped_inner * material.family4.rgb * shade
        + (surface - ramped_inner * material.family4.rgb);
    // **整层不再乘顶点色 R。** 70474 里 `v2.x` 只出现在两处:第 88 行给
    // `lerp(cb5[6], cb5[5], …) × 菲涅尔` 那一层调强度、第 147~152 行当液面门 ——
    // 没有一处是乘在最终颜色上的。原来那一乘把整泡液体乘成了黑的(R 实测 0)。
    return vec4<f32>(encode_linear_color(max(color, vec3<f32>(0.0))), 1.0);
}

/// `M_P_FakeFulid` 的目标 Low PS 42877 局部链。原 shader 从 PrimitiveSceneData 读取
/// 当前蒙皮盒中心/尺度，以 AbsoluteWorldPosition 对水平虚拟平面求交，再合成场景深度、
/// FuildMask、MatCap、边缘/渐变色；覆盖率是
/// `saturate(菲涅尔 + matcap_luma + fluid) * COLOR_0.g`。
///
/// **这里原来写的是「明确为 `saturate(matcap_luma + fluid) * COLOR_0.g`」(没有菲涅尔),
/// 那句是错的** —— PS 42877 第 235~246 行逐条是:
///
/// ```text
/// 菲涅尔 = saturate((N·X − cb3[31].y) / 宽度)      ← 235~238,是 saturate 不是 smoothstep
/// 玻璃色 = 菲涅尔 × FresnelColor + MatCap          ← 239
/// 输出色 = fluid × 液体色 + 玻璃色                  ← 240
/// r0.z   = 菲涅尔 + luminance(MatCap)              ← 243~244
/// α      = saturate(saturate(r0.z + fluid) × 顶点色G) ← 245~246
/// ```
///
/// 代码一直是对的,只有这段注释漏了菲涅尔那一项。**顺带一个排列上的例外**:
/// `MI_Ill_WuWu3_001_Fx1` 的 16 条 resource 里**一条 `quality=Num` 都没有**
/// (只有 Low/High/Medium/Epic),所以「实机默认 = Num ∧ lod=0 ∧ dsid=0」这条判据
/// 对它落空 —— 这里读的是 `Low/lod=0/dsid=0`(resource `A6367A32…`)。
fn shade_fake_fluid(in: VsOut, depth_coverage: f32) -> vec4<f32> {
    let n = normalize(in.normal);
    let view = view_direction();
    let ndv = saturate(dot(n, view));

    // UE Z-up 参数换到 glTF Y-up，与导出器处理法线/位置的轴变换一致。
    let axis = normalize(vec3<f32>(material.family6.x, material.family6.z, material.family6.y));
    let plane_offset = vec3<f32>(material.family7.x, material.family7.z, material.family7.y) * 0.01;
    let center = camera.object_bounds.xyz + plane_offset;

    // 42877 第 63–70 行以像素世界位置相对 ObjectWorldPosition 的平面坐标采样，
    // 不是网格 UV。UE 的水平 XY 平面换到 glTF 后是 XZ；除最长边保持原组件缩放语义。
    let plane_uv = (in.world_pos.xz - center.xz) / max(camera.object_bounds.w, 1e-5);
    let height_uv = plane_uv * material.family5.xy
        + camera.time * material.family5.zw;
    let mask_sample = srgb_to_linear(textureSample(base_color, base_sampler, height_uv).rgb);
    let height_noise = (mask_sample.r * 2.0 - 1.0) * material.family8.w * 0.01;
    let signed_height = dot(in.world_pos - center, axis) - height_noise;
    let plane_soft = max(material.family10.y * 0.01, 1e-4);
    // 原 PS 的 r0.z 是世界位置位于噪声液面下方的比较结果；场景深度差随后按
    // FadeDistance 衰减成 r0.w。用 smoothstep 只复现 TopEdgeSmooth 的连续边界。
    let below_plane = 1.0 - smoothstep(-plane_soft, plane_soft, signed_height);
    let fluid = below_plane * depth_coverage;

    let depth = saturate(-signed_height / max(material.family9.x, 1e-4));
    // 汇编第 203~208 行是**线性斜坡 + 一次 lerp**,两处我们原来都反着写:
    //   `t   = saturate((深度 − (GradientOffset − GradientSmooth)) / (2 × GradientSmooth))`
    //   `col = lerp(GradientColor01, GradientColor02, t)`      ← 0 那头是 01
    // 原来用的是 `smoothstep(o−s, o+s, 深度)`(形状不对,汇编是 `div_sat`,没有那条三次曲线)
    // 外加 `mix(gradient2, gradient1, t)`(**两个颜色调过来了**)。
    let gradient_t = saturate((depth - (material.family9.x - material.family9.y))
                              / max(2.0 * material.family9.y, 1e-4));
    let gradient = mix(material.family3.rgb, material.family4.rgb, gradient_t);
    let top_edge = depth_coverage * (1.0 - smoothstep(
        material.family10.x * 0.01,
        material.family10.x * 0.01 + plane_soft,
        abs(signed_height)
    ));
    var fluid_color = gradient;

    let matcap = srgb_to_linear(textureSample(matcap_tex, base_sampler, matcap_uv(n)).rgb)
        * material.matcap_color.rgb;
    // **这条菲涅尔是反的,我们原来写正了。** 汇编第 236~238 行:
    //   `f = saturate((N·V − (FresnelOffset + FresnelSmooth)) / ((FresnelOffset − FresnelSmooth)
    //                                                           − (FresnelOffset + FresnelSmooth)))`
    // 分母是 **−2 × FresnelSmooth**(负数)⇒ `f` 随 `N·V` **递减**:正对镜头 f = 0、
    // 掠射 f = 1 —— 一条正常的玻璃菲涅尔。cb 槽位不是猜的:vector=26 ⇒ 标量从 cb3[26] 起,
    // `cb3[31].y = scalar-slot[21] = FresnelOffset + FresnelSmooth`、
    // `cb3[31].z = scalar-slot[22] = FresnelOffset − FresnelSmooth`(字节码里 05=Add、06=Sub)。
    //
    // 代价很具体:克莱因龙的 `FresnelOffset/Smooth = 0.3/0.2`,正对镜头我们给 f = 1,
    // 于是液面**以上**那半个球被涂成 `MatCap + FresnelColor` 的蓝黑 (85,85,110)、
    // 而且 `α = saturate(f + matcap亮度 + 液体) × 顶点色G` 被顶成 1 ⇒ 整块不透明,
    // 把后面那层白壳全挡住。实机那块是 (204,217,250) 的浅蓝白 —— 正是「玻璃在这儿几乎全透,
    // 看到的是后面的身体」。用户报的「克莱因龙体内粉色液体上方是黑灰色内容」就是它。
    let fresnel = saturate((material.family9.z + material.family9.w - ndv)
                           / max(2.0 * material.family9.w, 1e-4));
    // 42877 第 178–185 行用“液面以下的世界距离 / BodyEdgeArea”构造边缘色，
    // 不是普通的视角 Fresnel。实例参数的 5/0.8/0.1 分别就是厘米范围与重映射阈值。
    let body_distance = max(-signed_height, 0.0);
    let body_proximity = 1.0 - saturate(
        body_distance / max(material.family8.x * 0.01, 1e-5)
    );
    let body_edge = smoothstep(material.family8.y,
                               material.family8.y + max(material.family8.z, 1e-4),
                               below_plane * body_proximity);
    fluid_color = mix(fluid_color, material.family2.rgb, saturate(top_edge));
    fluid_color = mix(fluid_color, material.family0.rgb, body_edge);
    let glass = matcap + material.family1.rgb * fresnel;
    let linear = glass + fluid_color * fluid;

    let matcap_luma = dot(matcap, vec3<f32>(0.3, 0.59, 0.11));
    let alpha = saturate(matcap_luma + fresnel + fluid) * in.color.g;
    let encoded = encode_linear_color(max(linear, vec3<f32>(0.0)));
    return vec4<f32>(encoded * alpha, alpha);
}

/// `M_P_MatCap_Masked` 的目标 ES3.1/Low PS 19654 材质局部链。
///
/// Color PS 最终 `o0.w` 明确写 1；基础遮罩在同 resource 的 Early-Z depth PS 15293
/// 中执行（函数末尾合并回来）。它也不是加色 VFX：19654 的 55–56 行是
/// `BaseColor * LightRamp + MatCap`，115–121 行依次接 Rim、Flat_Emissive、
/// MainColor/MainBright 与 SelectionColor。把 BaseColor=1.5 误当成“HDR tint ⇒ additive”
/// 会让这一层最后画且不写深度，正好把 FakeFulid 液面盖掉。
fn shade_matcap_masked(in: VsOut) -> vec4<f32> {
    let n = normalize(in.normal);
    let view = view_direction();
    let ndl = dot(n, normalize(camera.light_dir));

    // PS 19654 第 33–38 行：saturate((((N·L)+1)*.5-.3)*10)，再从
    // LightRampColor 混到白。MatCapTex 的资产标记 sRGB=1，显式恢复硬件解码。
    let ramp_t = saturate(((ndl + 1.0) * 0.5 - 0.3) * 10.0);
    let light_ramp = mix(material.family1.rgb, vec3<f32>(1.0), ramp_t);
    let matcap = srgb_to_linear(
        textureSample(base_color, base_sampler, matcap_uv(n)).rgb
    );
    var surface = material.family0.rgb * light_ramp + matcap;

    // cb3[5].xy / cb3[13].z：Rim Power、Rim Soft Edge、Rim Intensity。
    // 98–114 行先按观察方向 Z 缩窄轮廓区，再 pow、减 .5、除 SoftEdge。
    let ndv = saturate(dot(n, view));
    let rim_width = max((1.0 - abs(view.y)) * 0.4, 1e-4); // UE Z-up → glTF Y-up
    let rim_gate = 1.0 - saturate((ndv - 0.05) / rim_width);
    let rim_base = max((1.0 - ndv) * rim_gate, 1e-6);
    let rim = saturate(
        (pow(rim_base, max(material.family5.x, 1e-4)) - 0.5)
        / max(material.family5.y, 1e-4)
    ) * material.family5.z;
    // 原 r5 是 MobileBasePass 提供的中性环境/主光颜色；本渲染器的灯色同样为白。
    surface = mix(surface, vec3<f32>(1.0), saturate(rim));

    // 117–121 行。family6 = [FlatIntensity, FlatRatio, MainBright, XrayGate]。
    surface = mix(surface,
                  material.family2.rgb * material.family6.x,
                  saturate(material.family6.y));
    surface *= material.family3.rgb * material.family6.z;
    surface = mix(surface,
                  material.family4.rgb,
                  saturate(material.family4.a));

    // 基础 OpacityMask 不在上面的 color PS 里重复算：目标 resource 开着 masked
    // Early-Z，它由同资源的 depth PS 15293 第 30–45 行先写深度。那条链无条件采
    // MatCap、取亮度，与 pow(1-N·V,FresnelPow) 取 max，再减 cooked clip 0.3333。
    // 本渲染器是单遍，必须在这里合并同一条 depth-PS discard；只照抄 color PS 会让
    // 深度预通过滤凭空消失，整个闭合外壳写满深度并挡住内部 FakeFulid。
    let matcap_luma = dot(matcap, vec3<f32>(0.3, 0.59, 0.11));
    let fresnel = pow(max(1.0 - ndv, 1e-4), max(material.family5.w, 1e-4));
    if max(matcap_luma, fresnel) < 0.3333 {
        discard;
    }

    return vec4<f32>(encode_linear_color(max(surface, vec3<f32>(0.0))), 1.0);
}

/// `M_FairyBall_BallFront` 的目标 ES3.1 PS 52626 —— 沙漏 / 水晶球外面那层玻璃壳。
/// 这一族在 cooked 包里**只有一个 resource**(没有静态开关),所以不存在挑排列的问题。
///
/// **它不出固有色、也不吃光照**:整条链只采一张贴图,就是 MatCap(52626 第 80 行是
/// 全 shader 唯一的 `sample`)。颜色 = `MatCapColor.rgb × MatCap + BaseColor.rgb`,
/// 覆盖率 = `亮度(MatCap) × MatCap.a × MatCapColor.a + (BaseColor.a + Opacity)`,
/// 再叠一层按 `N·L` 在暗/亮两色之间取的边缘光。matcap26(等一等鸭那把)是张中间近黑、
/// 边上一圈亮的玻璃球图 —— 所以实机看到的正是「中间透得见紫沙、轮廓一圈白」。
///
/// **两层在这一族恒等于零,照抄进来只是白费指令,所以不写**:
/// - 第 89–90 行的 `lerp(色, SelectionColor.rgb, SelectionColor.a)` 是编辑器选中色,
///   运行时 alpha = 0;
/// - 第 28–44 行那整条 Fresnel 发光层的总强度是 `FresnelIntensity`,根默认 0 且
///   五个实例一个都没覆盖 —— 整层乘 0。
fn shade_fairy_ball(in: VsOut) -> vec4<f32> {
    let n = normalize(in.normal);
    let ndv = max(dot(n, view_direction()), 0.0);

    // 第 45–56 行:**`smoothstep(0.5 − RimSmoothness, 0.5 + RimSmoothness,
    // pow(1 − N·V, RimArea))`** —— 一条以 0.5 为中心、半宽 `RimSmoothness` 的过渡带,
    // 指数是 `RimArea`。两个名字到这儿才讲得通:`Area` 管边缘光铺多宽,`Smoothness` 管它多软。
    //
    // **这两格以前是配错的**(写着「cooked 参数表的名字对不上,按实机截图定」):
    // 那张表是被 CUE4Parse 的步长 bug 打乱的,只有第 0 条名字对。修掉之后
    // `cb3[19]` 四格逐个读出来是 `RimSmoothness` / `RimArea` / `0.5 + RimSmoothness` /
    // `0.5 − RimSmoothness`,一点都不用猜。旧配法把指数取成 `RimSmoothness`(等一等鸭
    // = 0.2254),`pow(x, 0.2254) ≤ 1` 又永远够不到高边 `0.5 + RimArea`,于是边缘光
    // 在整个球面上是一层最高只有 0.24 的淡雾,而不是「轮廓一圈、中间干净」。
    //
    // 汇编那句 `div 1, (高 − 低)` 在 `RimSmoothness = 0` 时是 ±inf → saturate 出一个
    // 硬阶跃;这里用 `max(…, 1e-4)` 得到同样的极窄过渡,同时不产生 NaN。
    // 求幂那步汇编是 `log → mul → exp`,再拿 `movc` 把「底 ≤ 0」那一格顶成 0(不然
    // log(0) 是 -inf)。
    let rim_lo = 0.5 - material.family11.y;
    let rim_hi = 0.5 + material.family11.y;
    let rim_fresnel = 1.0 - ndv;
    let rim_base = select(pow(max(rim_fresnel, 1e-6), material.family11.x),
                          0.0, rim_fresnel <= 0.0);
    let rim_t = saturate((rim_base - rim_lo) / max(rim_hi - rim_lo, 1e-4));
    let rim = rim_t * rim_t * (3.0 - 2.0 * rim_t);

    // 第 82–88 行:边缘光色按 `N·L` 在 RimDark/RimLight 之间取,rgb 再乘它自己的 alpha;
    // 这一层的覆盖率是 `saturate(rim × 该 alpha)`,颜色按同一个系数混到 MatCap 层上。
    let ndl = saturate(dot(n, normalize(camera.light_dir)));
    let rim_color = mix(material.family2, material.family3, ndl);
    let rim_alpha = saturate(rim * rim_color.w);

    // 第 80–81 行。MatCap 是 sRGB 资源,硬件那道解码在这儿补回来;UV 与其余 MatCap 同一条
    // (实机是 `cross(视线, 视空间法线)`,正交投影下退化成 `(Nv.x, -Nv.y)`,见 `matcap_uv`)。
    let mc = textureSample(base_color, base_sampler, matcap_uv(n));
    let mc_rgb = srgb_to_linear(mc.rgb);
    var color = material.family1.rgb * mc_rgb + material.family0.rgb;
    color = mix(color, rim * rim_color.rgb * rim_color.w, rim_alpha);
    // 第 91、94 行:先夹到非负,再乘 `MainColor × MainBright`(五个实例都是白 × 1)。
    color = max(color, vec3<f32>(0.0)) * material.family4.rgb * material.family4.w;

    // 第 99–102 行。亮度权重就是汇编里那三个常数;末尾那次 `add_sat` 才是唯一的饱和。
    let mc_alpha = dot(mc_rgb, vec3<f32>(0.3, 0.59, 0.11)) * mc.a * material.family1.w;
    let floor_alpha = material.family0.w + material.family11.z;
    let alpha = saturate(mc_alpha + floor_alpha + rim_alpha);
    return vec4<f32>(encode_linear_color(color) * alpha, alpha);
}
