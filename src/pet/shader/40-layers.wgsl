// 叠在固有色 / 发光上的各层:星贴、卷动色带、UV 流动、菲涅尔、火系、球内星、水体
//
// **这份文件不是独立的着色器** —— `src/pet/gpu.rs` 用 `concat!(include_str!(…))`
// 把 `src/pet/shader/*.wgsl` 按文件名顺序拼成一个模块再交给 wgpu。WGSL 的模块级条目
// 与顺序无关,所以拆分只是为了好读;**加新文件记得同步 gpu.rs 里那张 `concat!` 清单**。

/// 星点层。**一只宠物只有一份,盖在整只身上**(导出器统一好了,见 Program.cs):
/// 那两颗球身上的星星也是它 —— 球的基色在图集里是一片平色圆盘,星形完全来自这层
/// (所以幽星光一颗球是星、另一颗是圆点)。
///
/// **这里是照反汇编原样搬的**,来源:`MI_P_Object_XingGuang_FakeTrans01` 的世界 base pass
/// (shader 27803,7 个 uniform buffer ⇒ 材质 cb 是 cb6),第 375~403 行:
///
/// ```text
/// r12.w = frac(cb0[153].z * 0.25)                  ← View 时间 × 硬写的 0.25(4 秒一周)
/// θ     = r12.w * 2π
/// k     = 1.1 * lerp(|sin θ|, |cos θ|, tex.g)
/// uv    = v2.xy * cb6[130].w                       ← v2 = **网格 UV0**
/// x     = saturate((tex.b * (k - tex.r) - 0.01) * 25)
/// m     = x²(3 - 2x)                               ← smoothstep,×25 造出很硬很细的边
/// c     = 4 段渐变 cb68 →(t=⅓) cb67 →(⅔) cb70 →(1) cb72,t = saturate(k)
/// 出    = lerp(底色, cb6[131].x * m * c, saturate(m + cb6[131].y))
/// ```
///
/// 读这段有个坑:`sample` 写的是 `r11.xyz`,**把上面 `sincos` 存进 r11.x 的 sin 覆盖掉了**,
/// 所以后面 `add r3.z, -r11.x, r3.z` 减的是 `tex.r` 而不是 sin。
///
/// 由此贴图三通道的分工是 **r = 每颗星的阈值、g = 相位混合、b = 幅度**,星形完全烘在贴图里
/// (实测那张 512² 图:三通道基本共位、都是连续的 0..1、alpha 恒 1 未用)。
///
/// **这段汇编推翻了一条写进过文档的旧结论**:「采样坐标取 NDC、星点贴在镜头上不随模型转」
/// 是错的 —— 这个 shader 里 `v8`(SV_Position)只在 View 的抖动那条出现过。
///
/// ---
///
/// **这一层现在是照汇编实现的,四个渐变色是读出来的、不是猜的。** 定名的办法是把同一段代码
/// 在 `MI_P_Object_Masked` 的 shader 27931 里配到冻结块 9,再经 uexp 里 shader map 自带的
/// 名字表把 `paramId` 翻成名字(整条链见 rocom-capture 的 docs/shader.md「最后一步:名字」)。
/// 槽位对应:
///
/// | 汇编 | 名字 | 值 |
/// |---|---|---|
/// | `cb6[96].z` | `StarStickTiling` | 4(逐材质,走 `material.star.xy`) |
/// | `cb6[96].w` | `Stick_Intensity` | 1.5 |
/// | `cb6[97].x` | `GlassyMainColorOpacity` | 0 |
/// | `cb6[38]` | `StickRandomColor02` | (0.960, 0.160, 0.907) 洋红 |
/// | `cb6[37]` | `StickRandomColor03` | (0.049, 0.155, 0.977) 蓝 |
/// | `cb6[40]` | `StickRandomColor04` | (0.925, 0.742, 0.027) 黄 |
/// | `cb6[42]` | `00FX_BaseColor` | (1, 1, 1) 白 |
///
/// **`StickRandomColor01` 不在这条渐变里** —— 名字有 `01..04` 四个,用到的是 `02/03/04` + 白。
/// 除 `StarStickTiling`(2 处)与 `00FX_BaseColor`(2 处)外,这些参数**全库没有任何实例覆盖过**
/// (拿探针的 `--probe-material ALL` 那 395 条实例覆盖清单查的),所以写成常量是安全的。
///
/// **推翻的旧结论**:我曾写下「那 4 个色不要去套 `StickRandomColor01..04`,实机星点是淡白粉、
/// 和 HDR 的 `Color02` 才对得上」。汇编说了它就是这四个浓色 —— 之前那个观察站不住,
/// 因为浓色经 `× m`(m 多数时候很小)、HDR 曝光再 `sqrt` 编码之后本来就会往白里跑。
/// 旧的 `min(r, g, b)` 近似同时废弃:它连遮罩形状都和汇编不是一回事。
/// 星贴层的一次求值:`color` 已含 `Stick_Intensity`,`cover` 是 lerp 的混合系数。
struct StickLayer {
    color: vec3<f32>,
    cover: f32,
}

fn stick_layer(uv0: vec2<f32>, ndc: vec2<f32>) -> StickLayer {
    if material.flags.z < 0.5 {
        return StickLayer(vec3<f32>(0.0), 0.0);
    }
    // 这是原 shader-map 的质量排列差异,不是宠物特判。目标实机的 ES3.1/Low
    // `M_P_Object_Trans` 资源只声明 4 张纹理、没有 StarStick；它即使绑定了
    // MobileDirectionalLight 也一样。假半透族走另一张材质图,不受这个分支影响。
    let trans_star_stick = material.flags.y > 0.5 && material.noise_uv.w > 0.5;
    if trans_star_stick && camera.high_material_quality < 0.5 {
        return StickLayer(vec3<f32>(0.0), 0.0);
    }
    let theta = fract(camera.time * STAR_PHASE_SPEED) * 6.2831855;
    // **坐标系由 `UseNoiseUV0` 定。** 假半透族(幽星光一家)根默认是 0 ⇒ 走**相机空间**
    // (这里用 NDC),再按 `Mat_NoiseSpeedX/Y` 随时间滚动。`SpeedY = -0.1` 为负 ⇒
    // 采样坐标下移 ⇒ 图案**上浮**,与实机一致。
    // **屏幕参考尺度。** 实机里宠物只占屏幕一小块,`Mat_NoiseTiling`(5 / 2.5)铺的是
    // **整个屏幕**;而我们离屏渲染时宠物**填满画布**,同样的平铺落到宠物身上只剩一两次
    // —— 星点偏大、滚动相对星点也偏快(用户实测两条)。乘一个参考尺度同时校正两者:
    // 平铺变密 ⇒ 星点变小,而滚动是 UV 单位、相对格子就慢了同样的倍数。
    // **这个数是标定的**,不是读出来的:它取决于实机截图里宠物占屏幕多大。
    let base_uv = select((ndc * 0.5 + vec2<f32>(0.5)) * SCREEN_REF, uv0,
                         material.noise_uv.w > 0.5);
    let scroll = vec2<f32>(material.noise_uv.x, material.noise_uv.y) * camera.time;
    let tex = textureSample(star_tex, base_sampler, base_uv * material.star.xy + scroll);
    // `k = 1.1 * lerp(|sin θ|, |cos θ|, tex.g)`,每颗星按 g 通道拿到自己的相位
    let k = 1.1 * mix(abs(sin(theta)), abs(cos(theta)), tex.g);
    let ks = saturate(k);
    // 4 段渐变,每段 ⅓ 宽。第三段汇编用的是 `max(3k-2, 0)` 不是 saturate ——
    // k ≤ 1 时两者等价,照抄以免以后 k 的上界改了还对
    // **两族的着色不一样,不能共用四段渐变。** `StickRandomColor01..04` 属于
    // `StarStickTex` 那一族;而幽星光一族走的是**「假半透」**族(`NoiseTex` + `Color02`),
    // 它的颜色就是 `Color02` —— 导出器早就把它归一化后写成 `star_color` 了
    // (曜星光 `Color02` = (10, 8.07, 9.04) ⇒ `star_color` = (1, 0.807, 0.904))。
    //
    // **踩过**:我按汇编把渐变读出来之后,不分族地套到所有材质上,而退步的三只
    // (幽星光 0.086→0.115、曜星光 0.078→0.129、暮星辰 0.082→0.094)**正好全是假半透族**。
    // 「公式读对了」不等于「这条公式属于这个材质」—— 先确认材质属于哪一族。
    var c = material.star_color.rgb;
    if material.params.w < 0.5 {
        c = mix(STICK_RAMP_0, STICK_RAMP_1, min(ks * 3.0, 1.0));
        c = mix(c, STICK_RAMP_2, saturate(ks * 3.0 - 1.0));
        c = mix(c, STICK_RAMP_3, max(ks * 3.0 - 2.0, 0.0));
    }
    // 遮罩:`× 25` 造出很硬很细的边。**减的是 tex.r 不是 sin θ** ——
    // 汇编里 sample 的目标寄存器把 sincos 的结果覆盖掉了,踩过一次
    //
    // **两族的遮罩读法也不一样,和着色一样得分开** —— 分不开的代价见下面 `StarStickTex` 那支。
    var m: f32;
    if material.noise_uv.w > 0.5 {
        // **`StarStickTex` 族:照汇编算。** 这一族的贴图(全库 31 个材质都是
        // `Tex_PetGlassyStar_004`)**不是成品星场,是张噪声图** —— 三通道按汇编分工
        // (r = 每颗星的阈值、g = 相位、b = 幅度),星形是这条公式**算出来**的。
        // 实测那张 512²:r 均值 0.855(92% 亮过一半)、b 均值 0.121。
        let x = saturate((tex.b * (k - tex.r) - 0.01) * 25.0);
        m = x * x * (3.0 - 2.0 * x);
    } else {
        // **`NoiseTex`(假半透)族:贴图本身就是成品星场,不该再去切。**
        // 拿上面那条公式切它必出**空心环**:`tex.r < k` 是个随时间移动的阈值,
        // 会把亮的星芯排除在外、只留外圈辉光(实机报的"星点周围一圈光晕闪烁")。
        // **遮罩要收到只剩星芯。** 颜色是 `c·m·gain`(幽星光 15 × 0.05 = 0.75·m),
        // 而 cover 用裸 m —— 若把星芒外围那圈暗辉光(m ≈ 0.3)也算进 cover,
        // 就会往身体上混一层比底色更暗的**灰**:星芯亮不起来、暗区反而发灰(用户实测)。
        // 只保留 `c·m·gain` 能压过底色的那一段。
        m = smoothstep(0.5, 1.0, max(tex.r, max(tex.g, tex.b)));
    }
    // **增益也分族**:假半透族是 `Mat_NoiseIntensity`(0.05,与 HDR 的 `Color02` 配对,
    // 用 `Stick_Intensity` 会差三十倍);`StarStickTex` 族的汇编里乘的就是
    // `Stick_Intensity`(1.5),而它那条 `noise_uv.z` 是默认值 1、不是读出来的。
    let gain = select(material.star_color.w, material.noise_uv.z, material.noise_uv.w < 0.5);
    // **强度只进颜色,不进混合系数。** 汇编那条是
    //     lerp(底, Stick_Intensity × m × c, saturate(m + GlassyMainColorOpacity))
    // —— cover 用的是**裸的 m**。把 gain 也乘进 cover(0.05·m)会让这一层几乎不参与
    // 混合,星点整个看不见(踩过)。
    // **颜色取贴图自身。** `_Fx_D` 里那些星芒本来就是**品红 / 白 / 青**的成品色
    // (实机看是"浅青粉"),用一个平的 `Color02` 白色会把这层色相抹平(用户实测)。
    // `Color02` 只作为 HDR 增益。
    let lit_c = select(c, tex.rgb * c, material.noise_uv.w < 0.5);
    return StickLayer(lit_c * m * gain, saturate(m + STICK_BLEND_FLOOR));
}

/// 卷动色带:一张渐变图沿 UV 滚过表面,乘在固有色上。暮星辰的环带靠它出青↔粉渐变
/// (`FlowTexture` = 青↔粉竖条纹 + `Flow_U_Speed` = 0.25;基色贴图里环带那一条是纯粉的)。
fn flow_band(uv: vec2<f32>, albedo: vec3<f32>) -> vec3<f32> {
    if material.extra.w < 0.5 {
        return albedo;
    }
    // **两次采样都提到分支外面。** WGSL 的均匀性规则:`textureSample` 自己算 mip 层级,
    // 要靠同一个 quad 里四个像素的导数,所以**只能在均匀控制流里调**。
    // 原来这里是「先按 ID 决定要不要早返回,再采色带」,而 ID 本身是采出来的 ——
    // Dawn(Chrome)直接判整份 shader 非法,预览页一个像素都出不来。
    // naga 那边只当警告放过去了,所以桌面版一直没露馅,但那儿的 mip 选择同样是未定义的。
    // 无条件采两张,再拿采到的值决定要不要用:结果一样,而且到哪儿都合法。
    let scrolled = uv * material.flow.zw + vec2<f32>(material.flow.x, material.flow.y) * camera.time;
    let band = textureSample(noise_tex, base_sampler, scrolled).rgb;
    let id = textureSample(mask_id_tex, base_sampler, uv).a;
    // **只在 ID 遮罩选中的地方卷。** `MaskTex` 的 alpha 是离散的材质 ID 台阶,材质给的
    // `MaskID Min/Max` 划出该卷动的那一档:暮星辰环带是 0.72、额头与身体中央的黄装饰是 0.50,
    // 阈值 0.6~0.8 只选中环带。不门控就是黄装饰跟着在黄绿之间来回变(实机里它们是固定黄)。
    if material.mask_id.z > 0.5 && (id < material.mask_id.x || id > material.mask_id.y) {
        return albedo;
    }
    // **色带是黑的地方不混。** `FlowTexture` 这个槽位装的东西并不统一:暮星辰给的是一张
    // 青↔粉的**渐变色带**(`_Fx_D`),而水蓝蓝给的是一张 85% 全黑的**遮罩**(`_Fx_M`)——
    // 后者配上 `FlowPower = 1`,`mix(固有色, 色带, 1)` 直接把整只身体换成了黑,
    // 水蓝蓝/波波拉的触手就是这么黑掉的(实测暗像素占不透明区 14.5%、alpha 全是 1)。
    //
    // 汇编里那条链**从不替换固有色**:`MI_P_Object_UVFlow_*` 的流动贴图一路喂的是
    // 法线扰动与双层 UV 合成(`cb6[73]` 平铺 / `[74]` 偏移 / `[75]` 速度,两次采样再
    // `r4*(r7-1)`),不是拿来当颜色混的。完整复刻是另一件事(已记入待办),
    // 这里先只堵住「变黑」这一条 —— 它在任何读法下都是错的。
    let band_lit = max(band.r, max(band.g, band.b));
    // **是混色不是相乘。** 色带图本身就是成品颜色(青↔粉竖条纹),而基色图里环带那条是纯粉;
    // 相乘等于「粉 × 青」→ 出来是蓝,实机是真青。`FlowPower`(暮星辰 0.8)就是混色权重。
    return mix(albedo, band, material.extra.y * step(0.05, band_lit));
}

/// **`M_P_Object` 公共链上的加性流动层** —— 和上面那条「卷动色带」是两回事。
///
/// 读自波波拉 `_By` 的 quality=**Num** 排列(resource `0F1003EB…`,PS 49966 第 110~123 行);
/// 火系那条(PS 41058 第 160~177 行)是**逐指令相同**的一段,只是 cb 下标不同 ——
/// 所以这一层长在根图 `M_P_Object` 上,不是哪一族的专属件。
///
/// ```text
/// uv  = uv × (Flow_U_Tiling, Flow_V_Tiling) + frac(time × (Flow_U_Speed, Flow_V_Speed))
/// F   = pow(FlowTexture(uv).rgb, FlowPower) × FlowColor × FlowInt      ← ≤0 的通道取 0
/// vb  = 顶点色B + InverVertexColor × (1 − 2 × 顶点色B)
/// w   = m + `Inv Or Not` × (1 − 2m)        m = saturate((基色a − 0.04) × 1.1111)
/// 发光 += w × vb × F
/// ```
///
/// 三处值得记的:
///
/// - **`OpenRadialUV`**:打开时 UV 先换成极坐标 `(atan2(d)/2π 的小数部分, |d|)`,
///   `d = uv − 中心`。全库 10 份材质开着(小火苗一族在内)。汇编里那一大段多项式
///   就是 `atan2` 展开,不是什么别的东西。
/// - **`EmissContrast` 当 0 处理**:全库只有一份材质设过它、值还是 0,
///   `saturate(x × (2k+1) − k)` 于是化简成 `saturate` —— 而火系那条排列里连这步都没编进去。
/// - **过去把这一层读成「法线扰动」是从 Low 排列读的**(见 `flow_band` 的注释)。
///   实机跑 quality=Num,Num 里它进的是**加性发光层**。
fn uv_flow_layer(uv0: vec2<f32>, uv1: vec2<f32>, vertex_b: f32, m: f32) -> vec3<f32> {
    // `.w` = `FlowInt`;为 0 就是这一层不画(全库 33 份材质给了流动贴图,其中几份 FlowInt=0)。
    if material.uv_flow_color.w <= 0.0 {
        return vec3<f32>(0.0);
    }
    // 汇编第 75~76 行是 `lerp(v3.xy, v4.xy, saturate(UV Number))` —— 一个 UV 集选择器。
    // `saturate` 那个 opcode(0x18)是**按用途认出来的**:全库把所有排列的 preshader
    // 都译一遍,它只出现在 `UV Number`(56 次)与 `UseUV4`(9 次)两个参数上 ——
    // 两个都是 UV 集选择器,只有 `saturate` 讲得通(`UV Number = 2` ⇒ 取第二套)。
    // 实测佐证:波波拉的 UV1 只铺在 −0.26~0.38 那一小片,采到的流动贴图几乎全黑
    // (亮于 0.1 的顶点 0.1%),而 UV0 有 12.1% —— 用 UV0 会在身上糊出一片
    // 实机根本没有的紫(渲出来对着实机截图看过)。
    var src = mix(uv0, uv1, material.uv_flow_radial.z);
    if material.uv_flow_shape.w > 0.5 {
        let d = src - material.uv_flow_radial.xy;
        src = vec2<f32>(fract(atan2(d.y, d.x) * 0.15915494), length(d));
    }
    let scrolled = src * material.flow.zw
        + fract(camera.time * vec2<f32>(material.flow.x, material.flow.y));
    // **这张贴图可能是 sRGB 资源**,而运行时统一按 `Rgba8Unorm` 上传、没有硬件解码那一步 ——
    // 不自己解码,取到的值会大 4~5 倍,整层强度错一个量级。
    // **旗标只能逐材质查,不能按槽位一刀切**:火系的 `T_Fire_BJ_020` 是 sRGB,
    // 波波拉的 `T_Wat_ShuiLanLanBo_001_Fx_M` 不是(见导出器的 `Textures.IsSrgb`)。
    // 全库 26 份带这一层的材质里 20 份是 sRGB。
    let sampled = textureSample(noise_tex, base_sampler, scrolled).rgb;
    let raw = select(sampled, srgb_to_linear(sampled), material.uv_flow_radial.w >= 0.5);
    // 汇编是 `movc(raw <= 0, 0, exp(log(raw) × FlowPower))` —— 即「≤0 的通道直接取 0」。
    // 直接 `pow(0, p)` 在部分后端是 NaN,所以先夹再按原判据选。
    let shaped = select(pow(max(raw, vec3<f32>(1.0e-6)), vec3<f32>(material.uv_flow_shape.x)),
                        vec3<f32>(0.0), raw <= vec3<f32>(0.0));
    let vb = vertex_b + material.uv_flow_shape.y * (1.0 - 2.0 * vertex_b);
    let w = m + material.uv_flow_shape.z * (1.0 - 2.0 * m);
    return w * vb * shaped * material.uv_flow_color.rgb * material.uv_flow_color.w;
}

/// **同一条链上那圈菲涅尔发光**(PS 49966 第 128~149 行 / 火系 41058 第 178~197 行):
///
/// ```text
/// f    = pow(1 − saturate(N·V), FresnelExponent) × FresnelBoost
/// c    = f × FresnelColor × FresnelIntensity
/// g    = FresnelIntensity × (FresnelBaseMin − 1) + 1
/// 硬边 = smoothstep(0.99, 1, c.r × g) × HardLineCol × HardLineColMul
/// 发光 += lerp(硬边, c × g, FresnelSoftTohard)
/// ```
///
/// **`N` 是顶点法线,不是法线贴图扰动过的那个** —— 汇编第 130 行点的是 `v1.xyz`。
/// 全库只有 16 份材质设过 `FresnelIntensity`(8 份还设成 0),所以「强度 > 0」当门就够。
///
/// 那道「硬边」是给强度大的材质用的:波波拉代进去 `c.r × g` 的上界只有约 0.16,
/// 够不到 0.99,于是它那圈光化简成 `pow(1 − N·V, 8) × 0.94 × (0.087, 0.353, 1)`。
fn fresnel_layer(vertex_normal: vec3<f32>, view_dir: vec3<f32>) -> vec3<f32> {
    if material.fresnel.w <= 0.0 {
        return vec3<f32>(0.0);
    }
    let f = pow(max(1.0 - saturate(dot(vertex_normal, view_dir)), 1.0e-4),
                material.fresnel_shape.x) * material.fresnel_shape.y;
    let c = f * material.fresnel.rgb * material.fresnel.w;
    let g = material.fresnel.w * (material.fresnel_shape.z - 1.0) + 1.0;
    let hard = smoothstep(0.99, 1.0, c.r * g) * material.fresnel_hard.rgb * material.fresnel_hard.w;
    return mix(hard, c * g, material.fresnel_shape.w);
}

/// **幻星族那两颗球的菲涅尔换色层**(`MI_P_Object_Trans_XingGuang_Fresnel`)。
///
/// 全库 3393 份材质里只有**暮星辰 `_Fx2`** 一份用它 —— 也正是用户说「两颗球颜色差距最大,
/// 实机一个偏紫黑、一个偏粉紫」的那两颗。目标 PS **53466**(`Num/lod=0/dsid=0`,
/// resource `6CCB83FD…`)第 151~209 行:
///
/// ```text
/// fres = pow(max(1 − max(N顶点·V, 0), 1e-4), Range) × 0.96 − 0.46
/// t    = smoothstep(saturate(fres / (Soft × 0.1))) × Int
/// col  = UseVertexColorG ≥ 0.5 ? lerp(Color02, Color, 顶点色G) : Color
/// w    = lerp(已有不透明度, saturate(t), BottomLayer/TopLayer Opacity)
/// w    = lerp(w, saturate(t), UseOpacityMask)
/// w    = w + InversionMask × (1 − 2w)
/// 发光 = lerp(发光, t × col, OpenEmissiveBlend × w)     ← **替换**,不是相加
/// 不透明度 += OpenOpacityAdd × w                        ← 汇编第 304 行
/// ```
///
/// **两颗球的差别全在顶点色 G**:它们绑在两根不同的骨骼上(`Bone_Qhuan_M_00` 那颗 G=0、
/// `Bone_Qhuan_M_03` 那颗 G=1),而 UV / 遮罩 / 基色三样**完全一样**(逐顶点量过)。
/// 于是一颗取 `Color`(0.148, 0.059, 0.22 深紫)、另一颗取 `Color02`(0, 0.562, 1.5 青)。
/// 我们原来两颗都是黑的,就是因为这一层根本没画。
///
/// 那两个字面量 `0.96 / −0.46` 是汇编里折叠好的常数(`mad r2.w, r2.w, 0.96, -0.46`),
/// 不是参数;`cb6[12]/cb6[13] ↔ Color/Color02` 按 `vector-slot` 字节码定
/// (`vector-slot[12] = 04 06 00 …` ⇒ vector-param[6] = `Color`),`v2 = COLOR0` 由 ISGN 查实。
///
/// **`N` 用的是顶点法线**(汇编点的是 `v1 = TEXCOORD11`),不是法线贴图扰动过的那个。
fn xing_fresnel_coverage(vertex_normal: vec3<f32>) -> f32 {
    let ndv = max(dot(vertex_normal, view_direction()), 0.0);
    let shaped = pow(max(1.0 - ndv, 1.0e-4), max(material.family2.x, 1.0e-4)) * 0.96 - 0.46;
    let c = saturate(shaped / max(material.family2.y * 0.1, 1.0e-4));
    return c * c * (3.0 - 2.0 * c) * material.family0.w;
}

/// **火系族(`MI_P_Object_Fire*`)在同一个发光累加器上多的两层。**
///
/// 读自火神 `_By` 的 quality=**Num** 排列(resource `041D1E47…`,PS 41058 第 68~122 行,
/// `V=64 / S=75`,cb 槽位逐格读出来的):
///
/// ```text
/// base = toneInv(BaseTex.rgb)                       ← 与通用链同一条反色调映射
/// 层1  = base × lerp(Color1, Color2, pow(max(N·V,0), FresnelPower)) × FresnelInt
/// t    = saturate((pow(max(1 − max(N·V,0), 1e-4), Range) × 0.96 − 0.46) / (Soft × 0.1))
/// 色2  = UseVertexColorG ? lerp(Color02, Color, 顶点色.g) : Color
/// 层2  = base × 色2 × t²(3−2t) × Int
/// 两层各自 lerp(层, m × 层, `Use Opacity as Mask`)
/// ```
///
/// **这两层是加性发光,不是固有色** —— 汇编第 199 行把它们并进发光累加器 `r6`,
/// 而基色 `r5` 另走一路。这一点很容易读错:链子开头就是 `toneInv(BaseTex) × 颜色`,
/// 看着像在改固有色。
///
/// 火神代进去:`FresnelInt = 0` ⇒ **层1 整个为零**;`Range = 0` ⇒ `pow(x, 0) = 1`,
/// 那条带化简成恒 1 ⇒ 层2 = `toneInv(基色) × (1.2, 0.825, 0) × 0.4`,一层均匀的橙色自发光。
///
/// `N` 取**顶点法线**(汇编第 86 行点的是 `v1.xyz`),和菲涅尔那层一样。
/// 那个 `Soft × 0.1` 来自 preshader:`cb6[66].y = 0.5 + Soft × 0.1`,汇编再减 0.5。
fn fire_layers(vertex_normal: vec3<f32>, base_tex: vec3<f32>, vertex_g: f32, m: f32) -> vec3<f32> {
    if material.fire_shape.w < 0.5 {
        return vec3<f32>(0.0);
    }
    let base = game_tonemap_inverse(srgb_to_linear(base_tex));
    let ndv = dot(vertex_normal, view_direction());
    // 第 87、93 行:`N·V <= 0` 时那一支直接顶成 0(不然 log(负数))。
    let lit = max(ndv, 0.0);
    let f1 = select(pow(max(ndv, 1.0e-6), material.fire1.w), 0.0, ndv <= 0.0);
    var layer1 = base * mix(material.fire1.rgb, material.fire2.rgb, f1) * material.fire2.w;
    // 第 100~113 行那条带。`Soft` 为 0 时分母是 0 → ±inf → saturate 出硬阶跃,
    // 和沙漏那条一样用 `max(…, 1e-6)` 取同样的极窄过渡且不产生 NaN。
    let inv = max(abs(1.0 - lit), 1.0e-4);
    let raw = pow(inv, material.fire_shape.x) * 0.96 - 0.46;
    let t = saturate(raw / max(material.fire_shape.y * 0.1, 1.0e-6));
    let band = t * t * (3.0 - 2.0 * t) * material.fire3.w;
    let tint2 = select(material.fire3.rgb,
                       mix(material.fire4.rgb, material.fire3.rgb, vertex_g),
                       material.fire4.w >= 0.5);
    var layer2 = base * tint2 * band;
    // 第 97~98、120~121 行:两层各自按 `Use Opacity as Mask` 决定要不要再乘一遍基色 alpha 的遮罩。
    layer1 = mix(layer1, layer1 * m, material.fire_shape.z);
    layer2 = mix(layer2, layer2 * m, material.fire_shape.z);
    return layer1 + layer2;
}

/// glTF 导出把 UE `(X,Y,Z)` 换成运行时 `(X,Z,Y)`；材质里的三平面采样仍须按 UE 轴序。
fn runtime_to_ue(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(v.x, v.z, v.y);
}

/// HLSL/WGSL `refract(I,N,eta)` 展开式。显式写出是为了和两条 DXBC 的 k<0 清零分支一致。
fn refract_direction(incident: vec3<f32>, n: vec3<f32>, eta: f32) -> vec3<f32> {
    let ni = dot(n, incident);
    let k = 1.0 - eta * eta * (1.0 - ni * ni);
    if k < 0.0 {
        return vec3<f32>(0.0);
    }
    return eta * incident - (eta * ni + sqrt(k)) * n;
}

/// **玻璃内部那颗星。** 实机是这么做的(读 `MI_P_Object_Trans_MatCap` 的 pixel shader 汇编,
/// 见 docs/findings.md §1):把视线按 `GlobalRefraction`(=1.3)折射进物体内部,沿折射光线
/// march 一段(`GlobalDepth`),在**模型空间**按三向投影采 `StarTex`(= `T_EMeng003`,
/// 一张四角星场、alpha 是干净的稀疏星形遮罩),采样坐标再叠上时间卷动。
///
/// 于是球看着像「里面飘着一颗星」,而且那颗星**自己在动、与球的自转无关** —— 正是实机观感。
/// 这一层只给玻璃族(静态开关 `是否使用MatCap` 开着的那 17 个材质)。
///
/// **返回强度(标量),不带颜色。** 汇编里星场的采样结果是个标量,颜色是另外几个 cb 槽
/// 给的(`星点底色 + 强度 × 星点亮色`,再与按高度 lerp 的那对颜色混)。分开才对得上。
///
/// **是近似不是复刻**:游戏那边还有第二张三向投影贴图、两段 `pow` 相位曲线、以及那对按
/// 高度做的渐变色;这里只取「折射 + 三向投影星场 + 时间」这条主干。
/// 卷动速度实机是个 cb 里的向量参数,而 cb 槽位与参数名的对应还没解出来(§1),
/// 所以先用一个定值。
fn interior_star(start: vec3<f32>, n: vec3<f32>, forward: vec3<f32>) -> f32 {
    if material.interior.w < 0.5 {
        return 0.0;
    }
    // 顶点 shader 的配套写入已经查实：start 是预蒙皮局部位置，不是 UV1/UV2。
    // 换回 UE 轴序后再执行原材质的折射与三平面采样。
    //
    // **这一层离对上实机还很远**:实机那两颗球是「红球 + 居中的大号黄色四角星/圆点」,
    // 我们画出来只有几点很淡的紫斑(换成 UV1/UV2 起点那版连斑都没有)。
    // 起点之外,着色那一路缺的槽位更多 —— 见 docs/design.md 的待办表。
    let start_ue = runtime_to_ue(start);
    let n_ue = normalize(runtime_to_ue(n));
    let forward_ue = normalize(runtime_to_ue(forward));
    // eta 取 1/折射率(空气 → 介质)
    let eta = 1.0 / max(material.interior.x, 0.001);
    let dir = refract_direction(forward_ue, n_ue, eta);

    // **march 距离与平铺照汇编算,不再手挑。** 汇编(fx1/34529.asm 63..78):
    //   halfExtent = 0.5 * |包围盒尺寸|
    //   marchDist  = halfExtent * 0.01 * GlobalDepth        ← 代 100 进去正好 = halfExtent
    //   tiling     = <一个 cb 标量> / halfExtent            ← 那个标量取 1(中性),名字未解出
    //   p = (start + 折射方向 * marchDist) * tiling
    // 于是 p = start/halfExtent + 折射方向 —— 折射方向是单位向量,所以每颗球看到的是
    // 星场里以某点为心、约一格大小的一块,这正是「每颗球稳定居中一颗星」的机制。
    let half_extent = 0.5 * length(material.bounds_size.xyz);
    let march = half_extent * 0.01 * INTERIOR_DEPTH;
    let p = (start_ue + dir * march) * (INTERIOR_UV_SCALE / max(half_extent, 0.0001));

    // **三向投影不是「归一化权重加权和」,是两次嵌套 lerp。** 汇编(34529,83..88):
    //   k    = saturate(|n| * (2*StarTriPlannarBlendInt + 1) - StarTriPlannarBlendInt)
    //   s    = lerp(sample(p.xz), sample(p.yz), k.y)
    //   s    = lerp(s,            sample(p.xy), k.w)
    // 原来那版是 `pow(|n|, B)` 再归一化 —— 结构就不对(而且更早还写死次数 8,
    // 那让权重几乎完全偏向单一轴)。
    let blend = saturate(abs(n_ue) * (2.0 * STAR_TRIPLANAR_BLEND + 1.0) - STAR_TRIPLANAR_BLEND);
    let s0 = textureSample(interior_tex, base_sampler, p.xz);
    let s1 = textureSample(interior_tex, base_sampler, p.yz);
    let s2 = textureSample(interior_tex, base_sampler, p.xy);
    let s = mix(mix(s0, s1, blend.y), s2, blend.z);

    // **星点不是在移动、是在闪。** 汇编(同上 89..106):
    //   phase = frac(FlickerSpeed * 时间 + 星场.G)      ← 每颗星的相位来自 G 通道
    //   闪    = -1.2 * |sin(2π * phase)|^FlickerPower   ← 注意是**减**
    //   形状  = pow(星场.B, q)                          ← q 是未解出的 cb 标量,取 1
    //   强度  = saturate((形状 + 闪) * 星场.A * 强度)
    // 通道语义与贴图实测一致(T_EMeng003:G 均 0.328 且分散 = 相位;B 87% 为零 + 稀疏亮核
    // = 形状;A 是星形遮罩)。所以星是一明一暗地闪,而不是整片飘 —— 原来那版按时间卷动 UV
    // 是**猜的**,那会让星在球里滑动。
    let phase = fract(material.interior.z * camera.time + s.g);
    let twinkle = -1.2 * pow(abs(sin(phase * 6.28318548)), material.interior_color.w);
    return saturate((s.b + twinkle) * s.a * INTERIOR_GAIN);
}


/// **水体预设(`MI_P_Object_Water_NoMetal`)。** 逐指令来自 PS **16335**
/// (水灵 `_Fx` 的 `quality=Num / lod=0 / dsid=0`,resource `AC743E86…`)第 62~118 行。
///
/// ```text
/// a        = saturate((基色a − 0.04) × 1.1111)
/// mask     = 1                                       ← Inv Opacity = 0 ∧ UseOpacityAsMask = 0
/// cauUV    = uv × 平铺C + frac(时间 × 速度C)
/// c1       = caustics(cauUV).g × CausticsInt
/// flowUV   = uv × 平铺F + (frac(时间×速度F.u), frac(时间×速度F.v))
/// d        = caustics(uv).a × FlowDistort              ← 注意这一次采的是**未卷动**的 uv
/// c2       = caustics(flowUV + d × 0.5).r
/// 层一     = (c1 × c2 + c2) × 0.5 × Main Color × mask
/// 层二     = 反色调映射(基色) × lerp(Color1, Color2, pow(saturate(N·V), FresnelPower)) × FresnelInt
/// glow    += 自发光 × Emitter Intensity + 层一 + 层二
/// ```
///
/// **这一层以前被判成「实机一层都不画」并撤回过**(见待办里那条)。那个结论来自
/// shader 35663 的 `r4 × (1 − r2.y)`,而那是**另一份排列**;实机默认那份
/// (`lod=0 / dsid=0`)里根本没有那道门 —— 同一个「挑错排列」的坑,这本子里第四次。
///
/// **两处名字是反的**,按槽位不按名字:`FresnelInt`(cb6[59].w)是**增益**、
/// `FresnelPower`(cb6[59].z)是**指数**。slot ↔ 参数的对应是从
/// `PROBE_SHADER_DETAILS` 的 `scalar-slot` 编码读的,不是数出来的
/// (slot14/15 与 slot17/21 都跟参数序不一致)。
fn water_layer(uv: vec2<f32>, base_rgb: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    if material.family6.x < 0.5 {
        return vec3<f32>(0.0);
    }
    let color1 = material.family0;
    let color2 = material.family1;
    let main_color = material.family2;
    let cau = material.family3;
    let flw = material.family4;
    let shape = material.family5;

    // **caustics 那张是 sRGB 资源**(`main_color.w`),而运行时统一按 `Rgba8Unorm` 上传、
    // 没有硬件解码那一步 —— 不解码整层强 **9 倍**(实测把水灵的亮度比从 0.85 顶到 1.15)。
    let srgb = main_color.w >= 0.5;
    let cau_uv = uv * cau.xy + fract(camera.time * cau.zw);
    let s_cau = textureSample(noise_tex, base_sampler, cau_uv);
    let c1 = select(s_cau.g, srgb_to_linear(s_cau.ggg).g, srgb) * shape.x;
    let flow_uv = uv * flw.xy
        + vec2<f32>(fract(camera.time * flw.z), fract(camera.time * flw.w));
    // 这一次采的是**未卷动**的 uv(汇编第 84 行),用 alpha 通道当扰动量。
    // alpha 不是颜色,**不解码**。
    let d = textureSample(noise_tex, base_sampler, uv).a * shape.y;
    let s_c2 = textureSample(noise_tex, base_sampler, flow_uv + vec2<f32>(d * 0.5));
    let c2 = select(s_c2.r, srgb_to_linear(s_c2.rrr).r, srgb);
    let caustics = (c1 * c2 + c2) * 0.5;
    let layer1 = caustics * main_color.rgb;

    // 汇编:`r7 = (Color2 − Color1) × pow(N·V, FresnelPower)`,`N·V ≤ 0` 时那一项取 0,
    // 再 `+ Color1` —— 即「背面只剩 Color1」。
    let ndv = dot(n, view_direction());
    let k = select(0.0, pow(max(ndv, 1.0e-6), shape.w), ndv > 0.0);
    let tint = (color1.rgb + (color2.rgb - color1.rgb) * k) * shape.z;
    let layer2 = game_tonemap_inverse(base_rgb) * tint;
    return layer1 + layer2;
}
