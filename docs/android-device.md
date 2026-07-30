# 实机私有数据:把 shader 的归属从「猜」变成「查表」

`docs/android-glsl.md` 那条路卡在**归属**:APK 的基础 OBB 里有完整的安卓 shader library,
**却没有宠物资产** —— `ArtRes/AnimSequence` 下只有 `Human/*`。没有安卓自己的材质 uexp,
就没法拿 shader map 哈希去配,只能退回「结构指纹」猜一条(那份文档的 `#3119` 就是这么来的)。

宠物资产在**应用私有目录**里,游戏首启后按热更新下载。取到它,归属就变成 20 字节哈希的精确查表。

> 本文记的取法与实测数据是在本机核实过的;**下游那条完整链路(材质 → shader map → GLSL →
> preshader 求值)是在另一棵实验树 `~/Git/rocom-pets.old` 里跑通的**,工具是
> `tools/shader-research`(C#/CUE4Parse)。本仓库尚未合入,见文末。

## 1. 取法

### 设备要求

这条路**不是**普通手机能走的,三条都试过:

| 办法 | 结果 |
|---|---|
| `run-as com.tencent.nrc` | ❌ 应用 `debuggable=false`,被拒 |
| `adb backup` 全量备份 | ❌ Android 16 回 `Package com.tencent.nrc is not eligible for backup` |
| Shizuku | ❌ 它拿到的仍是 shell 权限,绕不过应用数据隔离 |

要么 `userdebug` ROM 的 `adb root`、要么设备已 root、要么有能解密 `/data` 的 recovery。
本机用的是 **OnePlus 7 Pro(GM1911)/ Android 16 / userdebug**,`adb root` 可用。

### 路径不能按常见 UE 约定猜

manifest 里 `bUseExternalFilesDir=true`,但游戏自定义的 `UNRCStatics.GetFilePathBase()`
在这个构建里实际指向**应用私有 files 目录**。正确算法在解包 Lua 的
`PufferUpdateResTask:MakePath` 里:

```text
GetFilePathBase()/UE4Game/NRC/NRC/Saved/Puffer
GetFilePathBase()/PreDownload/Puffer
```

⇒ 真实位置 `/data/user/0/com.tencent.nrc/files/UE4Game/NRC/NRC/Saved/Puffer`。
只扫外部 `/sdcard/Android/data/` 会一无所获 —— 那里只有热更新 APK 与 `ProgramBinaryCache`。

### 命令

```sh
adb root && adb wait-for-device
P=/data/user/0/com.tencent.nrc/files/UE4Game/NRC/NRC/Saved
adb shell "du -sh $P/Puffer"                       # 先看要拉多少
adb pull $P/Puffer            ~/Downloads/rocom/android-device/<版本>/
adb pull $P/Config/Android    ~/Downloads/rocom/android-device/<版本>/   # 质量档,见 §3
adb pull /sdcard/Android/data/com.tencent.nrc/files/ProgramBinaryCache \
         ~/Downloads/rocom/android-device/<版本>/                       # 不需要 root
adb unroot                                          # 用完复原
```

**素材不入仓库**,和 pak / 宠物包一样的约定(README 与 design.md §11)。

## 2. 有什么(2026-07-30 实测,版本 1.110.0.109)

| 内容 | 文件数 | 大小 |
|---|---:|---:|
| `Puffer/Paks/*.pak`(基础,2026-04-10) | 17 | 约 14 GB |
| `Puffer/Paks/Patch/*.pak`(补丁,最新当天 18:13) | 116 | 8.6 GB |
| `Puffer/puffer_res.eifs` | 1 | 54 MB |
| 合计 | **134** | **24,063,117,734 字节(22.4 GiB)** |
| `ProgramBinaryCache/`(驱动编译产物,在 `/sdcard`) | 5 | 76 MB |

导完**逐文件核尺寸**(设备侧 `find -printf '%s %p'` 与本地逐行比对,134/134 一致)。
`adb pull` 报 206 MB/s、111 秒 —— 这条比想象中便宜,不必怕重导。

全部是 `Android_ASTC` cook。这也解释了系统设置里那 28 GB 的「数据」占用。

**版本会漂**:上一次记录(见 `~/Downloads/android-shaders.md`)是 99 个补丁 pak,
今天是 116 个。哈希配对要求两边同版本,所以**导出时机要和 Windows 侧 pak 的更新对齐**。

挂载后按 `Miaomiao` 过滤可见:`Gra_MiaoMiao0_001` ~ `Gra_MiaoMiao3_*` 的模型与变体、
`MI_Gra_Miaomiao1_001_By/Es/Mh` 等目标材质、贴图、骨架、动画蓝图,以及睡眠/行走/奔跑/生气
等完整动作和叫声 bank —— 即缺的那一半确实在这里,不在 APK 也不在外部目录。

## 3. 它解决什么

### 3.1 归属:精确哈希取代结构指纹 —— 这是主要价值

同平台之内,材质 uexp 里序列化的 20 字节 shader-map 哈希可以直接在安卓 archive 里 memmem。
在喵喵上验过:

| 安卓 cooked 材质 | 不同 shader map | 哈希出现次数 |
|---|---:|---:|
| `MI_Gra_Miaomiao1_001_By` | 18 | 24 |
| `MI_Gra_Miaomiao1_001_Es` | 1 | 4 |
| `MI_Gra_Miaomiao1_001_By_Ol` | 6 | 12 |
| `MI_Gra_Miaomiao1_001_Mh` | 0 | 0(无静态 permutation,直接继承 parent) |

同三个 **Windows** cooked package 在安卓 archive 里是 **0 命中**,符合 platform-specific cook,
也再次证明 `docs/android-glsl.md` §3 那个「交集 0」不是 bug。

**为什么这条最重要**:本仓库栽得最狠的几次全在归属上 —— design.md 的「一次解析 bug 引发的
连锁误读」、「**公式读对了不等于这条公式属于这个材质**」、`docs/shader.md` 里反复出现的
「那个材质没有自己的内联 shader map,只能借父材质布局」。这些都是配对不确定的代价。

### 3.2 哪一条 permutation 是实机真跑的

一个 cooked material 会序列化多组 resource。选择键是四元组,**不能拿第一个当默认**:

```text
QualityLevel × FeatureLevel × LODUsed × DynamicSwitchId
```

实测本机设备 `Saved/Config/Android/GameUserSettings.ini`:

```text
sg.UtilityGroup=2
nrc.UtilityGroup=2
```

而 APK 的 `AndroidScalability.ini` 里 `[UtilityGroup@2]` 设 `r.MaterialQualityLevel=0`(Low)。
配上 `FeatureLevel=ES3_1`、模型 LOD0 ⇒ `LODUsed=0`、动态开关默认全 false ⇒ `DynamicSwitchId=0`,
四条唯一选出喵喵身体的 resource 12 / 轮廓 6 / 眼睛 2。

这一条直接冲着本仓库那个反复出现的坑去:「**参数在,但这一层实机不画**」
(`FragmentsColor.w`=0、`FresnelIntensity`=0、`OpenBlackMagicByIDMask` 全库零覆盖、
`RampID` 那道默认关的门)。有了确定的 permutation,静态开关不用再推断。

> 注:`DynamicSwitchId` / `LODUsed` 这两个字段名是从 `libUE4.so` 保留的原字段名交叉确认的,
> 不是按位置猜的。CUE4Parse 原有的 Roco 特判把 `QualityLevel`/`FeatureLevel` 读反了
> (会得到 `High + ES2/SM6` 这种不可能的组合),那个特判是错的。

### 3.3 不需要参数名也能拿到值

这是绕开 `docs/shader.md` 结尾那堵墙的路。那堵墙是:V=116 那个排列没有补丁表支撑的冻结块,
槽位 ↔ 参数名对不齐,而 `cb6[117]/[118]` 恰恰全是决定「哪一支生效」的模式开关。

安卓这条路不走名字:按 UE 4.26 `EvaluatePreshader` 把材质的 preshader opcode **求值**,
直接得到 shader 真正消费的数值缓冲。喵喵身体 PS 的结果是 152 floats 的 numeric buffer,
再按 header 的 `FUniformBufferCopyInfo` 复制出 `pc5_m` 的 14 个 float4(逐条校验 source/destination
偏移、无重叠无空洞)。⇒ **design.md §1.1 的「B 类:替未解出名字的 cb 槽位站着的标定系数」
整类都能变成查表。**

一条必须记住的限制:求值出来的是**静态默认值**。运行时 lua 仍可能通过 material render proxy
动态覆盖 —— 异色皮肤走的正是 `SetVectorParameterValue` 这条路(见 design.md 横向待办)。
和截图对照时必须区分「静态默认值」与「实机当帧值」。

### 3.4 附带

- **GLSL 源码而不是 DXBC**:公式是表达式,`refract()`/`smoothstep()` 原样可读,采样通道一眼可见。
- **PSO 状态**:`NRC_GLSL_ES3_1_ANDROID.stable.upipelinecache` 里有 blend/raster/depth-stencil
  与 vertex declaration。喵喵身体 base pass 命中唯一一个 VS/PS 同属 map 2693 的 PSO
  (`opaque blend / solid CW cull / depth write + GreaterEqual / front-face stencil replace`)。
  轮廓那个 map 在 cache 里**没有**成对的 PSO,不能伪造配对。
- **原生 uniform 布局**:扫 `libUE4.so` 的 AArch64 shader-parameter 注册代码可得成员名与 byte
  offset,证实 Roco 的 View 布局**不是**公开 UE 4.26 的原样复制(多出
  `SceneColorMultiplier`、`LightIntensityController` 等游戏字段)。

## 4. 它不解决什么

- **光方向、`AMBIENT`、曝光拿不到。** 它们在 View(UB0)与 `FMobileDirectionalLightShaderParameters`
  (UB3)里,是**运行时场景状态**,任何文件里都没有。`docs/android-glsl.md` 那条
  「光方向读不到 —— 这解释了 §8 落地为什么必然失败」原封不动地留着。
  **另一条没试过的线索**:宠物预览界面的平行光是关卡里的 actor,**Windows pak 里就有**,
  这条不需要安卓数据。
- **「硬分界从哪来」没有直接答案**,但从「三条假说全排除、无从下手」变成「有精确源码 + 真实
  参数值可查」。
- **shader 不能直接用**:只有 `GLSL_ES3_1_ANDROID`,没有 SPIR-V / Vulkan pipeline cache。
  本仓库要的是**公式与数值**,照旧手搬 WGSL,所以这条不构成阻塞;「原样提交 GLES」是另一条
  路线的要求。
- **贴图不用换源**:安卓是 ASTC cook,导出器继续用 Windows 那边的 BC7。
- **`ProgramBinaryCache` 价值低**:驱动编译后的二进制,既不替代 shader archive,也不提供
  材质 → shader-map 的关联。

## 5. 第一次产出:`#3119` 的归属是错的(2026-07-30)

数据到手后第一件事是复核 `docs/android-glsl.md` §4 —— 那一整章的结论都挂在结构指纹配出来的
`#3119` 上。现在可以拿幽星光(`Ill_XingGuang1_001`)自己的安卓 cooked 材质精确查哈希。
**结果是否定的,而且是三重否定:**

| 检验 | 结果 |
|---|---|
| 编号是不是同一套 | ✅ 是。archive entry 3119 = frag、`pc6_m[69]`、9 个采样器(含 §4 那句三向投影用的 `ps8`),与 `glsldump.py` 的 `#3119` 是同一条 |
| `#3119` 在不在幽星光的 shader 里 | ❌ **不在**。它属于 map **4734**;幽星光七个材质合计 **77 个 map**,没有 4734 |
| 全库有没有宠物用它 | ❌ **没有**。扫了 **2764 个宠物材质**(939 个形态目录,其中 2540 个有内联 shader map、合计 1237 个不同 map),**零命中** |
| 它到底是谁的 | `MI_P_Object_Masked` —— PetBase 里的**共享父材质**,不是任何一只宠物 |

**代价有多大:槽位下标整体作废。** 幽星光自己那批带三向投影的 `_By` 片元 shader 用的材质
uniform buffer 是 **`pc5_m[39]`**(1131~1146 行),而 `#3119` 是 **`pc6_m[69]`**(1437 行)——
**缓冲编号不同、槽位数也差 30**。所以 §4 里每一个 `pc6_m[i]` 下标(`pc6_m[46].z/.w`、
`cb6[117]/[118]`、`cb6[13]/[15]` …)**都不能往宠物身上代**。

这顺带解释了那篇文档自己的困惑:§「又撞回老墙」抱怨 `cb6[117]/[118]` 按 V=108 的命名读出来是
`PointLightIntLocal`/`MainBright`、「一个点光强度不会去选颜色模式」—— 语义对不上不是因为命名表
选错了排列,是因为**那压根是另一个材质的布局**。

**结构性结论仍然成立。** 同一批 shader 里查过:幽星光自己的 `_By` 有 18 条片元含三向投影采样、
54 条含 `refract()`,`_Fx1` 是 32 / 64 —— §4 描述的那些层(折射视差星层、滚动法线三向投影)
**确实在这只宠物身上**,错的是「哪条 shader、哪个槽位」,不是「有没有这一层」。

**不动运行时基线。** 全仓库没有一处代码依赖 §4 的槽位(`grep pc6_m/pc5_m/3119` 在 `src/` 与
`exporter/` 下零命中);§7 那条曲线当初就已整体回退,§8 两次落地也都失败回滚。所以这次更正
只作废一批**笔记**,`cmp_shots` 基线 0.93 / 0.077 / 0.95 / 1.01 不受影响。

**教训是老教训又中了一次**:design.md 记过「公式读对了不等于这条公式属于这个材质」。
这次更狠一档 —— 指纹配到的是**同族的共享父材质**,它 feature 全开、双面、11 次采样,
指纹的每一条判据都对得上,**恰恰因为它是那个超集**。⇒ **结构指纹天然偏向父材质,
不能用来定位具体某只宠物。**

## 6. 第二次产出:水体层的合成方式读到了,但落地仍然失败(第三次)

拿波波拉(`Wat_ShuiLanLan2_001`,`cmp_shots` 里最差的一只,调色板 **0.299**)走了一遍完整链路。

**读到的东西是实的,而且比 DXBC 那条路清楚得多:**

1. **合成方式有名字,不用猜。** 这个材质是**图层式**的,cooked 属性里图层与混合层的名字都在:

   ```
   Layers: ML_Com_EmptyBase → ML_P_StylizedWater → ML_P_BaseColorInnerFresnel → ML_P_LineVertexOffset
   Blends:                    MLB_P_EmissiveAdd    MLB_BaseColorMultiplyEmissiveAdd
   ```

   ⇒ 水体层走**自发光加法**。之前两次落地("整层替换" 0.631 / "加在着色结果上" 0.618)都卡在这一步靠猜。

2. **参数是具名读出来的**(不经 cb 槽位):`Color1` (0.325, 0.539, 0.887) / `Color2` (0.338, 0.367, 0.627) /
   `Emitter Intensity` 0.4 / `CausticsInt` **7.0** / `FresnelInt` 0.85 / `FresnelPower` 4.0 /
   `FlowDistort` 0.38 / caustics 平铺 2,2 速度 0,0.171。静态开关 `开启黑魔法效果` = false
   (与 design.md 记的"`OpenBlackMagicByIDMask` 全库零覆盖"对上)。

3. **`Main Color.w = 0`** —— 汇编尾部那个 `lerp(…, Main Color, Main Color.w)` **是关的**。
   上一次"整层替换成 Main Color"崩到 0.631,根因就在这:拿一个权重为 0 的层当主色。
   这是「参数在但这一层不可见」的**第五例**。

**落地结果:更差,已撤回。** 按"线性空间里加性叠加"实现(`Color1 × Emitter Intensity`
+ `Color2 × 两次卷动采样 × CausticsInt` + `FresnelInt × pow(1−N·V, FresnelPower)`,
全部参数取自包、无手挑系数):

| | 调色板 | 亮度比 |
|---|---|---|
| 波波拉 基线 → 加水体层 | 0.299 → **0.557** | 0.88 → **1.20** |
| 水灵 基线 → 加水体层 | 0.102 → **0.263** | 0.82 → **1.11** |

**病因是量级,而且能指出来在哪:`MLB_P_EmissiveAdd` 说的是「图层之间怎么合」,
不是「这一层内部已经算完了」。** 安卓 GLSL 里水体那几支进入结果时是

```
mix(基色支, (caustics × 增益) + 另一层, clamp(遮罩 × 权重, 0, 1))
```

—— **带遮罩的 mix**,不是裸加。`CausticsInt = 7.0` 乘一张 [0,1] 噪声再裸加,线性下能到 4.4,
必然冲白。缺的是那个遮罩(GLSL 里的 `v260.w` 与两个权重 `pc4_m[38].x/.y`),它来自一条我还没追完的链。

⇒ **下一次落地之前必须先把那个遮罩追出来**,不要再调系数。这一条和 design.md
「结构没读对时调参数只是换个方向错」是同一个教训。

### 遮罩链已经追出来了,卡在最后一步「槽位 → 参数名」

顺着 `v260.w` 展开(shader 28757 / map 3284):

```glsl
v240   = textureLod(ps7, v227, -1)                              // 一张贴图
h242   = smoothstep(pc4_m[61].y, pc4_m[60].w, pc4_m[60].z + v240.x)
h245   = (h223 - mix(pc4_m[62].y, pc4_m[62].x, step(pc4_m[61].x, 0))) / pc4_m[61].x + v240.x
v260.w = mix(h242, clamp(h245, 0, 1), pc4_m[62].z)              // ← 遮罩
// 水体支进结果的方式:
mix(基色支, (caustics × 增益) + 另一层, clamp(v260.w × pc4_m[38].y, 0, 1))
```

⇒ **遮罩 = 一张贴图的 R 通道 + 一个偏置,过 smoothstep**(两个界也来自材质),
再按 `pc4_m[38].x/.y` 两个权重分别喂给两支。

**这解释了上一次为什么亮度从 0.88 冲到 1.20**:我把这一层**无差别加在整只身上**,
而实机是 `mix` 且被遮罩限制在特定区域 —— 遮罩为 0 的地方基色**原封不动**。

**卡在哪**:`ps7` 是 bind 7 / resource **18**,要把它对回参数名,得读材质 frozen 对象里的
`UniformTextureParameters` 顺序。我们这边用 rocom-capture 导的属性 JSON **不含 material
resource 段**(工具直接回 `material resource index is out of range: 0`)——
那份调研当初是**临时给 CUE4Parse 打了补丁**才读出来的,而补丁已经撤回(见调研记录末尾)。

**一条不能当结论的线索**:材质的 `CachedReferencedTextures` 第 9 项(若按 resource/2 对位)
是 `DefaultTexture`,即**没被实例覆盖的基础默认值**。若真如此,这个遮罩可能是个常量,
那就又是一例「这一层实机不画」。但喵喵那次的实测表明 resource index 与贴图序号**不是**
简单的 /2 关系,所以这条只能当假说。

### 那个"补丁"根本不需要:`ReadShaderMaps` 是个开关

`UMaterialInstance.Deserialize` 里读内联 shader map 的条件是
`Ar is { Game: >= GAME_UE4_25, Owner.Provider.ReadShaderMaps: true }` ——
**provider 上的一个开关,默认关**。rocom-capture 的解包工具没开它,所以导出的属性 JSON 里
`LoadedMaterialResources` 是空数组;不是 CUE4Parse 少了什么补丁。

开了之后波波拉 `_Fx` 一次读出 **36 个 material resource**。
(工具放在 scratchpad,没进任何仓库;要固化的话给 unpack 加个 `--shader-maps` 开关即可。)

**顺带两条查实:**

- **36 个 resource 的 `ResourceHash` 全部命中安卓 archive**(24 个不同 map,0 未命中)
  ⇒ 这批 resource 确实是安卓那套 —— 尽管 CUE4Parse 把 `ShaderPlatform` 报成 `SP_PCD3D_SM5`。
- **调研记录里那条 Roco 特判错误复现了**:36 个 resource 全部读成
  `QualityLevel=High` + `FeatureLevel=ES2_REMOVED/SM6` —— 这个组合不可能存在,
  正是"两个字段被互换"的症状。所以**这两个字段在 vanilla CUE4Parse 上不可信**,
  四元组选 permutation 这条路仍然需要那个补丁。

### 现在卡在两件小事上

1. **哪个 resource 是实机跑的,仍未定。** 喵喵那次是 resource 12,我照着试了 ——
   **不成立**:波波拉 resource 12 对应 map 4956,它的片元只有 4 次采样、没有水体链
   (材质 UB 也换成了 `pc6_m[21]`,和 map 3284 的 `pc4_m[71]` 不是一个排列)。
   **这条要靠 `LODUsed`/`DynamicSwitchId`,而它们正卡在上面那个字段互换上。**
   (查了才知道不成立,没有照搬 —— 这类"位置类比"正是本文档反复栽过的坑。)
2. **带水体链的那个 permutation 求值不了。** map 3284 = resource 23,
   求值器在 offset 2598 拒绝 opcode **`Sin` (13)** —— 它按设计对未实现的 opcode 直接报错
   而不是猜。resource 12 里没有 `Sin`,所以能算(784 字节 opcode、40 标量 + 37 向量
   preshader、188 floats 缓冲)。

### 数值拿到了,而且**推翻了上面那条「遮罩把它限制在特定区域」的诊断**

给求值器补上 `Sin`(以及 `Cos/Tan/Asin/Acos/Atan/Sqrt/Saturate/Abs/Floor/Ceil/Round/
Trunc/Sign/Frac/Log2/Log10/Min/Fmod` 这批语义无歧义的算子;`Clamp`/`Atan2`/`Dot`/`Cross`/
`Length` 继续 fail-closed)之后,resource 23 求值通过,对 shader 28757 铺开 UB4。

**先验证铺得对**(四处独立对上,不是"看着像"):

| 槽位 | 求值结果 | 材质里的具名参数 |
|---|---|---|
| `pc4_m[1]` | (0.0566, 0.3791, 0.3837) | `Main Color` **逐位相同** |
| `pc4_m[44].x` | 7 | `CausticsInt` |
| `pc4_m[45].y/.z/.w` | 0.38 / 4 / 0.85 | `FlowDistort` / `FresnelPower` / `FresnelInt` |

**注意必须传实例链**(`--material-instance` 按 父 → 子:`MI_P_Object` →
`MI_P_Object_NoMetal` → `MI_P_Object_Water_NoMetal` → 本体)。不传的话拿到的是**根默认值**,
`pc4_m[1]` 会是 (0,0,0) —— 我第一次就是这么跑的,差点又照着一串 0/1 下结论。

**代进去之后,遮罩那条链是常量:**

```
pc4_m[60] = (0,0,0,0)   pc4_m[61] = (0.02, -0.02, 0.02, 0.04)   pc4_m[62] = (1, -0.02, 0, 0)
h242   = smoothstep(-0.02, 0, 0 + 贴图.x)   而贴图.x ≥ 0  ⇒  恒等于 1
v260.w = mix(h242, …, pc4_m[62].z = 0)                      ⇒  恒等于 1
```

⇒ **遮罩恒为 1,不存在"限制在特定区域"这回事。上一条诊断是错的。**

真正的结构是**两支的权重**(`pc4_m[38] = (1, 0, 0, 0)`):

```
支A: clamp(1 × pc4_m[38].y = 0) = 0   ⇒ 整支不画
支B: clamp(1 × pc4_m[38].x = 1) = 1   ⇒ 整支**完全替换**基色支
```

再往支B 里看,链上还有三个乘零/取零的因子:

| 因子 | 值 | 后果 |
|---|---|---|
| `pc4_m[58].x` | 1 | `mix(基色, 水体, 1)` ⇒ 取水体 |
| `pc4_m[57].x` | 0 | caustics 的**第二次采样关掉**,只采一次 |
| `pc4_m[28]` | (1,1,1) | caustics **不乘 `Color2`** —— 这一层在这个排列里根本没有着色 |
| `pc4_m[37]` | (0,0,0) | 那一整个附加子层归零 |
| `v248` 里那对 smoothstep | 参数与输入都相同 | 相减恒为 **0**,又一整层归零 |

⇒ **上一次落地失败的真实原因和我说的不一样**:不是"该被遮罩挡住却加满了",
而是**这一支是替换而不是相加**,并且我加进去的那几项(`Color1 × Emitter Intensity`、
`Color2 × caustics × CausticsInt`、菲涅尔)在这个排列里**大多根本不参与** ——
`pc4_m[28] = (1,1,1)` 直接说明 caustics 没有被 `Color2` 着色。

**仍未定的一条**:resource 23 是不是实机跑的那份。它是**带完整水体链**的那份
(24 个 map 里只有它的片元有这条链),但"实机跑哪份"要靠 `LODUsed`/`DynamicSwitchId`,
仍卡在 CUE4Parse 那个字段互换上。所以上面这些数字是**这条 shader 的真值**,
而"实机就是这条 shader"还差一步。

### 两条出颜色的链读完了,**参数映射又一次和我猜的不一样**

把已知的具名参数值反查进 71 个槽位,得到一张确定的对照表(不是推的,是值逐位对上的):

| 槽位 | 参数 | 值 |
|---|---|---|
| `pc4_m[1]` | `Main Color` | (0.0566, 0.3791, 0.3837) |
| `pc4_m[2]` | `Color2` | (0.3384, 0.3674, 0.6267) |
| `pc4_m[3]` | `Color1` | (0.325, 0.5389, 0.8872) |
| `pc4_m[42].x` | `Emitter Intensity` | 0.4 |
| `pc4_m[43]` | caustics [u 平铺, v 平铺, u 速度, v 速度] | (2, 2, 0, 0.1713) |
| `pc4_m[44].x` | `CausticsInt` | 7 |
| `pc4_m[45].y/.z/.w` | `FlowDistort` / `FresnelPower` / `FresnelInt` | 0.38 / 4 / 0.85 |

**`v226`(那个「两段明暗 × 高度渐变」)整条是零**:`pc4_m[29..32]` 四个颜色全是 (0,0,0)。
`v262` 同理(`pc4_m[40]/[41]` = 0),而它的混合权重 `pc4_m[68].y` 也是 0 ⇒ 那两个 `mix` 都是空操作。
于是支B 塌缩成 `v264 = (v161 + v202) × h159`。

**三条修正,每一条都推翻我上一版的实现:**

| 我上一版写的 | 汇编里实际是 |
|---|---|
| caustics 乘 `Color2` | **乘 `Main Color`**(`pc4_m[1]`,第 183 行) |
| 底色是 `Color1 × Emitter Intensity` | **`h27 × Emitter Intensity`** —— `pc4_m[0]` 是 (1,1,1),乘的是一个遮罩量不是颜色 |
| 菲涅尔用 `Color1` 着色 | **`v56 × FresnelInt × mix(Color1, Color2, h61)`**(第 197 行)—— `Color1`/`Color2` 是**菲涅尔层双色渐变的两端**,根本不是"水色"与"caustics 色" |

**最该记住的一条**:我因为 `Main Color.w = 0` 就把它整个排除了 —— 而它的 **rgb 是这一层唯一的着色**。
`.w` 关掉的只是末尾那个 lerp,不是这个参数。**"某个分量是 0"不等于"这个参数不参与"。**

读到的结构(参数已全部具名):

```
v17  = h27 × Emitter Intensity
v55  = v17 + h30 × (((noise₂.y × CausticsInt) × noise₁.x + noise₁.x) × 0.5) × Main Color
v62  = v56 × FresnelInt × mix(Color1, Color2, h61)
v64  = mix(v55 + mix(v62, h27 × v62, pc4_m[46].x), 0, h63)
v161 = max(v64 + v69, 0)
v202 = texture(ps3, 卷动UV)
v264 = (v161 + v202) × h159
```

### 追到底:**这条 shader 整条都不画,所以 resource 23 不是实机那份**

把剩下几个量追完,两条有独立价值的收获,以及一个终结性的结论。

**收获一:`h27` 就是我们已经在算的那个重映射。**

```glsl
v26 = texture(ps1, TEXCOORD0)                        // 基色贴图
h27 = clamp((v26.w - 0.04) * 1.1111, 0, 1)           // 基色 alpha 的重映射
```

和 `pet.wgsl` 里不透明度/线条遮罩用的 `saturate((tex.a − 0.04) × 1.1111)` **逐字相同** ——
design.md 记的那条重映射在这里第三次独立出现。

**收获二:`v56` 不是菲涅尔项,是「反色调映射」本身 —— 而且这次是在宠物自己的材质上。**

```glsl
v56 = ((-0.56·基色 + 0.047) - sqrt(-0.2072·基色² + 0.70896·基色 + 0.002209))
      / (2·(0.93·基色 - 1.36))
```

这正是 `docs/android-glsl.md` §4.1 那条 filmic 逆变换,系数逐个对上。**意义在于:§4 的结论
当初随 `#3119` 配错一起作废了,而这一条现在在波波拉自己的 `_Fx` 上独立复现** ——
所以「底色贴图是反色调映射编码的」这条**结论本身是对的**,错的只是当时挂在哪个材质上。

**终结性的结论:代进求值出来的槽位之后,这条 shader 什么都不画。**

| 步骤 | 依据 | 结果 |
|---|---|---|
| `h63 = mix(1, step(…), pc4_m[46].z = 0)` | `pc4_m[46] = (0, 0.4, 0, 8)` | **= 1** |
| `v64 = mix(v55 + v62, 0, h63)` | h63 = 1 | **= 0** ← caustics + 菲涅尔 + 自发光**整条归零** |
| `v68 = (h67 × 20) × pc4_m[4] × pc4_m[47].y × …` | `pc4_m[47].y = 0` | **= 0** |
| `v69 = v68 + pc4_m[6] × h27` | `pc4_m[6] = 0` | **= 0** |
| `v70 = v64 + v69` → `v161 = max(v70, 0)` | | **= 0** |

⇒ `v264 = (v161 + v202) × h159` = **`v202 × h159`** —— 整个材质输出就是**一次卷动贴图采样**
乘一个雾系数。实机里波波拉是有颜色的身体,不可能长这样。

⇒ **resource 23 / map 3284 不是实机跑的那份。** 从它读出来的一切
(包括上面那张槽位对照表 —— copy 表本来就是逐 permutation 的)**都不能移植**。

**这一层的失败史现在是四次,每一次的病因诊断都被下一轮推翻:**
「整层替换」→「加法太强」→「遮罩挡住了」→「参数映射错」→ **「读的根本不是实机那条 shader」**。
共同的根因只有一个:**没有先确定实机跑哪个 permutation 就开始读**。

⇒ **下一步唯一该做的事**:修 CUE4Parse 那个 Roco 特判(`QualityLevel`/`FeatureLevel`
被互换,症状是 36 个 resource 全读成不可能存在的 `High + ES2_REMOVED/SM6`),
解出 `LODUsed` / `DynamicSwitchId`,用四元组选出实机那份 resource。
**在那之前,不要再读任何一条水体 shader,也不要再落地。**

## 7. 下一步

1. **把幽星光实机那条 shader 定下来。** 已知它的 `_By` 有 18 个 map / `_Fx1` 有 24 个,
   还要用 §3.2 的四元组选出实机真跑的那一份 resource,再重读 §4 的每一条。
2. **查 `OpenCustomDepth` 那条通道。** 眼下 `tools/cmp_shots.py` 基线里最差的波波拉与火神
   都开着这个静态开关(见 design.md 横向待办),而我们完全没有这条通道。安卓侧是可读源码 +
   确定 permutation,是最有希望的一只。
3. **水体预设**:design.md 里那条「先把 35663 的 150~676 行读完」在 GLSL 侧是读表达式而不是
   读寄存器流,成本低一个量级。

### 复现

```sh
# 一、从 APK 取安卓 shader archive(基础 OBB 就有,不需要实机数据)
cd ~/Downloads/rocom/apk
unzip -o ../com.tencent.nrc.apk 'assets/main.obb.png' -d .
unzip -o assets/main.obb.png 'NRC/Content/Paks/*' -d .
~/Git/rocom-capture/scripts/unpack.sh --paks ./NRC/Content/Paks \
    --out ~/Downloads/rocom/apk-parsed --no-exclude --no-post \
    --filter NRC/Content/ShaderArchive --filter NRC/Content/PipelineCaches

# 二、从实机私有 pak 取宠物的安卓 cooked 材质(--raw:shader map 哈希在 CUE4Parse 不解的段里)
~/Git/rocom-capture/scripts/unpack.sh \
    --paks ~/Downloads/rocom/android-device/<版本>/Puffer/Paks \
    --out ~/Downloads/rocom/android-parsed --no-exclude --no-post --raw \
    --filter NRC/Content/ArtRes/AnimSequence/Pets/Ill_XingGuang1_001

# 三、配对(工具暂在 rocom-pets.old,见下)
dotnet run -c Release --project tools/shader-research -- \
    --archive .../ShaderArchive-NRC-GLSL_ES3_1_ANDROID.ushaderbytecode \
    --scan .../MI_Ill_XingGuang1_001_By.uexp        # 材质 → shader map 哈希
dotnet run -c Release --project tools/shader-research -- \
    --archive ... --shader 3119                     # 某条 entry 属于哪个 map
dotnet run -c Release --project tools/shader-research -- \
    --archive ... --extract <dir> --map 1076 --map 1247   # map → GLSL 源码
```

两个坑:① `--map … --extract` 在导出上千条时会在**最后写 manifest JSON** 那步 OOM 崩掉
(exit 134),但**文件已经全部落盘**,别当成导出失败;② zsh 不做单词切分,
把参数拼成字符串再展开要写 `${=VAR}`。

工具目前在 `~/Git/rocom-pets.old/tools/shader-research`(C#/net10.0,与 `exporter/` 同一套
CUE4Parse 引用方式,`CUE4PARSE_DIR` 环境变量)。**先在那棵树里验证第 1 步,证明这条路对本仓库
确有产出之后再合入 `scripts/`** —— 和当初 shader 逆向工具链从 rocom-capture 迁进来的做法一致。
