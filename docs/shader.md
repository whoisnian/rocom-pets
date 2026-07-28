# 从 pak 读出游戏的 shader

**为什么要做这件事。** [rocom-pets](https://github.com/whoisnian/rocom-pets)(桌宠)要还原宠物的
材质观感,而 cooked 包里**材质图是被剥掉的**(那是 editor-only 数据):CUE4Parse 能给的只有
参数值、静态开关、贴图引用,**公式没有**。于是「`Rim Intensity` 到底怎么参与」「星点强度是多少」
这类问题只能靠对着截图猜 —— 而猜法在这个项目里连续翻车过好几次(见 rocom-pets 的
docs/design.md §1)。

公式的唯一离线来源是**编译产物**:shader library 里的 DXBC 字节码。静态开关在编译期已经定死,
所以那就是「这个材质实际跑的是什么」。整条链路已经打通,**不需要抓帧、不需要 Windows、
不需要 root,零封号风险**。

解包数据来自游戏的 **Windows 客户端**,shader 平台是 `PCD3D_ES31`(D3D11 跑 ES3.1 特性级),
字节码是 DXBC。

## 数据在哪

```
NRC/Content/ShaderArchive-NRC-PCD3D_ES31.ushaderbytecode      229 MB  项目 shader
NRC/Content/ShaderArchive-Global-PCD3D_ES31.ushaderbytecode      5 MB  全局 shader
NRC/Content/PipelineCaches/Windows/NRC_PCD3D_ES31.stable.upipelinecache   18 MB
Engine/GlobalShaderCache-PCD3D_ES31.bin
```

都不在 `unpack.sh` 的默认导出范围里(默认排除三维美术/视频/音频那一批),要单独导:

```sh
./scripts/unpack.sh --out <dir> --no-exclude --no-post --filter "NRC/Content/ShaderArchive"
```

`.ush`/`.usf` 源码**不在包里**(cooked 会剥掉),尽管根材质的属性里能看到
`CustomShadingModelPath = /Project/NRC_NPR/NRC_Pet_Trans_New.ush` 这样的路径。
那个 `stable.upipelinecache` 里**只有哈希、没有 stable 字符串键**,给不出「shader ↔ 材质」的映射。

## 流水线

四步,每步一个工具:

| 步骤 | 工具 | 干什么 |
| --- | --- | --- |
| ① 取 shader | `scripts/shaderdump.py` | 解 archive 的表、LZ4 解压、抽出 DXBC |
| ② 认归属 | `scripts/unpack.sh --raw` + `scripts/matshader.py` | 哪条 shader 属于哪个材质 |
| ③ 反汇编 | `scripts/dxbcdis.c`(winegcc + wine) | DXBC → `ps_5_0` 汇编 |
| ④ 对语义 | `scripts/dxbcsig.py` | 汇编里的 `vN`/`oN` → `TEXCOORD3` 这种语义名 |

### ① 取 shader

```sh
uv run python scripts/shaderdump.py <dir>/NRC/Content/ShaderArchive-NRC-PCD3D_ES31.ushaderbytecode
uv run python scripts/shaderdump.py <archive> --extract 34529 -o /tmp/s.dxbc
uv run python scripts/shaderdump.py <archive> --find Material,MobileBasePass --freq 3
uv run python scripts/shaderdump.py <archive> --verify 300
```

实测规模:**5715 个 shader map、72407 条 shader**(pixel 52453 / vertex 19936 / compute 18),
解压后合计 678 MB。

### ② 认归属

archive 里**没有任何材质名**(搜 `M_P_Object`、`XingGuang` 零命中),所以要反向比对:

```sh
# 材质导成原始字节 —— shader map 的 SHA1 在 CUE4Parse 不解的 cooked resource 段里,
# 所以必须 --raw(它让包也按原始字节导,不转属性 JSON)
./scripts/unpack.sh --out <dir> --raw --no-exclude --no-post \
    --filter "NRC/Content/ArtRes/AnimSequence/Pets/<资产>/Mat"
```

然后拿 archive 的 5715 条 `ShaderMapHashes` 逐条 memmem 那个 `.uexp`,命中的就是这个材质的
shader map。实测幽星光的 `Fx1` 命中 24 个、`By` 18 个、眼睛 1 个,嘴 0 个(它的 uexp 只有
1786 字节、没有自己的静态排列);**不同材质之间基本不重叠**(`By` 与 `Fx` 共用 2 个,
同父族的相同排列,合理)。20 字节碰巧撞上的概率可以忽略。

这一步已经做成脚本:

```sh
uv run python scripts/matshader.py <archive> <dir>/…/MI_<材质>.uexp --groups   # 看有哪些排列
uv run python scripts/matshader.py <archive> <dir>/…/MI_<材质>.uexp --out /tmp/s
```

**挑对排列很重要。** 一个材质命中的 map 去重后有上百条 pixel shader(幽星光 `Fx1` 140 条、
暮星辰的裙子 203 条),那是同一材质的不同**排列**:光照 / 雾 / lightmap / 聚簇前向着色的组合。
排列之间材质图是一样的(静态开关已定死),差的是外围光照代码,但小排列会把整段公式优化掉。
分组的依据是尾部那串 uniform buffer 名 —— 宠物在世界里跑的是

    View + MobileBasePass + MobileDirectionalLight + Primitive + MaterialCollection0/1 + Material

这一组(`matshader.py` 默认只取它),同组里取**解压后最大**的那条,指令最全。
其余组是影子深度(`MobileShadowDepthPass`,1K 上下)、无平行光、或多带
`ClusteredForwardShading` 的版本。

### ③ 反汇编

原以为要另装 RenderDoc / 3Dmigoto / DXVK,实际上 **wine 自带的 `d3dcompiler_47.dll` 就导出
`D3DDisassemble`**(wine 11 里由 vkd3d-shader 实现),编译宿主用 wine 自己的 `winegcc`:

```sh
winegcc -o dxbcdis.exe scripts/dxbcdis.c
wine ./dxbcdis.exe out/*.dxbc            # 每个 x.dxbc 出一个 x.dxbc.asm
```

一个二进制都不用下,一台 Windows 都不用碰。抽查 31 条(覆盖整个 archive 的索引区间)全部成功。

### ④ 对语义

汇编里只有 `v2.zw` 这种寄存器号;反射段 `RDEF` 被剥了、`D3DDisassemble` 也不打印语义名。
签名段 `ISGN`/`OSGN` 里有:

```sh
uv run python scripts/dxbcsig.py out/34529.dxbc
#   v2  .xyzw TEXCOORD0
#   v3  .xyzw TEXCOORD1
```

配 UE 的 UV 打包规则(`TEXCOORD0` = UV0.xy + UV1.xy、`TEXCOORD1` = UV2.xy + UV3.xy),
汇编里 `r4.xy = v2.zw; r4.z = v3.x` 就定死是 **`(UV1.x, UV1.y, UV2.x)`** —— 不用猜。

### 采样取了哪个通道:资源 swizzle 怎么读

`sample` 指令的**资源操作数**带一个四分量 swizzle,规则是 `结果[i] = 贴图[swizzle[i]]`,
再由**目标写掩码**决定哪几个分量真的写进去。所以要看的是「目标掩码那一位对应的 swizzle 字母」:

```
sample r3.xyzw, v2.xyxx, t2.xyzw, s2      → 恒等,r3 = 整个 RGBA
sample r2.w,    r4.xzxx, t3.yzwx, s3      → 目标只写 .w、swizzle 第 4 位是 x ⇒ r2.w = 贴图.R
sample_l r0.xw, v2.xyxx, t1.xzwy, s1, l(-1)  → .x←第 1 位 x = R、.w←第 4 位 y = G
```

编译器就是这么排的:想取的通道摆在目标位上,其余位拿剩下的字母填。上面第三条能交叉验证 ——
紧跟着的 `mad r0.xw, r0.xxxw, 2, -1` 把这两个数映射到 [-1,1],再与切线/副切线组合、
`nz = sqrt(1 - nx² - ny²)`,是标准切空间法线贴图,正好对应「取 RG 两个通道」。

这条规则很小,但**不知道它就会把单通道遮罩当成 rgb 查表用**:那两颗球的 MatCap 实机是
`t3.R` 一个标量乘 `MatCapColor`,我照 rgb 做过一版还自己加了「减掉 0.35 的底」,越调越闪。

## 格式细节(踩过的坑都在这)

**`FShaderCodeArchive`(version 2)**:一串 `FSerializedShaderArchive` 数组,紧跟代码区。

```
uint32                          Version(= 2)
TArray<FSHAHash>                ShaderMapHashes       20 字节/条
TArray<FSHAHash>                ShaderHashes          20 字节/条
TArray<FShaderMapEntry>         ShaderMapEntries      16 字节(4 个 uint32)
TArray<FShaderCodeEntry>        ShaderEntries         **17 字节,无对齐填充**
TArray<FFileCachePreloadEntry>  PreloadEntries        16 字节
TArray<uint32>                  ShaderIndices
─── 表结束处即代码区基址 ───
```

- **`FShaderCodeEntry` 是紧凑写的 17 字节**(u64 Offset + u32 Size + u32 UncompressedSize +
  u8 Frequency)。按 20 字节读会把后面所有表的偏移全带偏,Frequency 会读成 534019 这种垃圾值。
  校验办法:`代码区基址 + max(Offset + Size)` 应当**正好等于文件长度**,shaderdump.py 构造时
  就断言这条。
- 每条 shader 是**裸 LZ4 块**压的。格式极简,自己写 40 行比拉依赖省事;抽查 301 条解出的长度
  与表里声明的完全一致。
- 解压后的一条是:`FShaderResourceTable`(~164 字节) + **DXBC** + 尾部。DXBC 的长度在它自己
  头部第 24..28 字节。
- **尾部那串 uniform buffer 名是唯一还留着名字的东西**(`View` / `MobileBasePass` /
  `Primitive` / `MaterialCollection0/1` / `Material` / `ClusteredForwardShading`…),
  只够给 shader 粗分类,`--find` 就是靠它。
- cbuffer 数量与那串名字**一一对应**,所以**材质参数在最后那个 cb 里**;贴图槽的顺序对得上
  材质属性里的 `CachedReferencedTextures`(有序数组)。
  **别把它硬编码成 `cb5`**:序号取决于这条 shader 有几个 uniform buffer ——
  幽星光球的 34529 是 6 个(没有 `MobileDirectionalLight`)⇒ 材质 = `cb5`;
  另一条 70710 是 7 个(多了 `MobileDirectionalLight`)⇒ 材质 = `cb6`。
  照 `cb5` 一路读下去会把平行光的常量当成材质参数。
- **`Frequency`**:0 = vertex、3 = pixel、5 = compute(UE 的 `EShaderFrequency`)。

## 已经读出来的(实例)

以宠物「幽星光」一族为例,详细结论在 rocom-pets 的 docs/design.md §1:

- **`MI_P_Object_Trans_MatCap`(幽星光/曜星光那两颗球)**:开头就是 `refract()` 的教科书实现
  (`k = 1 - eta²(1-cos²)`;eta 就是每个宠物材质都写着的 `GlobalRefraction` = 1.3),
  然后沿折射光线 march(对应 `GlobalDepth` = 100),按**三向投影**采 `StarTex`
  (根默认 = `T_EMeng003`,一张四角星场、alpha 是干净的稀疏星形遮罩),采样坐标叠
  `View` 的时间 —— 这就是实机「球内有颗星、自己在动、和球自转无关」的来源。
  **球内那颗星是「闪」不是「飘」**:采样结果的三个通道各有分工 —— `G` 是每颗星的随机相位、
  `B` 是形状、`A` 是星形遮罩,亮度 = `pow(B, q) - 1.2·|sin(2π·frac(速度·时间 + G))|^次数`,再 `× A`。
  采样坐标的量纲也定得下来:`halfExtent = 0.5·|包围盒|`、`marchDist = halfExtent × 0.01 × GlobalDepth`、
  `tiling = <cb 标量>/halfExtent` ⇒ `p = start/halfExtent + 折射方向`。**`GlobalDepth` = 100 代进去
  正好让 marchDist = halfExtent** —— 那个 0.01 的配合就是「这个槽位是 GlobalDepth」的强证据。
  **固有色就是基色贴图**(× 一对按 `N·L` 两段因子 lerp 的明/暗色),两颗球的蓝/琥珀在图集里
  本来就分好了;那对「按物体空间高度 lerp」的颜色喂的是**发光/星点层**,不是固有色 ——
  完整数据流见 rocom-pets 的 docs/design.md。**从汇编读出「某个槽参与了运算」不等于知道
  「它参与的是哪一层」**:那个高度渐变色隔了 80 多行才被消费,中途寄存器还被基色采样覆盖过。
- **`MI_P_Object_Trans_XingGuang_Fresnel`(暮星辰那两颗球)**:**完全另一套** —— 223 行 /
  4 张贴图,`N·L` + 遮罩 → 一维 `RampTex` 行查(采样 v 是常数 1/256),
  既没有折射也没有三向投影。
- **`MI_P_Object_Trans_XingGuang_WPO`(暮星辰的裙子)**:774 行 / 12 张贴图,输入签名里带
  **`SV_IsFrontFace`**(两面材质)。基色 alpha 一进来就被 `saturate((a - 0.04) * 1.1111)`
  重映射 —— 那就是**不透明度**(静态开关 `Opacity or OpacityMask` 点名的那 11 个材质)。
  另有一层 `ShinyStar`(`T_PetGlassyStar_001` + HDR 色 (10,10,10) + 强度 10)= 裙子上闪的小星。
- **边缘光**:`pow(1 - saturate(N·V), RimPower) × RimIntensity × RimColor × <一个 cb 标量>`,
  再和 matcap / 高光取 `max`。注意实机用的是 **`1 - saturate(N·V)`**,背面得满强度;
  另有一路 `1 - smoothstep(0.05, …, N·V)` 的软边遮罩把它收到轮廓附近。
- **身上那层星点(`MI_P_Object_XingGuang_FakeTrans01`,shader 27803,7 个 ub ⇒ 材质 cb 是 cb6)**
  —— 第 375~403 行,整段读全了:

  **槽位现在全带名字了**(名字表破了之后,把同一段代码在 `MI_P_Object_Masked` 的 27931 里
  配到块 9 —— `V=83` 正好 = 汇编最后一个 ≥3 分量 swizzle 槽位 82 + 1,`83+⌈134/4⌉+1 = 118 ≥ 117`
  唯一命中):

  ```text
  r12.w = frac(cb0[153].z * 0.25)                  ← View 时间 × 硬写的 0.25(4 秒一周)
  θ     = r12.w * 2π
  k     = 1.1 * lerp(|sin θ|, |cos θ|, tex.g)
  uv    = v2.xy * StarStickTiling(= 4)             ← v2 = **网格 UV0**,乘的是**一个标量**
  x     = saturate((tex.b * (k - tex.r) - 0.01) * 25)
  m     = x²(3 - 2x)                               ← smoothstep,×25 造出很硬很细的边
  c     = 4 段渐变,t = saturate(k),每段 ⅓ 宽:
          StickRandomColor02 →(⅓) StickRandomColor03 →(⅔) StickRandomColor04 →(1) 00FX_BaseColor
  出    = lerp(底色, Stick_Intensity(= 1.5) * m * c, saturate(m + GlassyMainColorOpacity(= 0)))
  ```

  幽星光 `Fx1` 的四个色(与根默认一致、没被覆盖):
  `01 = (0.946, 0.064, 0.021)` 红 → `02 = (0.960, 0.160, 0.907)` 洋红 →
  `03 = (0.049, 0.155, 0.977)` 蓝 → `04 = (0.925, 0.742, 0.027)` 黄。

  **这里一度记成「`02 → 03 → 04 → 00FX_BaseColor`(白),`01` 不在渐变里」,那是错的** ——
  向量参数数组被少认了两条(见下面「两个参数数组互相冒充」),槽位名**整体错位一格**。
  修完是连号且有序的 `01→02→03→04`,比原来那个「三个浓色 + 一个不相干的 FX 参数」自洽得多。
  **教训**:名字解出来后要看它**整体是否自洽**(连号、语义对位),单个槽位「看着像」不算数。

  **还有一条更要紧的**:这四个色属于 **`StarStickTex` 那一族**。宠物身上还有一族走
  **「假半透」**(`MI_P_Object_XingGuang_FakeTrans01/02`:`NoiseTex` + `Color02`),
  它的颜色是 `Color02`(曜星光 = (10, 8.07, 9.04))**不是这四段渐变**。
  两族的贴图通道语义也不同(`StarStickTex` 是彩色星形色块图集;`NoiseTex` 纯黑底、
  r/g/b = 阈值/相位/幅度)。rocom-pets 那边不分族地套了同一套渐变,三只星光族的调色板
  距离立刻退步(0.086→0.115 / 0.078→0.129 / 0.082→0.094);按族分开后反而**好过**原来
  (0.081 / 0.063 / 0.058)。
  ⇒ **「公式读对了」不等于「这条公式属于这个材质」**,读完先确认它属于哪一族。

  **读这段有个坑**:`sample` 写的是 `r11.xyz`,把上面 `sincos` 存进 `r11.x` 的 sin **覆盖掉了**,
  所以后面 `add r3.z, -r11.x, r3.z` 减的是 `tex.r` 而不是 sin。由此贴图三通道的分工是
  **r = 每颗星的阈值、g = 相位混合、b = 幅度**(实测那张 512² 的 `NoiseTex`:三通道基本共位、
  都是连续 0..1、alpha 恒 1 未用)。
  **这层不是「细碎星点」而是一层脉动的大面积柔光**:`m` 在 k=1 时覆盖贴图的 29.6%(k=0.4 时 10%),
  而且它是 **lerp(替换底色)不是加性**、混合系数就是 `m` 本身 —— 靠那 4 段渐变色着色才成立。
  rocom-pets 照抄过一版整只糊白,原因就是缺这 4 个色槽、拿 `min(r,g,b)` 当形状凑。
  同段的另外两个标量是**别的层**的:`StarDensity`(= 8)、`StarIntensity`(= 1)属于球内折射星,
  和这层无关 —— 「同一个 cb float4 里的四个分量属于同一层」是错的。
- **「cb 槽位 ↔ 参数名」这条链路已经端到端走通一次**(2026-07-28)。三步:
  ① 用 `scripts/uniexpr.py --cb` 解块 —— 布局是 `[向量 V 条,每条一个 float4][标量 S 条,4 条一个
  float4][UE 追加的一个 float4]`,两条 preshader 头链共用一个 opcode 缓冲,**首 opcode 偏移
  为 0 的那条是标量链**;`03 <uint16>` = 标量参数、`04 <uint16>` = 向量参数。
  ② 配对:`V = 1 + 材质 cb 里最后一个以 ≥3 分量 swizzle 出现的槽位`,且
  `V + ceil(S/4) + 1 >= dcl_constantBuffer 声明大小`。实测 32 条 shader 全部唯一定到块。
  ③ 用 `scripts/matparams.py` 把补丁表里的 paramId 经名字表对到名字。
  **验证**:宠物材质 `MI_Ill_XingGuang1_001_Fx1` 的 shader 19422(cb5[109] ⇒ V=79)配到块 5,
  其 `cb5[34]` 解出向量参数 15 = (9,0,5,0)、paramId 112,而根默认 `FragmentsColor` = (9,0,5,0)、
  名字表的 `[112]` 正是 `FragmentsColor`;同块 `cb5[35]`
  (`Direction`)与 `cb5[36]`(`FlowColor`,(0.5,0,0.6,0))也各自对上。
- **`Special/` 下的材质解不出冻结块**(`MI_P_Object_XingGuang_FakeTrans01/02`):全文件一条
  16 字节的名字补丁串都没有、参数记录数组也是 0 段。**破法:同一段代码也在
  `MI_P_Object_Masked` / `MI_P_Object_Trans` / `M_P_Object` 里**,而这些冻结块解得开(各 12 个)。
  在 `MI_P_Object_Masked` 的 27931 里对应槽位是:平铺 `cb6[96].z`、强度 `cb6[96].w`、
  混合下限 `cb6[97].x`、渐变四色 `cb6[38]→[37]→[40]→[42]`。找同一段代码的特征串用
  `l(0,0,3,3), l(0,0,-1,-2)`(那个 4 段渐变)—— 比 `×25` 可靠,后者在镂空遮罩那儿也有。
- **一个块可以有三条 preshader 头链,不是两条。** `M_P_Object` 那一族每块是
  `(63@989, 72@0, 17@0)` 这种形状 —— 第三条 17 条的是别的东西(还没认出是什么)。
  「首偏移为 0 的是标量、另一条是向量」这条判据在两条链时够用,三条时会挑错
  (挑成 17 那条,`V` 恒等于 17)。**可靠的判据是「首偏移非零的那条就是向量链」** ——
  这样得出的 V 列表 63/71/90/90/104/117 与汇编数出来的完全一致。
  想用更强的「向量链首偏移 == 标量链 opcode 总长」在这一族凑不上(777 + 95 ≠ 989),
  所以两条判据都留着,强的优先、弱的兜底。
- **冻结区起点不能取「第一个 `FrozenSize` 匹配」。** 随便一个 uint32 都可能碰巧等于距离。
  要用自检打分挑(补丁偏移落在参数记录头上)。**而且自检不总是 100%** ——
  有些补丁指向贴图参数一类别的记录:`MI_Ill_XingGuang1_001_By` 三个块是
  87/98、104/107、125/136(≈90%),幽星光 `Fx1` 才是满分。所以门槛只用来筛假锚点。
  改成打分挑之后,全量 275 个材质里 88 个有块、合计 736 块(比取第一匹配少 4 个文件,
  少掉的正是假块)。
- **`M_P_Object` 那一族:大排列的块没有补丁表,所以它们的 cb 槽位叫不出名字。**
  文件里的排布是「6 个带补丁的块 + 6 段空隙,每段空隙里有一组更大排列的 preshader 链」。
  以 `MI_Ill_XingGuang1_001_By` 为例:带补丁的块是 V=81/83/108(而且**整份存了两遍**,
  后三块与前三块字节相同),空隙里按文件序是 81 → 83 → 108 → 108 → **116** → 135,
  一个补丁表都没有(把补丁记录的 `N` 上限从 8 放宽到 1024 重扫,仍然只有 6 个表)。
- **一个宠物 MI 的 uexp 里混着它整条父链的 shader map,所以「在同一个 uexp 里配对」也不够。**
  踩得很实:水蓝蓝的水体材质 `MI_Wat_ShuiLanLan2_001_Fx` 有 **27 个块**,我把它的 shader 11159
  (V=71、`dcl cb6[94]`)配到块 5,解出 `RedChannel` = (0, 0.93, 0.83) 青、`GreenChannel` 蓝、
  `BlueChannel` 橙,还顺势推出「这就是实机偏青的来源」——**全错**。
  证伪只用了一步:去看这个材质**自己覆盖了哪些参数**(属性 JSON),答案是
  `Color1` / `Color2` / `Main Color` / `CausticsInt` / `FlowDistort` / `FresnelInt` /
  `FresnelPower` / `U_Tiling_Caustics` … **一个 `RedChannel` 都没有**;而块里那个
  (0.1, 1, 0.5)(它真正的 `RedChannel` 值)在 27 个块里**一次都不出现**。
  ⇒ 那些块全是父链(`MI_P_Object_NoMetal` / `MI_P_Object` / `M_P_Object`)的。

  **所以配对必须加一道验证:块里应当能找到这个材质自己覆盖过的那些值。**
  这条同时也是「跨材质配对不成立」那条的推广 —— 判据「32 条 shader 全部唯一定到块」
  只在**材质自己确实有内联块**时才成立。
- **水体那一族的参数在「材质图层」作用域里,现在的工具读不到。** 名字表(214 条)里
  压根没有 `Color1` / `CausticsInt` —— 因为水体是一个**材质图层**
  (`ML_P_StylizedWater`,在 `PetBase/MaterialLayer/` 下),它的参数带
  `Association` = 0/1(LayerParameter / BlendParameter)、`Index` = 图层下标,
  而 `param_arrays` 只认 `Association == 2`(GlobalParameter)。
  证据量级:`MI_P_Object_NoMetal` 里 `Index=0 / Association=0` 的记录有 **29076 条**,
  而 `Index=-1 / Association=2` 只有 1228 条。
  `param_arrays` 已经支持了(`layers=True`),实测 92 个有冻结块的材质**每一个**块内
  都有图层作用域的数组。**但这条路对水体不通**:水体预设的 `Color1` 值在
  `MI_Wat_ShuiLanLan2_001_Fx` 里只出现在 `CachedExpressionData`(0x1238,而第一个块起点是
  0x589c),27 个块里**一次都没有** —— 那个材质没有自己的内联 shader map。

## 水体预设:整条链已读全(2026-07-28)

**怎么配上的**(前面两条坑都绕过了):水体是材质图层 `ML_P_StylizedWater`,所以要
`param_arrays(..., layers=True)`;而 `MI_Wat_ShuiLanLan2_001_Fx` 的 27 个块里只有
**块 9(V=56)与块 15(V=83)** 带水体层参数,其余是父链的。块 15 配到 shader **35663**
(6 个 ub ⇒ 材质 `cb5`、`dcl cb5[106]`、末向量槽 82 ⇒ V=83,`83 + ⌈90/4⌉ + 1 = 107 ≥ 106`)。
**可信度**:解出来的参数集(`Main Color` / `Color1` / `Color2` / `U,V_Tiling_Caustics` /
`U,V_Speed_Caustics` / `CausticsInt` / `FlowDistort` / `FresnelPower` / `FresnelInt`)
与那个材质在属性 JSON 里**自己覆盖的那一整套逐个对上**。
(我之前反汇编的 11159 / 20009 / 26124 那几条 V=71/104 的都是**父链**的排列,别再用。)

```text
mask      = saturate((BaseTex.a − 0.04) × 1.1111)        ← 和不透明度同一个重映射
层1       = mask × Color1                                ← cb5[2]
caustics  = 两次采 t4:
              uv1 = UV × (U,V_Tiling_Caustics) + frac(时间 × (U,V_Speed_Caustics))
              c1  = sample(uv1) × CausticsInt
              d   = sample(t4, UV) × 0.5
              uv3 = UV × cb5[85].yz + frac(时间 × …) + d × FlowDistort
              c2  = sample(uv3)
              caustics = (c1 × c2 + c2) × 0.5
层2       = caustics × Color2 × maskVariant              ← cb5[4];maskVariant 由
                                                          `Inv Opacity` / `UseOpacityAsMask` 选
出        = 层1 × **Emitter Intensity** + 层2             ← cb5[83].x
菲涅尔     = lerp(HardLineCol, FresnelColor, pow(N·V, FresnelPower)) × FresnelInt
            × <基色的一段 sqrt 多项式重映射>
            → lerp(它, 它×mask, `Use Opacity as Mask`)
出       += 菲涅尔
出        = lerp(出, cb5[18], `Flat_EmissiveRatio`)
出        = lerp(出, **Main Color**, **Main Color.w**)    ← cb5[19] / cb5[90].x
```

**两条要点**:

1. **`Emitter Intensity` 在这个预设里不是「自发光强度」,而是 `Color1` 那层的增益。**
   波波拉覆盖成 0.4,而运行时把它当通用自发光(白 × 0.4 × 菲涅尔)加了上去 —— misattributed。
   这也解释了「关掉自发光波波拉一动不动、火神却恶化」:火神那边它确实是自发光。
2. **又一个「rgb 存颜色、a 存混合量」的参数**:最后一步的混合系数就是 `Main Color.w`,
   而波波拉的 `Main Color` 是 `{0.057, 0.379, 0.384, A: 0}` ⇒ 那一步对它是空操作。
   `Color1`/`Color2` 的 A 也都是 0。和 `FragmentsColor`(rgb = 星层色、w = 星层强度)同一个套路 ——
   **看到向量参数别只看 rgb**。

### 结论翻转:**这三层实机一层都不画,不要实现它**

四版实现全部让指标变差,原因不是公式没读全 —— 是**这一整段被数据门乘成了零**。
汇编第 114~117 行(紧接在三层累加之后):

```text
r2.w  = MaskTex.a                                  ← 第 47 行 sample t2,swizzle 取 A
r2.y  = (MaskTex.a ≥ MinID) ? 1 : (1 − OpenBlackMagicByIDMask)
r4    = r4 × (1 − r2.y)                            ← mad r4, r2.y, -r4, r4
```

`OpenBlackMagicByIDMask` **全库没有任何实例覆盖过**(探针那 395 条实例覆盖清单里只有
`MinID`,没有它),所以它恒为 0 ⇒ **两个分支都给 `r2.y = 1` ⇒ `r4` 无条件归零**。
`Color1` 那层、caustics 那层、菲涅尔那层,一个都到不了输出。

编译器折不掉是因为 `MinID` / `OpenBlackMagicByIDMask` 是 **uniform** 而不是编译期常量 ——
这已经是同一个坑的**第三次**了:

| 层 | 门 | 值 | 结果 |
|---|---|---|---|
| 球内星层(`interior_star`) | `FragmentsColor.w` | 0 | 关 |
| 玻璃菲涅尔 | `FresnelIntensity` | 0 | 关 |
| **水体三层** | `1 − OpenBlackMagicByIDMask` | 0 | **关** |

**所以「读出公式」之后必须再做一步:把这条链上每个乘法因子的值都查一遍,看它是不是 0。**
这套材质图里挂着大量数据门关掉的层,`ushaderbytecode` 里有代码 ≠ 这一层可见。

### 一条便宜又决定性的配对判据(**每次配完都该跑一遍**)

**配到的块,它的槽位列表里必须出现「这个材质自己覆盖过的那些参数」。** 不出现就是配错了。

两次都靠它当场发现配错:

- 水体壳 `_Fx`:块 5 解出 `RedChannel`/`GreenChannel`/`BlueChannel`,而那个材质自己覆盖的是
  `Color1`/`Color2`/`CausticsInt`… ⇒ 配到了父链的块。
- 最外层壳 `_Fx1`:shader 12030(cb5、`dcl cb5[43]`、V=33)唯一配到块 0(V=33/S=39/总槽 44),
  但块 0 的槽位里**根本没有 `MainColor` 和 `MatCapColor`** —— 而这个材质恰恰覆盖了这两个
  (`MainColor` = (0.181, 0.676, 0.898)、`MatCapColor` = (1,1,1))⇒ 12030 不是画这层壳的那条排列。

**结构性限制(第三次撞上,记牢)**:**真正渲染宠物的那条排列,通常在宠物自己的材质里
没有内联冻结块。** `GLASS_RIM_GAIN`(暮星辰裙子)、自发光遮罩(水系 body)、
水体壳与最外层壳 —— 四处卡住全是这个原因。可用的块来自**共享的父 MI**,
它只能提供**布局**(值是父链的默认值),而且必须用上面那条判据验证布局真的对得上。

### 波波拉的 0.337:量化拆解(排除了「只是偏暗」这种解释)

| | R | G | B |
|---|---|---|---|
| 我们(中位) | 39 | 153 | 204 |
| 实机(中位) | **61** | **196** | **240** |
| 最外层壳的 `MainColor` 经曝光 + `sqrt` 编码 | 75 | 146 | 168 |
| 基色贴图(显示空间) | 16 | 139 | 202 |
| 两者 0.5 混合 | 46 | 142 | 185 |

我们的中位很接近「壳与基色 0.5 混合」,说明**合成大致是对的**;而实机比我们
`R +22 / G +43 / B +36`(比值 0.64 / 0.78 / 0.85,**不是统一缩放**),
而且实机的 `G = 196` 比壳的 146 与基色的 139 **都高** —— 两层都解释不了,
缺的是一项**额外的青色贡献**。这一项还没找到,而它所在的排列没有可用的块(见上)。

### 最外面那层壳:读到的与还差的

`M_Wat_ShuiLanLan_PP` 的属性(直接读 uasset 属性 JSON,不用碰 shader):

- **`ShadingModel = MSM_Unlit`** —— 这层壳**不受光**,颜色直出。
  运行时的 `fs_effect` 恰好也完全不受光,这条对上了。
- `BlendMode = BLEND_Translucent`、`bUsedWithSkeletalMesh = True`、`AllowTranslucentCustomDepthWrites`。
- **没有 `MaterialDomain` 键 ⇒ 仍是 `MD_Surface`。名字里的 "PP" 只是命名,不是 post-process。**
  (我一度以为是,查属性直接否掉了。)
- 波波拉那片壳自己覆盖的:`MainColor` = (0.181, 0.676, 0.898)、`Opacity` = 0.5、
  `MatCapColor` = (1,1,1) + `MatCap` = `Matcap18_1`、`FresnelIntensity` = 0.1、
  `FresnelSoftTohard` = 0.5、`ExponentIn` = 0.58、`BaseReflectFractionIn` = −2、`SwingIntensity` = (0,3,5)。

**这个图小得多**:名字表 62 条、4 个冻结块(V=33 / 39 / 39 / 39)。但**穷举验证过,拿不到**:

- 写颜色的那条是 **26073**(`o0.xyz` 在第 166 行、`o0.w` 在第 216 行,cb5、`dcl cb5[49]`、V=33);
  4 个块里 V=33 的只有块 0,而它总槽 `33 + ⌈39/4⌉ + 1 = 44 < 49`,判据②直接否掉。
- 12030(`dcl cb5[43]`、V=33)倒是唯一配到块 0,但它**只写 `o0.w`** —— 是深度/不透明度那一趟,
  不产颜色。它给出的是壳的**不透明度公式**:
  `alpha = saturate(max(pow(1 − N·V, ExponentIn) + BaseReflectFractionIn, r0.x) × Opacity)`
  (波波拉:`ExponentIn` = 0.58、`BaseReflectFractionIn` = −2、`Opacity` = 0.5)。
- 父材质 `MI_ShuiLanLan_PP`(`Special/` 下)也是**同样的 4 个块**(V=33/39/39/39),
  一样配不上 26073。
- 按那条判据复核:**`MainColor` 与 `MatCapColor` 在 8 个块(子 4 + 父 4)里一个都没有** ——
  而这层壳恰恰覆盖了这两个。

⇒ **这层壳的颜色 composition 在现有解包数据里读不到。** 子材质、父材质、全部块、两条判据都试过了。

**一处结构性差异(已确认,值得记)**:运行时 `fs_effect` 把 matcap **只用来算 alpha**
(`strength = mask.a × flow × rim`),颜色恒等于 `tint`;而一个 `MSM_Unlit` 材质里
`MatCapColor × matcap` 是**加进自发光**的,也就是会**提亮**。这与量化拆解对得上 ——
实机比「壳 tint 与基色 0.5 混合」更亮。但要照着改得先拿到 composition,所以没动。

### 一个更大的模式:两个色差离群项都开着 `OpenCustomDepth`

全库只有 **11 个材质**开了 `OpenCustomDepth`,而它们正好是**水系与火系两族**:

    Fir_JiZai3 / Fir_XiaoHuoMiao1,2,3,Bo / Wat_DiMo2 /
    Wat_ShuiLanLan1,2,3,3_011,Bo

15 只实机对照里的两个最大色差项 —— **波波拉(`Wat_ShuiLanLan2_001`,0.337)与
火神(`Fir_XiaoHuoMiao3_001`,0.090)—— 都在这张表里**,而且都是那个 `_Fx`/`_Fx1` 材质。
也就是说这两只的观感依赖一条**我们完全没有的通道**(自定义深度 + 半透写入),
不是某个参数标错了。这条比「逐参数追」更值得先弄清。

### 之前那个目标:最外面那层壳的图

水蓝蓝身体分三块,`_Fx1`(`MI_ShuiLanLan_PP` ← `M_Wat_ShuiLanLan_PP`)是**最外面、最大的一层**
(1671 顶点、bbox 0.72 × **1.054** × 0.51,比水体壳的 1.025 还高)、**半透 0.5**,
而且它是**另一个图**(我们完全没读过),运行时现在把它当「纯特效层」近似。
`MI_ShuiLanLan_PP.uexp` 自己**有冻结块**。实机那种发白发淡的观感,最可能就是它。

### 四版实现的数字(留档,别重走)

| 版本 | 波波拉 调色板 | 水灵 | 备注 |
|---|---|---|---|
| 基线(不画这一层) | **0.337** | **0.097** | |
| ① 整层替换**着色结果** | 0.631 | 0.335 | 渲图一片平色、纹理全没 |
| ② **加在**着色结果上 | 0.618 | 0.403 | |
| ③ 整层替换**固有色**(乘 `shade` 之前) | 0.543 | 0.571 | 亮度飙到 1.44 —— caustics 采到白图 |
| ④ ③ + 补上 caustics 贴图槽 | 0.605 | 0.285 | 亮度回到 0.86;再把基色接进菲涅尔项只降 0.026 |

**已经查实的(这几条是对的,别再怀疑)**:

- `r4` **就是固有色**,不是着色结果 —— 第 342 行 `max r3.xyz, r4, 0` 把它写进 `r3`,
  而 `r3` 正是后面光照段吃的反照率;`基色 × 两段明暗`(`r6`,第 157 行)是**另一路**,
  在第 292~293 行以 `lerp(r6, X, r2.y)` 的形式进入 `r1`(另一个累加器)。所以版本 ①② 结构就错。
- **caustics 贴图必须单独一个槽**:运行时那个「第二贴图」槽对有基色的材质绑的是**色带图**,
  水体没有色带 ⇒ 绑到白图,于是 `caustics = (1 × 7 + 1) × 0.5 = 4`,整只爆亮(版本 ③ 的 1.44)。

**还没读到、下一版必须先补的**:

1. **菲涅尔那一项的两个端点色是 `cb5[7]` = `FresnelColor` 与 `cb5[8]` = `HardLineCol`**
   (`lerp(HardLineCol, FresnelColor, pow(N·V, FresnelPower))`),导出器**没有导它们** ——
   我拿 `rim_color` 顶了,而水体材质的 `Rim Intensity` ≤ 1 ⇒ `rim_color` 根本没写进包、退成白。
2. **那一项还乘着基色的一个有理函数**(第 85~95 行):
   `(0.047 − 0.56b − sqrt(0.0022 + 0.709b − 0.207b²)) / (2(0.93b − 1.36))`。
   拿基色本身近似它只把调色板从 0.631 降到 0.605,不够 —— 得照抄这个式子。
3. **第二次 caustics 采样用的是另一对平铺/速度**(`cb5[85].yz` / `cb5[85].w`),名字还没定。
4. **`Use Opacity as Mask` / `Inv Opacity` / `UseOpacityAsMask` 选出的 mask 变体**没实现。
5. ~~最要紧的一条还没查:`_Fx` 是身体本体还是一层壳?~~ **查了,是壳,但前提仍然成立。**
   水蓝蓝的 glb 里身体分三块(顶点数 / 包围盒尺寸 / y 区间):

   | 材质 | 顶点 | bbox | y | 混合 |
   |---|---|---|---|---|
   | `_By`(`MI_P_Object_UVFlow_WPO_NoMetal`) | 1211 | 0.80 × 0.56 × 0.84 | 0.32~0.88 | 不透明 |
   | `_Fx`(**水体预设**) | 1572 | 0.69 × **1.03** × 0.47 | 0.01~1.04 | 不透明 |
   | `_Fx1`(`M_Wat_ShuiLanLan_PP`) | 1671 | 0.72 × **1.05** × 0.51 | 0.01~1.06 | **半透 0.5** |

   也就是:`_By` 是里面一小块,`_Fx` 是**满高的不透明外壳**(实机看到的主体就是它),
   `_Fx1` 是再外面一层半透壳。**所以「把 `_Fx` 的固有色整层换掉」是对的** ——
   问题只出在公式本身缺那四项(上面 1~4),不在合成位置。

参数那一侧不用再做:导出器已经把 `water_color1`(a = 增益)/ `water_color2` /
`water_main`(a = 混合系数)/ `water_caustics` / `water_shape` 和 caustics 贴图
(走 `noise_tex` 键)写进宠物包了,值与属性 JSON 逐字对得上。
波波拉的实际取值(属性 JSON):`Color1` = (0.325, 0.539, 0.887)、
`Color2` = (0.338, 0.367, 0.627)、`Main Color` = (0.057, 0.379, 0.384, 0)、
`CausticsInt` = 7、`FlowDistort` = 0.38、`FresnelInt` = 0.85、`FresnelPower` = 4、
`U,V_Tiling_Caustics` = 2/2、`V_Speed_Caustics` = 0.171。

- **规律(值得先看这条再动手)**:**会内联 shader map 的是共享的父 MI**
  (`PetBase/MaterialInstance/` 下那批 `MI_P_Object_*`)和少数宠物材质;
  宠物自己那些带独特预设的材质(水体、UVFlow、暮星辰的裙子)基本都不内联。
  所以「能定名的」是共享预设,不是逐宠物的预设 —— `GLASS_RIM_GAIN`、自发光遮罩、
  水体预设三处卡住,都是同一个原因。要继续,得先找到「哪个材质内联了这个排列」。
- **跨材质按 (V, S) 配对不成立 —— 别这么干。** 水系宠物 body 的 shader 33729
  (V=104、`dcl cb6[140]`,`104 + ⌈140/4⌉ + 1 = 140`)在 `MI_P_Object_StepFlicker_NoMetal`
  的块 14 里能找到**完全相同**的 V=104 / S=140,但解出来 `cb6[94]` 是
  `06FX_SmearBaseColor` = (0,0,0,0) —— 乘遮罩再相加是空操作,而那条遮罩链解出来是
  `03FX_LightSwept*` 混着 `00FX_*` 的一堆,明显是**另一个排列碰巧撞上了同样的 V 和 S**。
  配对判据只在**同一个材质内部**成立(那才是「32 条 shader 全部唯一定到块」验过的场景)。
- **配对判据会「无解」,那也是信息:说明那条 shader 的内容不内联。** 暮星辰裙子的 51729
  (7 个 ub ⇒ 材质 cb6、`dcl cb6[142]`、末向量槽 101 ⇒ V=101)在
  `MI_Ill_XingGuang3_001_Fx1` **自己的 17 个块里一个都配不上** —— 唯一 V=101 的块 10 是
  `101 + ⌈139/4⌉ + 1 = 137 < 142`,判据②直接否掉。这跟「24 个哈希只有 12 个块有内联内容」
  是一回事:命中的 map 里只有一部分把冻结内容写进了这个 uexp。
  于是 `GLASS_RIM_GAIN`(替 `cb5[56].w`,标定值 0.0532)还是标定的。
  **顺带查实一条别的**:在幽星光球的块 10 里 `cb5[56].w` 的名字是 `FresnelIntensity`,
  值 **0** —— 那是**另一个材质的另一个排列**,不能直接搬去当裙子的答案,但它提示
  「那个槽位是个菲涅尔强度」这个语义方向。
- **因此还卡着的:自发光那层的遮罩输入。** 33729 已用 `matshader.py --out` 确认属于
  `MI_Wat_ShuiLanLan2_001_By`,而它 **0 个块**;它的图 `MI_P_Object_Water_NoMetal`
  连一条 preshader 链都没有(纯参数覆盖 MI);同族的 `_By` 全都 0 块
  (查过水蓝蓝 1/2/3/Bo、火神、迪莫)。于是 `cb6[133].z/.w`、`cb6[134].x/.w`、`cb6[135].x`
  拼出的那条 ramp 还是叫不出名字 —— 而波波拉与火神正是 17 只实机对照里唯二的
  非构图色差离群项。**下一步只能是搞清楚空隙里那 6 段为什么没有补丁表**,
  或者找一只 body 材质自带大排列补丁块的宠物。
- **教训**:同一族的两个变体可以是完全不同的 shader,**必须逐个反汇编确认**,别按父链名推。
  暮星辰那两颗球的祖父就是 `Trans_MatCap`,按「父链里含」判会把它算进来。
- **两段明暗的过渡上下界也是参数,不是写死的**(shader 34529,第 130~137 行):

  ```text
  r0.x = (N·L + 1) × 0.5 − BlackMagicSoftMin       ← cb5[59].x
  r0.w = 1 / (BlackMagicSoftMax − BlackMagicSoftMin) ← cb5[58].w
  r0.x = saturate(r0.x × r0.w)
  r0.x = x²(3 − 2x)                                 ← smoothstep
  ```

  即 `smoothstep(BlackMagicSoftMin, BlackMagicSoftMax, (N·L + 1) / 2)`。
  两个参数 = **0.50 / 0.52**、**全库零覆盖**,换算到 `N·L` 空间就是 `smoothstep(0.00, 0.04)`
  —— 一条**很窄且不对称**的过渡带。rocom-pets 原来按扫参数取的是 `(-0.04, 0.04)`(宽一倍、
  偏低),换成读出来的之后 15 只对照的对比中位 0.96 → **1.01**、全库过曝 5 → **4**。
- **块之外还有成对的链(`uniexpr --gaps`),但那不解决配对问题。** 一个材质的 uexp 里除了
  带补丁的块,空隙里还有若干组 `(向量链, 标量链, 一条 17 项的第三链)`,结构齐全
  (V、S、opcode 流、参数记录的值都能解),只是**没有补丁表给名字**。名字可以按值反查,
  但实测只有**有辨识度的值**才唯一:`MI_P_Object_NoMetal` 空隙里那个 V=104 排列的 9 个关键
  标量,`0.02` / `0.25` / `−1` 各唯一,而 `0` / `1` / `0.5` 分别有 25 / 16 / 6 个候选。
  **更要紧的是配对本身没解决**:那个 V=104 的排列钉出来的名字是
  `01FX_DissSoftExp` / `06FX_SmearColorMaskSmooth` / `06FX_SmearDirectionY` 这种语义混杂的一堆
  —— 又是跨材质撞上了同样的 V/S。而水系 body 材质自己(`MI_Wat_ShuiLanLan2_001_By`)
  **连一条真链都没有**(64 条全是首偏移上万的假阳性),**0 个块**。
  ⇒ 自发光遮罩由两条独立路径确认**不可得**。
- **「段内 `paramId` 按名字字母序递增」这条是错的,别再拿它当约束。**
  实测 `MI_P_Object_NoMetal` 块 0 的标量数组是 `8, 22, 15, 13, 16, 12, 17, 20` —— 不单调。
  (早先那条是在很少的锚点上观察出来的,样本不够。)
- **`RampID ≥ 0.4` 那个门管的是一整个效果分支,不是星贴层。** 鸭吉吉的 `_By` 也有
  `RampID2 = 5`(≥ 0.4)却明显没有星点层;分支内每一层还有各自的门
  (`FragmentsColor.w`、`Stick_Intensity`、贴图本身)。别拿它当某一层的开关。
- **像素 shader 的尾部:整条链跑在 HDR,靠曝光压回来再 `sqrt` 编码**(shader 20284,714~727):

  ```text
  min r0, 100.0                          ← 钳到 100
  ... 高度雾: mad r0.xyz, r0, v5.w, v5.xyz   ← v5.xyz 是雾的 inscatter,**加性**
  mul r0.xyz, r0, cb0[145].y             ← View 预曝光
  mul r0.xyz, r0, cb6[84].xyzx           ← 材质的整体色(算式:Parameter(61) 0x19 Constant(2) 0x07)
  mul r1.xyz, r0, cb1[79].w              ← 曝光
  sqrt r1.xyz
  movc o0.xyz, (cb1[79].w < 1), r1, r0
  ```

  两条推论:① 编码是 `sqrt` ⇒ 这套资产的隐含解码是**平方**(gamma 2.0,**不是 sRGB**);
  ② 材质里所有大于 1 的值(两段明暗的亮端 1.5、`MatCapColor` 的 (2,1.76,1.45)/(3,3,3)…)
  都是靠这两个曝光压回来的,**离线读不出它们的运行时值**。
  ③ 拿实机截图当参考时要记住**雾是加性的**:截图里那层淡白不属于宠物材质。
  rocom-pets 已按①落地(基色平方进线性、末尾 `sqrt(色 × 曝光)`),见它的 docs/design.md §1.1。
- **两段明暗那对是标量广播出来的灰度对**,不是颜色对:shader 20284 里
  `add r6.xyz, cb6[60].xyzx, -cb6[61].xyzx` / `mad r6.xyz, r0.w, r6, cb6[61]`,两个槽都是
  `Parameter(下标)` 且下标落在标量段(值 **1.5 / 0.5**)。它被 `r6 * r11 + r13` 消费,
  r11 是那对「按物体空间高度 lerp」的颜色 —— 所以 1.5 并不是直接乘在固有色上。
  阈值那三项也都是参数:`mad r0.w, N·L, 0.5, cb6[104].y` 的偏置、以及上下界 `cb6[104].w`/`.z`。
- **教训**:`dxbcdis.exe` 把汇编写到 `<输入>.asm` **旁边**,不走 stdout;重定向 stdout 会得到空文件。

## 半程:cb 槽位的结构已经能读了(`scripts/uniexpr.py`)

shader map 存成 **FMemoryImage**(UE4.26+ 的冻结布局,**不走 FName 序列化**,所以
`FMaterialUniformExpression*` 这种类型名在文件里搜不到)。一个材质的 uexp 里有多个冻结块
(实测 12 个,对应不同 quality/feature level,uniform 条数各不相同)。块内已经认出来的记录:

```
FMaterialScalarParameterInfo    24 字节  名字(8,文件里是零) + int32 Index(-1) + uint8 Association(+3) + float 默认值 + 4 对齐
FMaterialVectorParameterInfo    36 字节  同上,默认值换成 FLinearColor(16) + 4 对齐
FMaterialUniformPreshaderHeader  8 字节  uint32 OpcodeOffset + uint32 OpcodeSize
```

- `Association == 2` = `GlobalParameter`、`Index == -1` = 非图层参数,这两个常量就是数组的特征;
- 默认值能**交叉验证**:根材质 `M_P_Object_Trans` 的 `FresnelColor` = (0.087,0.353,1,0)、
  `StarColor` = (0.333,0.667,2,0)、`FlowColor` = (0.5,0,0.6,0)、`BlackMagicDarkColor` =
  (0.05,0.02,0.1,1) 都能在数组里按原值原序找到;
- preshader 头数组靠「opcode 连续写」认:`off[i+1] == off[i] + size[i]`,向量那条从 0 起、
  标量那条紧接其后。**这直接给出 cb 的分区** —— cb 布局是
  `[向量每条一个 float4][标量每条一个 float,4 个装一个 float4]`,所以向量条数一定,
  标量的起始槽位就定了。

## 参数名的哈希:**已破**

`CachedExpressionData` 把参数名剥成了 `NameHashes`(`ParameterInfos` 是空壳),但**包的名字表是全的** ——
把名字表里每个名字哈希一遍反查即可:

    hash = CityHash64WithSeed(名字的**大写** ASCII, 0)
    # 名字形如 `Foo_3` 时(FName 的 Number):CityHash64WithSeed("FOO", 3 + 1)

实测:`M_P_Object_Trans` 的 205 个参数 **204 个**反查出名字(99.5%);哈希**跨材质稳定**
(与 `M_P_Object` 共享 195/205),所以是内容哈希、可移植。两个坑:
`CityHash64WithSeed(x, 0)` 与 `CityHash64(x)` **不是一回事**(CityHash 里两条码路),
名字必须**转大写** —— 任一处错就全不中。

反推完才发现 **CUE4Parse 自己就有同一份实现**(`UE4/Assets/Exports/Material/HashedNamesProvider.cs`
的 `TryAdd`),连 `Name_N` 那条分支都在。**下次先 grep 上游再动手。**

这条取代了原来的 GUID 桥(只能命名「至少被某个实例覆盖过」的参数)。落地效果见 rocom-pets:
根材质默认值的命名从「向量 16/43、标量 42」变成「向量 43/43、标量 148」。
新拿到名字的里头有一批是之前在猜的:`StarTiling` = 0.4、`FlickerSpeed` = 0.3、
`FlickerPower` = 5(语义猜对了)、`StarTriPlannarBlendInt` = **2**(猜错了,原来写死 8)、
以及 `StarUVScale` = 3、`StarColorDepth` = 15、`StarColorRefract` = 0.05、`StarSpeed` = 0.2。

**拿到名字 ≠ 知道它接在哪。** 上面这几个「新拿到名字」的里头,只有
`StarTriPlannarBlendInt` 是**在汇编里确认了接口**才代进去的;`StarTiling` / `FlickerSpeed` /
`FlickerPower` 是**按语义匹配**上某个未解名的 cb 槽位 —— 值对得上、名字读音也对得上,
但槽位本身没查实。`StarUVScale` = 3 曾经很像 rocom-pets 那个手挑的 ×3 倍率,
后来查实那个倍率其实是「读错了 `StarStickTiling`(标量/向量同名)」造成的,**和 StarUVScale 无关**
—— 这是「名字对上了就以为找到了」的一个现成反例。代任何一个之前,先去汇编里确认它乘在哪。

## 最后一步:名字 —— 已打通(2026-07-28)

冻结镜像里名字字段是**空的**,由 `FMemoryImageResult` 尾部的补丁表打进去,每条 16 字节
`{int32 paramId, int32 Number(0), int32 count(1), uint32 块内偏移}`(正好是 UE 的
`FMemoryImageNamePointer` = `FName(8) + count(4) + offset(4)`)。

**`paramId` 是 uexp 里 shader map 自带的那张名字表的下标,不是包名字表的。**
判据很硬:实测 `paramId` 取值是**稠密的 0..167**,而 `MI_Ill_XingGuang1_001_Fx1` 的
包名字表只有 139 条 —— 而且 0..138 会把 `ArrayProperty`、`StructProperty` 这类属性名
全覆盖进去,根本不可能。那张表就在 uexp 前部,格式与包名字表一致
(`int32 长度 + 字符 + NUL + uint32 哈希`,**负长度 = UTF-16**,本作的中文参数名
`黑魔法or噩梦污染` 排在后段),前面一个 `uint32` 是条数。实现在
`uniexpr.param_names()`(按「条数合理 + 整张表逐条自洽」扫,取最长的一张),
成品输出用 `scripts/matparams.py`。

**双重验证**:① 此前用「按值钉死」独立确认过的 13 个锚点(87 `06FX_SmearDirectionY`、
112 `FragmentsColor`、132 `StarUVScale`、137 `HighLightSpecPow` …)**逐条命中 13/13**;
② 把全部 929 条参数记录的名字拿去和根材质默认值比,**805 条一致、0 条不一致**
(剩 124 条的名字不在根默认表里 —— 贴图/静态开关,或探针没列出的)。

**表的顺序不是全局字母序**,而是**先按参数的来源层分段**(`__SubsurfaceProfile` /
根图自身 / 各图层函数 `00FX_`…`06FX_`),段内才字母序。这解释了之前
「按全局字母序排的秩对不上,而且差值一正一负」—— 两份名单互有出入不是因为条目缺失,
是排序键根本不同。条数随图变化,且**只含该图实际用到的参数**
(`MI_Ill_XingGuang1_001_Fx1` 与 `MI_P_Object_Trans_MatCap` 各 168 条、根图 `M_P_Object_Trans`
166 条、眼睛图 42 条,末条都是 `05FX_FlowV_Tiling`)—— 这正是「这个图有哪些参数、什么顺序」
那个一直循环的问题的答案。

**注意**:宠物身上的 `_By` / `_FX` 一类 MI **自己没有冻结块**(继承父 MI 的 shader map),
但名字表照样在。要读值就去带块的那个 MI(`_Fx1`、`MI_P_Object_Trans_MatCap`、根图)。

打通后的链路是:`UniformVectorPreshaders[k]` → opcode 流 → 若是单条 `Parameter(idx)` 则
`UniformVectorParameters[idx]` → 补丁表 → 名字表 → 名字。

**顺带一条相关的、更早解决的问题**:根材质(`UMaterial`)的参数默认值是能读的 ——
`CachedExpressionData.Parameters` 里有 149 个标量 / 43 个向量 / 13 张贴图的**有序默认值**。
名字被剥了只剩哈希,但同结构里有一份与值数组**同序**的 `ExpressionGuids`,而**实例**那边
每条参数同时带名字和 `ExpressionGUID` —— 按 GUID 一对就能配上名字。
实现在 rocom-pets 的 `exporter/RootDefaults.cs`,GUID 表在 `exporter/data/param-guids.tsv`
(由 `--probe-material ALL` 全量扫实例生成,395 条)。
`StarTex` = `T_EMeng003` 就是这么找到的:**没有任何宠物实例覆盖过它**,顺父链完全看不见。
