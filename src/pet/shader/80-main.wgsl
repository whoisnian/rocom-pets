// 主着色链 `shade_main`,以及本体 / 玻璃 / 玻璃球内胆三个片元入口
//
// **这份文件不是独立的着色器** —— `src/pet/gpu.rs` 用 `concat!(include_str!(…))`
// 把 `src/pet/shader/*.wgsl` 按文件名顺序拼成一个模块再交给 wgpu。WGSL 的模块级条目
// 与顺序无关,所以拆分只是为了好读;**加新文件记得同步 gpu.rs 里那张 `concat!` 清单**。

fn shade_main(in: VsOut, depth_coverage: f32) -> vec4<f32> {
    if material.family_flags.x > 0.5 {
        return shade_xiaoyou(in);
    }
    if material.family_flags.y > 0.5 {
        return shade_yutu_ear(in);
    }
    if material.family_flags.z > 0.5 {
        return shade_fake_fluid(in, depth_coverage);
    }
    if material.family_flags.w > 0.5 {
        return shade_matcap_masked(in);
    }
    if material.family11.w > 0.5 {
        return shade_fairy_ball(in);
    }
    // 眼神:脸那两个槽的贴图各是一张 2×4 的图集,网格 UV 落在左上那一格,
    // 换眼神就是整格地偏一下 —— 眼和嘴各偏各的,见 `face_uv_offset`。
    let uv = in.uv + face_uv_offset();
    let tex = textureSample(base_color, base_sampler, uv);
    // **alpha 有三种含义,由材质决定**(params.x / params.z):
    // - 镂空遮罩(眼/嘴的眼神图集,params.x):按阈值剔,不剔就是一块方糊;
    // - **不透明度**(params.z,静态开关 `Opacity or OpacityMask` 点名的 11 个材质);
    // - 线条遮罩(其余本体):RGB 是完整固有色,alpha 里画着身上的纹路(水灵的竖条纹就在这儿)。
    //   这种**绝对不能拿来剔像素**——本体贴图的 alpha 覆盖率普遍很低(813 张里 60 张 <5%),
    //   剔了就只剩眼睛(火花)甚至整只消失(迪莫)。要做的是照着它提亮。
    let cutout = material.params.x > 0.5;
    if cutout && tex.a < 0.35 {
        discard;
    }
    let alpha_is_opacity = material.params.z > 0.5;
    let exact_object_trans = material.depth_fade.z > 0.5;
    let line = select(select(tex.a, 0.0, alpha_is_opacity), 0.0, cutout);

    // **法线图接在这儿**:汇编里主光照那一路 `N·L` 用的就是贴图法线(PS 8409 第 157 行)。
    // 没有这张图(纯特效层、专用族、旧包)时 `mapped_normal` 原样返回几何法线。
    let n = mapped_normal(in, normalize(in.normal));
    let ndl = dot(n, normalize(camera.light_dir));
    // 两段明暗:亮部原色,暗部压到 0.72,过渡带 0.08 宽度避免锯齿。
    //
    // 实机是 `smoothstep(thr, hi, (N·L + 1) * 0.5)` 再 `lerp(暗色, 亮色, 结果)`
    // (汇编:`mad r0.x, N·L+1, 0.5, -cb5[59].x` → `div/mul_sat` 归一 → `t*t*(3-2t)`
    //  → `mad r4.xyz, r0.x, cb5[24]-cb5[25], cb5[25]` → `mul r3.xyz, 基色, r4.xyz`)。
    // 两处差别:
    // ① **半兰伯特只是换参数,不是结构差异** —— `smoothstep(a, b, (x+1)/2)` 恒等于
    //    `smoothstep(2a-1, 2b-1, x)`,所以这里照旧对 `ndl` 取阈值。不过实机那个偏置
    //    **也是个参数**(`mad r0.w, N·L, 0.5, cb6[104].y`),不是写死的 0.5;
    //    阈值上下界是另外两个槽(`cb6[104].w` / `.z`)。三个都没解出名字。
    // ② **「实机两端是颜色对」这条是错的,已更正:它是灰度对。** 重解冻结块查实(见
    //    rocom-capture/scripts/uniexpr.py 的「cb 布局」):那两个槽装的是 `Parameter(下标)`,
    //    下标落在**标量**参数段 —— 一个标量广播成 float4。所以 `mix(暗, 亮, lit)` 这个
    //    **灰度结构本身是对的**。
    //
    // **那对值已经读出来并代进来了:亮 = 1.5、暗 = 0.5**(`MI_P_Object_Trans_MatCap` 的
    // shader 20284,`cb6[60]`/`cb6[61]`,标量 #29 / #32)。它被 `r6 * r11 + r13` 消费
    // (r11 是那对高度渐变色),确认是乘在颜色上的明暗因子。
    //
    // 代进来之前踩过三条,记下来免得再走:
    // ① 直接换成 `mix(0.5, 1.5)`(还在显示空间时):幽星光圆顶中位 (238,143,200) →
    //    (255,209,255),对实机 (255,197,242) 的误差和 111 → 25,但**冲白 30.4%**(实机 0%),
    //    全库 `过曝` 9 → **109**。原因是亮端 1.5 是靠曝光压回来的,少了曝光就硬顶到白。
    // ② 只取暗部 `mix(0.5, 1.0)`:全库更干净(过曝 7),但**对比比实机更强** ——
    //    幽星光的裙子、暮星辰的翅膀都明显发暗,而实机那两处更亮更均匀。缺的是环境光。
    // ③ 只取比值 `mix(0.333, 1.0)`:误差 115,比原来的 111 **还差** —— 差距不在暗部在亮面。
    //
    // 另外那个「实机圆顶更亮更不饱和」的差距**不全是材质**:按加性白项拟合,线性下加 0.283
    // 白能让三个通道**同时**吻合 (1.0,0.773,0.948) vs 实机 (1.0,0.773,0.949) —— 而汇编尾部
    // 正好有 `mad r0.xyz, r0, v5.w, v5.xyz`(**高度雾的 inscatter**,加性)。也就是说参考截图里
    // 那层淡白是**场景的雾**,桌宠不该有,所以圆顶颜色存在一个不可消的偏差,别去追。
    let lit = smoothstep(SHADE_TERM_LO, SHADE_TERM_HI, ndl);
    // **直接光项照汇编:暗 0.5 / 亮 1.5**(见上面 ② 那段),再加一层环境光。
    //
    // **环境那一项是必须的,不是补丁。** 汇编那对只乘在**直接光**上,而实机的 mobile base pass
    // 还叠着天光/间接光(`cb0` 那批 View 常量,离线读不出来)。只代 0.5 不加环境,暗部就比实机
    // **更深** —— 实测幽星光的裙子、暮星辰的翅膀都明显发暗,而实机那两处更亮更均匀。
    // 所以这里的自由度只剩**一个** `AMBIENT`(替掉原来凭空的 0.72,它本来是「直接+间接」
    // 揉成一个数),取值让亮/暗两端与标定过的观感等值:
    //   亮 `sqrt((1.5 + A) · E) = 1`、暗 `sqrt((0.5 + A) · E) = 0.72` ⇒ A = 0.5765、E = 0.4816。
    let shade = mix(0.5, 1.5, lit) + AMBIENT;
    let facing = facing_ratio(n);
    // 边缘光:**汇编里没有这一层,是我们自己加的**(桌宠场景下让轮廓从背景里浮出来)。
    // 系数 0.25 调于 `466326f`,那时 `facing_ratio` 的视线还写死世界 +Z(修于 `ba49e56`)、
    // 法线还是切线(修于 `1daa75e`)—— 两个前提都变了,这个数从没重新标过。
    // 系数按 `旧² / EXPOSURE` = 0.25² / 0.4816 ≈ 0.13 换算到线性(**指数不动**:
    // 平方会把 `pow(facing,3)` 变成 `pow(facing,6)`,那是改形状不是改强度)。
    let rim = pow(facing, 3.0) * 0.13;

    // 固有色:卷动色带 → 两段明暗 → 纹路提亮(alpha 高的地方比底色亮一档)。
    //
    // **不再乘 `MainColor`。** 原来对半透族乘了一层 `MainColor`(暮星辰裙子 (0.39,0.4,0.63)),
    // 理由是「不乘裙子会偏白」—— 那也是在错法线上看到的。对着实机截图量:裙子实测
    // (71,91,232),而基色贴图在那块 UV 是 (66,64,197),**几乎就是基色原样**;乘上去只有
    // (26,26,124),暗了三倍。另外静态开关 `GlassySwitch` 全库一个没开,而 `MainColor`
    // 属于那条 glassy 通路 —— 两边都指向「这一乘是多余的」。
    // 纯特效层的主色仍走 `tint`(那些材质压根没有基色贴图),不受影响。
    // **平方 = gamma 2.0 解码**,把基色贴图从显示空间搬进线性(见 `EXPOSURE`)。
    // 卷动色带那张也是显示空间的成品颜色,所以在 `flow_band` 里混完再一起平方。
    var albedo = flow_band(in.uv, tex.rgb);
    if exact_object_trans {
        albedo = game_tonemap_inverse(srgb_to_linear(albedo));
    } else {
        albedo = pow(albedo, vec3<f32>(DECODE_GAMMA));
    }
    // 基色 alpha 的那个重映射:`Glow Color × Glow Intensity` 与 `Emitter Color ×
    // Emitter Intensity` 两层共用它当遮罩(水灵 Low PS 68952 第 60~65 行与第 145 行,
    // 两条都是 `颜色 × 这个遮罩`,而且**都进发光累加器 r1**、在光照之后才相加)。
    let detail_mask = saturate((line - 0.04) * 1.1111);
    // 加上去的光。**不透明层不叠 MatCap**——游戏那边靠遮罩通道选择性反射,
    // 无条件叠会把宠物冲白(试过,整只发白),而 toon 着色本身对着截图已经够像。
    //
    // 那层白色 `rim` 是我们自己加的(汇编里没有,桌宠场景下让轮廓从背景里浮出来)。
    // **玻璃族不加**:它有材质自己的边缘光(`RimColor`/`RimIntensity`/`RimPower`),
    // 两层叠起来轮廓会糊成一圈白 —— 暮星辰的裙子就是这么被冲成淡青的。
    let generic_rim = select(rim, 0.0, material.flags.y > 0.5);
    // `params.y` = `Glow Color × Glow Intensity`(实测全库根默认 0、只有 2 处实例覆盖),
    // 位置按汇编放进发光层;原来它加在固有色上,那是从另一条 shader 读串了。
    var glow = vec3<f32>(generic_rim + material.params.y * detail_mask);
    // **自发光**:`Emitter Color × Emitter Intensity × 基色 alpha 的那个重映射`,
    // 线性空间里加性叠加在光照**之后**。遮罩不再是猜的了 —— 水灵本体那条
    // ES3.1/Low/LOD0 的 `M_P_Object` 像素着色器(资源 `BF0167AE…`,PS 68952)写着:
    //
    //     add     r2.z, r3.w, l(-0.04)          ← r3 = BaseTex 采样
    //     mul_sat r2.z, r2.z, l(1.1111)         ← 和不透明度用的是同一个重映射
    //     mul     r4.xyz, r2.z, cb6[2].xyzx     ← 颜色 × 那个遮罩
    //     mul     r5.xyz, r4.xyzx, cb6[39].z    ← × 强度标量
    //     …
    //     add     r0.xyz, r0.xyzx, r1.xyzx      ← 第 268 行:发光累加器 + 已着色的颜色
    //
    // 也就是「**基色 alpha 画在哪儿就发光在哪儿**」,整条链里没有任何 Fresnel/N·V 项。
    // 两张贴图都能对上眼:水灵 `_By_D` 的 alpha 就是身上那几道竖向条纹(>200 占 21%),
    // 火神 `_By_D` 的 alpha 只圈住火焰爪/角上的高光块(>200 占 1.1%),黑翅膀是 0。
    //
    // 原来这里拿 `facing` 当遮罩、把整层糊在所有像素上:水灵的条纹一根都看不见(遮罩没参与),
    // 火神那对黑翅膀反被橙色自发光染成褐黄 —— 实机报的「多了一层黄色遮罩」就是它。
    let emissive_layer = select(vec3<f32>(0.0),
                                material.emissive.rgb * material.emissive.w * detail_mask,
                                material.emissive.w > 0.0);
    // **流动层和自发光层进的是同一个累加器,而且末尾一起过一次 `saturate`**
    // (汇编 `mad_sat r7, (E + 流动), 2·EmissContrast+1, -EmissContrast`;`EmissContrast`
    // 全库实测恒 0,那一步就化简成 `saturate`)。分开加会让 `FlowInt` 大的材质
    // (火系有 20 的)冲过 1 而不被夹住。
    //
    // 流动层还带一道 ID 门:`MaskTex.a` 落在 [`MaskID Min`, `MaskID Max`] 之外时,
    // 汇编末尾那步 `lerp(带流动, 不带流动, 门)` 把它整个换掉 —— 全库 50 份材质设过
    // 这个下界,不接就会整只盖上流动色。
    //
    // **采样提到分支外面**:WGSL 的均匀性规则要求 `textureSample` 在均匀控制流里
    // (`flow_band` 那儿踩过一次,Dawn 直接判整份 shader 非法)。
    let uv_flow = uv_flow_layer(in.uv, in.uv1, in.color.b, detail_mask);
    let flow_id = textureSample(mask_id_tex, base_sampler, in.uv).a;
    let flow_gated = material.mask_id.z > 0.5
        && (flow_id < material.mask_id.x || flow_id > material.mask_id.y);
    let flow_layer = select(uv_flow, vec3<f32>(0.0), flow_gated);
    // 没有流动层的材质保持原样(不夹),免得给既有的那批凭空改行为。
    glow += select(emissive_layer,
                   saturate(emissive_layer + flow_layer),
                   material.uv_flow_color.w > 0.0);
    // 菲涅尔那层是在这一步**之后**才加进累加器的(汇编第 151 行),不参与上面那次 saturate。
    glow += fresnel_layer(normalize(in.normal), view_direction());
    // 火系族那两层也进同一个累加器(汇编第 199 行)。传的是**未解码的**基色贴图值:
    // 这一族自己做反色调映射,和外面那条 `pow(albedo, DECODE_GAMMA)` 不是一回事。
    glow += fire_layers(normalize(in.normal), tex.rgb, in.color.g, detail_mask);
    // **水体预设那两层没接上,是有意的** —— 公式已经按汇编写好了(`water_layer`),
    // 接上去实测:水灵 调色板 0.106 → **0.293**、亮度比 0.85 → **1.17**;
    // 波波拉 0.126 → 0.277、1.16。原因**量清楚了**:那一层里
    // `层二 = 反色调映射(基色) × lerp(Color1, Color2, …) × FresnelInt`
    // 的量级和身体本身相当(线性里 ≈ 0.3 对 0.43),而实机的身体是
    // `固有色 × 色带`(`T_AllDebugRamp` 实测 256 行全在 0.947~1.000,≈ 恒 1),
    // 我们的 `shade` 是两段明暗 + `AMBIENT`,最高到 **3.0** —— 身体先大了 2~3 倍,
    // 再加一层等量的光当然过曝。
    //
    // ⇒ 这是待办里那条「`M_P_Object` 的实机着色链…直接接上更差,要整包做」的**同一堵墙**,
    // 而且现在有了数:**要动 `shade` 就得连这一层一起动**,单接一边必崩。
    // 参数与贴图都已导出并接到 uniform 上(`family0..6`),重新打开只要加回这一行。
    // **不透明度**:`alpha_is_opacity` 的材质取基色 alpha,并照汇编做那个重映射
    // (`add r1.z, a, -0.04` → `mul_sat r1.z, r1.z, 1.1111`,即把 0.04..0.94 拉到 0..1)。
    // 暮星辰裙子那块 UV 的 alpha 中位 0.537 → 0.55,与从实机截图水印衰减反推的 0.50 对得上。
    var alpha = select(1.0, saturate((tex.a - 0.04) * 1.1111), alpha_is_opacity);

    // **玻璃 / 薄纱**(`MI_P_Object_Trans_*` 族:幽星光那两个球、暮星辰的裙子与球)。
    // 只有这一族叠 MatCap 高光与材质自己的边缘光。
    //
    // 材质里的边缘光是**加在边上的一层光**,不能拿去染固有色:球的颜色就是基色图集里
    // 那片平色圆盘。导出器只把「`Rim Intensity` 真的大于 1」的边缘光写进来(见 Manifest.cs)——
    // 曜星光那两颗球写着强度 1 + 绿色 `Rim LightColor`,而实机里它们是橙的和紫的。
    if material.flags.y > 0.5 {
        let spec_coverage = trans_spec_coverage(n);
        // **加上去的几层光是 `max` 合的,不是相加。** 汇编里连着两条:
        // `max r2.yzw, matcap*MatCapColor, spec*SpecColor` 再 `max r2.xyz, 上一步, rim`。
        // 相加会让高光与边缘光在轮廓处叠成一圈白边;取 max 则是「哪层亮听哪层」。
        // `extra.x` = `Rim Power`、`extra.z` = `Rim Soft Edge`、`star.z` = `Rim Intensity`。
        // **不透明度吃的是「没乘强度」的那份覆盖率。** 汇编里这两路是岔开的:
        //   PS 53987:  220~223 得到覆盖率 r0.w → 259 `max r0.x, r0.w, r0.x`(进 α)
        //              224~225 才 `× RimIntensity` → 只进颜色
        //   PS 53466:  89~92 覆盖率 r0.x → 138 `max r0.x, r0.x, r1.x`(进 α)
        //              93~94 才 `× RimIntensity` → 只进颜色
        // **高光那一路正好相反**(53987 第 256 行先 `× HighLight SpecInt` 再进 α),
        // 所以不能两条一起处理。
        //
        // 代价很具体:莫比乌乌的壳 `Rim Intensity = 0.2`,乘上去之后管子边缘的 α 只有 0.2,
        // 实机那圈白边在我们这儿是一条 2px、α=110 的虚边,而实机是 5px 的实白。
        let rim_coverage = trans_rim_coverage(n, material.extra.x, material.extra.z);
        let rim_strength = rim_coverage * material.star.z;
        // 这组 SpecCol 属于 `M_P_Object_Trans` 的 alpha-opacity 排列；MatCap 族是另一张
        // 材质图，继续只走自己的查找表，不能被这一组根默认高光改色。
        let spec_light = select(vec3<f32>(0.0),
                                material.highlight_color.rgb * max(spec_coverage, 0.0),
                                alpha_is_opacity);
        glow += max(spec_light,
                    max(matcap_light(n) * GLASS_MATCAP_GAIN,
                        material.rim_color.rgb * rim_strength * GLASS_RIM_GAIN));
        // 目标实机实际选中的 Low 排列先做
        // `lerp(max(基色 alpha, spec), 基色 alpha, ForceUseDefOpacity)`，再加场景深度淡化。
        // 这里没有别的排列里的 rim/Fresnel alpha，也没有 MatCap 采样值。
        if alpha_is_opacity {
            let covered = max(alpha, spec_coverage);
            alpha = mix(covered, alpha, saturate(material.rim_color.w));
            alpha = saturate(alpha + depth_coverage);
        }
        // **rim 与 MatCap 那两层也顶不透明度,这一条不能省。** 汇编
        // (`MI_P_Object_Trans_MatCap` 37998)里输出 alpha 是 `max(基色a, 高光, 菲涅尔)` ——
        // 上面那段只补了「高光」那一路(而且只在 alpha 是不透明度时),菲涅尔/MatCap 那一路
        // 仍要在这儿取 max。**不接的代价**:幽星光那两颗球的基色 alpha 中位 0.000、p90 也是
        // 0.000(形状压根不在基色里),球会整个消失;暮星辰的裙子会薄掉一档
        // (量过:去掉这行 0.074 → 0.091,再叠上 Low 分支是 0.084)。
        alpha = max(alpha, max(rim_coverage, saturate(matcap_strength(n))));
        // **幻星族那两颗球**多一层菲涅尔换色(见 `xing_fresnel_coverage`)。
        // 位置照汇编:在 alpha 的 `max` 链之后、球内那颗星之前;它**替换**发光累加器,
        // 顺带按 `OpenOpacityAdd` 给不透明度加一份。判据是 `family6.y`
        // (`.x` 已经给了水体那一层)。
        if material.family6.y > 0.5 {
            let cov = xing_fresnel_coverage(normalize(in.normal));
            let col = select(material.family0.rgb,
                             mix(material.family1.rgb, material.family0.rgb, in.color.g),
                             material.family2.z >= 0.5);
            var w = mix(alpha, saturate(cov), material.family2.w);
            w = mix(w, saturate(cov), material.family3.y);
            w = w + material.family3.z * (1.0 - 2.0 * w);
            glow = mix(glow, cov * col, saturate(material.family1.w * w));
            alpha = saturate(alpha + material.family3.x * w);
        }
        // **球内那颗星是「混进固有色」,不是加在上面。** 汇编最后一步是
        // `out = lerp(基色 × 明暗色, 发光层色, 混合系数)`(fx1/34529.asm ⑥,见 findings.md §1),
        // 而发光层色 = `星点底色 + 星点强度 × 星点亮色`,再与「按物体空间高度 lerp 的那对颜色」混。
        // 我们只拿到其中的 `StarColor`(根默认 (0.33,0.67,2) 的 HDR 蓝),底色那两对与混合
        // 系数都是还没解出名字的 cb 槽位,所以这里退化成「按星点强度往 StarColor 混」——
        // 结构照汇编(lerp 而不是相加),缺的那几项当成中性。
        // **HDR 的材质色现在直接用,不再预先 sqrt。** 原来那个 `sqrt` 是在「整条链跑在
        // 显示空间」时代的补偿(把线性 HDR 值硬编码成显示值);现在固有色链路本来就在线性里、
        // 末尾统一 `sqrt(色 × 曝光)`,再单独 sqrt 一次就是编码两次了。
        let star_color = material.interior_color.rgb;
        // **折射必须在物体空间算**(见 `interior_star`),所以这里传物体空间的法线与视线,
        // 不是世界空间的 `n`。
        albedo = mix(albedo, star_color,
                     saturate(interior_star(in.local_pos,
                                            normalize(in.local_normal),
                                            normalize(in.local_view))));
        // 玻璃族自身的整体不透明度;`alpha_is_opacity` 的材质已经从基色 alpha 拿到了,别覆盖
        if !alpha_is_opacity {
            alpha = clamp(material.star.w, 0.0, 1.0);
        }
        // **玻璃也吃两段明暗。** 这一族走的是同一条固有色链路:同一个 pixel shader 里
        // `mul r3.xyz, 基色, lerp(暗色, 亮色, smoothstep(N·L))` 就在折射/matcap 那些
        // 分支的下游,没有任何开关把玻璃排除掉。原来这儿硬写 `lambert = 1.0`,理由是
        // 「开口薄壳自转时会在 0.72↔1.0 之间跳」—— 那个跳动是法线被写成切线造成的
        // (见 design.md 法线那条),法线修好后不复存在,所以这个特例撤掉。
    }
    // **线条遮罩是「加一个颜色」,不是「乘一个亮度倍数」** —— 查实于罗隐(阿米亚特)的 body
    // shader 51377 第 99~103 行:
    //     r1.w = saturate((基色.a − 0.04) × 1.1111)      ← 和不透明度用的是同一个重映射
    //     mad r6.xyz, cb6[7].xyzx, r1.w, r6.xyzx          ← 加上 cb6[7] × 那个遮罩
    // 原来这里是 `× mix(1.0, LINE_BOOST, alpha)`(乘法),形状就不对。
    // `cb6[7]` 那个颜色的名字还没解出来(这条 shader 的 V=112,全库没有材质带这个块),
    // 所以先取中性白 × 一个标定强度 —— 但**形状按汇编改对了**。
    // **星贴层是 lerp 替换已着色的颜色,不是加一层光、也不是染固有色。** 汇编:
    //     mad r7.xyw, Stick_Intensity, r7.xyxw, -r0.xyxz    ← 强度 × (m × 渐变色) − 底
    //     mad r0.xyz, r9.w, r7.xywx, r0.xyzx                ← 底 + 混合系数 × 上面那个差
    // 合起来 `lerp(底, Stick_Intensity × m × c, saturate(m + GlassyMainColorOpacity))`。
    //
    // **位置很要紧**:那一步作用在 `r0`(效果累加器)上,而固有色累加器是 `r6` ——
    // 两者到第 487 行才合并。所以渐变色**不该再乘 `shade`**。先放在 `albedo * shade`
    // 之前试过:`shade` 最高 3.0(两段明暗 1.5 + AMBIENT 1.5),把浓色直接顶到过曝,
    // 全库过曝 11 → 14,多出来的正好是开着这层的星光族三只。
    //
    // 渐变色是材质参数(本来就在线性空间),所以**不过 `DECODE_GAMMA`** —— 只有贴图要解码。
    let stick = stick_layer(in.uv, in.ndc);
    // **假半透族那层是加光,不是 lerp 替换。** 上面那条 lerp 是 `StarStickTex` 那一族的
    // (汇编查实);两族公式不同,合并成一套是已知的简化。这一族的星点色是
    // `Color02 × Mat_NoiseIntensity`(幽星光 15 × 0.05 = 0.75),**比被照亮的身体还暗** ——
    // 替上去就是一片深色麻点(用户实测「星点的黑灰色明显不对」)。加上去才是星芒。
    // **判据要跟着坐标系走,不能用 `params.w`。** `star_fake_trans` 那个标记只有 `_Fx` 有,
    // 而身体是 `_By` 画的 —— 按 `params.w` 判会让 `_By` 走 lerp 替换那一支,
    // 拿一个比底色暗的星点色替上去 ⇒ **身上一片黑斑**(用户实测)。
    // `noise_uv.w < 0.5` 表示"这个材质用相机空间的噪声坐标",与加光是同一族。
    let fake_trans = material.noise_uv.w < 0.5;
    // 高质量 `M_P_Object_Trans` 排列还会在 ForceUseDefOpacity 之后
    // `alpha = max(alpha, starMask)`；目标实机的 Low 排列在 `stick_layer` 已返回空层。
    if alpha_is_opacity && material.flags.y > 0.5 && !fake_trans
        && camera.high_material_quality > 0.5 {
        alpha = max(alpha, stick.cover);
    }
    // **星贴层混的是固有色,不是已经着色的颜色。** 汇编(`M_P_Object_Trans` 51670)里
    // 那条 lerp 作用在 `r0` 上,而 `r0` 一路攒到第 690 行才 `mul r1.xzw, r0.xxyz, r1.xxzw`
    // **乘上光照** —— 也就是星点色要跟着这一点的明暗一起变。原来写在 `albedo * shade`
    // **之后**,等于把一块不受光的平色贴上去:亮处不亮、暗处不暗,实机看不见的这一层
    // 在我们这儿成了一身灰紫斑(实机报的「果冻…看不到星点」)。
    // **`else` 那支的位置本来就是对的**:`mov r0.xyz, r9.xyzx`(第 511 行)—— 不开这层时
    // `r0` 就是干净的固有色,同样在乘光照之前。
    let surface_albedo = select(mix(albedo, stick.color, stick.cover), albedo, fake_trans);
    // **逐 `MatID` 的高光是乘在这一步上的**(汇编 PS 8409 第 220~221 行把
    // `基色 × 光照` 与 `基色 × 光照 × 高光` 相加,提出来就是这个 1 + …),
    // 不是加一层白光 —— 见 `matid_specular`。四档强度全 0 的材质返回 0,原样通过。
    var body = surface_albedo * shade * (1.0 + matid_specular(in.uv, n));
    if exact_object_trans {
        body = object_trans_low_light(in.uv, n, surface_albedo);
    }
    let stick_add = select(vec3<f32>(0.0), stick.color, fake_trans);
    // **末尾统一编码到显示空间**:`sqrt(色 × 曝光)`,照汇编尾部那条
    // `movc o0.xyz, (曝光 < 1), sqrt(色 × 曝光), 色`。
    //
    // **`glow` 也在线性里了**,所以几层光**先在线性里相加、再一起编码一次**——
    // 这才是对的:加性光就该在线性里加。原来是各自在显示空间加,等于把小值各编码一次
    // (`sqrt` 会放大小值:0.25 → 0.418),几层叠起来偏亮。
    // 四个系数按 `旧² / EXPOSURE` 换算过,保持观感等值(见各自的定义处)。
    //
    // UE 的 BLEND_Translucent 在 pixel shader **完成颜色输出以后**才由固定功能混合器执行
    // `SrcColor * SrcAlpha + DstColor * (1-SrcAlpha)`。我们的管线使用等价的预乘混合，
    // 因而必须先把整条直色链（包括高光/星点/边缘光）编码到输出空间，最后才预乘 alpha。
    //
    // 旧代码在这里先在线性空间做 `body * alpha`、随后再 sqrt：半透明贡献实际变成
    // `sqrt(alpha)`，例如 alpha=.2 会按约 .45 的强度盖住内层；同时 glow 又完全没乘
    // alpha。两处都与游戏的 BLEND_Translucent 顺序相反，会系统性冲亮所有透明壳。
    var combined = body + glow + stick_add;
    // 炫彩整层盖在最后:游戏那边也是这个位置(`lerp(原着色, 玻璃色, BlendWeight)`),
    // 而且它**替换**而不是叠加 —— 原着色只通过 `surface_albedo` 的亮度参与调制。
    combined = glassy_layer(in, surface_albedo, combined);
    if exact_object_trans {
        // PS 尾部 cb6[29].xyz，覆盖基色与高光在内的整条材质颜色。
        combined *= material.tint.rgb;
    }
    var encoded: vec3<f32>;
    if exact_object_trans {
        // 2109/55790 的真实尾段；精确分支不能再经过项目为通用 toon 增补的软肩。
        encoded = encode_linear_color(combined);
    } else {
        var lin = max(combined, vec3<f32>(0.0)) * EXPOSURE;
        // **软肩**:尚未逐材质还原的通用 toon 分支仍用它补 LDR 余量。目标 Low
        // ObjectTrans 与 M_ShuiMu_ByIn 已有原 PS，均不走这里。
        if SHOULDER_WHITE > 0.0 {
            let w2 = SHOULDER_WHITE * SHOULDER_WHITE;
            lin = lin * (1.0 + lin / w2) / (1.0 + lin);
        }
        encoded = sqrt(lin);
    }
    return vec4<f32>(encoded * alpha, alpha);
}

// 脸的种类由 `material.flags.x` 区分:
//   0 = 不是脸、2 = 网格脸(挑一张卡)、**10 + 槽号** = 图集脸(偏 UV)。
//
// 图集脸要偏的那一格在 `camera.face_uv[]` 里,槽号定死在 `pack::face_slot`
// (0 眼、1 眼#1、2 嘴、3 嘴#1、4..7 Dynamic1..4)。**逐槽分开**是因为游戏把它们写成
// 一条条独立的动画曲线(`EC_Eye` / `EC_Mouth` / `EC_Dynamic1`…),同一段动作里可以指向
// 不同的格子(幽星光的 `Shock` 是眼第 3 格、嘴第 7 格;幽影树的 `Relax` 是眼第 6 格、
// 两条藤第 4 格)。
// **网格脸(2)一格都不能偏**:它八张卡的 UV 早各自钉在一格上了,再偏就串格。
fn face_uv_offset() -> vec2<f32> {
    if material.flags.x < 9.5 {
        return vec2<f32>(0.0, 0.0);
    }
    let slot = u32(material.flags.x - 10.0);
    let pair = camera.face_uv[slot / 2u];
    return select(pair.zw, pair.xy, (slot & 1u) == 0u);
}

/// 网格脸(`M_P_Eyes_Mesh`)只画当前眼神那一张卡,其余七张整片剔掉。
///
/// 卡号写在顶点色 G 里(`floor(G × 10)` ∈ 1..8);八张卡是**叠在同一处的独立几何**,
/// 不剔就是「眉毛、眼睛、腮红搅在一起」。整张卡的顶点同色,所以插值出来的值也稳。
fn cull_face_card(in: VsOut) {
    // ⚠ 只认 2(网格脸)。10 以上是图集脸(眼/嘴/Dynamic),它们的顶点色 G 里没有卡号,
    // 拿这条剔等于把那一片整个剔掉。
    if material.flags.x > 1.5 && material.flags.x < 2.5
        && abs(floor(in.color.g * 10.0) - camera.face_card) > 0.5 {
        discard;
    }
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    cull_face_card(in);
    return shade_main(in, 0.0);
}

@fragment
fn fs_glass(in: VsOut) -> @location(0) vec4<f32> {
    cull_face_card(in);
    return shade_main(in, trans_depth_coverage(in));
}

/// **玻璃球的「内胆」:把球的远半球当不透明件先画一遍(写深度)。**
///
/// 那三对球里各封着一块**不透明小饰件**(黄色四角星 / 圆点 / 立体四角星,画在 `_By` 上,
/// 与球蒙在同一根或相邻骨骼、位置就在球心),实机三只球正中都看得见它;而球本身按颜色
/// 必须是不透明的(曜星光两颗的实机色 (240,120,61) 橙与 (120,44,235) 蓝正是它们各自的
/// 图集基色,而 `_Fx1_Ol` 的五档两颗挑的是同一档暗红,半透叠上去解不出一橙一蓝)。
///
/// 这一遍把两条都满足了,而且不动任何顺序:
/// ```text
/// 不透明遍:描边壳(暗) → 饰件 → **球的远半球(实心)**      ← 都写深度,重叠自然排序
/// 混合遍  :球的近半球(α ≈ 0.2)
/// 合成    :球身 = α·C + (1−α)·C = C        ← 和把球顶成不透明一样
///           饰件 = α·C + (1−α)·饰件         ← 饰件比远半球近,深度测试让它留下
///           轮廓 = 描边壳                    ← 那圈暗边保住了
/// ```
///
/// **法线要沿视线镜像回来**:远半球的法线朝背面,`n' = n − 2(n·V)V` 把视线分量翻正,
/// 得到的正是**同一屏幕位置上近半球的法线** —— MatCap 高光块与边缘光因此不动。
/// (直接取 `-n` 是错的:那是球心对称的另一点,高光会整个翻到对面。)
///
/// **这是画法的选择,不是从汇编读出来的**:`M_P_Object_Trans` 与实例都没有 `TwoSided`。
/// 实机究竟靠什么让不透明的球露出内部还没查到,最可能是那块饰件走 WPO 面向相机被推到球前
/// (`_By` 的父链是 `..._UVFlow_Morph`,带整组 WPO 参数,而我们整条 WPO 没实现)。
/// 试过的两条都被实测否决:只让 MatCap 那一支保持半透(曜星光偏暗)、
/// 只画远半球不补近半球(两球重叠时排序坏掉)。见 docs/design.md。
@fragment
fn fs_glass_fill(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    cull_face_card(in);
    var v = in;
    if !front {
        let view = view_direction();
        v.normal = in.normal - 2.0 * dot(in.normal, view) * view;
    }
    let shaded = shade_main(v, 0.0);
    return vec4<f32>(shaded.rgb / max(shaded.a, 1.0e-4), 1.0);
}
