# 从安卓 APK 读 shader：GLSL 源码这条路

`docs/shader.md` 那条路走的是 **Windows 客户端**的 `PCD3D_ES31`(DXBC 字节码,反射段被剥,
只能靠反汇编读)。本文记录另一条:**安卓客户端 cook 的是 `GLSL_ES3_1_ANDROID`,载荷是
HLSLcc 生成的 GLSL 源码文本** —— 公式直接能读。

两边编译自同一份材质图,静态开关同样已在编译期定死,所以读到的就是"实机跑的那一套"。

> 这份文档是**过程记录**,结论稳定后应择要并入 `docs/shader.md`,本文可删。

## 1. 取包

APK(2.0 GB)里 UE 的数据藏在 `assets/main.obb.png` —— 名字是 png,实为 **store 压缩的 zip**:

```sh
unzip -o <apk> 'assets/main.obb.png' -d ~/Downloads/rocom/apk
unzip -o ~/Downloads/rocom/apk/assets/main.obb.png 'NRC/Content/Paks/*' -d ~/Downloads/rocom/apk
./scripts/unpack.sh --paks ~/Downloads/rocom/apk/NRC/Content/Paks \
    --out ~/Downloads/rocom/apk-parsed --no-exclude --no-post --filter "NRC/Content/ShaderArchive"
```

`scripts/unpack/Program.cs` 本来就有 `ApkFileProvider`,但**直接喂 .apk 不行** —— pak 在
obb 里,得先剥两层 zip。

得到:

| 文件 | 大小 |
|---|---|
| `ShaderArchive-NRC-GLSL_ES3_1_ANDROID.ushaderbytecode` | 631 MB |
| `ShaderArchive-Global-GLSL_ES3_1_ANDROID.ushaderbytecode` | 5 MB |

**没有 Vulkan 那份。** 尽管 `DefaultDeviceProfiles_Template.ini` 里 Adreno6xx / Mali-G7x /
PowerVR GM9xxx 都有 `r.Android.DisableVulkanSupport=0`,基础 OBB 里只 cook 了 GL ES 一套;
Vulkan 的 SPIR-V 若存在,应在运行时下载的 pak 分块里(基础包只有 chunk 0/1/2/4/6,约 950 MB)。

## 2. 格式

**容器与 Windows 那份完全一样**(`FShaderCodeArchive` version 2),所以 `scripts/glsldump.py`
直接继承 `shaderdump.ShaderArchive`,只换载荷解析:

```
魔数 'LSLGS' + 频率字母(P 像素 / V 顶点) + 头(输入属性名) + GLSL 源码 + b'\x00' + uint32
```

**坑:按 `LSLGSP` 整串判魔数会把 20577 条顶点 shader 全判成"解不出"**,而顶点 shader 里有蒙皮。

内容长这样(名字都在):

```glsl
uniform vec4 pc6_m[33];              // 第 6 个 uniform buffer 的 mediump 区
uniform highp vec4 pc0_h[25];        // 第 0 个的 highp 区
uniform highp sampler2D ps0;
layout(location=3) in highp vec4 in_TEXCOORD0;
layout(location=0) out vec4 out_Target0;
```

比 DXBC 强的地方:

- **输入带语义名** —— `scripts/dxbcsig.py` 那一步不需要了;
- **采样通道直接可见** —— `texture(ps3,uv).x` 一眼是 R,不用再解资源 swizzle;
- 公式是表达式而不是寄存器流,`refract()`/`smoothstep()`/`fract()` 都是原样的内建函数。

弱的地方:**`pcN_*[i]` 与 DXBC 的 `cbN[i]` 不是同一套下标** —— GL 这边 UE 走"打包 uniform",
按缓冲分组、再按精度(`_m` mediump / `_h` highp / `_u` uint)分家重排。
所以 `uniexpr` 那套「cb 槽位 ↔ 参数名」的成果**不能直接搬到 `pc6_m` 上**。

## 3. 归属:SHA1 这条路在跨平台上是断的

`docs/shader.md` 的归属办法是拿材质 uexp 里的 shader map SHA1 去 archive 的哈希表里 memmem。
跨平台**行不通**,实测:

```
Windows 5715 个 map-hash,Android 6065 个,交集 0
shader-hash 72407 / 95814,交集 0
```

平台是 `FMaterialShaderMapId` 的一部分,哈希必然不同。而安卓基础 OBB 里**没有宠物资产**
(`ArtRes/AnimSequence` 下只有 `Human/*`),所以也没法用"安卓自己的材质 uexp"去配。

### 换成结构指纹:哪些判据有用、哪些没用

| 判据 | 效果 |
|---|---|
| **字面量常数多重集** | ❌ 几乎无区分度。52 个常数里 47 个能在 6 万条上命中 —— 都是引擎公用的雾/tonemap 常数。而真正独特的那批(Bayer 抖动矩阵)在 DXBC 里是**十六进制** `dcl_immediateConstantBuffer`,正则压根扫不到。 |
| **输入语义集** | ⚠️ 有用但要放宽。宠物 DXBC 是 `TEXCOORD0..4,7,8,10,11`;GLSL 那边 `TEXCOORD7` 随排列有无,精确相等会把真值筛掉。 |
| **`gl_FrontFacing`** | ✅ 强。宠物材质双面(DXBC 有 `SV_IsFrontFace0`)。全库 12889 条双面像素 shader,配上插值器集合后只剩 36 条。 |
| **采样次数与槽位模式** | ✅ 强。宠物 DXBC 是 11 次采样,且 t8/t9/t10 共用同一套坐标、t9 连采两次。 |
| **`frc`+`sincos` 配对** | ✅ 用来做最终确认(闪烁的相位项),两边都有。 |

最后落到 **`#3119`**(1437 行)。确认它同源的硬证据是三向投影那一行:

```glsl
v121 = mix(mix(texture(ps8,v114.xz),texture(ps8,v114.yz),v118.xxxx),texture(ps8,v114.xy),v118.zzzz);
```

## 4. 读出来的东西(这才是重点)

以下全部来自 `#3119`,`pc6_m` = 材质参数块、`pc5_m` = MaterialCollection、`pc0_h` = View。

### 4.1 底色贴图是"反色调映射"的 —— `DECODE_GAMMA = 2.2` 是个粗糙近似

```glsl
v90 = ((-0.56*c + 0.047) - sqrt((-0.2072*c*c + 0.70896*c) + 0.002209))
      / (2*(0.93*c - 1.36))
```

这是一个二次方程的求根公式。反推正向曲线:设 `t = f(x) = x(ax+b) / (x(cx+d)+e)`,
则 `x²(ct−a) + x(dt−b) + et = 0`,与上式逐项对齐得

> **`t = x(1.36x + 0.047) / (x(0.93x + 0.56) + 0.14)`**

即一条 **ACES 风格的自定义 filmic 曲线**(ACES 拟合是 2.51/0.03/2.43/0.59/0.14 —— 常数项
`0.14` 一模一样,其余是这个项目自己调的)。材质对底色贴图做的是它的**逆**:美术按"所见即所得"
在显示空间画贴图,shader 先反解回线性再参与光照。

对照几个点:

| 贴图值 | 本式反解 | `pow(x, 2.2)` |
|---|---|---|
| 0.0 | 0.000 | 0.000 |
| 0.5 | **0.439** | 0.218 |
| 1.0 | **1.422** | 1.000 |

`x=1` 解出 1.42 而不是 1 —— 白处会被推到 HDR。rocom-pets 现在用的 `pow(albedo, 2.2)`
在中间调差了一倍,这多半就是"调色板"指标一直卡在 0.077 的一个来源。

正向式是手推的,**已数值复核**:`inv(fwd(x)) == x` 在 x ∈ [0,4] 上往返误差 ≤ 6e-16。
顺带得到白点:`fwd(1.422) = 1.0`,即这条曲线把 [0, 1.422] 映到 [0, 1](再往上到
`x→∞` 只涨到 1.462 就压死)。rocom-pets 里手调出来的 `SHOULDER_WHITE = 1.5`
正是在逼近这个 1.422 —— 现在可以换成读出来的值。

**还有一个推论**:既然这条 filmic 是游戏的**正向** tonemap,而基座 pass 的尾部只做
`sqrt(lin × 曝光)`(写进 8 位 RT),那么真正的曲线是在**后处理**里加的。rocom-pets 离线渲染
不跑后处理,所以解完 `sqrt` 之后应当**补上这条正向 filmic**,而不是直接当成显示值。

### 4.2 自发光遮罩:在底色贴图的 A 通道

```glsl
v38 = texture(ps6, in_TEXCOORD0);                    // 底色 RGBA
h39 = clamp((v38.w - 0.04) * 1.1111, 0.0, 1.0);      // ★ A 通道,[0.04,1] 重映射到 [0,1]
v27 = h39 * pc6_m[0].xyz * pc6_m[42].x;              // × 自发光颜色 × 强度
```

找了很久的自发光遮罩就是**底色贴图的 alpha**,带一个 0.04 的下限剪裁(`1.1111 = 1/0.9`)。

### 4.3 两色调的分界量 `h89`

```glsl
h89 = smoothstep(pc6_m[46].w, pc6_m[46].z, (dot(v35, pc5_m[5].xyz) + 1.0) * 0.5);
```

`v35` 是法线、`pc5_m[5].xyz` 来自 **MaterialCollection**(全局光方向,不是 `MobileDirectionalLight`)。
注意是 `(N·L + 1)/2` 再过 smoothstep,**上下界都是材质参数**(而且 `w` 在前、`z` 在后,
即下界 > 上界时会反向)。它一口气驱动了五组明暗配色:

```glsl
mix(pc6_m[9], pc6_m[8], h89)      // 底色染色(暗/亮)
mix(pc6_m[11], pc6_m[10], h89)    // 星层底色
mix(pc6_m[13], pc6_m[12], …)
mix(pc6_m[15], pc6_m[14], h89)
mix(pc6_m[47].w, 1.0, h89)
```

rocom-pets 现在是 `smoothstep(SHADE_TERM_LO, SHADE_TERM_HI, ndl)` 直接作用在 `N·L` 上,
少了 `(x+1)/2` 的重映射,而且只用来调明暗、没有分开的暗部/亮部配色。

### 4.4 高光是硬边的,不是 Blinn-Phong

```glsl
v41 = mix(N, -lightDir, floor(pc0_h[13].w + 0.1));   // 用视线还是光向,由 View 里一个开关选
v44 = v41 + pc4_m[0].xyz + pc6_m[1].xyz;             // 全局偏移 + 材质偏移
h45 = dot(v40, normalize(v44));
h48 = pow(h45, pc6_m[42].y);
h49 = smoothstep(0.4, 0.5, h48) * pc6_m[42].z;       // ★ 硬边:0.4→0.5 卡一刀
v50 = max(0.0, h49 * pc6_m[2].xyz);
```

### 4.5 边缘光(rim)与最外面那层壳

rim:

```glsl
h65 = saturate(dot(v41, v40));
v66 = (1-h65) * (1 - smoothstep(-0.1, mix(0.3,-0.1,abs(dot(v41,vec3(0,0,1)))), h65-0.15));
v74 = pow(v66, v63.x);                       // v63 来自 pc6 与 pc5 的 lerp
v76 = smoothstep(vec3(0.5), vec3(0.5 + v63.y), v74);
```

外壳(此前记为 `GLASS_RIM_GAIN` 打不通的那一层):

```glsl
h84 = max(1 - saturate(dot(v40, v41)), 1e-4);
h86 = pow(h84, pc6_m[44].y);                                   // 菲涅尔指数
v87 = h86 * pc6_m[44].z * pc6_m[4].xyz * pc6_m[44].w
      * mix(1.0, pc6_m[45].x, pc6_m[44].w);
v88 = mix(v83 + (mix(pc6_m[45].y * pc6_m[5].xyz * smoothstep(0.99, 1.0, v87.x),
                     v87, pc6_m[45].z)                          // ★ 软/硬两版,按 pc6_m[45].z 选
                 + pc6_m[6].xyz),
          pc6_m[7].xyz, pc6_m[46].y);                           // ★ 整体可被 pc6_m[7] 顶掉
```

**注意 `h82`**:

```glsl
h82 = mix(1.0, step(pc6_m[43].w, pc6_m[43].z), pc6_m[44].x);
v83 = mix(自发光 + max(高光, rim), vec3(0.0), h82);              // h82=1 时整块归零
```

自发光 + 高光 + rim 这一整块可以被一个参数**一刀关掉** —— 又一个"代码在字节码里 ≠ 这一层可见"。

### 4.6 星层:是折射视差,不是简单的 UV 缩放

```glsl
v106 = (in_TEXCOORD3.y, in_TEXCOORD4.x, in_TEXCOORD4.y);   // 物体空间法线
v107 = normalize(v106);
v109 = refract(normalize(v105), v107, pc6_m[48].z);        // ★ 折射,IOR 是材质参数
f112 = length((包围盒max - 包围盒min) / 2);                 // 物体尺度
f113 = f112 * 0.01;
v114 = (物体空间位置 + v109 * (pc6_m[48].w * f113))
       * ((pc6_m[49].x / f113) * 0.01);                    // 深度推移 + 缩放
v118 = clamp(mix(vec3(-B), vec3(1+B), abs(v107)), 0, 1);   // B = pc6_m[49].y
v121 = mix(mix(tex(ps8,v114.xz), tex(ps8,v114.yz), v118.x), tex(ps8,v114.xy), v118.z);
```

三向投影的混合权重 `abs(n)*(1+2B) - B` 与 rocom-pets 现在写的
`saturate(abs(n)*(2*BLEND+1) - BLEND)` **完全一致**,这部分之前逆对了。
但采样位置是**沿折射向量推移**的假内部体积,不是我们现在的「UV 缩放 + 深度」。

星贴纹理四个通道各有用途:

```glsl
v122 = mix(v102, v104, v121.x);                        // R:混两种底色
h127 = pow(v121.z, pc6_m[49].z);                       // B:基础亮度
h130 = fract(pc6_m[49].w * 时间 + v121.y);              // G:每颗星的相位偏移
h128 = abs(sin(h130 * 2π));
h132 = pow(h128, pc6_m[50].x);
h135 = v121.w * (h127 - h132 * 1.2);                   // A:遮罩;闪烁是"减"出来的
v133 = mix(v122, pc6_m[17].xyz, saturate(pc6_m[50].y * h135));
```

**闪烁项是被减掉的**(`h127 - 1.2*h132`),不是乘上去的 —— rocom-pets 现在是相位直接调亮度。

### 4.7 第二个三向投影:滚动的法线贴图

```glsl
v136 = 物体空间位置 * (1/max(pc6_m[50].z * f112, 1e-4)) + fract(时间 * pc6_m[18].xyz);
v150 = pow(abs(v106), pc6_m[51].x);                    // 逐分量
v153 = v150 / dot(v150, vec3(1.0));                    // ★ 归一化权重(与 4.6 那种不同!)
v154 = tex(ps3,v136.xy)*v153.z + tex(ps3,v136.yz)*v153.x + tex(ps3,v136.xz)*v153.y;
v157 = 切空间法线解码(v154.xy);
h158 = sin(π * pc6_m[51].y * (dot(v157, v17) + 1.0)) * v154.w;
```

**两种三向投影在同一条 shader 里都存在**:星层用嵌套 lerp(4.6),这一层用归一化 pow 权重。
`docs/shader.md` 里"三向投影是嵌套 lerp、不是归一化 pow 权重"那句话只对星层成立,应改口径。

### 4.8 尾部:没有引擎光照,而且输出前 clamp 到 100

```glsl
v576 = clamp(v174, vec4(0.0), vec4(1.0e+02));    // ★ 上限 100,不是 1
v581 = v13.xyz * pc6_m[41].xyz;                  // 整体再乘一个材质颜色
v582 = sqrt(v579 * vec3(h3));                    // h3 是曝光(uniform)
v583 = (h3 < 1.0) ? v582 : v579;
out_Target0 = v583;  out_Target1 = gl_FragCoord.z * h4;
```

**整条 shader 里没有任何引擎光照乘法** —— 没有 `MobileDirectionalLight` 的
`N·L × 光色` 那一套。明暗全部由 §4.3 的 `h89` 在**两组材质颜色之间 lerp** 出来
(暗色 `pc6_m[9]/[11]/[13]/[15]` ↔ 亮色 `pc6_m[8]/[10]/[12]/[14]`)。
也就是说这个材质实质上是**自定义着色 + 走自发光通道输出**。

这一条推翻了 rocom-pets 的一个基础假设,见 §7。

## 5. 工具

`scripts/glsldump.py`:

```sh
uv run python scripts/glsldump.py <库> --index          # 建结构索引(全量 41s)
uv run python scripts/glsldump.py <库> --info 3119      # 一条的概况
uv run python scripts/glsldump.py <库> --extract 3119 -o /tmp/s.glsl
uv run python scripts/glsldump.py <库> --grep 'refract' # 全量搜源码
```

索引写在库文件旁的 `*.index.json`,记录每条的采样器、打包常量分组、输入/输出语义、行数。

## 6. 关于"直接套用 shader 产物"

原始动机是不再模拟算法、直接跑游戏的 shader。实测下来这条路的成本主要不在 shader 本身:

一条宠物基座 pass 的 pixel shader 要 7 个常量缓冲,**声明 295 + 80 + 52 + 31 + 16 + 3 + 75
= 552 个 vec4,但实际只读到 22 + 2 + 4 + 7 + 7 + 2 + 35 ≈ 79 个**。`View` 那个巨型结构体
绝大部分是天光/大气 LUT/时序抖动,宠物不碰。其中 `MaterialCollection0/1` 对应
`MPC_*` 资产,解包目录里就有,能直接读出来。

真正的拦路虎仍是 `Material` 那 35 个槽的值 —— 也就是 `docs/shader.md` 里那堵墙。
GLSL 这边**没有绕开它**:`pcN_*` 是重排过的,反而比 DXBC 的 `cb6[i]` 更难与参数名对齐。

所以当前结论:**GLSL 的价值是"把公式读明白",不是"把 shader 搬过来跑"。**
按 §4 把 rocom-pets 的近似逐条换成读出来的公式,比移植整条管线的收益/成本比高得多。

## 7. 试着把 §4.1 那条曲线落进 rocom-pets:失败,留档

按 §4.1 改了两处(都是**读出来的**,不是调出来的):

- `pow(albedo, 2.2)` → `filmic_inv(pow(albedo, 2.2))`;
- 末尾的 extended Reinhard 软肩 → 游戏那条 `filmic()`,并把 `sqrt` 换成真正的 sRGB 编码
  (`sqrt` 只是基座 pass 写 8 位 RT 的编码,后处理会平方回去再 tonemap + sRGB)。

`AMBIENT` / `EXPOSURE` 用原来那两个测量点(亮部出 1.0、暗部出 0.72)重解,**不再自由**:

| | AMBIENT | EXPOSURE |
|---|---|---|
| 旧(extended Reinhard + sqrt) | 1.5 | 0.4816 |
| filmic + sqrt | −0.025 | 0.964 |
| **filmic + sRGB** | **−0.087** | **1.006** |

两个凭空标定的自由量同时塌到 0 和 1 —— 这本身是个很强的旁证:原来的
`AMBIENT = 1.5` 干的活,就是这条 filmic 曲线的活。

15 只对照也好了:

| | 亮度 | 调色板 | 描边 | 对比 | 全库过曝 |
|---|---|---|---|---|---|
| 基线 | 0.93 | 0.077 | 0.95 | 1.01 | **4** |
| filmic + sRGB(推导值) | **1.02** | **0.076** | **0.97** | 1.03 | **65** |

**但全库过曝从 4 炸到 65**,集中在纯白/冰系那批(雪绒鸟 0.40、雪灵兽 0.31)。
原因是 `filmic_inv(1.0) = 1.422`,再乘上亮部 1.41 就是 2.0,直接削平。

压曝光能把过曝压回来,但那是**拟合**,正是要消掉的东西:

| EXPOSURE(AMBIENT=0) | 亮度 | 调色板 | 描边 | 对比 | 过曝 |
|---|---|---|---|---|---|
| 0.78 | 0.95 | 0.071 | 0.97 | 1.06 | 16 |
| 0.68 | 0.90 | 0.070 | 0.99 | 1.10 | 5 |
| 0.60 | 0.85 | 0.083 | 1.03 | 1.12 | 2 |

**真因不在曝光,在 §4.8**:rocom-pets 的 `shade = mix(0.5, 1.5, lit) + AMBIENT` 是一个
**乘性**明暗,而实机根本没有这一乘 —— 明暗是在两组材质颜色之间 lerp。曲线换对了、
乘性明暗还在,白色底色就必然被顶穿。

⇒ **已全部回退**,基线恢复到 0.93 / 0.077 / 0.95 / 1.01、819 形态 0/1/4。
要落这条曲线,得先把 §4.3 + §4.8 的着色结构一起换掉。

## 8. 那八个颜色的值:**不必**走 cb,它们有名字

§7 末尾原本写"卡在 cb 那堵墙上",这是错的。`h89` lerp 的那几组颜色是**具名材质参数**,
CUE4Parse 从 MI 的 `VectorParameterValues` 直接读得出来 —— 和导出器现在读
`MatCapColor` / `StarColor` 是同一个通道,**完全绕开 `uniexpr` 那套 cb 槽位反推**。

幽星光 `_By` 的名字表里就有:

```
MetalLightColor / MetalDarkColor / MetalShadowColor / MetalBright / MainBright
```

### 一个差点重犯的坑

`matparams.py` 从冻结块里读出来的是

```
MetalDarkColor = (0.2, 0.0, 0.23)   MetalLightColor = (0.0, 1.0, 0.1005)
MetalShadowColor = (0.1, 0.002, 0.04)
```

一组很鲜艳的紫/绿/红。**这是父材质的值,不是这只宠物的** —— 正是
`docs/shader.md` 记过的那条:宠物 MI 的 uexp 里带着整条父链的 shader map。
它自己的覆盖值从属性 JSON 读出来完全是另一回事:

```
MetalLightColor  = 0.984, 0.984, 0.984     (近白)
MetalDarkColor   = 1.0,   0.963, 0.922     (暖白)
MetalShadowColor = 0.832, 0.832, 0.832     (灰)
MetalBright = 1.0   MainBright = 0.9
Parent = .../PetBase/MaterialInstance/Special/MI_P_Object_XingGuang_Emissive
```

⇒ **凡是要参数值,一律读属性 JSON / `VectorParameterValues`,不要读冻结块。**
冻结块只在"要知道 cb 布局"时才有用。

### 通用性:量对以后是 13/14,不是"星光专属"

先前写过"不通用,只有星光那一支有",**那是量错了东西** —— uexp 名字表只列该 MI
**自己的 shader map 引用到**的参数,与"MI 覆盖了哪些参数"是两回事(前者按 `_By.uexp`
grep `MetalLightColor` 只有 228/806,后者要读属性 JSON)。

按属性 JSON 重量,15 只对照里 **13 只**都覆盖了 `MetalLightColor` / `MetalShadowColor`,
而且横跨**六种不同父材质**:

| 宠物 | 亮部 | 暗部 | 父材质 |
|---|---|---|---|
| 火神 | (1.00, 1.00, 1.00) | (1.00, 0.37, 0.20) | `MI_P_Object_Fire_NoMetal` |
| 罗隐 | (1.00, 1.00, 1.00) | (1.00, 0.08, 0.47) | `MI_P_Object` |
| 菊花梨 | (1.00, 1.00, 1.00) | (0.78, 0.59, 0.21) | `MI_P_Object_NoMetal` |
| 波波拉 | (1.00, 1.00, 1.00) | (0.87, 0.66, 0.34) | `MI_P_Object_UVFlow_WPO_NoMetal` |
| 水灵 | (1.00, 1.00, 1.00) | (0.93, 0.80, 0.58) | `MI_P_Object_UVFlow_WPO_NoMetal` |
| 迪莫 | (0.51, 0.41, 0.29) | (0.10, 0.002, 0.04) | `MI_P_Object_NoMetal_Morph` |
| 魔力猫 | (0.85, 0.65, 0.75) | (0.58, 0.62, 0.54) | `MI_P_Object_NoMetal` |
| 幽星光 | (0.98, 0.98, 0.98) | (0.83, 0.83, 0.83) | `MI_P_Object_XingGuang_Emissive` |
| 曜星光 | (1.00, 1.00, 1.00) | (1.00, 0.95, 0.28) | `MI_P_Object_XingGuang_Emissive` |
| 暮星辰 | (1.00, 1.00, 1.00) | (1.00, 1.00, 1.00) | `MI_P_Object_XingGuang_UVFlow_Morph` |
| 点点 | (1.00, 0.99, 0.80) | (1.00, 1.00, 1.00) | `MI_P_Object` |
| 岚鸟 | (0.92, 1.00, 0.93) | — | `MI_P_Object_NoMetal` |

**连名字里带 `NoMetal` 的父材质也在用这两个** ⇒ 尽管前缀是 `Metal`,它们就是**通用的
亮部 / 暗部颜色**,不是金属专用。这正好对上 §4.3:`h89` 在两组材质颜色之间 lerp。

暗部色是**强烈染色**的(火神橙红、罗隐品红、菊花梨土黄) —— 而 rocom-pets 现在是
一条硬编码灰阶 `mix(0.5, 1.5, lit)`,**造不出任何色相偏移**。"调色板"这个指标量的
正是色彩保真,这多半是它长期卡在 0.077 的主因之一。

`MainBright` 则是 806/806 全覆盖(幽星光 0.9、曜星光 0.8、其余多为 1.0 或不覆盖),
导出器同样一个都没读。

### 查证结果:双色调**没有遮罩**,而且乘子是颜色不是增益

`#3119` 第 271 行(原样):

```glsl
v90 = filmic_inv(baseTex.rgb) * mix(pc6_m[9].xyz, pc6_m[8].xyz, h89);
```

**没有任何遮罩** —— 双色调是直接乘在整个底色上的一个**颜色**。
`h89` 的其余三处(287 / 289 / 299 行)是星层的颜色,与本体无关:

| 槽位 | 用途 |
|---|---|
| `pc6_m[8]` / `[9]` | **本体**亮部 / 暗部色 |
| `pc6_m[10]/[11]`、`[12]/[13]`、`[14]/[15]` | 星层的三组颜色 |

这一条同时解释了 §7 的过曝崩盘:**实机的明暗乘子是幅度 ≤ 1 的颜色,
而 rocom-pets 是增益到 3.0 的标量**(`mix(0.5, 1.5, lit) + AMBIENT`)。
曲线换对了而这个 3× 还在,白色底色必然顶穿 —— 不是曝光没调好。

### 参数名:两组候选,都可读

`_By` 的名字表里有两组语义完全吻合的:

```
根图:    MetalLightColor / MetalShadowColor              (+ MetalBright、MainBright)
图层函数:02FX_LightingColor / 02FX_ShadowColor
         02FX_LightSoftMin / 02FX_LightSoftMax           ← 正是 smoothstep 的两个界
         02FX_LightOffset
```

`02FX_` 那组是四个名字对四个槽的精确匹配(两个界 + 两端颜色),`Metal*` 那组来自根图。
`#3119` 具体用的是哪一组,取决于它是哪个材质 —— **两组都是具名可读参数,
导出器两组都导、着色时按存在与否择一即可**,不需要先定材质身份。

### 完整的替换模型(全部读自文件)

```
albedo = filmic_inv(pow(baseTex, 2.2))                       §4.1
h89    = smoothstep(LightSoftMin, LightSoftMax, (N·L + 1) * 0.5)   §4.3
body   = albedo * mix(ShadowColor, LightingColor, h89)       §8(本节)
…各层…
display= srgb(filmic(lin * 曝光))                             §7
```

`AMBIENT` 整个消失 —— 它当初同时在假冒**这条 filmic 曲线**和**缺失的明暗颜色**两件事。

### 落地第二次:管线打通了,但效果更差 —— 卡在"分界怎么来"

按 §8 整条改进 rocom-pets(导出器 → manifest → `pack.rs` → `model.rs` → `gpu.rs` → wgsl
六个文件全通,341/517 个包带上了这两个色),连同 §4.1 的 filmic 一起上。结果:

| | 亮度 | 调色板 | 描边 | 对比 |
|---|---|---|---|---|
| 基线 | 0.93 | **0.077** | 0.95 | **1.01** |
| 双色调 + filmic | 0.91 | 0.094 | 1.07 | 0.81 |

比基线**更差**,迪莫尤其崩(亮度 0.53、调色板 0.145 —— 它的暗部色是 (0.10, 0.002, 0.04),
极暗)。已全部回退,基线复核 0.93 / 0.077 / 0.95 / 1.01、819 形态 0/1/4。

**病因在导出器探针的输出里,不用猜**:

```
根num 02FX_LightSoftMin  = 0      ← "根num" = 根默认,材质并没有覆盖
根num 02FX_LightSoftMax  = 1
```

`smoothstep(0, 1, (N·L+1)/2)` 是一条**平滑的 half-lambert,根本没有 toon 硬分界**,
而实机截图明显是硬分界。所以:

- 要么 `pc6_m[46].z/.w` **不是** `02FX_LightSoftMin/Max`(名字对得上,值对不上);
- 要么真正的分界另有来源。探针里还有 **`RampTex`** 与 **`RampID2 = 33.0`** ——
  很可能明暗是**一张 ramp 贴图查表**(`RampID` 选第几行),`h89` 只是查表的输入之一。

### ramp 假说:已推翻

`RampTex` 查过了,**不是硬分界的来源**。

探针显示它是**根默认**(MI 没有覆盖)的一张共享图集
`AnimSequence/Human/CommonTex/CommonRampTex/T_AllDebugRamp`,256×256,一行一条 ramp;
幽星光的 `RampID2 = 33` 是 MI 覆盖的行号,说明这一层确实在用。

把整张图集扫了一遍:

| | |
|---|---|
| 有硬台阶(逐像素跳变 > 0.3)的行 | **0 / 256** |
| 行内色彩跨度 > 0.3 的行 | **0 / 256** |
| 全图最大逐像素跳变 | 0.067 |
| 整体亮度 < 0.9 的行 | **0 / 256** |
| 幽星光用的行 33 | 跳变 0.024、跨度 0.122、近白 (0.93~0.98) |

⇒ 这是一张**轻微的色调偏移表**(暖↔冷),不是明暗分界。名字里的 `Debug` 也提示
它可能就是个没被替换掉的占位图。

### 光方向读不到 —— 这解释了 §8 落地为什么必然失败

`h89` 的输入是 `dot(N, pc5_m[5].xyz)`,而 `pc5` 是 **MaterialCollection**。
这些是 pak 里的 `MPC_*` 资产,值本该可读。导出 `MPC_S_Global` 一看:

```
C_MainLight_Direction   = (0, 0, 0, 0)      ← 全零
C_MainLight_Color_Intensity = (1, 1, 1, 1)
C_RimColor_LightColor   = (0.918, 1.000, 0.978, 0)
C_RimColor_DarkColor    = (0.701, 0.271, 0.776, 1.0)
C_RimColor_Control      = (0.3, 1.5, 0.26, 0.7)
C_RimColor_Dir          = (0, 0, -1, 0)
标量:C_CharacterShadowInt=1.0、C_CharacterInShadowColorInt=1.0、
      C_CharacterEnvSkyInt=0.8、C_CharacterMainLightInfInt=1.0、C_CLVNormalInt=0.5 …
```

**`C_MainLight_Direction` 是全零 —— 它是运行时由蓝图写入的状态,不是资产数据。**

这是个硬限制:`h89` 的输入永远无法离线求出。而实机参考截图是在**某个未知的游戏内光照方向**
下拍的,所以:

> 拿一个**强烈染色**的暗部色,配一个**猜出来的**光方向,颜色只会往错的方向搬。

§8 那次落地调色板从 0.077 掉到 0.094,原因就在这儿 —— 不是双色调这个结构读错了,
是它的**输入拿不到**。原来那条灰阶 `mix(0.5, 1.5, lit)` 之所以"能用",正是因为
灰阶对光方向不敏感:方向错了只是明暗位置错,不会串色。

**⚠ 上面这个推断错了,已被实测否掉 —— 见下。**

### 光向估准之后重试:**还是不行**,所以病因不在光向

光向确实能从截图**估**出来(读不到 ≠ 估不出):rocom-pets 的 `light_dir` 原来是
`(-0.4, 0.8, 0.6)` 这个猜值,**X 的符号是反的**;翻正之后调色板 0.077 → 0.060
(15 只普遍变好,`a05f02e`)。于是把 §8 整套按同样的六个文件重打一遍再测:

| | 亮度 | 调色板 | 描边 | 对比 |
|---|---|---|---|---|
| 基线(光向已修正) | 0.95 | **0.060** | 0.96 | 0.87 |
| 双色调 + 旧光向 | 0.91 | 0.094 | 1.07 | 0.81 |
| 双色调 + **新光向** | 0.92 | 0.092 | 1.08 | 0.81 |

**两次几乎一样**(0.094 → 0.092)。⇒ 光向不是病因,上面那句"输入拿不到所以必然负收益"
是过度归因。已再次全部回退,基线 0.95 / 0.060 / 0.96 / 0.87、819 形态 0/1/5。

那么真正的病因还在这三者之一(按可能性排):

1. **`smoothstep` 的两个界不对**。`02FX_LightSoftMin/Max` 是根默认 0/1,
   `smoothstep(0, 1, (N·L+1)/2)` 就是一条平滑 half-lambert、没有 toon 分界 ——
   这一条在 §"ramp 假说"里已经指出,只是当时误以为 ramp 能补上。
2. **`pc6_m[8]/[9]` 未必就是 `MetalLightColor`/`MetalShadowColor`**。名字语义吻合,
   但 `#3119` 到底是哪个材质从未确认(结构指纹只锁到"宠物族"),两组候选
   (`Metal*` 与 `02FX_*`)也一直没分开。
3. **`#3119` 未必是本体材质**。它有星层折射视差,更像 `_Fx`;而双色调那一乘
   是不是同样出现在 `_By` 上,没有单独查证过。

### 病因找到了:**§4.1 的解读是错的**,那一项有强度门控

不必去钉 `#3119` 的身份 —— DXBC 那边归属本来就是精确的。拿**确认是 `_By`** 的
shader(`matshader` 配 `MI_Ill_XingGuang1_001_By.uexp`,取 `23766`)一看,
filmic 的常数**同样存在**(所以这条曲线本身没读错),但它后面紧跟着:

```
add  r7.xyz, r7.xyzx, l(-1,-1,-1)             ; filmic_inv(c) − 1
mad  r7.xyz, cb6[118].z, r7.xyzx, l(1,1,1)    ; = lerp(1, filmic_inv(c), 强度)   ★
mul  r7.xyz, r7.xyzx, cb6[13].xyzx
mul  r7.xyz, r7.xyzx, cb6[15].xyzx            ; 乘的是**两个**颜色,不是一对 lerp
mad  r7.xyz, cb6[118].x, -r7.xyzx, r7.xyzx    ; × (1 − cb6[118].x)
```

两处与 §4.1 / §8 的实现假设直接冲突:

1. **有强度门控 `cb6[118].z`**。整项是 `lerp(1, filmic_inv(c), 强度)` ——
   强度为 0 时**退化成 1.0,什么也不做**。我把它当成无条件的底色解码套了上去,
   形状就错了。又是一次"代码在字节码里 ≠ 这一层生效"。
2. **乘的是两个独立颜色 `cb6[13]` × `cb6[15]`**,不是 `mix(暗, 亮, h89)` 那样的一对。
   §8 把它实现成一对颜色的 lerp,结构也不对。

⇒ 这解释了为什么两次落地都变差、且**换光向也救不回来**:错的不是输入,是形状。

`#3119` 与本体 shader 的差异应当就是材质不同(它带星层折射视差,更像 `_Fx`)。
**教训:GLSL 好读,但归属只能靠结构指纹;凡是要落地的公式,应当回到 DXBC 那边
用 SHA1 精确归属的 shader 复核一遍。** GLSL 负责"看懂",DXBC 负责"认准"。

### 定名结果:名字猜对了,但**两个颜色是相乘不是 lerp**

`uniexpr` 解 `MI_Ill_XingGuang1_001_By.uexp`,块 1/4(V=83)给出:

```
cb[13] = MetalLightColor   (0, 1, 0.1005, 1)
cb[15] = MetalShadowColor  (0.1, 0.002, 0.04, 0)
```

名字与 §8 的猜测一致。**但 DXBC 那两行是**

```
mul r7.xyz, r7.xyzx, cb6[13].xyzx
mul r7.xyz, r7.xyzx, cb6[15].xyzx
```

—— **相乘**,不是 `mix(暗, 亮, h89)`。§8 把它实现成一对颜色的 lerp,结构从一开始就错。
这也解释了迪莫为什么崩得最狠:它的 `MetalShadowColor = (0.1, 0.002, 0.04)`,
作为 lerp 的一端只在暗面生效,作为**乘数**则是全身压暗 —— 两种形状差别巨大。

另外 `cb6[118].z`(块 2/5 的 V=108 布局)解出来是一个含
`CustomColorORMatcapColor` 的运算,而探针显示这只宠物
**`CustomColorORMatcapColor = 0.0`**。若门控确实由它给,则
`lerp(1, filmic_inv(c), 0) = 1` —— **整项对这只宠物是空操作**。

**须注明的不确定性**:没有任何块的 V=116,也就是 shader `23766` 那个排列
仍然没有补丁支撑(老墙)。上面的槽位来自 V=83 / V=108 的排列,而 `cb[13]`
在三组块里给出三个不同答案(`FX_Emissive` / `MetalLightColor` / `BlackMagicRimColor`)
—— 取的是"与本材质实际覆盖的参数吻合"的那一组(§"配对判据"),**不是定论**。

### 门控查实了:**是 1,这一项全开** —— 顺带把四个 opcode 定了名

`CustomColorORMatcapColor` 的**根默认是 0**(探针 `根num`),采样到的宠物里只有暮星辰
覆盖成 1.0。所以门控是 0 还是 1,全看 `op0x05` 是哪个运算 —— 这一步以前一直含糊。

杠杆在同一个块的 `cb[124]`:

```
.x = f(01FX_DissU_Offset) op0x1a 常量(-1) op0x05
.y = 同上 常量(2) op0x06
.w = 同上 常量(1) op0x06
```

`.w = t op0x06 1`:若 `op0x06` 是 **Mul 或 Div**,`.w` 就恒等于 `.x`,一个冗余槽 ——
编译器不会这么排。⇒ **`op0x06 = Sub`**。配上已知 `op0x07 = Mul`
(在 `标量 PointLightIntLocal × 向量 FX_Emissive .xyz` 那处),落到 UE 的常见排布:

| opcode | 运算 | 依据 |
|---|---|---|
| `0x05` | **Add** | 由 0x06/0x07 夹出 |
| `0x06` | **Sub** | `t ⊖ 1` 不能是 Mul/Div(否则冗余) |
| `0x07` | **Mul** | 标量 × 向量那处 |
| `0x08` | Div | 顺位(未单独验证) |

⇒ `cb6[118].z = CustomColorORMatcapColor + PointLightIntLocal = 0 + 1 = **1**`,
即 `lerp(1, filmic_inv(c), 1) = filmic_inv(c)` —— **这一项是全开的,不是空操作**。
(上一版猜"门控恒为 0、这层不画",反了。)

⇒ 所以 §4.1 那条曲线**该用**;两次落地失败的唯一病因是 §8 的形状:
`cb6[13]` 与 `cb6[15]` 是**相乘**,而我实现成了 `mix(暗, 亮, h89)`。
下一次实现应当照 `albedo → filmic_inv → × MetalLightColor × MetalShadowColor` 来,
并且先在 DXBC 里把 `h89` 那一乘到底出现在哪一步查清楚(本体 shader 里未必在同一处)。

### 又撞回老墙:这一支挂在一串**模式选择**里,而选择器命名不可靠

追 `23766` 的数据流:`r6` 在第 94 行是 `sample(t2, v2.xy, s1)` —— **底色贴图**,
所以 §4.1「filmic 的逆作用在底色贴图上」在本体材质里也成立。但它后面是:

```
r7 = lerp(1, filmic_inv(底色), cb6[118].z)     ; 门控(= 1,见上)
r7 = r7 × cb6[13] × cb6[15]                    ; × MetalLightColor × MetalShadowColor
mad r7, cb6[118].x, -r7, r7                    ; = r7 × (1 − cb6[118].x)
```

而它**前面**还有一串同构的:

```
mad r5, cb6[117].y, r5, r6                     ; lerp(r6, r5·…, cb6[117].y)
mad r5, cb6[118].x, (cb6[10] − r5), r5         ; lerp(r5, cb6[10], cb6[118].x)
mad r5, cb6[118].y, (cb6[11] − r5), r5         ; lerp(r5, cb6[11], cb6[118].y)
```

这是一组**互斥的模式选择**,`cb6[117]/[118]` 那几个标量是模式开关。
按 V=108 那块的命名它们叫 `PointLightIntLocal` / `MainBright` —— 语义明显对不上
(一个"点光强度"不会去选颜色模式),**说明这套命名不适用于真正的排列**。

而真正的排列是 **V=116**(`cb6[153]`,由 `V + ⌈S/4⌉ + 1 ≥ 153` 与
`0x06f800` 那条 116 项向量链定出),它**没有补丁表支撑的冻结块** —— 正是
`docs/shader.md` 记了多次的那堵墙。取 V=83/108 的命名去读 V=116 的槽位是**错位**的,
上面 `cb6[13]/[15]` 那两个名字同样只是"看起来吻合",不能当定论。

⇒ **结论:这一层暂时不可实现。** 不是公式没读懂,是**槽位↔参数名在这个排列上对不齐**,
而 `cb6[117]/[118]` 恰恰全是决定"哪一支生效"的开关 —— 猜错一个,整片颜色就换一种画法。

顺带一条正面印证:`C_RimColor_Dir` 的 `.w = 0`,而 GLSL 里
`v63 = mix(材质自己的, pc5_m[6], pc5_m[9].w)` —— 所以**全局那套边缘光默认是关的**,
走材质自己的参数。rocom-pets 现在读 `Rim Power` / `Rim Intensity` 是对的。

### 那么硬分界到底从哪来:还没有答案

三条已排除:①`smoothstep(02FX_LightSoftMin, ...Max, ·)` —— 两个界是根默认 0/1,
出来是平滑 half-lambert;②`RampTex` —— 整张图集没有台阶;
③乘性标量明暗 `mix(0.5, 1.5, lit)` —— 那是我们自己发明的,实机没有。

还没查的方向:
- `pc6_m[46].z/.w` 的**真正**参数名(`02FX_LightSoftMin/Max` 只是名字对得上、值对不上);
- 实机截图里那道"硬边"是否根本不来自材质,而来自**投影阴影**(shadow map)或
  `MaskTex` 的离散 ID 台阶 —— 后者已知是离散的(见 rocom-pets 里暮星辰那条)。

**在把这一条弄清楚之前,不要再往 `shade` 上试值。** 这一轮已经证明:
结构没读对时调参数只是换个方向错。

### 顺带:一批没被读的具名参数

幽星光 `_By` 自己覆盖的标量里还有
`Rim Power = 1.7`、`Rim Soft Edge = 0.5`、`Rim Intensity = 1.0`、`Offset Percent = 0.5`、
`SpecPow2 = 0.8`、`SpecIntensity2 = 8.0`、`RampID2 = 33.0`,导出器大多没读。
另外 `Emitter Intensity = 0.0` —— 这只宠物的自发光确实是关的,
又一例"参数在但这一层不可见"。
