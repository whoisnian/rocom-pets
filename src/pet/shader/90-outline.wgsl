// 描边那一遍:五档颜色、MatID、逐档高光、法线图、炫彩描边
//
// **这份文件不是独立的着色器** —— `src/pet/gpu.rs` 用 `concat!(include_str!(…))`
// 把 `src/pet/shader/*.wgsl` 按文件名顺序拼成一个模块再交给 wgpu。WGSL 的模块级条目
// 与顺序无关,所以拆分只是为了好读;**加新文件记得同步 gpu.rs 里那张 `concat!` 清单**。

@fragment
fn fs_outline(in: VsOut) -> @location(0) vec4<f32> {
    cull_face_card(in);
    let tex = textureSample(base_color, base_sampler, in.uv);
    // 只有镂空遮罩才剔;本体的线条遮罩要是拿来剔,描边壳会跟着被啃掉
    if material.params.x > 0.5 && tex.a < 0.35 {
        discard;
    }
    // 描边取基色的暗版而不是纯黑,卡通渲染里这样更自然。
    //
    // **⚠ 下面这条 0.80 是在描边粗了 4.6 倍的时候标出来的,别再拿它当准。**
    // 宽度后来按描边 VS 的公式改对了(见 exporter/Materials.cs 的 `OutlineWidthOf`),
    // 「描边环 ÷ 主体」那个比值随之从 1.01 走到 1.06 —— 也就是当年是拿颜色补宽度的账。
    // 实机的描边色是 `OutLineOtherColor1..5` 按 `MatID` 分 5 档挑再乘 `Outline Intensity`
    // (描边 PS 58499 第 32~41 行),都接近黑;要换成那套得连 `MatID` 贴图一起导,
    // 单独一轮。在 0.39 厘米这个宽度上差别很小,而桌宠要在任意背景上认得出轮廓。
    //
    // **0.25 → 0.55 是量出来的**(2026-07-28,17 只有实机截图的宠物全覆盖)。做法:把两边的
    // 不透明遮罩各腐蚀 2 像素,分成「描边环」与「主体」,比 `中位(环)/中位(主体)`。
    // 原来我们的描边环只有实机的 **0.50** 倍亮(即深得多);0.70 抬到 0.98,整只的
    // 跨度比也从 1.14 降到 1.03。**先取过 0.55(留一档辨识度),后来取景守卫修好后重标,0.70 才落在 1.00**
    // —— 桌宠要在任意背景上认得出轮廓,所以留一档。
    //
    // **这条是从一次差点走偏的排查里捞出来的**,过程值得记:先量到「我们整体比实机更花」
    // (跨度比 1.29),差点去调 `AMBIENT`;做了两步分解才找对地方 ——
    // ① 抗锯齿对照(渲 4 倍再缩)只把比值从 1.54 拉到 1.47,**不是走样造成的**;
    // ② 换成对边缘不敏感的统计量,`p75−p25` 的比值只有 **1.05** —— 身体主体的对比其实是对的,
    //    超出的全在暗尾;腐蚀 2 像素后暗端比从 0.75 跳到 0.95,**暗尾就是这条描边**。
    // 所以调 `AMBIENT` 会是错的:它会为了掩盖描边而把整只的明暗压平。
    //
    // **实机侧的抠图也踩过一次**:按「与角落背景色的距离」判,好几张截图里宠物很小、
    // 背景是带花纹的卡片,会把大片背景算成宠物(菊花梨的「色偏」因此虚高到 1.46,
    // 修正后只有 0.16)。判据要加两道:**取最大连通块**、面积占比 > 55% 视为抠图失败。
    // **不是每个材质绑在 `base_color` 上的都是固有色贴图。** 四个专用族里绑的分别是
    // MatCap 查找表 / 泡泡图 / 液面遮罩 / 全库共享的图案图 —— 按网格 UV 采出来是一块
    // 与外观无关的乱色。其中走不透明通道、因而真的会画描边的是下面两族;
    // 耳膜(special_opaque)与液面(混合通道)本来就不进这一遍。
    var albedo = tex.rgb;
    if material.family_flags.w > 0.5 {
        // `M_P_MatCap_Masked`:克莱因龙的玻璃壳。整只泡泡原来罩着一层黑,就是这一遍 ——
        // 外扩的背面壳用 `MatCap34` 按网格 UV 采出近黑色,而正面壳按遮罩 discard 掉了,
        // 于是背面壳直接露出来铺满整个泡泡。这里补上同一条遮罩(与 depth PS 15293 一致),
        // 描边底色改用材质自己的 BaseColor。
        let n = normalize(in.normal);
        let ndv = saturate(dot(n, view_direction()));
        let matcap = srgb_to_linear(
            textureSample(base_color, base_sampler, matcap_uv(n)).rgb);
        let matcap_luma = dot(matcap, vec3<f32>(0.3, 0.59, 0.11));
        let fresnel = pow(max(1.0 - ndv, 1e-4), max(material.family5.w, 1e-4));
        if max(matcap_luma, fresnel) < 0.3333 {
            discard;
        }
        albedo = encode_linear_color(material.family0.rgb);
    } else if material.family_flags.x > 0.5 {
        // `MI_P_Object_XiaoYou`:绑的是 `Tex_PetGlassy_007_D`(红/绿平铺图案),
        // 描边因此在小灵面身上镶了一圈橙绿。改用与 `shade_xiaoyou` 同一条固有色。
        albedo = encode_linear_color(mix(material.xiaoyou_base1.rgb,
                                         material.xiaoyou_base2.rgb,
                                         saturate(1.0 - in.color.g)));
    }
    // 有五档颜色就走实机那条(近黑),没有(旧包)才退回「固有色压暗」。
    let id = outline_mat_id(in);
    let base = select(albedo * 0.80, outline_ramp_color(id), material.outline.y > 0.5);
    return vec4<f32>(mix(base, glassy_outline(in), glassy_outline_zone(id)), 1.0);
}

/// **逐 `MatID` 的高光**(`M_P_Object` 的 quality=Num 排列独有,PS 8409 第 192~221 行)。
///
/// 返回的是**乘性加亮量**:调用处写 `body *= 1 + matid_specular(...)`。汇编里这两项共用
/// 同一个光照因子 —— `r13 = 基色 × 光照 + 基色 × 光照 × Int × SpecColor × v × edge`,
/// 提出来就是 `基色 × 光照 × (1 + …)`。所以**不是加一层白光**,是把已着色的颜色整体推亮。
///
/// ```text
/// 挡位 = floor(min((1 − MatID) × 5 + 1, 5))          // 和描边同一张图同一套刻度
/// (Pow, Int, R) = 挡位 1 ? (0.35, 0.001, 0.5)        // 第 1 档是汇编里硬写的立即数
///                        : (SpecPow_n, SpecIntensity_n, SpecRadius_n)
/// α    = Pow²
/// D    = min((α / ((N·H)²(α² − 1) + 1))², 2048)      // GGX,少了 1/π
/// k    = 0.25 × Pow + 0.25
/// v    = lerp(saturate(k·D), k·D, R²)
/// edge = saturate((saturate((k·D + 0.5) × 0.5) − (0.49 − R)) / (2R))
/// ```
///
/// `R = 0` 时 `edge` 退化成 `k·D > 0.48` 的**硬阶跃**(友爱星飞身上那种硬边高光块);
/// `R = 1` 时是一片宽而柔的加亮(蛋煲蛋)。第 1 档那个 `0.001` 让「美术没设过的部位」
/// 实际等于关闭(代进去加亮量 0.006)。
///
/// **全库 2539 份材质里只有 213 份真的开着**(四档 `SpecIntensity` 有一档非 0);
/// 早先按 120 个资产的抽样得出的「只有 2 份」是样本太小,已更正。
///
/// `MatID` 走 `glassy_id_tex` 那一路 —— 它和炫彩区域门是**同一张图同一个通道**
/// (`MaskTex` 的 alpha,那张图同时是法线图),见 gpu.rs 里那条上传规则。
/// 采样无条件做:一致控制流。
fn matid_specular(uv: vec2<f32>, n: vec3<f32>) -> vec3<f32> {
    let id = textureSample(glassy_id_tex, base_sampler, uv).a;
    let slot = i32(floor(min((1.0 - id) * 5.0 + 1.0, 5.0)));
    // 第 1 档不在 uniform 里:汇编把它编成了立即数
    let s = select(material.spec_slots[clamp(slot, 2, 5) - 2].xyz,
                   vec3<f32>(0.35, 0.001, 0.5), slot <= 1);
    let pow_ = s.x;
    let intensity = s.y;
    let radius = s.z;
    let h = normalize(view_direction() + normalize(camera.light_dir));
    let ndh = saturate(dot(n, h));
    let a = pow_ * pow_;
    let denom = ndh * ndh * (a * a - 1.0) + 1.0;
    let d0 = a / max(denom, 1e-6);
    let d = min(d0 * d0, 2048.0);
    let k = 0.25 * pow_ + 0.25;
    let kd = k * d;
    let v = mix(saturate(kd), kd, radius * radius);
    // `radius = 0` 时汇编那条 `div_sat` 就是 x/0 —— ±inf 被 saturate 夹成 1/0,
    // 也就是在 0.49 处的**硬阶跃**。WGSL 的除零是实现定义的,所以把分母垫一个极小量:
    // 结果一样是阶跃,但不依赖 inf 的行为。
    let edge = saturate((saturate((kd + 0.5) * 0.5) - (0.49 - radius))
                        / max(2.0 * radius, 1e-6));
    return select(vec3<f32>(0.0),
                  material.spec_color.rgb * intensity * v * edge,
                  material.spec_color.w > 0.5);
}

/// **法线图**:`MaskTex` 的 RG 是切线空间法线的 xy,z 由 `sqrt(1 − x² − y²)` 补出。
/// PS 8409 第 44~52 行读它、第 157 行拿它算主光照的 `N·L` —— 我们一直只用了这张图的 alpha。
/// 抽 14 张 `_By_M` 量过,**14 张全都带真实扰动**(nx/ny 的 p2~p98 到 ±0.3~±0.7)。
///
/// **切线基从屏幕空间导数反解,不走顶点切线。** 两个理由:
///
/// 1. glb 里那份 `TANGENT` 的 **w(副切线符号)是坏的** —— CUE4Parse 每个网格只写一个值
///    (抽 8 只:7 只全 +1、1 只全 −1),而拿网格自己的 位置+UV 重算出来的副切线方向,
///    与 `cross(N, T) × w` 的**同向率只有 0.00~0.61**。也就是说镜像 UV 那半边会整片翻掉
///    (角色左右对称,镜像 UV 很常见)。方向本身是对的(`dot(存的T, UV推的T)` 中位 +0.996),
///    坏的只有符号。
/// 2. 导数法(Mikkelsen 的 cotangent frame)对镜像 UV **自动正确**,而且不用改顶点格式、
///    不用改导出器。代价是切线基逐三角常量 —— 但 `N` 仍是插值来的平滑法线,
///    重正交之后接缝上看不出来。
///
/// 汇编里最后还有一步 `lerp(几何法线, 贴图法线, cb0[149].w)`,那是 View 常量、读不到;
/// 而**主光照那一路用的是没 lerp 的贴图法线**(第 150~157 行),所以这里也不 lerp,
/// `normal_map.y` 留作强度旋钮(默认 1)。
fn mapped_normal(in: VsOut, n: vec3<f32>) -> vec3<f32> {
    let t = textureSample(glassy_id_tex, base_sampler, in.uv).rg * 2.0 - 1.0;
    let dp1 = dpdx(in.world_pos);
    let dp2 = dpdy(in.world_pos);
    let duv1 = dpdx(in.uv);
    let duv2 = dpdy(in.uv);
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let tan = dp2perp * duv1.x + dp1perp * duv2.x;
    let bit = dp2perp * duv1.y + dp1perp * duv2.y;
    let inv = inverseSqrt(max(max(dot(tan, tan), dot(bit, bit)), 1e-20));
    let xy = t * material.normal_map.y;
    let nz = sqrt(max(1.0 - dot(xy, xy), 0.0));
    let mapped = tan * (xy.x * inv) + bit * (xy.y * inv * NORMAL_MAP_GREEN) + n * nz;
    let ok = material.normal_map.x > 0.5 && dot(mapped, mapped) > 1e-12;
    return select(n, normalize(mapped), ok);
}

/// 描边那一遍挑档用的 `MatID`。**优先用 `_Ol` 自己那张** —— 描边材质有自己的 `MatID` 槽,
/// 854 份里 38 份指着与本体 `MaskTex` 不同的图、81 份本体压根没有这个槽。
/// 旧包(`outline.y = 0`)没导这张,退回炫彩那道门用的 `_M`,保持老行为。
///
/// **两张都无条件采**:采样带隐式导数,只能在一致控制流里调(和 `glassy_layer` 里
/// 踩过的那条同一个道理),所以是 `select` 而不是 `if`。
fn outline_mat_id(in: VsOut) -> f32 {
    let own = textureSample(outline_id_tex, base_sampler, in.uv).a;
    let fallback = textureSample(glassy_id_tex, base_sampler, in.uv).a;
    return select(fallback, own, material.outline.y > 0.5);
}

/// 描边的颜色。**实机是五档的**,按 `MatID` 遮罩挑,不是「固有色压暗」——
/// 描边 PS 58499(鸭吉吉 `_By_Ol` 的 quality=Num / LOD0 / DSId=0 排列)第 32~41 行:
///
/// ```text
/// 挡位 = floor(min((1 − MatID.a) × 5 + 1, 5))     // MatID=1 → 1 档,MatID=0 → 5 档
/// out  = OutLineOtherColor[挡位] × Outline Intensity
/// ```
///
/// 汇编里这之后还有三步,**全库都是恒等的**,所以导出器不传:
/// 混 `Flat_EmissiveColor`(权重 `Flat_EmissiveRatio` = 0 × 851)、混 `SelectionColor`
/// (引擎的编辑器选中色,打包后 alpha = 0)、以及乘 `saturate(2 × 灯色亮度)`
/// (权重 `OutlineIgnoreEnvColor` = 1 × 850,名字的意思是「只吃环境光的**亮度**、
/// 不吃它的颜色」;白光下那个 saturate 就是 1,我们没有等价量,取 1)。
///
/// 挡位与炫彩那道门是**同一张图同一套刻度**:`MinID` = 0.4 ⇒ 炫彩区正好是 1~3 档,
/// 4/5 档是喙、脚这类非炫彩部位。所以第 5 档最常被美术改(全库 597 种不同的值),
/// 而 1~4 档大多留着父材质那个近黑的紫(0.0144, 0.0056, 0.0356)。
///
/// **颜色是从实机截图上量得到的**:加益边界处有 1~2 个像素明显低于背景(0.50)与身体
/// (0.74)的暗环,最低到 0.265 —— 「固有色 × 0.80」那条**永远画不出**这个凹陷。
fn outline_ramp_color(id: f32) -> vec3<f32> {
    let slot = i32(floor(min((1.0 - id) * 5.0 + 1.0, 5.0)));
    return encode_linear_color(material.outline_ramp[clamp(slot, 1, 5) - 1].rgb);
}

/// 炫彩那一遍描边该不该换色。**`_Ol` 也有一条 `GlassySwitch` 排列** ——
/// lua 的 `processAdditionalMaterial` 往本体材质的 `AdditionalMaterials`(就是那份 `_Ol`)
/// 上写 `GlassySwitch=true` + 两个 Channel 色,所以描边跟着一起变。
///
/// 门与本体那条**是同一个阈值、同一套刻度**(`MinID` = 0.4);图取 `_Ol` 自己的 `MatID`
/// (见 `outline_mat_id` —— 它与本体的 `MaskTex` 大多数时候是同一张,但不总是):
/// 汇编 `ge r0.w, r0.w, cb3[34].y` 之后 `mad r1.xyz, r0.w, (glass − 描边色), 描边色`
/// —— 门外仍是 `OutLineOtherColor × Outline Intensity` 那条老路。
///
/// **赛季那一族(`glassy_red.w = 2`)不走这里**:lua 对它先找描边材质上的
/// `MutationSwitch`,找到了就只开那个、不写 Channel 色(`bFoundMutationSwitch`)。
///
/// **门只当值用,不当分支用** —— 和 `glassy_layer` 里那条踩过的坑同一个道理:
/// 采样带隐式导数,只能在一致控制流里调,提前 return 会让浏览器那边整份 shader 拒编。
/// (贴图采样本身已经挪到 `outline_mat_id`,这里只收那个值。)
fn glassy_outline_zone(id: f32) -> f32 {
    let on = material.glassy_red.w > 0.5 && material.glassy_red.w < 1.5;
    return select(0.0, 1.0, on && id >= GLASSY_MIN_ID);
}

/// 描边那条 `GlassySwitch` 排列的颜色(PS 52499 第 40~52 行,鸭吉吉 `_By_Ol` 的
/// LODUsed=0 / DSId=1 那份)。比本体那条短得多 —— **没有折射、没有屏幕空间 UV、
/// 没有星贴层、也不按固有色亮度调制**,就是拿网格 UV0 采一次共享花纹图再混两个 Channel 色:
///
/// ```text
/// uv    = UV0 × GlassyUV.xy + frac(time × GlassyUV.zw)
/// glass = lerp(lerp(RedChannel × t.r, GreenChannel, t.g), BlueChannel, t.b)
/// ```
///
/// **注意是 lerp 不是本体那条加法**(本体是 `Red × t.r + Green × t.g` 再乘 1.62 的增益):
/// 汇编里 `mad r3.xyw, r3.y, (Green − Red×t.r), Red×t.r` 写得很清楚,而且这一支
/// **一个增益都不乘**。所以描边环比本体略暗一点,实机也是这样。
fn glassy_outline(in: VsOut) -> vec3<f32> {
    let uv = in.uv * GLASSY_OUTLINE_UV.xy + fract(camera.time * GLASSY_OUTLINE_UV.zw);
    let t = textureSample(glassy_outline_tex, base_sampler, uv).rgb;
    var glass = mix(material.glassy_red.rgb * t.r, material.glassy_green.rgb, t.g);
    glass = mix(glass, GLASSY_OUTLINE_BLUE, t.b);
    // 和玻璃层同一条换算:游戏那边这个值直接进 `sqrt(x × 曝光)` 的尾段输出,
    // 而这一遍我们是**直接写显示空间**的,所以先除掉 `EXPOSURE` 再走同一个编码
    // —— 两处必须一致,否则描边环与本体差一整档,反倒比原来的原色描边更显眼。
    return encode_linear_color(glass / EXPOSURE);
}
