// 炫彩:闪点层与整层合成
//
// **这份文件不是独立的着色器** —— `src/pet/gpu.rs` 用 `concat!(include_str!(…))`
// 把 `src/pet/shader/*.wgsl` 按文件名顺序拼成一个模块再交给 wgpu。WGSL 的模块级条目
// 与顺序无关,所以拆分只是为了好读;**加新文件记得同步 gpu.rs 里那张 `concat!` 清单**。

/// **闪点层** —— 实机淡色身体上那一片「极小的白色亮点」。
///
/// 这一层**只在高质量那条排列里**(鸭吉吉 `MI_Com_YaJiJi1_001_By` 的 resource `[15]`,
/// quality=Num / LODUsed=0 / DSId=1,PS **5710**);我们一直读的 Low 那条(PS 50659)
/// 整段都没有,所以过去怎么调相位、调分辨率都调不出来。同一条排列里还有
/// **`StickRandomColor01..04` 四段渐变**(向量参数 14~17),与既有 `stick_layer` 那条
/// 按实测颜色接出来的完全一致 —— 两件悬案一起结了。
///
/// 汇编是一段**三重循环的 Voronoi**(3×3×3 邻域),逐字如下:
///
/// ```text
/// cell = vec3(UV0 × 20 × StarTiling, frac(time × 0.0056)) × StarDensity
/// p    = saturate(2 × (MainTex.r − 法线投影 × NormalEffectAmount)) × 距离系数 + 0.1
/// 对 27 个邻格:
///     hash = frac(sin(vec3(dot(n,(1,57,113)), dot(n,(57,113,1)), dot(n,(113,1,57)))) × 43)
///     d    = min((Σ |格内偏移 + hash|^p)^(1/p), 1)
/// 闪点 = max((1 − min d) × StarIntensity, 0)
/// ```
///
/// 两处值得记:
///
/// - **`p` 恒小于 1**(0.1~0.5),闵可夫斯基单位球因此是**凹的四角星** —— 实机截图里那些
///   亮点确实是四芒星而不是圆点,形状就是这么来的,不是 bloom。
/// - **`StarIntensity` 就是这一层的亮度**,而 lua 给常规炫彩把它设成 **10**
///   (`COLOR_RANDOM_CONF.shine_strength` 全表 39 条都是 10)。这条参数一度被记成
///   「实机空转」,因为 Low 那条排列里它根本不存在。
///   配置表还能反过来验:狂欢怪谈与黑白把它写成 **0** ⇒ 那两款不该有亮点 ——
///   实机奔波鼠(狂欢怪谈)与卡波(常规)同倍数放大一比,正是一个没有、一个满身。
///
/// 格子边长按根默认是 UV 的 1/64(`20 × 0.4 × 8`),而星贴层是 UV × 4 ——
/// 密上十几倍,这就是「密度是方块的好几倍」。
fn glassy_sparkle(uv: vec2<f32>, pattern_r: f32, normal_proj: f32) -> f32 {
    let intensity = material.glassy_sparkle.z;
    if intensity <= 0.0 {
        return 0.0;
    }
    let p = saturate(2.0 * (pattern_r - normal_proj * material.glassy_p0.w))
        * GLASSY_SPARKLE_DIST + 0.1;
    let cell = vec3<f32>(uv * 20.0 * material.glassy_sparkle.x,
                         fract(camera.time * GLASSY_SPARKLE_DRIFT)) * material.glassy_sparkle.y;
    let inner = fract(cell);
    let base_cell = floor(cell);
    var best = 1e9;
    for (var z = -1.0; z <= 1.0; z += 1.0) {
        for (var y = -1.0; y <= 1.0; y += 1.0) {
            for (var x = -1.0; x <= 1.0; x += 1.0) {
                let offset = vec3<f32>(x, y, z);
                let cell_id = base_cell + offset;
                let hash = fract(sin(vec3<f32>(
                    dot(cell_id, vec3<f32>(1.0, 57.0, 113.0)),
                    dot(cell_id, vec3<f32>(57.0, 113.0, 1.0)),
                    dot(cell_id, vec3<f32>(113.0, 1.0, 57.0)),
                )) * 43.0);
                let delta = pow(abs(offset - inner + hash), vec3<f32>(p));
                best = min(best, min(pow(delta.x + delta.y + delta.z, 1.0 / p), 1.0));
            }
        }
    }
    return max((1.0 - best) * intensity, 0.0);
}

/// 炫彩(游戏里 `MDT_GLASS`)。**照 `GlassySwitch = true` 那条 shader 排列写的**,
/// 不是观察出来的近似 —— 排列怎么找到的、每一步对应哪几行汇编,见 `src/pet/glassy.rs`
/// 的模块注释与 docs/findings.md「炫彩那条 shader 分支」。
///
/// `mode`(`glassy_red.w`):0 = 不画、1 = 常规炫彩、2 = 隐藏/赛季炫彩。
///
/// 传进来的 `base` 是这个材质的**固有色**(线性),`shaded` 是原本要输出的着色结果。
/// 返回值直接顶替 `shaded` —— 原 PS 尾部那两次 lerp 在本作的参数下都退化成「整层替换」:
/// `cb6[62].x`(星点偏置)根默认 0 ⇒ 第一次 lerp 系数恒为 1;
/// `cb6[62].y` = `BlendWeight` = 1.0 ⇒ 第二次也是 1。
/// 原宠物的明暗结构靠第 ⑤ 步那道亮度门保留下来,不靠回混。
fn glassy_layer(in: VsOut, base: vec3<f32>, shaded: vec3<f32>) -> vec3<f32> {
    if material.glassy_red.w < 0.5 {
        return shaded;
    }
    let season = material.glassy_red.w > 1.5;
    // ⓪ **区域门**。整段玻璃层在原 PS 里包在 `if (MaskTex.a >= MinID)` 里,门外走 `else`
    //    直接输出原着色。这是实机「只给部位上色」的来源:鸭吉吉的喙与脚、白金独角兽的
    //    身体,alpha 都是 0,于是一点玻璃色都不沾。没导那张遮罩的旧包绑的是白图 ⇒ 门恒开。
    //
    //    **赛季那一族的金属环画在门外面**(汇编里门做完之后才做),所以这里不能直接返回:
    //    门只挡玻璃色那一半。挡错了整圈银色扑克花纹会被切碎、胳膊与肩膀上那圈直接没有。
    //
    //    **门只当值用,不当分支用。** 门外那条 `return shaded` 曾经是条快路,可它让底下
    //    所有 `textureSample` 落进「依赖非一致值的控制流」——  WGSL 规定带隐式导数的采样
    //    只能在一致控制流里调,浏览器(Tint)据此**整份 shader 拒编**,网页预览连一只
    //    普通宠物都画不出来(桌面的 naga 放行,所以只有网页会炸)。
    //    删掉它是**逐字等价**的:第 ⑧ 步本来就写着 `select(shaded, …, gated)`,
    //    而金属那一步的 `season_metal_zone` 在非赛季材质上恒为 0。那条快路只省了
    //    「整个 quad 都在门外」时的几次采样,不值得拿整个网页端去换。
    let gated = textureSample(glassy_id_tex, base_sampler, in.uv).a >= GLASSY_MIN_ID;
    let n = normalize(in.normal);

    // ① 折射。`cb6[58].z` 的 preshader 是 `1 / GlobalRefraction`,倒数在 CPU 侧算好了
    //    (见 `GlassyRender::refraction_eta`),这里拿到的已经是 eta。
    //    判别式为负(全反射)时 `refract_direction` 整支置零,与汇编的 `and r2.xyz, …, r3.w` 一致。
    let incident = -view_direction();
    let refracted = refract_direction(incident, n, material.glassy_p0.x);
    // `GlobalDepth` 是游戏单位(厘米),我们的模型是米。
    let hit = in.world_pos + refracted * (material.glassy_p0.y * 0.01);

    // ② 屏幕空间 UV,**相对物体中心**。原 PS 取的是折射落点与包围盒中心各自的**裁剪空间
    //    xy(不做透视除法)**之差,再除以物体→世界矩阵的最大轴缩放(宠物那边恒为 1)。
    //    这一步弄成「除以 w 的 NDC」就会得到「蒙在镜头前」的观感。
    //
    //    原式尾巴那个 `MainTexTiling × 0.01`(preshader 里明写的)是**厘米→米**:UE 的
    //    世界单位是厘米,我们的模型已经是米,所以那个 0.01 在我们这边正好抵消掉,
    //    剩下的就是下面这一行。**不需要任何标定常数** —— 之前那个 `GLASSY_SCREEN_REF`
    //    是在读错乘数、花纹被推成一片白的前提下对着截图目视凑的。
    let hit_clip = camera.view_proj * vec4<f32>(hit, 1.0);
    let center_clip = camera.view_proj * vec4<f32>(camera.object_bounds.xyz, 1.0);
    var uv = (hit_clip.xy - center_clip.xy) * 0.25 * material.glassy_p0.z + vec2<f32>(0.5);
    uv += fract(camera.time * material.glassy_p1.xy);
    // ③ 法线扰动:偏移量是法线在投影矩阵 z 列上的分量 × `NormalEffectAmount`,
    //    u/v **同一个标量**(汇编 `mad r0.xy, -r0.x, cb6[60].x, r2.xyxx`)。
    let normal_proj = dot(n, camera_forward_direction());
    uv -= normal_proj * material.glassy_p0.w;

    // ④ 着色。这两行是整条链的核心,`RedChannel`/`GreenChannel` 就是玩家选的那两个颜色。
    //    乘的是 `cb6[61].x` = **(BaseColorDetail + 1) × FlowColorIntensity**(常规炫彩 = 1.62),
    //    **不是 `StarIntensity`(=10)** —— 接错那个会把整只推到过曝白,两色渐变就没了。
    let pattern = textureSample(glassy_main_tex, base_sampler, uv);
    var glass = material.glassy_red.rgb * pattern.r + material.glassy_green.rgb * pattern.g;
    glass *= material.glassy_green.w;

    // ④′ **赛季传说精灵那一族**(`MI_P_Object_SeasonMutation*`)在这里多两块区域。
    //     骨架与上面完全相同 —— 同一条折射屏幕 UV、同一个 1.62 增益、同一道
    //     `MaskTex.a` 区域门、同一条亮度门;换掉的只是输入(花纹图是材质自带的
    //     `FlowNoise`,两个 Channel 色也是材质自己的)。多出来的就是这两块:
    //     `MixMask.b` 按 `pow(x × FlowMaskInt, FlowMaskPow)` 混向 `BlueChannel`;
    //     `MixMask.a ≥ 0.79` 的地方整片换成 `MetalColor` —— 机幕方舟那圈**银色**
    //     扑克花纹就是它(它的 `MetalColor` = (1.5,1.5,1.5)),而「只在翅膀」
    //     「只在身体与肩顶」是 `MixMask` 这张**每宠物一张**的图划的。
    var season_metal_zone = 0.0;
    if season {
        let mask = textureSample(season_mask_tex, base_sampler, in.uv);
        let curve = mask.b * material.glassy_stick0.w;
        let m = select(min(pow(max(curve, 0.0), material.glassy_stick1.w), 1.0), 0.0, curve <= 0.0);
        glass = mix(glass, material.glassy_stick0.rgb, m);
        season_metal_zone = select(0.0, 1.0, mask.a >= 0.79);
    }

    // ⑤ 按固有色亮度调制。`mean ≤ 0` 时整片归零(汇编那条 `ge` + `movc`),
    //    否则 `min(pow(mean, BaseColorDetail), 1)`。炫彩之所以还看得出原宠物的
    //    明暗结构,全靠这一步。
    let mean = (base.r + base.g + base.b) * 0.3333;
    let detail = select(min(pow(max(mean, 0.0), material.glassy_p1.w), 1.0), 0.0, mean <= 0.0);
    // ⑤′ **闪点层**,加性、不吃这道亮度门(汇编 `mad r0.xyz, 玻璃色, 亮度门, 闪点`)。
    glass = glass * detail + glassy_sparkle(in.uv, pattern.r, normal_proj);

    // ⑥ 星点层。相位公式与既有的 `stick_layer` 逐字相同(`1.1 × lerp(|sin θ|,|cos θ|, g)`,
    //    θ = `frac(time × 0.25) × 2π`)—— 两条排列共用同一段材质图。
    //    这里的 RGB **不是颜色**:g 给相位、r 给阈值、b 给幅度,颜色全来自参数。
    //    **平铺(`glassy_p1.z`)来自材质自己的 `StarStickTiling`,不是粒子配置表里那个** ——
    //    lua 只在**随机蛋**那条路上写 `StarStickTiling`(`PARTICLE_RANDOM_CONF` 的 2.2 / 1.0),
    //    宠物身上一个字都不写,用的是材质(鸭吉吉 `_By` = 4.11)或根默认 4。见 glassy.rs。
    // 赛季那一族没有星贴层(它的 `StarStickTex` 不参与这条分支),绑的是白图,
    // 直接跳过免得在身上糊一层白方块。
    let star = select(
        textureSample(glassy_star_tex, base_sampler, in.uv * material.glassy_p1.z),
        vec4<f32>(0.0),
        season,
    );
    let theta = fract(camera.time * STAR_PHASE_SPEED) * 6.2831855;
    let k = 1.1 * mix(abs(sin(theta)), abs(cos(theta)), star.g);
    let t = saturate((star.b * (k - star.r) - 0.01) * 25.0);
    let cover = t * t * (3.0 - 2.0 * t);
    // **星点色是一条四段渐变,按每颗粒子自己的 `k` 取** —— 和既有 `stick_layer` 同一条
    // 公式、同一族(`StarStickTex`),色标就是 `StickRandomColor01..04`。所以粒子**一边
    // 涨缩一边换色**:`k` 既是覆盖率的阈值,也是取色的位置。
    //
    // 实机验证:鸭吉吉截图里量到的方块 黄 (255,252,51) / 蓝 (116,148,240) / 紫 (201,155,255),
    // 对应 `ks` = 1.00 / 0.67 / **0.50** —— 那个紫落在品红与蓝**之间的过渡段**上,
    // 四选一取不出这个颜色,只有渐变取得出。
    let ks = saturate(k);
    var star_color = mix(material.glassy_stick0.rgb, material.glassy_stick1.rgb, min(ks * 3.0, 1.0));
    star_color = mix(star_color, material.glassy_stick2.rgb, saturate(ks * 3.0 - 1.0));
    star_color = mix(star_color, material.glassy_stick3.rgb, max(ks * 3.0 - 2.0, 0.0));

    // **`cover` 在原式里乘了两次**:目标色是 `Stick_Intensity × cover × 星色`,混合系数
    // 又是 `saturate(cover + GlassyMainColorOpacity)`(汇编 306~309 行)。少乘一次的话
    // 星点会从玻璃色直接跳到纯白,而不是像实机那样淡淡地浮出来。
    glass = mix(
        glass,
        GLASSY_STICK_INTENSITY * cover * star_color,
        saturate(cover + GLASSY_STICK_BIAS),
    );

    // ⑦ 边缘光。**颜色与强度都是材质自己的 `RimColor` / `RimIntensity`** ——
    //    lua 写的那个 `MutationRimColor` 在这条排列的向量参数表里根本不存在
    //    (和当年的 `StarIntensity` 一个处境:设了没人读),所以原来那句
    //    「写死的 (0.6,0.6,0.6) × 标定 0.0532」两处都是错的。
    //
    //    汇编(PS 5710 第 270~290、396 行)是三项相乘再乘强度:
    //        菲涅尔 = max(1 − N·V, 0)^3            ← **不取绝对值**
    //        A      = (2 × (MainTex.r − 0.5) × (L·N) + 1) × 0.5
    //        B      = (h³ − 1) × 0.98 + 1,h = (L·N + 1) × 0.5;L·N ≤ −1 时取 0.02
    //    A 把花纹图的红通道也拌进边缘光里,所以那圈光会跟着花纹一起流动。
    let ndl = dot(normalize(camera.light_dir), n);
    let rim_fresnel = pow(max(1.0 - dot(n, view_direction()), 0.0), 3.0);
    let rim_pattern = (2.0 * (pattern.r - 0.5) * ndl + 1.0) * 0.5;
    let half_lambert = (ndl + 1.0) * 0.5;
    let rim_light = select(
        (half_lambert * half_lambert * half_lambert - 1.0) * 0.98 + 1.0,
        0.02,
        ndl <= -1.0,
    );
    glass += material.glassy_rim.rgb * material.glassy_rim.w
        * rim_fresnel * rim_pattern * rim_light;

    // ⑧ 换算到我们的线性空间再合成。**玻璃层是整帧里唯一一个不过光照的量** ——
    //    原式第 ⑧ 步是 `lerp(原着色, glass, BlendWeight)`,BlendWeight = 1 ⇒ 整层替换,
    //    而 glass 是拿固有色量级的参数直接算出来的、游戏那边直接输出的显示值。
    //    我们这条管线的约定不一样:材质输出的是**受过光照的**线性辐亮度,末尾按
    //    `sqrt(x × EXPOSURE)` 编码,`EXPOSURE` 正是用来抵掉光照那一档增益的。
    //    所以把游戏那边的显示值搬进来要先除以 `EXPOSURE`,否则玻璃层会比同一帧里
    //    别的东西暗一整档(实测鸭吉吉腹部 (124,113,134),实机是 #bac8fb / #f3c05a)。
    //    这和 `STICK_GAIN` 那条「旧² / EXPOSURE」的换算是同一个道理,不是新标定。
    // 门外保持原着色;门内才换成玻璃色。
    // 赛季那一族的颜色是**显示尺度**的(`BlueChannel` 就是 1.0、`MetalColor` 1.5),
    // 不像玩家选的那 39 组是 HDR 系数(到 1.6)。再除一次 `EXPOSURE`(×2.08)会把它们
    // 顶到纯白 —— 实机机幕方舟头顶那道竖条是亮红与暗红交替,不是红白交替。
    let gain = select(1.0 / EXPOSURE, 1.0, season);
    var out = select(shaded, mix(shaded, glass * gain, GLASSY_BLEND_WEIGHT), gated);

    // ⑨ **赛季那一族的金属区画在门外面**(汇编 231~238 行:门做完之后才做),
    //    所以整圈银色扑克花纹是连续的,胳膊与肩膀上那圈也在。
    //    先整片换成 `MetalColor`,再按同一块区域里那条噪声混向 `MetalColor02`。
    //    **平涂,不乘 matcap**:汇编那句就是 `mix(r0, MetalColor, zone)`。曾经为了解释
    //    机幕方舟那圈「银」的明暗自作主张乘了 `Mutation_MatCap`,结果把龙息帕尔翅膀上
    //    本该是白的星月染成了紫(它的 `MetalColor` 是 (1,1,1),matcap 才是紫的)。
    //    实机那点明暗来自后面统一乘的那个阴影项,不是 matcap。
    //    **要把光照乘回去。** 实机里这一族的输出后面还要走一整段 toon 光照(ramp/rim),
    //    `MetalColor` 是被点亮的**固有色**;而我们这条链上 `glassy_layer` 在最后,替换掉的
    //    是**已经点亮的** `shaded` —— 平铺就成了一片死白,机幕方舟那圈「银」的明暗全没了。
    //    `shaded` 与 `base` 的亮度比就是当地那一档光照,乘回去金属才立体。
    let lum_base = max(dot(base, vec3<f32>(0.3, 0.59, 0.11)), 1e-3);
    let lum_lit = dot(shaded, vec3<f32>(0.3, 0.59, 0.11));
    let relight = clamp(lum_lit / lum_base, 0.0, 4.0);
    //    金属光泽:乘材质自己的 `Mutation_MatCap`(按视空间法线查表)。没导到就是白图,
    //    乘 1 等于平涂 —— 这一层的开关是材质的 `MetalSpecInt`,在导出器那边判。
    let metal_gloss = textureSample(season_matcap_tex, base_sampler, matcap_uv(n)).rgb;
    out = mix(
        out,
        material.glassy_stick1.rgb * metal_gloss * gain * relight,
        season_metal_zone,
    );
    return out;
}
