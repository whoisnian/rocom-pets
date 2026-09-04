# 设计方案

跨平台桌面宠物：把《洛克王国：世界》的宠物模型/动作/叫声做成本地生成的「宠物包」，
由一个原生运行时在桌面上播放、交互、互动。本文是实现前的方案定稿，含待验证项与分阶段计划。

- 目标平台：**Windows 10+** 与 **KDE Plasma Wayland**(kwin_wayland)。**不支持** GNOME/Mutter 等
  不实现 wlr-layer-shell 的合成器，也不做 X11 回退——只维护两个后端，省下的复杂度换取实现深度。
- 资产提取链路已在 [rocom-capture](../../rocom-capture) 里验证过(CUE4Parse 解包 + 骨骼网格/动画导出)，
  本仓库只做**运行时**与**打包导出器**。
- 原始素材与生成的宠物包**都不入仓库、不分发**，见 §11。

## 0. 目标与非目标

**做**：宠物在桌面上待机/行走/奔跑/睡觉/情绪动作；鼠标交互(点击受惊、摸头、拖放)；
点击穿透可开关；多宠物同时在场并有跨物种互动；部分宠物叫声；按需启用的宠物包。

**不做**：还原游戏的战斗/技能演出、场景与 BGM；1:1 复刻游戏的自研卡通着色器；
移植游戏的行为树；任何联网/账号功能(抓包统计是 rocom-capture 的事)。

## 怎么改这个项目

**这一节是给下一个接手的人(含 AI)的入口。**

### 文档怎么分

| 文件 | 装什么 | 什么时候读 |
| --- | --- | --- |
| **design.md**(本文) | 目标、技术选型、运行时架构、包格式、阶段计划、横向待办、风险 | 想知道**为什么这么搭** |
| [findings.md](findings.md) | 着色 / 材质 / shader 的**逐条实测记录**(只增不改,含被推翻的结论) | 想知道**某个数是怎么来的**;动着色之前必读 |
| [findings-assets.md](findings-assets.md) | 网格 / 动画 / 音频 / 命名 / 平台的实测记录 | 动这几块之前 |
| [shader.md](shader.md) | 从 pak 里取 shader → 认归属 → 反汇编 → 对语义的**流水线** | 要读汇编时 |
| [petindex.md](petindex.md) / [android-*.md](android-device.md) / [spike-s*.md](spike-s1.md) | 图鉴归并 / 安卓侧那条路 / 早期技术验证 | 按需 |

### 动着色之前,先照这几条来

1. **先挑对排列,再读汇编。** 实机跑的是 `quality=Num ∧ LODUsed=0 ∧ DSId=0` 那份。
   `PROBE_SHADERS=1 dotnet run --project exporter -- --probe-material <资产>` 会给每条打上
   `← 实机默认`。**挑错不会报错** —— 拿到的仍是一份能反汇编、能读出完整公式的 shader,
   只是不是实机那份。这个坑在 findings.md 里记了**四次**。
2. **读出公式之后,逐个因子查它吃到的数。** 「代码在字节码里」≠「这一层可见」:
   `FresnelIntensity` / `Glow Intensity` / `HighLight SpecInt` 这类根默认是 0 的参数,
   会让一整层恒等于 0。`--probe-material PARAM:<名字>` 做全库普查。
3. **cb 槽位不要按顺序猜。** `PROBE_SHADER_DETAILS=1 PROBE_SHADER_INDEX=<i>` 打出
   `vector-param[i]` / `scalar-slot[i]` 的字节码,`03 <u16>` / `04 <u16>` 就是参数下标。
   标量槽与参数**不同序**(见 findings.md「水环」那节的对照)。
4. **默认值有两个来源,而且会打架。** 根材质的 `CachedExpressionData` 对同名参数只留一条;
   实机排列自带的 uniform 表才是 GPU 真拿到的。查默认值的顺序是
   **实例链 → 实机排列的编译期默认 → `CachedExpressionData` → 硬编码兜底**;
   `--probe-material DEFAULTDIFF` 列出两张表对不上的全部条目。
5. **改完必须过闸门**,三样一起看:

   ```sh
   dotnet run --project exporter -c Release -- --all --out ~/Downloads/rocom/packs-<本轮名>
   CMP_PACKS=~/Downloads/rocom/packs-<本轮名> uv run --with numpy --with pillow python tools/cmp_shots.py
   SWEEP_PACKS=~/Downloads/rocom/packs-<本轮名> uv run --with numpy --with pillow python tools/sweep.py
   cargo test --release
   ```

   基线:27 只中位 **亮度 0.95 / 调色板 0.065 / 描边 1.06 / 对比 1.04**;
   617 形态 **失败 0 / 空白 0 / 过曝 3**;**146 个测试**。
   `cmp_shots.py` 的模块注释里有九条「指标会骗人」的判据,动它之前先读。
6. **闸门也有量不到的东西。** 三对球在实机侧被 `game_mask` 的「取最大连通块」整个剔掉了,
   在我们这侧却进了选区 —— 那三只的「对比 / 亮度」不能信,只能逐球取像素对。
   水环只占轮廓一小块,调色板距离同样量不到。**这种时候要在提交里写清用的是哪条直接测量。**
7. **负结果照样写进 findings.md。** 这个项目里「查了、是关的 / 拿不到 / 撤回」的条目
   比落地的还多,它们省下的重复劳动最大。被推翻的结论**保留原文 + 开头标注**,不要删。

### 代码住在哪儿

| 要改什么 | 去哪儿 |
| --- | --- |
| **着色公式** | `src/pet/shader/*.wgsl` —— 11 份按主题拆开,`gpu/mod.rs` 用 `concat!(include_str!(…))` 拼起来。**加文件要同步那张清单。** |
| **打包 uniform / 加一个材质族** | `src/pet/gpu/mod.rs` 的「④ 逐材质」那段(文件里有 ①~⑥ 分节标) |
| **manifest 的材质字段** | `src/pack/material.rs`(`RawMaterial` 字段表 → `Material` + 各族结构 → `material_table`) |
| **导出器读材质参数** | `exporter/Materials.cs`(基色/不透明度/描边/炫彩/查参数的四层回退)与 `exporter/MaterialFamilies.cs`(七个族各自的参数) |
| **实机排列的默认值** | `exporter/ShaderDefaults.cs`;探针在 `exporter/MaterialProbe.cs` |
| **行为 / 手感 / 多实体** | `src/stage.rs`(测试在 `src/stage/tests.rs`) |
| **闸门与量化工具** | `tools/cmp_shots.py`(27 只对照)、`tools/sweep.py`(617 形态)、`tools/edge_profile.py`、`tools/posematch.py` |

### 渲染那几遍的顺序(改管线前先看这个)

```text
不透明遍(写深度):本体 → 描边壳 → 背板族 → 玻璃球的远半球 → paint_order / 专用不透明件
混合遍(只测深度不写):内层特效 → 玻璃件 → 加色特效
```

**混合遍不写深度**,所以同一遍里的先后就是遮挡关系 —— 需要互相遮挡的东西必须在不透明遍
留下深度(玻璃球那颗「远半球」就是为这个加的,见 findings.md「三对球」)。

**两遍都画在离屏画布上,而那张画布按 2 倍开、合成时缩回去**(`pet::target::SUPERSAMPLE`)。
管线本身一个采样点都没有(到处 `sample_count: 1`),而描边在默认那档只有 **0.6 个屏幕像素**宽
—— 不超采样就画成一圈会随姿势重掷的虚线。合成那块四边形因此必须**吸到设备像素网格上**
(`platform::shared::quad_rect`),对齐了双线性才退化成精确的盒式降采样。
见 findings.md「描边在动作里跟着抖、发糊」。**离屏 `--render` 那条路仍是 1×**,
四项闸门的基线不受影响。

## 1. 已验证的数据事实

**整节搬到 [findings.md](findings.md) 了。** 那是一份只增不改的实测日志(含被推翻的结论),
单独放一份是为了让这份 design.md 保持「架构与计划」的体量。

## 2. 技术选型

**结论：Rust + wgpu + 自写平台窗口层。**

项目的成败不在渲染，而在两个平台集成点：Wayland 的置顶/定位/输入区，Windows 的逐像素
alpha 置顶窗口 + 命中穿透。现成引擎恰好都在这两点撞墙：

| 方案 | 优 | 致命处 |
| --- | --- | --- |
| **Rust + wgpu + 自写窗口层** | 两个平台集成点都能精确控制；单二进制、低内存、多实体便宜；包加载就是读 zip | 场景/骨骼动画/混合/toon 着色要自己写(工作量可控)；无编辑器 |
| Godot 4 | glTF、AnimationTree、PCK 资源包、音频、导出全免费，出原型最快 | Wayland 后端无置顶与定位；`window_set_mouse_passthrough` 不覆盖 Wayland → Linux 只能退回 XWayland |
| Electron/Tauri + three.js | Web 技术栈熟，Windows 上 `setIgnoreMouseEvents` 可用 | Wayland 透明+置顶不可靠；常驻多实体内存代价大；GB 级资产在 JS 侧流式加载别扭 |
| Go(现有栈) | 与 rocom-capture 同语言 | 无可用的 wayland layer-shell 绑定，GPU 生态太薄 |

选定栈的组件：`smithay-client-toolkit`(wlr-layer-shell) / `windows-rs`(Win32 + DirectComposition) /
`wgpu`(Vulkan+DX12，`CompositeAlphaMode::PreMultiplied`) / `gltf` / `kira` 或 `rodio`(带播放速率，
正好复刻叫声变调) / `mlua` 或 `rhai`(行为脚本) / `egui`(配置与包管理 UI，与 wgpu 同栈)。

## 3. 运行时架构

### 3.1 窗口模型：一屏一个透明 stage，宠物是其中的实体

不采用「一只宠物一个窗口」：跨宠物互动、互相拖放、遮挡排序都需要同一个坐标空间与同一个场景，
单 stage 让这些几乎免费；代价(全屏 alpha 合成)可以用提交策略压掉，见 §3.3。

```
 ┌─ stage(每个显示器一个透明置顶表面) ────────────────────────┐
 │  ECS/slotmap: 实体 = {物种/形态, 位置, 状态机, 需求, 脚本VM}  │
 │  ├ 场景更新 → 骨骼动画采样/混合 → wgpu 渲染(premultiplied)  │
 │  ├ 每 N 帧渲一张 64×64 alpha mask → 命中测试 + 输入区        │
 │  └ 事件总线: 鼠标 / 邻近 / 屏幕边界 / 定时器 / 脚本 Intent    │
 └───────────────┬──────────────────────────┬────────────────┘
       平台层 trait│                          │
   ┌───────────────▼────────┐   ┌─────────────▼──────────────┐
   │ KDE Wayland:           │   │ Windows:                   │
   │ wlr-layer-shell        │   │ layered 窗口 + DComp       │
   └────────────────────────┘   └────────────────────────────┘
```

### 3.2 平台层

| 关注点 | KDE Plasma Wayland | Windows |
| --- | --- | --- |
| 表面 | 每 output 一个 layer surface，`layer=top`(不用 `overlay`，那会盖住菜单/通知)，四边 anchor，**`exclusive_zone=0`**，`keyboard_interactivity=none` | 每显示器一个 `WS_EX_LAYERED|TOPMOST|TOOLWINDOW|NOACTIVATE` 窗口 + DirectComposition 交换链 |
| 逐像素 alpha | wgpu `CompositeAlphaMode::PreMultiplied` | 必须 `CreateSwapChainForComposition`(GDI 的 `UpdateLayeredWindow` 路径不适合 GPU 渲染) |
| 命中/穿透 | `wl_surface.set_input_region` = 宠物轮廓并集；全局穿透 = 置空区域 | ~~`WM_NCHITTEST` 返回 `HTTRANSPARENT`；全局穿透 = 加 `WS_EX_TRANSPARENT`~~ — **这一格两处都错**，实机各栽了一次，见 §9 Phase 8 |
| 定位 | layer surface 的 anchor + margin | `SetWindowPos`(整屏窗口，宠物坐标在窗口内) |
| 多屏 | `zwlr_layer_shell_v1.get_layer_surface` 指定 `wl_output`，跟随 output 热插拔重建 | 枚举显示器，每个一个窗口 |
| 缩放 | `wp_fractional_scale_v1` 拿精确 scale(1/120 单位) + `wp_viewporter` 把物理像素 buffer 映射回逻辑尺寸;**此时 `set_buffer_scale` 必须留 1**,且要忽略 `wl_output` 的整数 scale 事件 | DPI 感知 + `GetDpiForWindow` |

已确认：开发环境 KDE Plasma 6.7.3 / kwin_wayland，`libkwin.so` 导出 `zwlr_layer_shell_v1` 与
`zwlr_layer_surface_v1`，layer-shell 可用。KWin 相关注意点：

- `zwlr_layer_shell_v1` 是 wlroots 系的**非正式协议**，KWin 只是兼容实现，跨大版本可能变化；
  Phase 0 S1 要记录实测的 KWin 版本，升级 Plasma 后重跑 S1 的验收项。
- `layer=top` 与全屏窗口、锁屏、通知/OSD 的叠放次序由 KWin 决定，不可假设，S1 里逐项实测。
- KWin 的窗口规则/脚本(KWin Script、`kwriteconfig` 规则)可作为定位与置顶的**备选**手段，
  但那是 xdg-toplevel 路线，交互不如 layer surface 干净，仅在 S1 失败时才考虑。
- **`exclusive_zone` 取 0 而不是 -1**:0 是「自己不占地方，但尊重别人占的地方」，合成器给的
  configure 就是**去掉任务栏后的工作区**(实测 2560×1440 → 2560×1368)，宠物正好踩在任务栏
  上沿；-1 是「连别人的独占区一起无视」，那样宠物的脚会藏到面板后面。两者都不会挤压其他窗口布局。

### 3.3 渲染与帧率

- 每形态一个 glb：mesh + skin + 全部所需 clip。骨骼动画在 GPU(或 CPU 蒙皮 + 顶点缓冲上传，
  实体数少时都够)；clip 间做交叉淡入淡出。
- 卡通着色：base color + ramp 光照 + 描边(法线外扩或屏幕空间)。**目标是「像」不是「同」**——
  游戏是自研 shader，含 RampTex/MatCap/描边/StarStick/Fragments 等几十个参数。
  **这条已经部分推进**:材质实例参数能完整读(§1),MatCap / StarStick / 玻璃内部层后来是照
  反编译出的公式做的(docs/shader.md);仍然是「像」而不是「同」的是基础 toon
  那几个数(见 §1「还是猜的」那一节)。
- 提交策略(全屏透明层的合成开销主要靠这些压掉)：
  - 无动画/交互时不提交帧；
  - 用 `wl_surface.damage_buffer` / DXGI dirty rect 只提交宠物所在矩形；
  - 空闲降帧(待机 15fps、睡觉 5fps)，交互中 60fps；
  - 前台全屏窗口(游戏/视频)不用自己处理:KWin 会把全屏窗口排在 `layer=top` 之上,
    宠物自然被遮住(S1 实测,见 spike-s1.md W1)。
- 命中测试与输入区共用**低分辨率 alpha mask**：每隔几帧把宠物渲到 64×64 离屏 RT 回读，
  延迟一帧无感，避免每帧 CPU 侧算轮廓。

## 4. 宠物包(插件)

### 4.1 分包原则

**一条进化链一个包**，包内多形态，启用后可在 UI 切换形态(不重导)。链的切分完全由
`PETBASE_CONF.stage / evolution_pet_id` 推出，资源目录名的数字后缀作为交叉校验。

**上面这段是旧方案**(2026-08-08 换掉了)。它有两个坑:`evolution_pet_id` 在分支链上是
**一串**(矿晶虫指着六个),只取第一个会把另外五种外观整条丢掉;而同名的两条链
(海盔虫的「本来的样子」与「磨损的样子」)本该是一个包的两套外观,却被切成两个、
靠补链首 id 区分。

现在**按图鉴号归并**:包名 `<图鉴号>-<链首名>`(`076-海盔虫.rkpet`,没图鉴号的记 `000`),
形态取 `PET_EVOLUTION_CONF`(权威表,王者形态也在里面),判重看资产目录名,
排序看资产名里的阶段位。归并后 **265 个包 / 755 个形态**,而旧方案在同一份数据上会出 678 个。
规则、边角与全量清单见 [docs/petindex.md](petindex.md) —— 那套规则**两边各实现了一遍**
(`tools/petindex.py` 与 `exporter/Config.cs` 的 `Packs()`),
用 `tools/petindex.py` 与 `dotnet run --project exporter -- --index` 对账,结果必须一致。

**包名与包的边界都变了,得全量重导一次**;`roster.toml` 里按名字记的阵容也会指不到新包。

### 4.2 结构

```
<链名>.rkpet                     # zip 归档
├── manifest.toml
├── forms/<asset>/model.glb      # mesh + skin + 已合并的全部 clip
├── forms/<asset>/tex/*.ktx2     # 基色/遮罩，已修正 BC7 通道序
├── forms/<asset>/voice/*.ogg    # 叫声(嗓子),按动作逻辑名命名
├── forms/<asset>/sfx/*.ogg      # 动作音效(身体动静),同一套名字
└── behaviors/*.lua              # 可选:该物种特有行为/互动
```

### 4.3 manifest schema(草案;实际产物见 spike-s3.md 与导出器 `Manifest.cs`)

```toml
schema = 1            # manifest 格式版本
runtime_abi = 1       # 需要的运行时 ABI，运行时拒绝不兼容包
source_version = "…"  # 导出时的游戏版本/pak 指纹，便于排查
generated_at = "2026-07-25"

[species]
id      = 3001        # 链首 PETBASE_CONF.id
name    = "喵喵"
chain   = [3001, 3025, 3007]

[[forms]]
id        = 3001
name      = "喵喵"
stage     = 1
asset     = "Gra_MiaoMiao1_001"
model     = "forms/Gra_MiaoMiao1_001/model.glb"
scale     = 1.00      # MODEL_CONF.model_scale / 100
height    = 80        # 绑定姿势包围盒高度(cm)，用于换算屏幕像素
locomotion= "ground"  # ground|hover|swim ← PETBASE_CONF.move_type
tags      = []        # 互动能力标签，如 ["cleaner"]/["commander"]

  [forms.clips]              # 由 ANIM_CONF 自动生成
  idle   = { clip = "World_Idle",   ms = 1333, loop = true }
  walk   = { clip = "Walk", ms = 1133, frames = 35, in_place = false, root_motion_cm = 53.06, speed_cm_s = 46.8 }
  run    = { clip = "Run",  ms =  600, frames = 19, in_place = false, root_motion_cm = 180,   speed_cm_s = 300 }
  happy  = { clip = "Common_Happy", ms = 1500 }
  anger  = { clip = "Common_Anger", ms = 1500 }
  shock  = { clip = "Common_Shock", ms = 1500 }
  sleep  = { start = "Common_Sleep_Start", loop = "Common_Sleep_Loop", end = "Common_Sleep_End" }
  callout= { clip = "Common_Show",  ms = 1500 }

  [forms.face]               # 眼神:动画自带的 EC_* 曲线,见 §4.3.1
  # [毫秒, 眼神图集第几格(1..8)] 的阶梯。**一个脸槽一条**,键是槽名
  # (eye / eye_1 / mouth / mouth_1 / dynamic1..4,与材质表的 face_track 同一套),
  # 同一段里各槽可以完全不一样;没写的槽 = 那段没驱动它,用性格那张脸
  Happy  = { eye = [[0, 1], [167, 2], [1367, 1]], mouth = [[0, 1], [167, 2], [1367, 1]] }
  Shock  = { eye = [[0, 1], [167, 3], [1333, 1]], mouth = [[0, 1], [167, 7], [1333, 1]] }
  Relax  = { eye = [[0, 1], [1233, 6]], dynamic1 = [[0, 1], [1233, 4]] }   # 幽影树的两颗球

  [forms.morph]              # 形变目标(脸的 blendshape),顺序即 glb 里 morph target 的顺序
  targets = ["Xi", "Jing", "Nu", "Shui", "Ai", "Shou", "Yun"]

  [forms.voice]              # 叫声:键与 [forms.clips] 同一套(动作逻辑名)
  cents_low  = -300          # voice = -100(粗嗓门);运行时按 2^(音分/1200) 调播放速率
  cents_high =  300          # voice = +100(婉转声)
  Happy   = { path = "forms/…/voice/Happy.ogg",  ms = 2366 }

  [forms.sfx]                # 动作音效:同一把键,**不变调**
  Happy   = { path = "forms/…/sfx/Happy.ogg",    ms = 2354 }

  [forms.materials]          # 从游戏材质实例解出来的「这个槽画什么」,见 §1
  # base_color 缺失 = 纯特效层(材质里没有 BaseTex/EyeTex),运行时整片跳过;
  # 这类条目额外记下父链与全部贴图参数,留给将来的特效通道用。
  MI_Gra_Miaomiao1_001_By = { base_color = "forms/…/tex/T_…_By_D.png", mask_alpha = false, mask_clip = 0.3333, blend = "BLEND_Opaque" }
  MI_Gra_Miaomiao1_001_Es = { base_color = "forms/…/tex/T_…_Es_D.png", mask_alpha = true,  mask_clip = 0.3333, blend = "BLEND_Opaque", face_track = "eye" }
  # face_track:这个脸槽跟 [forms.face] 里的哪一条走。序号是按**网格槽序**数的
  # (第 k 个 _Es 跟 eye_k),运行时的材质表按名字查、数不出来,所以导出器写好

[report]              # 导出覆盖率,缺失动作让运行时降级而不是报错
missing_clips = ["hide"]
```

实际产物比这份草案更细(每 clip 带 `frames`/`root_motion_cm`/`speed_cm_s`，贴图带槽位与尺寸)，
见 [spike-s3.md](spike-s3.md) 与导出器的 `Manifest.cs`。

#### 4.3.1 眼神是**动画数据**,不是猜的

游戏把「这段动作眼睛/嘴是哪一格」写在 AnimSequence 自己的浮点曲线上:每段带
`EC_Eye`(眼)与 `EC_Mouth`(嘴),值 = **眼神图集第几格 × 100**;整段还盖着一个
`ANS_SetFacialExpressionIntegrated` 的 AnimNotifyState —— 那就是把曲线值刷到材质
`Number` 参数上的东西(`M_P_Eyes` 根材质上默认为 1 的标量)。

全库实测(1000 个宠物资产 / 23557 段动画):`EC_Eye` 22884 段(97.1%)、
`EC_Mouth` 8901 段(只有做了嘴图集 `_Mh` 槽的宠物才有)。取值几乎全落在八档:
`100:44923 200:6031 300:2178 400:10430 500:13500 600:1792 700:5809 800:1633`。
格号编码与 `M_P_Eyes_Mesh` 的顶点色卡号是同一套(`col + 2·row + 1`),两边独立对上。

三条只有读了曲线才做得到的事:

- **眼与嘴分开**。8636 段两条都有的动画里,主值不一致的约占三成
  (眼1嘴2 181 段、眼4嘴1 138 段、眼2嘴1 101 段…)。原来一个 `face_uv` 喂两个槽,
  嘴永远跟着眼睛跑。
- **逐帧**。待机段里 `EC_Eye` 在 1 与 5(闭眼)之间来回跳 —— 那就是**眨眼**。
- **每形态一份**。加灵一阶 Fear 用第 6 格,二/三阶用第 7 格;同一条进化链都不同,
  全库压成一张表必错。

`EC_Dynamic1..3`(全库 40 个 `_Dynamic*` 材质)还没接,那些槽暂时跟着眼睛走。

还有一条**不是贴图**的路:37 个资产(22 个是同一套脸的 blendshape
`Xi 喜 / Jing 惊 / Nu 怒 / Shui 睡 / Ai 哀 / Shou 收 / Yun 晕`)把脸做成了**形变目标**。
里奥一/二阶就没有嘴的图集槽,游戏里它只有 By/By_Ol/Es 三个材质,嘴是本体网格上的形变:
每个 target 影响本体那一段 184~234 个顶点、最大位移 2.1~4.4cm,影响点正好落在头前面
口鼻那一块。权重由**同名的动画曲线**驱动,导出器转成标准的 glTF morph target +
`weights` 通道(见 `exporter/MorphTargets.cs`)。

**一个形态可以有好几个脸槽**,各有各的图集与曲线:`_Es`(眼)、`_Mh`(嘴)、
`_Dynamic1..6`,而且同一族可以有第二个(一窝蜂二/三阶身上是两只蜜蜂,各一个 `_Es`)。
配对关系写在动画的通知实例上:`ANS_SetFacialExpressionIntegratedBase` 有一张
`LayerFunctions`(`Eye → ML_FacialModelClip_Eye`…),每段动画的通知带一串
`FacialExpressionConfigs`(`{类型, 下标, 目标, 目标下标}`),**曲线名就是 `EC_{类型}` 加下标**
—— 一窝蜂三阶那 5 条配置 `(Eye,0)(Eye,1)(Eye,2)(Mouth,1)(Mouth,2)` 与它动画里的
`EC_Eye/EC_Eye_1/EC_Eye_2/EC_Mouth_1/EC_Mouth_2` 逐条对上(`--probe-face` 打得出来)。
「下标 → 哪个材质」那一步是推的:cooked 包里读不到图层栈,所以按**槽序**数同后缀的材质。
全库 28 个形态用得上第二个槽或 `_Dynamic*`;`EC_*_By`(把脸刷到本体材质上,17 个资产)
还没接,见 findings.md。

**用词:「眼神」与「表情」是两套,别混**(2026-09-04 定的口径,两端界面与文档一致):

| | 是什么 | 那几个词 | 出处 |
|---|---|---|---|
| **眼神** | 脸那张 2×4 图集里的一格 | 默认 / 微笑 / 惊讶 / 生气 / 困倦 / 哭哭 / 闭紧 / 晕眩 | 前六个里五个是 `NATURE_CONF.emotion_desc` 的原词;没有官方名的三格照美术给形变目标起的名(`Jing` 惊 / `Shou` 收)与行为表的 `dizzy` 晕眩来 |
| **表情** | Happy/Sad 那几段**动作** | 开心 / 放松 / 炫耀 / 生气 / 伤心 / 惊恐(动作表还有 震惊 / 警觉 / 召唤) | 对得上 `LLM_PET_BEHAVIOR_CONF` 的抄官方词;`Alert`/`CallOut` 两条例外,见 `stage::RUNTIME_CLIPS` |

眼神只在下载站的下拉框里让人直接挑(`?face=生气`,**不带「眼」字** —— 桌面版性格行里
那句「急躁『生气眼』」的后缀是那一行自己加的);表情那套是配置窗口的「表情池」与
两端的动作按钮。核对过程见 findings.md 同名那节。

### 4.4 加载与体积

- 发现路径 `~/.local/share/rocom-pets/packs/`、`%LOCALAPPDATA%\rocom-pets\packs\`；
  启动只读各包 manifest(轻)，**启用某形态时**才流式读该形态的 glb 与贴图。
- **目录与 `.rkpet` 两种形态运行时都直接读**,不解压到临时目录。做法是「虚拟路径」:
  manifest 里的相对路径照旧拼在包的位置后面(`…/喵喵.rkpet/forms/x/model.glb`),
  `assets::read` 看路径上有没有一段是**真是文件**的 `.rkpet`,有就开归档读余下那段。
  这样 `Form` 里那二十多个 `PathBuf`、三张资产缓存的键、日志与阵容存档全都不用动 ——
  代价是**包内资产一律走 `assets::read`**,别再直接 `fs::read`(见 src/assets.rs)。
- 体积(S3 实测，喵喵链 16 个动作 + 1024 贴图)：每形态 **2.1–5.0MB** glb + 贴图，
  一条链目录 13MB、`.rkpet` 6.9MB。比原估的 2MB/形态高一倍,动画通道是主要占比
  (骨骼数 × clip 数 × 帧数)。
- 压体积手段(已做)：只导桌宠动作白名单、恒定轨道不写通道/只写单帧。
  (待做，Phase 4)：关键帧精简、贴图降到 512、KTX2/BasisU、只导当前启用的形态。
- **打包只调得动「留不留源目录」,压缩本身没有余量**。zip 的压缩级别量过:
  `SmallestSize` 比默认档只小 **0.3%**、耗时多 57%;把已压过的 png/ogg 改成仅存储
  反而更大。全库体积构成是 glb 2008MB(63%)/ png 1121MB / ogg 31MB ——
  glb 就是那个大头,所以下一步只可能是 KTX2 或关键帧精简,不是换压缩参数。
  归档**必须 deflate 或 store**:运行时的 `zip` crate 只链了 `deflate-flate2`。
- 全量导出用 `--zip-only`(打完删源目录):`--zip` 两份都留是 3.3GB + 2.0GB,
  只留归档是 2.0GB(25 条链抽样压缩比 0.61)。`--skip-existing` 认得 `.rkpet`,
  所以只留归档也能分批续跑。

## 5. 动作与行为

- **逻辑动作层**：运行时只认 `idle/walk/run/happy/anger/sad/fear/shock/show/relax/sleep/callout/…`，
  具体 clip 由 manifest 映射，缺失则降级(如无 `run` 就用 `walk` 提速)。
- **三段式(Start/Loop/End)是一等公民**：睡觉、隐藏、技能都是这个结构；Loop 时长由状态机需求决定。
- **野外那些「特殊动作」在数据里没有独立 clip**(2026-08-09 查实)：游戏的行为树把**战斗技能的
  Loop 段**按住重播,再叠上导航的位移。点点野外「一边转圈一边走」= `BT_Patrol_Diandian` 在
  `LuaActionMoveTo` 上挂一个每 0.5s 重播 `Skill2Loop1` 的 service,而那段片段本身就是
  `Bip001` 整只自转两圈(33 帧,首尾重合可无缝循环);捕尘长绒的「清扫」同样是
  `Skill2Loop1`/`Skill3Loop`。全库 18 棵行为树是这个写法,147 只宠物会在野外状态重播战斗档片段。
  **所以白名单收了四段 Loop**(`Skill1Loop`/`Skill2Loop1`/`Skill2Loop2`/`Skill3Loop`,
  620/622 个 anim_conf 都齐)：每形态 glb +9~16%(实测点点一阶 2378→2688KB),
  `RUNTIME_CLIPS` 末尾追加四格,只能手动点,自发行为不挑它们。
  **Start/End/Trans 不收**:抽样 60 个形态 648 段量根骨位移,起落段有一半偏出 30cm 以上
  (噬影蚕 `Skill_2_End` 4.8m),桌宠站位固定、画布只有 1.64 倍取景余量,那是瞬移;
  Loop 段 228 个里 68% 不到 0.25 身高、没有一个超过一整个身高。
  连带的两件事都已办掉:声音那层查出来是**有的**(`Fight_Skill_1/2/3`,见 §7),
  `act.rs` 的「珀尔鼬指挥捕尘长绒清扫」也换成了游戏里那几段真片段。
- 每实体一个状态机 + 需求值(困倦/心情/无聊) + 作息时钟；转移由事件驱动：鼠标、邻近实体、
  屏幕边界、定时器、脚本 Intent。
- 进阶：LookAt BlendSpace → 视线跟随鼠标。
- **已由 S3 定论**(详见 [spike-s3.md](spike-s3.md))：走跑动画**逐 clip 不一致**——同一条链里
  有的带 root motion 有的原地，方向恒为 glTF +Z(= UE +Y)。故 manifest 逐 clip 给
  `in_place`/`speed_cm_s`；运行时有速度就用它推进位置并原地循环播放，没有就按 locomotion
  取默认值，并对离谱值钳制(魔力猫 Run 反推出 7.5m/s)。单位：glb 米制，`height_cm` 取
  `ImportedBounds` 全高(喵喵链 80/104/204cm)。
- **摆一段给人看时,循环段按整周期按住约 3 秒**(`stage::play_hold`)：走/跑与技能那四段
  一个周期只有 0.5~1.6s(捕尘长绒 `Run` 0.53s),原来「播一遍就回待机」的算法让配置窗口
  点一下只看得见抽一下。按住的是**整周期数**——掐在半路收尾姿势会跳。演出脚本那一拍也走同一条
  (下一拍到点照样打断)。一次性动作(表情/召唤/落地)不受影响,仍是演完就完。
- **待验证**：
  - `MODEL_CONF.SMR`、`PET_SHOW_SPEED_CONF` 各自的含义(现在速度直接从 root motion 反推，
    够用；要与游戏内手感对齐再查)；
  - `INTERACTIONTREE_CONF` 的 `anim_key*` 到动作表的确切映射(「摸头」指向的 id 20 在
    `ANIM_ID_CONF` 里叫 `Sad`，字面对不上，需实机核对)；
  - stage 0 目录(如 `Gra_MiaoMiao0_001`)只有 Mat/Tex 没有 SKM，是蛋还是共享皮，包里怎么表达。

## 6. 多实体与跨宠物互动

- 同一 stage 内多实体：同物种可多开(一个包创建多个实体)，不同包可同时启用。
- **事件总线**：`Intent{from, kind, target}` + `Perception{邻近实体, 鼠标, 屏幕边界}`。
  **已实现**(见 §9 Phase 5 第 5 步)：邻近距离按**脚底点 / 身位**算，行为只往总线塞意图、
  转身与播动作在 `dispatch_intents` —— 演出脚本要插的就是这两者之间。
- **互动包(interaction pack)**声明依赖，双方都在场且距离够近才可触发：

  ```toml
  [interaction]
  id = "peel_commands_cleaner"
  requires = [{ species = 3758 }, { species = 3604 }]   # 珀尔鼬 × 捕尘长绒
  trigger  = { kind = "proximity", max_distance = 200, cooldown = "3m" }
  ```

- 编排用**演出脚本(时间轴)**而非让两个状态机自发协商：谁在第几秒播哪个 clip、走到哪、
  何时出声，可靠且可调。脚本用 Lua(自产包)，若将来接受第三方包则换 WASM 沙箱。
  **第一版已实现,硬编码在 `src/act.rs`**(见 §9 Phase 5 第 6 步)——「抬到 Lua」还没做,
  也还没确认值得做:整场演出就是一张 11 行的表,Lua 换来的表达力未必抵得上多一个 VM 的代价。
  上面那段 `[interaction]` TOML **也还没落地**(选角/触发/冷却现在都在 Rust 常量里),
  且里面的 `species = 3758` 是笔误:3758 是**形态** id,物种是 3757。
- ~~诚实的限制：「清扫」这类游戏里由行为树驱动、没有独立 clip 的行为，只能用现成动作拼近似~~
  **解掉一半(2026-08-09)**:「清扫」不是没有 clip,而是行为树按住**战斗技能的循环段**重播
  (捕尘长绒 `BT_Xiaozhu_SelfAction` 用 `Skill2Loop1`+`Skill3Loop`,珀尔鼬「指挥大扫除」
  用 `Skill1Loop`),那四段已进白名单(见 §5),演出用的就是真片段。
  仍是近似的部分:游戏那边还叠着泡泡特效、清扫目标点与导航位移,我们只有动作和走位。

## 7. 音频

- 来源:rocom-petvo 已跑通的 `Pet_Vo_*.bnk` + wem → vgmstream 管线,转 **ogg vorbis**
  进包(草案写的 opus,实际用了 vorbis:rodio 自带解码、不必再拉一个依赖)。
  **两层**:叫声 `Pet_Vo_*` + 动作音效 `Pet_Action_*`,见 §1「动作音效是另一层声音」。
- 粗嗓门/婉转声是运行时 pitch RTPC,用播放速率/变调复刻,不需要额外音频文件;
  **只作用在叫声那层**。
- 触发点:**每一段有配音的动作**(开心/震惊/惊恐/伤心/生气/炫耀/放松/警觉/召唤,
  外加技能那三条),由受惊、摸头、睡醒、启用召唤、待机表情、配置窗口那张动作表分别触发。
  默认低音量、可静音、可全局关。
- **技能循环段也有声**(2026-08-09 补):把 1192 个 `Pet_Action_*`/`Pet_Vo_*` 全枚举了一遍
  (解 HIRC 的 Event 对象,拿候选名的 FNV-1 去命中),`Fight_Skill_1/2/3` 的覆盖率
  606~609/610 与 579~580/582,和八条 `Common_*` 一样齐。**一技能一条事件、不分段**:
  `Skill2Loop1` 与 `Skill2Loop2` 共用 `Fight_Skill_2`,同一段 wem 只转一次、两把键指同一个 ogg。
  顺带复核了 `World_Walk/Run/Idle/Jump_Fall`、`Common_Sleep_Loop` —— 两族库里**一个都没有**,
  走跑与睡觉仍然是哑的。
- **自发的声音一分钟至多一次**(`SELF_SPEAK_COOLDOWN`):待机表情大约每 20~40 秒一个,
  做一次响一次的话桌上那只每半分钟叫你一嗓子。人点出来的不受这条管。
- **不做 BGM**(体积、版权、干扰)。

## 8. 导出器

`pak → 宠物包` 的本地工具，输入是用户自己的游戏安装。

1. 读配置(`PETBASE_CONF`/`MODEL_CONF`/`ANIM_CONF`/`ANIM_ID_CONF`)，过滤测试与重复行，
   按 `stage/evolution_pet_id` 归成链，输出待导清单。
2. 用 CUE4Parse 导每个形态的 `SKM_*`(glb) + 所需 `AnimSequence`(psa) + `Tex/*`(png)。
3. **把 psa 动画合并进 glb**：glTF 导出器不产动画，且 glb 做过 UE→glTF 轴转换而 psa 保持
   UE 空间，合并时要补变换(或统一走 psk+psa / UEFormat 对再转)。psa 结构简单
   (BONENAMES + 逐帧 quat/pos)，已在 rocom-capture 侧验证过这条数据通路正确。
4. 贴图修正 BC7 通道序、按材质名后缀 `_By/_Es/_Mh` 接槽位、转 KTX2/webp。
5. 叫声转码，生成 manifest 与覆盖率报告，打包 zip。

依赖与坑：
- **CUE4Parse 克隆必须先打补丁**:`git -C "$CUE4PARSE_DIR" apply <本仓库>/exporter/patches/*.patch`。
  三条上游没修的(`0003`/`0004` 是本作特有的格式,`0002` 是通用的):`0002` 网格没有顶点色缓冲时 `COLOR_0` 该是白的(三个族拿它当遮罩,
  给 0 等于整层关掉)、`0003` 标量参数表的步长(不改则探针只有第一个参数有名字,见 findings.md
  「步长差 4 字节」那节)、`0004` luac 前面那 7 字节头(只影响手工读 lua,导出不用)。
  `0002` 导出器启动时会自检并拦住,不打跑不起来。
  **法线那条(`FPackedNormal` 少括号)上游 9893d83b 自己修了**,补丁已撤;
  自检仍在,克隆太旧会被拦下。
- CUE4Parse-Natives **必须带 ACL 编译**，否则动画解压报 `nAllocate` 找不到：
  `git submodule update --init --recursive CUE4Parse-Natives/ACL/external/acl`，
  再 `cmake -B builddir -DCMAKE_BUILD_TYPE=RelWithDebInfo . && cmake --build builddir`。
  build type **必须避开 Debug/Release**——那两个会命中 `install(TARGETS … RUNTIME DESTINATION)`，
  Linux 上 SHARED 库属 LIBRARY 产物无 destination，cmake 报错会让 `dotnet build` 挂在 MSB3073。
- 语言：导出器留在 C#(CUE4Parse 在那边)，运行时是 Rust；两者只通过包格式耦合。

## 9. 实施阶段

### 9.0 原始需求对照(2026-07-26)

立项时定下的九条需求，逐条对当前代码核过一遍：

| # | 需求 | 状态 |
| --- | --- | --- |
| 1 | 独立新仓库、不限定语言 | ✅ Rust 运行时 + C# 导出器 |
| 2 | 至少支持 Windows 与 Linux Wayland | ✅ 两个后端都实机可用(KDE Wayland 日常在跑;Windows 2026-08-01 实机确认) |
| 3 | GB 级数据 → 主程序 + 宠物插件、按需启用 | ✅ 包目录 + `--list`/`--pack` + 托盘「加一只」+ 配置窗口的包管理(导入/查找/删除)，启动只读 manifest;包可以是目录或 `.rkpet` |
| 4 | 同一进化链形态封装进一个包、启用时可切 | ✅ 一链一包，托盘「形态」子菜单单选切换，配置窗口里也能切 |
| 5 | 同时启用多个包 / 一个包开多个实体 | ✅ `Stage` 持实体集合,托盘里加一只/撤下,阵容存 `roster.toml` 重启恢复 |
| 6 | 跨宠物互动(珀尔鼬指挥捕尘长绒) | ✅ 感知总线 + 演出脚本,两只挨近就开演(`src/act.rs`) |
| 7 | 普通动作：睡觉/行走/奔跑/生气 | ✅ 睡觉、行走、生气(表情池)、奔跑(远处目标才起跑)都有;落地 `JumpFall` 一并补上 |
| 8 | 部分支持宠物叫声 | ✅ 九种情绪各一段叫声 + 一段动作音效,叫声按 `voice` 属性变调(2026-08-09 从四段扩上来) |
| 9 | 穿透开关 + 点击受惊 + 摸头 + 把一只拖到另一只旁边 | ✅ 拖动按脚底 z 序只拎起被点中的那只，其余照常待机 |

结论(当时)：**多实体是最大的一块缺口，且同时卡着 #5/#6/#9**，所以它是下一步；`Run` 是几小时
的小补丁，顺手在同一阶段做掉。剩下的 Windows 与叫声都是独立块，不互相阻塞。

**Phase 5 / 6 / 8 都做完之后:九条原始需求全部结掉。** 之后 Phase 7 又补上了
`.rkpet` 直读与配置窗口(#3 的「按需启用」从「命令行 + 托盘」变成了真的能点着管);
剩下的是分发(安装包/自启)与横向待办里那些渲染保真度的条目。

### Phase 0 — 技术验证(spike，各 1–2 天，失败即换路线)

必须先做，因为结论会改架构。

| # | 内容 | 验收标准 |
| --- | --- | --- |
| S1 | 平台层：KDE Wayland(layer-shell) 与 Windows(DComp) 各画一张半透明贴图 **(Wayland ✅ / Windows 未开始,见 [spike-s1.md](spike-s1.md))** | 两平台都能：置顶于普通窗口之上、指定坐标、逐像素 alpha 正确(无黑边/无不透明底)、贴图内点击被自己接到而贴图外点击落到下层窗口、运行时切换全局穿透生效、多显示器各自一个 stage 且 output 热插拔不崩；另记录 KWin 下 `layer=top` 与全屏窗口/锁屏/通知的实际叠放次序，以及空闲与活动时的 CPU/GPU 占用。**这是全项目成败点。** |
| S2 | 渲染：wgpu 加载 glb 播骨骼动画 + toon 着色 | **✅ 见 [spike-s2.md](spike-s2.md)**：形体与 CPU 参考实现(`tools/verify_glb.py`)一致；淡化中点是平滑中间态；单只 0.040–0.054ms/帧(60fps 预算的 0.3%) |
| S3 | 导出器：动画合并进 glb，跑通喵喵整条链(3001/3025/3007) | **✅ 见 [spike-s3.md](spike-s3.md)**：三形态动画正确(途中修掉 CUE4Parse 的骨骼旋转 bug)；root motion/朝向/单位已定论；manifest 已产出 |

### Phase 1 — 单宠物 MVP

**已完成**:`--pack` 载入宠物包(读 manifest)、宠物站在工作区底边(踩任务栏上沿)、
`Idle` 循环、随机挑目标点用 `Walk` 走过去并平滑转身、拖放(松手落回地面)、
`SIGUSR1` 切穿透、宠物按 `height_cm × --px-per-cm` 换算屏幕尺寸。
实测:自身 CPU **1.2% 单核**(30fps 推进动画)、RSS 152MB(debug 依赖 + NVIDIA Vulkan)。

**已补**:配置文件(`~/.config/rocom-pets/config.toml`,首次运行生成带注释模板,
命令行参数优先)、托盘菜单(StatusNotifierItem:鼠标穿透勾选 / 召回宠物 / 退出)、
自己的 D-Bus 控制接口。

全局热键当时做了两条路,后来**只留下第二条**(2026-08-02,见 Phase 7 第四轮):

1. ~~**XDG GlobalShortcuts portal**(`org.freedesktop.portal.GlobalShortcuts`)~~。
   应用只能*建议*按键,KDE 会**弹窗让用户确认**——在用户点之前 portal 不回应,
   所以代码里放了看门狗提示去看弹窗(一开始误判成「KDE 丢弃了请求」,实机确认是等确认)。
   要桌面实现 portal、要用户点授权,而下面那条把同一件事做完了,于是整套删掉。
2. **`org.rocom.Pets` D-Bus 接口** + `rocom-pets --toggle-passthrough|--recall|--quit`。
   在 KDE「自定义快捷键」里把任意键绑到这条命令即可,键位归系统管,
   桌宠一个组合键都不抢;顺带让宠物可脚本化。

**待做**:落地用 `JumpFall` 动作、damage 局部提交、开机自启(`packaging/rocom-pets.desktop`
复制到 `~/.config/autostart/`)。

### Phase 2 — 鼠标交互

**已完成**:轮廓命中与轮廓输入区(离屏画布 alpha 异步回读成 8 物理像素的格子掩码,
腿与尾之间的空隙能点穿,实测输入区 60–87 个矩形随动画变化)、点击受惊(`Shock`)、
摸头(指针在头部区域来回蹭够 3 次换向 → `Happy`)、拎起来害怕(`Fear`)/放下落地、
~~**按姿势变化速度**自适应降频~~(2026-08-02 取消,见下)。行为逻辑有 10 个单测(用 `Model::for_test` 的合成模型,
不碰 GPU 也不需要宠物包)。
实测:CPU **1.3% 单核**、RSS 219MB。

降频这条踩过一次:一开始按状态硬分档(「待机」→ 12Hz),实机反馈**明显发顿**——
待机动画本身带起伏,实测关节最大速度约 6m/s(行走 4.7m/s),根本不算静止。
改成用关节速度连续映射成帧率(1m/s 以上跑满 30Hz,越接近静止越省,下限 10Hz):
待机/行走都稳稳跑满,睡觉那类真正近乎静止的动作会自动落到下限,不需要给每段动作手工标注。

**这条后来整个删掉了**(2026-08-02,见 Phase 7 第三轮):帧率改成配置里的一个
**目标值**(`fps`,托盘给 20/30/60 三档),台上在干什么都按它推进。
理由是「什么时候降、降到多少」全凭它自己判断,而帧率是用户看得见、也说得出偏好的东西;
省下的那点 CPU 不值得让人猜「怎么有时候看着比较顿」。跟着删掉的还有
`Player::measure_motion` —— 那是**每只宠物每帧**一次的 Vec 分配 + 全关节距离扫描,
存在的唯一理由就是喂给降频判断。实测 8 秒的 CPU 时间:20 帧 30 个时钟节拍、
30 帧 39 个、60 帧 71 个。

**待做**:多显示器(手上只有单屏,没法验)、HiDPI 分数缩放(见下)、
掩码回读的内存开销(比 Phase 1 多 ~65MB,疑似 wgpu 的可映射缓冲内存池;
若要抠可以改成渲一张 64×64 的专用掩码附件而不是回读整张画布)。

### Phase 3 — 行为引擎

**已完成**:需求值(困倦/无聊)驱动的状态机、睡觉三段式(入睡 `SleepStart` → 睡着 `SleepLoop`
循环到睡饱 → 醒来 `SleepEnd`)、被戳会醒(而不是原地受惊)、待机时随手做表情
(`Happy/Sad/Anger/Show/Relax/Alert` 里随机)、指针悬在身上时侧身「瞥一眼」。
时间尺度是手感常量(困倦 8 分钟攒满、睡 90 秒睡饱、无聊 6 秒攒满),
`ROCOM_PETS_NEEDS_SPEED=20` 可整体加速,几十秒看完一轮作息。
(睡着时姿势几乎不动,Phase 2 的自适应帧率原本会自动把它降到 10Hz;那条优化后来取消了,
见上面 Phase 2 那一段。)
新增 5 个行为单测(作息三段、戳醒不受惊、无聊消涨、瞥视方向、睡着时帧率不变)。

**不做**:真正的视线跟随。它要 LookAt BlendSpace(没导出),而且 Wayland 下**输入区之外
根本收不到指针事件**——要追全屏光标就得把输入区扩大到吃掉点击,代价不划算。
现在只在指针落在身上时侧身,读起来已经像在瞥。

**待做**:按真实时钟的作息(std 没有时区,要引依赖)、心情影响表情选择、饥饿/喂食。

### Phase 4 — 包格式定稿与导出器成品

**已完成**:

- 导出器 `--all`:遍历全部宠物、**按进化链去重**、写 `report.txt`(每个形态的动作命中/缺失、
  体积、警告 + 汇总);`--limit` 试跑、`--skip-existing` 分批续跑
  (过滤在计数**之前**,否则 `--limit` 永远只覆盖前 n 条链);
- manifest 加 `source_version`(pak 文件名+长度+挂载文件数的短哈希):换版本重导后会变,
  便于排查「这包是哪版导的」;
- 包目录 `~/.local/share/rocom-pets/packs`:`--list` 列出包与形态、`--pack` 既接受路径
  也接受包名/物种名;
- **运行时形态切换**:托盘「形态」子菜单(单选),切换时重建模型与那套 GPU 资源
  (管线/画布/合成四边形/掩码缓冲全跟形态绑),位置重新落地。实测喵喵 161px ↔ 魔力猫 481px。

**全量导出暴露的问题(单只喵喵试跑时看不出来,都已修)**:

1. **一个形态缺资产不该拖垮整条链**。有些进化阶段这版本根本没做资产目录
   (如 `Roc_MeiQiu1_001` 不存在,只有 `2_001`),原来整条链直接失败;
   现在按形态 try/catch,跳过并记进报告,全链皆缺才跳过整条(实测 110 条链属于这种)。
2. **网格名不能硬编码 `SKM_<资产>_Skin`**:改成枚举目录直属的 `SKM_*` 并优先 `_Skin` 结尾
   (`LOD_`/`ABP_` 前缀的不是网格)。
3. **197/827 个形态自己没有 `Animation/` 目录**(24%)。它们是变体资产
   (`Win_ShiJiu1Ar_001` 的 `Ar`)或换了属性前缀的同族(`Gra_DiMo2_001` vs `Lig_DiMo2_001`),
   与同族基础资产共用骨架与动画。现在两级回退:先找同 `anim_conf_id` 的资产(配置层面的显式共享),
   再按**族名 + 阶段**找(族名 = 资产名中段去掉末尾 `Ar` 与阶段数字)。
   实测圣草迪莫 0/16 → 16/16,借自 `Lig_DiMo2_001`,渲染姿态正确。
   借错也安全:骨骼名对不上时 `GlbBuilder` 会跳过那段动画,只会少动作而不会渲出乱形。
4. **部分材质槽指向共享贴图**:890 个资产目录里 352 个至少缺一张 `<槽>_D`
   (眼睛等用的是 CommonTexture 里的共享图集),而「用哪张」只写在材质实例参数里,
   那份参数在本作解不出来(§1 的 OverflowException)。运行时现在退用本体槽贴图
   而不是留一块纯白——**是权宜之计,不是正确解**。真要修得先解出材质参数,
   这也是目前**性价比最高的保真度改进项**。

**并行化**:导出按链并行(`Parallel.For`,默认并行度 = CPU 核数,`-j` 可调)。
链之间没有共享可变状态(各写自己的包目录),provider 的并行只读在 rocom-capture 的解包脚本里
已经压过;控制台与报告文本按链攒着、跑完按原顺序合并,否则并行下输出会交错。
实测 16 核:全量 **10.6 分钟 → 2.4 分钟**(采样 CPU ≈1000%,峰值 RSS 2.3GB);
产物与单核跑**逐字节一致**(`diff -r` 验证),报告内容与顺序也一致。

并行顺带暴露一个原本就存在的 bug:**72 个物种名被多条链共用**(「棋契陛下」有 10 条),
而包目录直接拿名字命名 → 互相覆盖(530 条链成功却只剩 395 个目录),并行下还可能两条链
交错写同一个目录。重名的现在追加链首 id(`名字-3001`)。

**全量回归的做法(值得固化)**:光看导出器「0 失败」不算数——它只证明写出了文件。
真正的检查是**拿运行时把每个形态都载入并渲一帧**,再看两个指标:
① 退出码(能不能加载),② 渲出来的不透明像素覆盖率(**是不是真画出了东西**)。
就是这一步逮到下面三个 bug——它们全都能在「导出成功」的产物里安静地待着。
两个踩过的坑:`-o /dev/null` 会让 PNG 编码器报「格式判不出来」,看着像 831 个全崩,
其实是渲完才失败;以及只测喵喵一只是不够的,喵喵恰好是唯一躲过 alpha bug 的宠物。
当前基线:**767 个形态加载+渲染全部成功、0 失败**,另 64 个形态没有任何动作(素材本身不全)。

**全量渲染回归逮到的三个 bug(2026-07-26,导出器报告全绿也照样存在)**:

1. **32 个形态根本加载不了**:CUE4Parse 把空 morph target 写成没有 bufferView 的 accessor,
   Rust `gltf` crate 拒收(见 §1)。导出器关掉 `ExportMorphTargets` 即可——我们从不驱动它们。
   修完重导了受影响的 26 个包。
2. **贴图 alpha 被当成不透明度**:`if tex.a < 0.35 { discard; }` 把 160 张 `_By_D` 里的身体
   啃掉一部分,火花只剩眼睛、迪莫整只消失(见 §1)。
   **这条改了两轮。** 先整个去掉 alpha 测试:身体是回来了,但眼/嘴的**眼神图集**没人剔,
   菊花梨的眼睛糊成一块方斑、学院呱呱的圆眼镜黏成一团——**是拿实机截图逐只比才看出来的**,
   全量渲染回归不会报错(它只看「有没有画出东西」)。
   正解按槽区分:载入时把**本体贴图**的 alpha 刷成 255,shader 保留统一的 alpha 测试。
   判据要看**最终用的是哪张贴图**而不是槽名——火神的肌肉是 Fx 槽退用本体贴图,
   按槽名判会被整片剔光。全量 A/B 验过:开/不开 alpha 测试,764 个形态**没有一个**
   掉覆盖率超过 25%。
3. **取景按绑定姿势包围盒**,伸展类动作被裁(120 个抽样 × 四个动作,11 个被裁)。
   改成按动作包围盒(`Model::motion_bounds`)取景后剩 1 个。
   这条也绕了一圈:动作并集里混着**召唤落地**类动作(喵喵 `CallOut` 从 1.5m 高处掉下来),
   一并算进去会让画布白涨——33 个形态实测总面积 2.08 倍;丢掉「整只挪走」的姿势后降到 1.64 倍。
   中心偏移阈值要取一整个身高:取 0.4 会误伤**悬浮类宠物**(空空颅的 `Alert` 常态浮在
   45–56%,而它就在表情池里,于是运行时照样顶出画布)。
   另一个坑:采样时剥的位移必须**和运行时一模一样**(只剥 root 的 X/Z、保留 Y),
   否则量出来的盒子比实际渲的低,带纵向起伏的动作会顶出去。

**分批重导踩的一个坑**:包目录的重名后缀原来是按「这次要导的链」统计的,于是单独重导「迪莫」时
这批里没有同名链、目录就叫 `迪莫`,而全量导时它叫 `迪莫-3004`——增量重导会另起一个目录、
把原来的孤立掉(实测重导 14 条链后 3 个包名变了)。改成按**全部宠物**统计重名,与批次无关。

~~**待做**:直接读 `.rkpet`(zip);egui 的包管理 GUI~~ **两条都已完成**(见 Phase 7)。
剩下:贴图转 KTX2 与关键帧精简(体积);材质参数解析(见上面第 4 点)。

### Phase 5 — 多实体与跨宠物互动 ✅ 已完成

一次性补掉 §9.0 的 #5/#6/#7/#9，是剩下最大的一块。**先做它再做 Windows**：多实体会改动
`Stage` 与平台层的接口(单 actor → 实体集合)，接口定稿后 Windows 后端只写一遍。

按依赖排的子步骤：

1. ~~**`Stage` 单 actor → 实体集合**~~ **已做**。`Entity { actor, pos, coverage, drag_* }` +
   `EntityId`(稳定标识,**不是下标** —— 移除一只后下标会滑动,而托盘与掩码回读跨帧持有它)。
   命中测试 `pick()` 取**最上面那只**(z 序按脚底 y,相同则取后加入的);输入区取**各实体的并集**
   (不做合并:合成器接受重叠矩形);拖动状态挪进实体,于是拎起一只时其余照常待机;
   `tick_interval` 取各实体里**最快**的那一个。
   平台层暂时仍按「一只」渲染,`actor()`/`actor_pos()`/`replace_actor()`/`set_pet_mask()`
   先落到第一只上(标了过渡注释,第 2 步一并删)。
   原有 33 个测试全过,另加 4 个多实体测试(spawn/despawn 按标识、输入区并集、
   z 序取最上面、只有被点中的那只跟着走)。
2. **同物种多实体必须共享资产**。**模型侧已做**:`PetActor.model` 改成 `Arc<Model>`,
   App 上按 glb 路径(= 包 + 形态)缓存;多实体/多屏共享同一份网格与贴图,原来是每个 stage
   各加载一份(那行注释写着"Model 不便共享")。切形态时按 `Arc::strong_count == 1` 清掉
   没人用的,避免访问过的形态永久占着。测试钉住契约:两只同形态实体 `Arc::ptr_eq` 且
   引用计数随 despawn 下降。
   **GPU 侧也已做**:`PetGpu`(管线/顶点缓冲/贴图)按 `Model::source`(= 包 + 形态)缓存成
   `Arc`,与模型同一把键;`PetSurfaces` 变成**每实体一份**,只装画布/合成四边形/掩码回读。
   渲染改成按 `draw_order()`(脚底 y,从后往前)逐只 update + render + submit ——
   共享那份 camera/joints 缓冲因此不会串。掩码回读也逐只做,按 `EntityId` 装回去。
   **实测:1 只 RSS 234MB、3 只 237MB**(每多一只 ≈ 1.3MB,就是它自己的画布与回读缓冲),
   共享前应是按只数翻倍。(验证用的调试开关 `ROCOM_PETS_ENTITIES` 在第 7 步接上托盘后已删。)
   **过渡访问器已在第 7 步清掉**:`replace_actor()` 改成按标识换那一只,
   `actor()`/`actor_pos()`/`is_dragging()` 降级成 `#[cfg(test)]` 的便利函数。
3. ~~**掩码回读要错峰**~~ **已做**。`StageWindow` 上一个轮转游标,**一帧只回读一只**;
   从游标处起找**第一个到点的**(还在 140ms 节流里的跳过),否则轮到一个正被节流的,
   这一帧的名额就白费 —— 判据是新加的 `MaskReadback::is_due()`。
   **实测没有饿死任何一只**:输入区矩形数 1 只 ≈ 30、3 只 ≈ 93(正好三份的并集)。
   **开销是次线性的**:CPU 1 只 1.2% → 3 只 1.9% 单核,RSS 228MB → 230MB。
4. ~~**补 `Run` 与落地 `JumpFall`**~~ **已做**(需求 #7 的缺口)。
   - **`Run`**:速度按走速夹在 `[1.2×, 3.0×]`。反推值全库中位 417cm/s、p90 563、最高
     1125(魔力猫 7.5m/s),照搬会一瞬间横穿屏幕;钳制的代价是极端那几只脚会打滑,
     比「一眨眼跑没影」划算。触发阈值**必须按可走范围 `max_x` 取**,踩过两版:
     按宠物画布取(画布带 1.64 倍取景余量,水灵在 2560px 屏上画布 805px、三个身位 2415px)、
     按屏幕宽取(可走范围只有 `max_x`,站中间最远只能走一半)—— **这两版跑动作一次都不触发**。
     现在是 `distance > max_x × 0.4`,实测 30 秒内起跑 2 次。
   - **`JumpFall`**:原来**根本没导出来** —— 它在 ANIM_CONF 里有,却不在导出器的桌宠动作
     白名单里,全库 831 个形态一个都没有。补进白名单重导后 405 个包 / 550 个形态有。
     松手不再瞬移到地面线,改成 `Activity::Falling` 按重力下落(`FALL_GRAVITY` 2600px/s²、
     上限 1600px/s),落到地面线才回待机;没有这段动画的形态用待机姿势落。
   - 「受惊逃跑用跑」**在第 5 步补上了**(它要读指针位置与可走范围,正好是感知的活)。
5. ~~**感知与事件总线**~~ **已做**。`Perception{nearest, pointer, max_x}` +
   `Intent{from, kind, target}`,见 §6。要点:
   - **感知先整台算完再推进**。边算边动的话,后面那几只看到的是同伴已经走过的位置,
     同一帧里的距离判定就不对称了。
   - **距离用脚底点、单位是身位**。脚底:两只站在同一条地面线上时,脚底距离才是「看着有多近」。
     身位:同台上 161px 的喵喵与 481px 的魔力猫,「挨着站」在像素上差三倍;
     而且**不能用画布尺寸**(带 1.64 倍取景余量,第 4 步在跑动阈值上已经栽过一次)——
     为此给 `PetActor` 加了 `body_px`(= `height_cm × scale × px_per_cm`)。
     身位取**两只的均值**:大个子挨小个子时,谁算「近」不该只由其中一方说了算。
   - **发出与执行分开**。行为只往总线里塞意图,转身/播动作在 `dispatch_intents`。
     这条分界是为第 6 步留的:演出脚本要能在意图**落地之前**看到它
     (宠物被戳了,正在跑的时间轴得让位)。
   - 落地了两个意图,都不是摆设:**注意到邻居**(2 身位内互相转身打招呼,同一对 25 秒冷却 ——
     没冷却的话挨着站的两只会没完没了地致意)与**受惊逃跑**(补上第 4 步欠的那条:
     先把受惊动作播完,在 `React` 结束那一下才起跑,读起来才是「惊 → 逃」而不是
     「惊被跑打断」;往远离指针的一侧跑 3 个身位,没有 `Run` 退 `Walk`)。
   - 顺手修了个**第 7 步留下的坑**:`Entity::new` 一律摆在正中,托盘连加两只同物种的会
     **精确重叠**(做邻近感知时才撞见 —— 距离恒为 0)。`spawn` 改成按已在场只数左右轮流
     错开一个身位。
   - 实测两只喵喵:上台即互相注意到,26 秒后(过了冷却)又打了一次招呼。48 个测试
     (新增身位换算、最近邻、打招呼+冷却、独自一只不打招呼四条)。
     **受惊逃跑只有测试覆盖**:手上没有 Wayland 下的输入注入工具,没法脚本化点一下宠物。
6. ~~**演出脚本 + 第一个互动样例**~~ **已做**(珀尔鼬 3758 × 捕尘长绒 3604,见 `src/act.rs`)。
   按 §6 定的走**时间轴**而非两个状态机自发协商;第一版硬编码在 Rust 里,先把编排与打断语义
   跑通。要点:
   - **选角按形态 id,不是物种 id**。珀尔鼬 3758 是「点点」(物种 3757)这条链的二阶形态,
     捕尘长绒 3604 属于「毛头小蛛」3603 —— §6 原文写的 `species = 3758` 是笔误。
     为此给 `PetActor` 加了 `form_id`;参数一多(到 8 个,其中四个 `f32`)就换成了
     `PetBuild` 结构体,位置传错编译器拦不住。
   - **打断语义只此一处**:`PetActor.acting` 由演出置位,而**所有外部打断都从 `react()` 过**
     (受惊/摸头/被拎起),那里清掉它,演出下一帧看见就收场。不必在每个分支里各写一遍。
     被打断的**也记冷却** —— 不然人一松手它俩立刻又演一遍。
   - **时间轴可以打断自己**。`Alert` 有 4.2s,第 2.0 秒就被下一拍接管;一段动作没播完就换
     下一件事是允许的,不然拍子全被最长的那段动作绑死。
   - **缺动作只跳过那一拍**,整场照演 —— 全库动作覆盖不齐,不能让一段缺失卡死整场。
   - **`acting` 期间待机不触发 `choose_next`**:两拍之间的空档要站着等,不然演员会自己溜达走。
   - 诚实的限制仍在 §6:「清扫」没有独立 clip,这里是 `Run` 过去 + 两次 `Show` + 退半步凑往返,
     是近似不是复刻。**(2026-08-09 更正:是有片段的,见 §5;这场演出已改用 `Skill2Loop1`
     与 `Skill3Loop`)**
   - **顺带修了个第 5 步的坑**:stage 是先建再等 configure 的,那之前 `size` 是 (1, 1),
     spawn 的错开量会被 `clamp_to_surface` 整个吃掉 —— 实测两只 315px 的宠物双双落在 x = 0,
     开演日志写着「相隔 **0.0** 身位」。改成首次拿到真实尺寸时重摆一遍;顺手让「召回」也错开
     (三只叠在一起的召回等于把它们藏成一只)。
   - 实测:两只上台即开演,11 拍全部按时触发,「相隔 1.0 身位」;从托盘撤下演员当场收场
     (`《…》被打断,收场`)。57 个测试(新增选角/同形态不能自己跟自己演/戳一下收场/
     撤下演员收场/整场跑完并放开/缺动作只跳过那一拍/走位真的停在 1.3 身位)。
7. ~~**托盘与配置**~~ **已做**。托盘菜单长出「在场的每一只 ▸ 换形态 / 撤下」与「加一只 ▸ 整个
   包目录」,阵容存 `roster.toml`,重启恢复。要点:
   - **阵容存档与 config.toml 分开两份文件**。config.toml 是**手写**的、带注释,而阵容托盘
     每改一次就要机器重写一次 —— 序列化一遍注释就没了。谁写谁负责:config.toml 归用户,
     roster.toml 归程序。启动优先级 `--pack` > roster.toml > config 的 `pack`;
     给了 `--pack` 就**不读也不动**那份存档(调试时要的就是「只看这只」)。
   - **存档里的包读不动只警告,命令行/配置里点名的读不动是硬错误**。前者是程序自己写的
     (包可能被删被改名),不该拦住启动;后者是用户当场写的,不生效必须让他看见。
   - **托盘发的是插槽下标,不是 `EntityId`** —— 每台(每个 output)上的实体标识各自独立发号。
     `StageWindow::slots: Vec<EntityId>` 按插槽下标与 `App::roster` 严格对齐。因此加/切形态时
     **先把每台的角色都建出来再提交**:中途失败会让插槽错位,那比「加不上」难查得多。
   - **画布尺寸必须逐只取**。原来 `resize_surfaces` 拿第一只的尺寸套全台 —— 单一物种时看不出来,
     阵容混起来就露馅(实测同台上 喵喵 画布 280px、幽星光 458px、魔力猫 970px)。
   - **允许空台**:撤掉最后一只之后 `Stage` 里一只都没有,**这时仍要出一帧** ——
     不画的话合成器留着的还是上一帧,看着像是没撤掉。空 `draws` 正好清成透明。
   - **调试精灵改成「一只都没有时的占位」**:加了真宠物就撤掉,之后撤空也不再回来 ——
     用过托盘的人再看见测试图案只会以为是坏了。动画定时器也跟着改成**按需挂**
     (空着台起来时不挂),精灵模式的「空闲 CPU 0」是 S1 的验收项。
   - **「加一只」菜单按名字切段**。全库 525 个包平铺出来没法用,按中文首字分组又会分出上百个组;
     切成 22 段、段标签取首尾两个名字(「一窝蜂 … 伊里斯」),像翻通讯录。
   - **列包只读名字**(`Pack::list_entries`)。`Pack::list` 会把每个包的动作表与材质表全解析出来,
     而菜单只需要一行字。
   - 实测(单屏):1 只 RSS 231MB → 3 只 275MB;托盘的加/撤/切形态、重启恢复、
     以及「存档里写了个不存在的包」都跑通了。**验证走的是真托盘菜单** ——
     用 `com.canonical.dbusmenu` 的 `GetLayout`/`Event` 点菜单项,不是给代码开后门。

### Phase 6 — 音频 ✅ 已完成

叫声 + 变调。管线在 rocom-petvo 已经跑通(`Pet_Vo_*.bnk` + wem → vgmstream),这里是把它
接进导出器与运行时。要点:

- **导出器直接从 pak 里读 WwiseAudio**(`exporter/Audio.cs`)。那 3.3GB 就在包里,provider
  手上已经有 —— 不必像 rocom-petvo 那样先侧解一份出来。bnk 解析是那条链路的最小 C# 移植:
  FNV-1(**先转小写**)拿 Event id → Action → 容器 → Sound 的 `sourceID`,
  下行**必须靠 `directParentID` 反建父子树**(rocom-capture 那边踩过:按「扫描 4 字节命中
  已知 id」的启发式会一路爬到 ActorMixer 根,三个事件返回同样的 67 个 wem)。
- **一个形态一套四段**,对上 §7 当时的四个触发点:`Common_Happy`(摸头满意)、`Common_Shock`
  (受惊)、`Fight_CallOut`(托盘加一只时的召唤)、`Common_Relax`(睡醒),各带若干后备事件。
  源数据里后缀大小写不统一(`Common_SAd`/`Fight_Callout`),但**不用管** —— FNV-1 先转小写,
  各种写法命中同一个 id。同一事件挂 3 个随机变体,固定取**最小 sourceID**,导出可复现。
  (2026-08-09 扩到九种情绪 × 两层,见 §1「动作音效是另一层声音」。)
- **存 ogg vorbis 不是 opus**(§7 原文写的是 opus)。理由很实际:Rust 侧没有纯 Rust 的
  opus 解码器(symphonia 至今不支持),而 vorbis 在同样码率下对一秒的叫声没有可闻差别。
  单声道 `-q:a 0`,一段 6~17KB,一个形态四段约 50KB。
- **变调 = 变速**。游戏里每只宠物有个 −100~100 的 `voice` 属性,喂给 Wwise 的
  `Pet_Vo_Pitch`,由**逐宠手调**的三点 RTPC 曲线换成音分(最常见 ±300,也有 −300/+500);
  而 Wwise 的 pitch 本身就是重采样,所以运行时按 `2^(音分/1200)` 调播放速率就是等价实现。
  曲线两端写进 manifest。(**后来改成不自动掷**:不设就是原调,想要不一样在配置窗口里重掷
  —— 自动掷的话同一只每次启动都换个嗓子。)
- **`rodio::Decoder` 直接丢进 mixer 一声不响**,排了很久:解码器本身是好的(单独 collect
  出 102976 个样本、峰值 0.36),同一个 mixer 换成自带样本的源就正常。改成**加载时解码成
  PCM**、播放时套一个自己写的 `Source`(`current_span_len()` 返回 `None`),顺带也免了
  每次叫都重解一遍。教训:验证要**录声卡**,只看「调用返回成功」什么都证明不了。
- 默认音量 0.30(常驻程序必须小声,而且要正好落在托盘那几个档位上),托盘里一个「叫声」
  勾选临时静音,`volume = 0` 干脆不开音频设备。**不做 BGM**。
- **已知问题**:多显示器下每个 output 是各自独立的一只,tick 驱动的叫声(睡醒)两边会
  同时响。手上只有单屏,留着。

**2026-08-09 补:从四段扩到九种情绪 × 两层。** 解包数据里躺着的远不止四段 ——
`Pet_Vo_*` 与 `Pet_Action_*` 两族库对同一批情绪各有一套,而后者是**另一层声音**
(不是叫声的重复,包络相关只有 0.11~0.42),细节与量法见 §1 那两节。落地时的三处:

- **键从触发点名改成动作逻辑名**,与 `[forms.clips]` 同一把;`VoiceKind` 枚举与它的翻译层
  一起删掉。降级也因此统一走 `stage::fallbacks`。
- **待机表情与配置窗口那张动作表现在也出声**。原来只有受惊/摸头/睡醒/召唤会响,
  一只自己在桌上生气、难过、展示的宠物全程是哑的。
- **自发的声音一分钟至多一次**(`SELF_SPEAK_COOLDOWN` = 60s)。待机表情大约 20~40 秒
  一个,做一次响一次的话桌上那只每半分钟叫你一嗓子 —— 这和「默认音量 0.30」是同一条
  产品约束。人点出来的不受它管:连点就该连响。

### Phase 8 — Windows 后端 ✅ 可用(2026-08-01 实机确认)

`src/platform/windows.rs` + `src/control/windows.rs`。四轮实机来回之后可用:
上桌、置顶、逐像素 alpha、宠物之外点击穿透、命中与拖放、托盘(含加一只 / 撤下 / 切形态)、
阵容存盘恢复。**开发机是 Linux**,整条路是「交叉编译 + wine 冒烟 + 实机反馈」磨出来的。

先说**怎么在 Linux 上写这一块**(命令见 README「编译 rocom-pets.exe」):

1. `cargo check --target x86_64-pc-windows-msvc` —— 只要 std,连链接器都不用。全部 Win32
   调用的签名、常量所在模块、句柄类型都是编译器逐个纠出来的,不是照文档抄的。
2. `cargo xwin build` —— **能真的链出 exe**。这一步比类型检查强:它证明每个 Win32 导入
   符号都解析得了(`Shell_NotifyIconW`、`DCompositionCreateDevice` 这类要对上库与
   feature),而类型检查只看得到声明。
3. **`wine` 跑一遍**。DX12 在 wine 下起不来(DXVK 那层没给出 wgpu 认的适配器),但**在此
   之前的整条启动路径都真的执行了**:注册窗口类、建消息窗口、`EnumDisplayMonitors` +
   `GetMonitorInfoW`(拿到真实工作区 3840x2052)、`CreateWindowExW` 带
   `WS_EX_NOREDIRECTIONBITMAP`、`GetDpiForWindow`、`Shell_NotifyIconW` 挂托盘、
   打开音频设备。**这一步当场逮到一个真 bug**:配置与包目录只按 XDG 找
   (`HOME`/`XDG_CONFIG_HOME`),Windows 上一个都没有 —— 日志里直接是「定不出配置文件
   位置」,配置和阵容全丢。已改成 `%APPDATA%` / `%LOCALAPPDATA%`,重跑确认落在
   `C:\users\…\AppData\Roaming\rocom-pets\config.toml`。

这三步都不能代替实机验收,但它们把「写完全靠猜」压缩到了只剩**渲染与交互**没法验。

已经落地的:

- **DComp 那段 COM 不用手写**。上面原计划写「自己建 `IDCompositionDevice` 再把 visual
  交给 `SurfaceTargetUnsafe::CompositionVisual`」;wgpu 30 的 dx12 后端有
  `Dx12SwapchainKind::DxgiFromVisual`,给个 HWND 它就自己建一棵最小合成树。
  于是只要 `WS_EX_NOREDIRECTIONBITMAP` 的窗口 + `Backends::DX12` + 这个开关。
  (`CompositionVisual` 那条路仍然通,将来要自己管合成树时再用。)
- **穿透三件事一起做**:窗口区域**照旧按掩码设**(不清!),`WM_NCHITTEST` 恒返回
  `HTTRANSPARENT`,**并且**加 `WS_EX_TRANSPARENT`。前两件把屏幕上宠物之外的部分让出去,
  后一件负责宠物身上那几十个格子(顺带让系统连 hover 都跳过我们)。
  ~~原本是「开穿透时 `SetWindowRgn(None)` 恢复整窗渲染,穿透全交给 `WS_EX_TRANSPARENT`」~~
  —— **实机第二次栽在这**,见下面的反馈 5。
- **输入区在 Win32 叫「窗口区域」**(`SetWindowRgn`),和 `wl_surface::set_input_region`
  是同一件事,只是**穿透时两边分岔**:Wayland 交空输入区(不影响渲染),Win32 不能
  (区域同时裁渲染),于是 `Stage` 出两个方法 —— `input_regions()` 给 Wayland(穿透时空)、
  `shape_regions()` 给 Windows(不看穿透开关)。
  ~~原本以为 Win32 只能逐点回答 `WM_NCHITTEST`~~ —— **那是错的,而且是实机第一次跑就
  暴露的错**:`HTTRANSPARENT` 只在**同一线程**的窗口之间往下转发命中,穿不到别的进程去。
  于是那个铺满工作区的窗口把整屏的点击全吃了,除了任务栏(不在 `rcWork` 里)哪儿都点不动。
  改成按掩码矩形设窗口区域之后,区域外的像素压根不属于这个窗口,点击自然落到下面的程序上。
  代价是**窗口区域同时裁剪渲染**(Wayland 的输入区只管输入)。矩形跟着位置走是准的
  (`coverage` 是角色局部坐标,每帧按当前位置平移),但**姿势**是异步回读来的、滞后约
  140ms —— 跑起来时甩动的四肢可能越出上一帧的格子被裁掉一角,所以区域往外放了两格
  (`REGION_MARGIN` = 16 逻辑像素)当保险,代价是宠物周围多一圈十几像素也吃鼠标。
  **这个余量是拍的,没在实机上调过**:要是跑动时仍看到边缘被切,加大它;要是觉得
  宠物周围「点不到桌面」的圈太大,减小它。`WM_NCHITTEST` 仍然留着,但只负责在区域内
  按掩码逐点细化。
- **窗口铺满 `MONITORINFO::rcWork`**(已去掉任务栏),与 Wayland 那边 `exclusive_zone(0)`
  拿到的区域同义:宠物踩在任务栏上沿而不是藏到后面。`WM_DPICHANGED`/`WM_DISPLAYCHANGE`
  时重新贴合(重新量工作区 + `SetWindowPos` + 重配表面 + 逐只重建画布)。
- **窗口过程里的 App 要套 `RefCell`**。Win32 有一堆调用会**同步**把消息派回窗口过程 ——
  `CreateWindowExW` 发 `WM_CREATE`、`SetWindowPos` 发 `WM_WINDOWPOSCHANGED`、
  `TrackPopupMenu` 干脆自己跑一个模态消息循环。这时外层已经握着 `&mut App`,
  裸指针再借一次就是**别名 UB**。借不到就把消息交回系统。
- **动画定时器只挂一个,挂在那个隐藏的控制窗口上**。每个 stage 窗口各挂一个的话,
  两块屏一个间隔里会 tick 两次(而 `tick` 自己已经遍历了所有 stage),动画直接快一倍。
- 阵容为空时也有**调试精灵**(与 Wayland 同义),没有宠物包也能验这一层。

**还没做的**(都不属于「平台层」,但用起来会缺):

- ~~托盘只有四项~~ **已补齐**(2026-08-01,实机反馈之后):加一只 / 撤下 / 切形态
  与 Linux 托盘同构。趁这一步把两个后端共用的那一半抽进了 `platform/shared.rs`:
  资产缓存(模型/管线/叫声,三张表同一把键、同一套「没人用就清掉」)、
  manifest → 角色的换算、阵容与托盘状态。**划界依据是「碰不碰窗口系统」** ——
  shared 里一句 Wayland/Win32 都没有。
  抽的时机是**故意压后**的:实机验通之前先抽公共层是本末倒置,那时连它能不能跑都不知道;
  验通之后再抽,才知道哪些是真共用、哪些是某个平台的特例(比如输入区两边差得很远)。
- **没有全局热键**。Win32 只有 `RegisterHotKey`,那是**抢**一个组合键而不是像
  XDG GlobalShortcuts 那样向桌面申请,冲突了别人就用不了。
  `rocom-pets --toggle-passthrough` 仍然可用(按窗口类名找到实例 `PostMessage` 过去),
  要热键就挂在快捷方式上。
- 显示器**插拔**(多一块/少一块)没处理,只处理了已有显示器的分辨率/缩放变化。
- ~~仍然是控制台程序~~ **已关掉黑窗口**:release 版按 GUI 子系统链接,而启动时
  `AttachConsole(ATTACH_PARENT_PROCESS)` 挂回父控制台 —— 双击干净、从命令行跑仍有日志。
  **必须自己把 `CONOUT$` 设成标准句柄**(AttachConsole 不替进程改这几个),
  而且要在任何输出之前调:Rust 的 stdout 会缓存第一次拿到的句柄。

**实机反馈(2026-08-01)**,两轮:

1. 启动后除任务栏外整屏点击失效 —— 根因是上面那条「`HTTRANSPARENT` 穿不到别的进程」,
   已改用 `SetWindowRgn`。
2. 改完之后:**双击起来什么都不显示**,点托盘「召回宠物」才冒出来;之后拖动、落到
   任务栏上沿都正常。根因是**首帧从来没画**:之后的帧全由 `WM_TIMER` 驱动,而 tick 只在
   `Reaction::redraw` 时才出帧,**调试精灵永远不产生 redraw**(`tick_entity` 对非宠物
   直接返回 `Reaction::NONE`)—— 于是窗口一直是空的,而「召回」里正好有一次 render。
   Wayland 那边碰不到,因为首次 configure 的处理里就渲了一帧。已在建完 stage 时补上首帧;
   顺带修了 `recall()` 只 render 不更新窗口区域(宠物走到新位置会被停在原处的旧区域裁掉)。

3. 桌面点击正常、真宠物(火花)显示正常;**托盘菜单只有四项** —— 这条不是 bug,是上面
   那个「先不抽公共层」的决定留下的缺口。实机既然验通了,前提就满足了,于是抽出
   `platform/shared.rs` 并把加/撤/切形态补齐。
4. 补齐之后回报**「可用」**。

**实机反馈(2026-08-04)**:

5. **开了全局穿透之后,除任务栏外整屏点击又失效了** —— 和第 1 条一模一样的症状,
   也是同一个根因换了件衣服:穿透那条路径把窗口区域清成了整窗
   (`SetWindowRgn(hwnd, None)`,当时的理由是「免得画面被裁」),于是又变回一个铺满
   工作区的实心窗口,而 `WS_EX_TRANSPARENT` **并没有**像指望的那样把点击放过去
   (那个样式的穿透行为是写在「分层窗口」那篇文档里的,我们这个 DComp 窗口不是分层的)。
   **修法是别再让穿透碰形状**:区域始终按掩码设(`Stage::shape_regions()`,不看穿透
   开关),`WS_EX_TRANSPARENT` 只负责宠物身上那几十个格子。这样最坏情况也只是宠物身上
   点不穿,屏幕其余部分照常 —— 而不是整屏点不动。顺带给样式那次 `SetWindowPos` 补了
   `SWP_FRAMECHANGED`(用 `SetWindowLongPtr` 改完样式本来就该让系统重算一遍)。
   **教训**:Wayland 那边「穿透 = 交空输入区」照搬不到 Win32,因为窗口区域**同时裁剪
   渲染**;凡是想清区域的地方都要先问一句「清完还剩多少像素属于这个窗口」。

**实机反馈(2026-08-08)**:

6. **左键或右键点开托盘菜单,菜单开着的整段时间宠物定住不动**,菜单一关就接着走 ——
   正是下面验收清单里挂着的那条「`TrackPopupMenu` 那个模态循环期间宠物会不会僵住」,
   答案是会。但**根因不是 Win32 那条模态循环本身**:那条循环照样在 `GetMessage`/
   `DispatchMessage`,`WM_TIMER` 一条不落地派进来了。卡住的是我们自己这一侧 ——
   `control_proc` 收托盘回调时先 `with_app`(拿 `RefCell` 的可变借用),**在借用里头**
   调的 `TrackPopupMenu`,而那个调用要等菜单关掉才返回。于是菜单开着的每一次 `WM_TIMER`
   都走到 `with_app` 的 `try_borrow_mut` 上失败、被静静丢掉(它的约定就是「借不到就当没这回事」,
   本来是防重入的),一拍都没推进。
   **修法**:把弹菜单要的那点状态(勾选项 + 三组档位的当前值)抄进一个 `TrayMenu`,
   `with_app` 里只做这一次抄写,**出了借用再 `popup()`**;选中项是 `TPM_RETURNCMD`
   返回之后才送进通道的,所以菜单关掉后再 `drain_control()` 收一次。
   **教训**:`with_app` 那条「借不到就跳过」既挡重入、也会把一整段时间的 tick 悄悄吞掉 ——
   凡是**会阻塞到用户操作完**的调用(模态菜单、模态对话框、同步等待),
   都得在借用之外做。目前这类调用只有 `TrackPopupMenu` 一处;开配置窗口那条是
   `Command::spawn`,不阻塞。

**实机验过的**:窗口建得出来、置顶、逐像素 alpha、真宠物渲染、命中与拖动、
落地停在任务栏上沿(`rcWork` 那条是对的)、宠物之外的点击穿透、托盘(含加一只 / 撤下 /
切形态)。**Windows 后端到此可用。**

**待你复验**:全局穿透开着时,宠物身上的点击能不能落到下面的程序上
(靠 `WS_EX_TRANSPARENT`,没别的后手了)。屏幕其余部分现在走的是已经验过的窗口区域那条路。

**仍然没验的**(都不拦日常使用):跑动时边缘会不会被窗口区域裁掉(`REGION_MARGIN`
那 16px 是拍的)、多显示器、叠放次序、空闲/活动占用、显示器插拔。

**已经能在 Linux 上确认的**:链接通过(每个 Win32 导入符号都解析得了)、
启动路径跑到 GPU 初始化为止(窗口/显示器/DPI/托盘/音频都真的调过)、
配置与包目录落在 Windows 该在的位置、exe 不依赖 VC++ 运行库(`+crt-static`)。

**验收清单(全部待验,需要你的 Windows 机器)**:置顶、逐像素 alpha 无黑边、
命中/穿透、多显示器、叠放次序、空闲/活动占用。另外几条这个后端特有的:
`WS_EX_NOREDIRECTIONBITMAP` 窗口在没出第一帧前会不会闪、~~`TrackPopupMenu` 那个模态循环
期间宠物会不会僵住~~(**验了,会;根因与修法见上面第 6 条 —— 待复验**)、
`WM_TIMER` 在鼠标忙的时候会不会被饿死(它是低优先级合成消息)。

### Phase 7 — 打磨与分发(进行中)

**切排序不再卡住(2026-08-09)** —— 换一次排序,主线程被一块 133~179ms 的 long task 独占
(headless Chromium + 生产构建,201 张卡片实测);改完 49~74ms,零 long task。

- **排序本身从来不是瓶颈**:201 个包排一次 0.3ms。真正的开销是 201 张卡片全量重渲染,
  每张里三个 Radix Tooltip,各带 context 与 effect —— 一次换序六百个 tooltip 重建。
  `memo` 掉 `PackCard`,换序就只动顺序,卡片一个都不重跑;代价是回调必须在上游 `useCallback` 稳住。
- **搜索与排序拆成两个 memo**。合在一起时换排序也要重扫一遍 607 个形态名,更要命的是
  `searchPacks` 每次都产出新的 hit 对象,下游 `memo` 一张也拦不住。
- 换序放进 `useTransition`(下拉立刻收起,重排可打断),输入框走 `useDeferredValue`。
- `Intl.Collator` 复用一个实例:`localeCompare` 每调一次都现建一个 collator,
  按名称排 0.306ms → 0.149ms。省的绝对值不大,但是白捡的。
- 卡片加 `content-visibility: auto` + `contain-intrinsic-size`,视口外的不参与布局与绘制。
- 顺带:**无图鉴号的一律沉到末尾**,不管按哪一列排。它们的占位是「000」,按图鉴号排时
  正好顶在第一页最前面,而那 22 个是游戏里查不到号的边角料。

**下载站的浏览器预览(2026-08-09)** —— 卡片上点「预览」,在网页里直接看这只宠物:
换形态、挑眼神、点按钮做动作、拖着转视角、右键平移、滚轮缩放,默认站着待机。

- **同一份渲染代码**,不是另做一套。为此把 crate 拆成 lib + bin(`src/lib.rs`):
  跟平台无关的 11000 行(`pet`/`pack`/`stage`/`persona`/`assets`/`act`/`sprite`/`audio`)
  两个目标都编,平台外壳(窗口/托盘/配置窗口/离屏)按 `cfg(not(target_arch = "wasm32"))` 排除。
  动作清单、降级表(`stage::fallbacks`)、眼神图集都直接复用 —— 网页上能点的动作
  就是装上之后点得动的那些。
- **原生依赖以前是无条件的**,`ksni`/`zbus`(Linux 托盘)连 Windows 版都编,而 wasm 上
  `errno` 直接 `compile_error!`。挪进 target 段之后三个目标都干净。
- **没有文件系统**:资产由 JS 逐个喂进 `assets::memory`,键是那条「虚拟路径」——
  当初为了读 `.rkpet` 留的那道缝,正好让 `Pack::load`/`Model::load` 一行不改就能跑。
- **点开才加载**:wasm 是动态 `import`(1.4MB,brotli 后 380KB),`.rkpet` 按 HTTP Range
  只取当前形态那一份(中位 2.9MB,整包是 6.8MB)。前端那个最小 zip 读取器见 web/README。
- **相机能拖**:桌宠只绕 Y 转、画布恒为正方,预览要俯仰也要宽高比,
  加了 `pet::orbit_view`(原来那个 `orthographic_view` 变成它 pitch=0/aspect=1 的调用)。
- **缩放与平移(2026-08-09 补)**,绑定照常见的模型查看器(three.js `OrbitControls`、
  `<model-viewer>`、Sketchfab)来:左键拖转视角,**右键 / 中键 / Shift+左键拖平移**,滚轮缩放;
  触屏单指转、双指同时管缩放(间距)与平移(中点)。缩放 0.5×~5×,「复位」把角度、缩放、
  平移一起还原。
  - 投影是正交的,所以**缩放不动相机**,只把取景余量按比例收紧(`orbit_view` 收到
    `PADDING / zoom`);**平移改的是轨道中心**,而且存**世界坐标**——存屏幕偏移的话,
    平移完再转视角,被推到一边的宠物会跟着镜头甩。
  - 「一个画面高」在正交下正好是 `2 * radius`,于是把指针位移折算成**画布高度的比例**后,
    平移精确跟手,拉近了也不会突然变快。这条有测试钉着(`gpu.rs` 的
    `panning_one_screen_height_moves_the_subject_exactly_one_screen`)。
  - 平移夹在 1.5 个取景半径内;换形态时归零(偏移是按上一只的半径算的,新的一只可能
    小得多,不清的话切过去第一眼人就在画面外)。缩放不清 —— 那是「想看多近」,跟哪只无关。
  - **两处单位的坑**:① `drag` 原先拿 `config.width/height`(设备像素)去除指针位移
    (CSS 像素),2 倍屏上转速正好只有一半;② 横向除宽、纵向除高,724×352 的画布上竖直
    方向快一倍,斜着拖不跟手。现在两轴都按**画布 CSS 高度**折算,折算放在 `preview.ts`
    ——只有那一层同时知道两种尺寸。
  - 滚轮得是原生监听 + `passive: false`:React 的 `onWheel` 挂在根容器上且被动,
    在里面 `preventDefault` 只会换来一句警告,表现是缩放的同时弹窗跟着滚。右键要拦
    `contextmenu`,中键要拦 **`mousedown`**(拦 `pointerdown` 挡不住 Windows 的自动滚动)。
- **只支持 WebGPU**:骨骼矩阵是只读 storage buffer,WebGL2 没有。检测不到就不加载,
  弹窗里给一句说明。想补 WebGL2 的话关节数中位 56、最大 149,塞 uniform 数组够用。
- 两处坑记在 §1:着色器那句非均匀控制流里的 `textureSample`(Dawn 直接拒),
  以及网页画布只能不透明。另有一处在前端:**Radix 的 Portal 在 layout effect 里才挂**,
  用 `useRef` 拿画布的话 effect 第一次跑就是 `null`、依赖没变也不会再跑 ——
  表现是弹窗开着、画布停在 300×150、既没进度也没报错。改用回调 ref 存进 state。
- **预览不计下载数**:单开 `/api/preview/:id`,不 302 到 R2 自定义域(`fetch` 跨源要 CORS)。

**Reload 分成两条(2026-08-08)**,起因是「切个形态要卡半天」:

- `Control::Reload` —— 重读配置与阵容,**不碰包目录**。配置窗口改形态/大小/性格都走它。
- `Control::ReloadPacks` —— 外加重扫包目录。**只有三种时候该发**:`--reload`(人手动
  要求的)、托盘「重新载入」、配置窗口里导入或删除了包。

原来两者是一条命令,于是切个形态也要把整个包目录的 manifest 读一遍 ——
201 个包实测热缓存 40ms、**冷缓存 400ms**,而形态跟包目录八竿子打不着。
拆开之后切形态从 400ms 降到 **50~70ms**,剩下的全是真活(新形态的模型、GPU 上传、
叫声解码),`--reload` 仍然照扫不误。

两条路径都收敛到同一份缓存(`App::available`),阵容解析与「加一只」那张表共用它 ——
在这之前 `Pack::resolve` 是**每只宠物各扫一遍整个包目录**,六只在场就是读七遍(242ms)。

**资产缓存留最近两份**(`Assets::prune` 的 `KEEP_RECENT`)。原来是照
`Arc::strong_count == 1` 一律清,而 `respawn_all` 是**先把台上全 despawn 再重建** ——
于是刚换下去的那个形态必然被清掉,**切回去就是全新加载**。切形态本来就是来回切的,
留两份把 A↔B 变成缓存命中:实测 44~68ms → **1.3ms**(release)。

**排查这类问题必须问清是哪个档**。debug 档(`cargo run`)同一次切形态要 0.9~1.5 秒,
而 release 只要 44~68ms —— 差 20~30 倍,重活全在依赖里(gltf + 贴图解码、symphonia 解 ogg)。
`[profile.dev.package."*"] opt-level = 2` 把依赖按优化档编、自己的代码仍是 debug,
0.9~1.5 秒降到 0.3~0.5 秒;配上缓存,切回去 1.9ms。
**曾经在这上面翻过车**:第一次怀疑缓存被清时拿 `volume = 0` 的配置做 A/B,
叫声那条路压根没走,量出来「没差别」就把改动撤了 —— 而它其实是对的。
测性能前先确认待测的那条路真的被走到。

`reload` 里那行「已重载:N 只在台上,用时 X」是常驻的;`build_actor` 超过 150ms 会补一行
分段(读模型 / 解叫声),平时不吭声。再有「卡一下」的反馈,先看这两行。

**兜底报错窗口与画布钳制(2026-08-08)**,起因是一条实机崩溃:阵容里写上
`000-荆棘笼` 的二阶(`Dem_JingJiLong2_001`)就崩,**而且重启还是同一条崩** ——
坏状态存在 roster.toml 里,人就此进不去了。

- **根因**:那个形态的 manifest 写着 `height_cm = 3162`(31 米,模型包围盒本身就离谱),
  算出来的画布边长 10881px 超过 GPU 的 8192,`create_texture` 直接把进程 panic 掉。
- **两道钳子都要**:`Assets::build_actor` 钳逻辑尺寸(把 px_per_cm 按比例缩,画布、脚底、
  走跑速度一致地跟着缩),`shared::canvas_size` 钳物理尺寸 —— 缩放是平台层才乘上去的,
  逻辑边长 8192 在 1.5 倍缩放的屏幕上就是 12288,**光钳逻辑那道拦不住**(实测踩过)。
- **弹窗必须在 panic 钩子里弹,不能靠 `catch_unwind`**:release 档是 `panic = "abort"`,
  根本不展开,`catch_unwind` 一次都不会命中。钩子是 abort 之前一定会跑的那一步。
  窗口给「复制报错信息」与「重置配置」(把 config.toml 与 roster.toml 一起挪进
  `backup/<时间戳>/`,**挪而不是删**)。只在本来就要开窗口的两条路上弹,
  `--list`/`--reload` 那些照旧只往 stderr 写一行。
- **仍然没解决的**:那一只钳完还是 7143px 高,屏幕上就是一堵墙 —— 不崩了,但没法看。
  真要用得给「按屏幕高度封顶」再加一道,那是产品决定,没顺手做。

**已完成(2026-08-01)**:`.rkpet` 直读、配置窗口、托盘重排、每只宠物的选项。

- **`.rkpet` 直读**:见 §4.4。喵喵链 14MB 目录 → 6.9MB 归档,读出来的渲图与目录包逐像素一致。
- **配置窗口**(`--settings`,src/settings/):三页 —— 宠物包(列表/查找/导入/删除)、
  活跃宠物(加/撤 + 形态/大小/性格/叫声 + 动作表)、常用配置(帧率/整体大小/音量/穿透)。
- **托盘重排**:两个后端同构的「常用配置」「宠物配置」子菜单,ksni 与 Win32 各写一遍。
- **每只宠物的选项**:`roster.toml` 的 `[[pet]]` 多了 `scale`/`persona`/`emotes`,
  默认值一律不落盘。性格(src/persona.rs)是**五个倍率**乘在 stage.rs 那几个手感常量上,
  「乖巧」逐项等于 1.0 —— 于是加这个功能没有改变任何已有用户的宠物脾气。

三条定下来的做法:

1. **配置窗口是独立进程**。两个后端各跑着手写的事件循环(calloop / Win32 消息循环),
   而 egui 要 winit 的;塞进同一个线程是两套循环抢方向盘。开第二个进程还顺带保证
   配置窗口崩了不带走桌宠。
2. **两个进程之间只靠磁盘上那两份文件** + 一条 `Reload` 命令。没有第二套 IPC 协议要维护,
   桌宠没在跑时改动也照样存下来。`Reload` 是**整个阵容推倒重建**,不做差量 ——
   形态/大小/性格/表情每一项都会换掉角色,算「哪几只没变」比重建还长,
   而重建时模型与 GPU 资源本来就命中缓存。
3. **`config.toml` 用 `toml_edit` 写回**。那份是手写的、带一整篇说明,
   `toml::to_string` 重新序列化一遍会把注释全抹掉。`roster.toml` 不需要(它本来就归程序)。

### 网页预览也能挑异色与炫彩(2026-08-23)

同一份 `Mutation`、同一张配置表、同一套写法(`异色+炫彩:3/33`),前端只负责拼字符串。
两个轴在 `web.rs::build_model` 里分头落地,和桌面版 `Assets::model` 逐字相同。

**异色一分钱不多花**:那套材质与它的贴图本来就在 `forms/<资产>/` 底下,预览早就整个下了。
所以只多了一个 `FormInfo.shiny` 字段(`Form::has_shiny`)和一个开关。

**炫彩那 13 张共享贴图不烘进 wasm。** 那是点开预览才下的一个 chunk(1.5MB),
再塞 3.6MB 进去等于让每个点开的人先付一遍,而其中最大的一张(铅字幻梦的流动噪声)
自己就有 2MB、多数人一次也用不上。改成**问 wasm 要名单、按需 fetch**:
`Mutation::shared_assets` 说这一身要哪几张,`glassy_missing` 滤掉手上已有的,前端取回来喂
`put_glassy`。挑一次常规炫彩只多下两张(实测 184KB + 49KB)。于是 `glassy::embedded`
(构建期烘的那张表)之上多了一层 `glassy::shared` / `has_shared` —— **桌面与浏览器唯一的
分岔就在这一个函数里**,两条加载路径都只认它。

素材和包一样是「谁部署谁提供」:传到桶的 `glassy/` 下(见 web/README.md 第 3b 步),
Worker 那条回落路由 `/api/glassy/:name` **必须自己把名字关死**(`[A-Za-z0-9_]+`,
不许有点也不许有斜杠)—— 这几张不在 `catalog.json` 里,key 是按名字直接拼的,
放开一个 `.` 就等于把整个桶交给客户端遍历。没传就是 404,前端把炫彩那几档禁掉并说一句,
和桌面版「这个二进制没烘炫彩素材」是同一句话。

**顺带发现网页预览已经坏了一阵子。** 重编 wasm 之后浏览器整份 shader 拒编:

```
error: 'textureSample' must only be called from uniform control flow
note: control flow depends on possibly non-uniform value: if !gated && !season
```

`glassy_layer` 门外那条 `return shaded` 是条快路,可它让底下所有 `textureSample` 落进
「依赖非一致值的控制流」—— WGSL 规定带隐式导数的采样只能在一致控制流里调。
**桌面的 naga 放行、浏览器的 Tint 不放行**,所以只有网页会炸,而且是**整份 shader**、
连一只普通宠物都画不出来。删掉那条快路是逐字等价的(第 ⑧ 步本来就写着
`select(shaded, …, gated)`,金属那步的 `season_metal_zone` 在非赛季材质上恒为 0),
三张渲图(异色炫彩 / 赛季 / 原样)与改前**逐像素相同**。

教训:**wasm 那份要跟着 shader 一起重编才看得见这类错**。`web/src/wasm/` 是生成物、
不入仓库,改完 `pet/shader/*.wgsl` 只跑桌面测试是发现不了的。

**踩到的坑(都是「看起来对、跑起来错」那一类)**:

- **除了宠物包那页,整个 `CentralPanel` 里一个滚动区都没有**。窗口能拉到 480 高,
  而「活跃宠物」那页光表单就有十来行(加上炫彩的配色 / 粒子两行,按默认的 620 高就已经
  装不下)—— 底下的「动作 / 位置」两行**被裁掉:看不见、点不着,也没有滚动条提示**。
  改成**每一页自己管滚动**(`theme::scroll_page`),不在外面统一套一层:宠物包那页是
  虚拟化的(两百多个包只画看得见的十几行),外面再套一层会把它的可用高度变成无穷,
  那套算法当场失效。
  **`theme::scrollbar()` 那套 Breeze 取色只能给滚动条**:它改的 `bg_fill` /
  `extreme_bg_color` 同时是复选框、滑杆轨道、数值框的填充色,而宠物页里这三样都有 ——
  所以进了内容那一层要先把 `Visuals` 还回去(滚动条由外面那层的样式画,还回去不影响它)。
- **配置窗口第一版满屏豆腐块**。egui 自带字体只有拉丁字母,而我为了躲开「ab_glyph 读不了
  字体集合」的顾虑把 `.ttc` **整个跳过了** —— 可 Linux 上的 Noto CJK、Windows 上的雅黑与宋体
  **全都是 `.ttc`**,于是一份都没找到。实际上 epaint 0.35 走 `skrifa::FontRef::from_index`,
  字体集合是支持的,`FontData` 就有 `index` 字段。
- **字面下标不能省**。`NotoSansCJK-Regular.ttc` 里装着日/韩/简/繁四套字面,取错只会
  静悄悄显示成日文字形。问 fontconfig 要 `%{file}:%{index}` 就有正确的下标。
- **查询里不能带 `sans-serif`**。`fc-match "sans-serif:lang=zh-cn"` 的第一名是
  `NotoSans-Regular.ttf`(纯拉丁的那份 Noto,「sans 的最佳匹配」),一个汉字都没有。
  只按 `:lang=zh-cn` 问才对。
- **长路径会把整行撑出窗口**。包目录那一行后面的按钮在实机上根本看不见。
  路径一律 `Label::truncate()`,按钮排在路径**前面**。

**验收怎么做的**:窗口本身用 `spectacle -a` 抓真窗口、逐页与设计稿对照
(eframe 的 `__screenshot` 抓的是第 2 帧,Wayland 下那时窗口还没映射,存出来是白的);
托盘走**真的** `com.canonical.dbusmenu` `GetLayout`/`Event` —— 菜单树逐项核对、
`enabled: false` 确认分组标题不可点、点「大 · 1.50×」确认宠物真的重建了、
`config.toml` 真的改了**且注释还在**、勾「静音叫声」确认语义没反;
嗓音与落脚点的回写读 roster.toml 核对;`.rkpet` 走 `--render` 与真实上台各验一遍。

**第二轮(照设计稿重做,2026-08-01)**:托盘与配置窗口按 claude.ai/design 上那份
「Rocom Pets 系统菜单设计」重排了一遍,顺带补齐设计里隐含的几项运行时能力。

- **托盘只放菜单表达得了的东西**。DBusMenu 与 Win32 菜单能给的就是文字、勾选、单选、
  子菜单、分隔线、禁用项(当分组标题)—— **没有滑块**。所以整体大小与音量降级成几个
  档位 + 一条「自定义…」通向窗口;不在任何一档上时**一个都不勾**(`nearest_step`
  返回 `None`),硬勾最近的会让人以为菜单里就是那个值。
- **顶层不再逐只展开宠物**。加/撤、切形态、改性格都要先列阵容再逐级展开,菜单一深就
  没法用;这几条全部搬进窗口,`Control` 因此少了五个变体、两个后端各少一百多行。
- **配置窗口改成即时生效**。桌宠是看得见的,盯着屏幕就知道对不对,「先改再按保存」是
  多余的一步。顶上那条只说「已修改 N 项」并给撤销(基线 = 打开窗口时那一份)。
  例外只有滑杆:拖动时每帧发 `Reload` = 每帧重建宠物,所以**松手才落盘**。
- **设计稿里隐含的运行时能力**,一并补上:
  - 每只可以单独「不参与叫声」;
  - **嗓音改成持久的**。原来每次启动重掷,于是同一只每天听着都不一样;现在第一次上台
    掷完就写回 roster.toml,窗口里能看见那个值、也能「重掷」。
  - 「记住上次落脚点」:存**可走范围的百分比**而不是像素(换分辨率/换显示器后像素值
    毫无意义)。踩到一个坑:`replace_actor` 一律居中,于是改一次整体大小就把每只记下的
    位置抹成正中,而那个正中还会被当成新落脚点存回去 —— 记了等于没记。
  - **动作覆盖率**:设计稿画的是 manifest 的 `[report]`,但全库没有一个包写了那一节。
    改成按 `RUNTIME_CLIPS` 现算「这只在桌面上有哪些事做不了」,更有用而且真有数据。
    降级路径要算作「有」(只有 `SleepStand` 的那批照样睡得着)。
  - ~~第二个全局热键(召回宠物),两个动作一次 `BindShortcuts` 申请完。~~
    **整套热键在第四轮删掉了**,召回改由 `rocom-pets --recall` 承担。

**又一个坑**(字体那两条在上面「已完成」那段里,不重复):

- **egui 里「限宽」不等于「折行」**。横向布局里的 label 不折行,`allocate_ui` 给它一个
  有限宽度也没用 —— `Label::wrap()` 才是那个开关。同理表格:`ScrollArea` 加了
  `auto_shrink(false)` 就会吃掉所有剩余高度,得显式 `max_height`,否则底下那行统计与
  三个按钮被挤出窗口。

**第三轮(菜单再收一次,2026-08-02)**:照使用反馈把托盘又砍了一遍。

- **顶上那条「N 只在场」去掉了**。菜单是用来做事的,一条点不动的统计只是在占位置;
  数量仍在图标的悬停提示里(和 Windows 那边一样)。
- **三个开关按「多久用一次」排**:点击穿透 / 静音叫声 / 召回宠物 —— 前两个是随手切的,
  召回是宠物跑丢了才找的。
- **「宠物配置…」与「打开配置窗口…」并成一条「完整配置」**。两条都开同一个窗口、
  只差落在哪一页,并排放着就是让人多读一行再选一次;留一条,**落在常用配置页** ——
  从托盘点进来的人本来就在找那几项,只是想要更精确的那一版。
- **常用配置多了一组「帧率设置」(20/30/60)**,大小那组的标签从「小 · 0.75×」这种
  改成直白的 50% / 100% / 150%(档位值也跟着从 0.75 挪到 0.5)。
  帧率是**整数档**,所以回显要求正好相等(`exact_step`),而不是像倍率那样取最近的一档
  —— 手写进配置的 45 就该一个都不勾。
- 帧率这一项**顺带要求 `Setting::Int`**:走原来的 `Num` 会写成 `fps = 30.0`,
  而字段是 `u32`,下一次读配置直接报格式错 —— 一次写回就把配置文件弄成读不了的。
- **大小与音量的滑杆右边多了个能打字的框**(`DragValue`:点一下就编辑、回车提交、
  超范围自动夹回上下限)。界面上一律按「百分之几」走并取整 —— **显示的数字就是存下的
  数字**,不会出现「框里写 124%、其实是 123.7%」。倍率写法(`1.50×`)也一并换成
  百分比:要在脑子里换算一次才知道是「大了五成」,而托盘那三档本来就写着百分比。
- **顶上那条改成常驻**。原来没改动就整条不画,于是拖滑杆拖出第一处改动的那一瞬间,
  底下整页往下跳一截 —— 而正在拖的那根滑杆就在这页上,手还按着,它自己从指针底下溜走。
  现在两种状态共用同一条,行高按 `CONTROL_H` 兜住(只有一种状态里有按钮,而按钮比字高)。
- **托盘的「退出」把配置窗口也带走**。两个进程原本只有「配置窗口 → 桌宠」这一个方向
  (`Reload`);反方向复用了配置窗口**占单实例的那个凭据**:Linux 上它已经占着
  `org.rocom.Pets.Settings` 这个名字,那就在同一条连接上挂个 `Quit` 方法;Windows 上
  它已经拿着一个具名互斥量,那就再加一个具名事件、开条线程等着。都不用新起一套 IPC。
  窗口不能在别的线程上关,所以收到喊话只是置位 + `request_repaint`,
  下一帧由 `ui()` 发 `ViewportCommand::Close`。**没有 `request_repaint` 的话
  要等到下一次鼠标动**,那可能是很久以后。
- **自适应降频取消了**,`fps` 从「上限」变成「目标」(详见 Phase 2 那一段)。

**第四轮(再收一次,2026-08-02)**:

- 三组档位从「常用配置」子菜单里**提到顶层**,各自一个子菜单。套一层的时候调个音量
  要点两次才看得见选项,而这三样正是最常调的;顶层那条「常用配置」自己什么也不做。
  「完整配置」改叫「首选项」,与「重新载入」「退出」并成分割线后的一组。
- 配置窗口里**常用配置排到宠物包上面**(托盘那几项都落在这一页,进来得最多),
  侧栏底下那行配置目录路径去掉。
- 一批**看着不对**的地方:两张弹窗的内边距(默认那圈 6px 让标题几乎贴着边框)、
  搜索框里的字没上下居中(`add_sized` 把框撑到 28 高,而 TextEdit 默认贴顶边)、
  表格左右留白、导入拆成两个按钮(原生对话框没有「文件和目录都行」这个模式)、
  统计行最右那个按钮的描边被裁掉一条边。
- **「导入目录…」认两种选法**(src/settings/packs.rs `packs_under`):选中的自己就是个
  解开的包目录(里面有 `manifest.toml`)就导它自己 —— 这是本来就有的那种;否则当**收纳夹**
  看待,把里面的 `.rkpet` 与解开的包目录全导进来(导出器一次导一批、网盘拖下来一堆,
  摆的都是后一种)。**往下最多两层**(`SCAN_DEPTH = 2`):既覆盖 `收纳夹/某批次/喵喵.rkpet`
  这种摆法,也是**误选家目录时的刹车** —— 没有这个上限,选中 `~` 就会把整棵盘扫一遍。
  认出是包就不再往它里面递归(包里面本来还有一层层 `forms/…`,继续找纯属浪费,
  万一里面真躺着个 `.rkpet` 还会导出个莫名其妙的东西)。一个都没找到时单独报一句 ——
  交给 `import` 的话报的是「不是宠物包(目录里没有 manifest.toml)」,那是第一种选法的说辞。
- **空状态只说话、不放按钮**:导入那两个按钮在搜索框右边一直画着(空不空都画),
  空状态里再摆一组就是同一个动作的第二个入口,还得让人分辨两组有没有区别;
  那条 `dotnet run --project exporter …` 的命令示例也一并去掉。
- **滚动条**:egui 默认拿 `fg_stroke.color` 画手柄,深色主题下那是近白色,一条白杠
  比表格内容还显眼;而且默认是浮动的,直接盖住最右边那列。改成 `solid()` 那一套
  (占位、取 `bg_fill`),颜色与宽度照本机 KDE Breeze Dark 量:rgb(42,84,107)、7px、
  不画槽。**只在滚动区那一层改**:`bg_fill` 同时是滑杆轨道的颜色,全局改会把滑杆染蓝。
  连带把滚动条设成常显 —— 表头画在滚动区外面,忽有忽无的话四列就对不齐了。
- **全局热键整套删掉**(portal 申请、两个动作、按键录制界面、config 里那两项)。
  理由是它要桌面实现 GlobalShortcuts、要用户点一次授权弹窗,而
  「把系统自定义快捷键绑到 `rocom-pets --toggle-passthrough`」把同一件事做完了,
  还不用抢组合键。配置里那两个键跟着删掉:`deny_unknown_fields` 见了不认识的键是
  直接报错的,所以**带着老 `hotkey` 行的 config.toml 会读不了** —— 现在还在本地测试期,
  删掉重新生成即可,报错信息里也写了这句。
  一并删掉的是 `Player::measure_motion`:每只宠物每帧一次 Vec 分配 + 全关节距离扫描,
  存在的唯一理由就是喂给降频判断。它的三个单测里有一个测的其实是**根骨骼水平位移
  必须被剥掉**(Phase 1 的修正),那条改成直接看矩阵、留下了。

**第五轮(性格照搬游戏数据,2026-08-02)**:

- **⚠ 这一轮有一条结论是错的,2026-09-04 推翻**:下面写着「游戏那边换脸是行为逻辑
  直接设材质参数,没有第二张『动作 → 表情』的表」——**有,而且就在动画里**
  (`EC_Eye`/`EC_Mouth` 曲线,见 §4.3.1)。于是那版 `face_for_clip` 是按动作名的意思猜的,
  八条里错两条(Fear 猜「哭哭」实为第 7 格、CallOut 猜「大笑」实为「微笑」)、漏一条
  (Alert 猜「不改脸」实为「生气」),而且眼嘴同一格、没有眨眼、全库一张表。
  下面这段保留原样,是为了留着当时的推理链。
- **表情是眼睛,不是动作。** 第一版把 `emotion_desc` 当成「待机时播哪段动作」,
  对着三方攻略那张「幽星光不同性格的眼睛」才发现错了:眼睛与嘴各是一张 **2×4 的
  眼神图集**(`M_P_Eyes` 那族材质),网格 UV 落在左上那一格,换表情 = 整格偏 UV。
  实现:材质那份 uniform 记「这是不是脸」(`parents` 里有 `P_Eyes`),
  相机那份记每只的 UV 偏移(**每只**不同,而材质是按形态共享的)。
  八格逐个渲出来比对,五种脸与攻略图一一对上。
- **踩了一个 uniform 对齐的坑**:WGSL 里 `vec2<f32>` 按 8 字节对齐,`time: f32` 之后
  是 84,着色器会把 `face_uv` 放到 88;Rust 那边不补 4 字节填充就整体错开一个字段 ——
  表现是「u 偏移完全没反应,v 偏移却在横向换格子」。查了半天才反应过来。
- **性格 → 表情的规则是从解包数据里查出来的**,不是编的:
  `NATURE_CONF` 每条性格带一个 `emotion_desc`(31 条里只有 6 条不是「默认」:
  天真/开朗→微笑、懒散/悠闲→困倦、胆小→哭哭、急躁→生气);
  `LLM_PET_BEHAVIOR_CONF` 的 84 条行为各自标着 `nature_id`,反过来读就是
  「这个性格爱做哪些动作」(调皮→happy/jump/run_to_player,冷静→relax/nap/deep_sleep,
  悠闲→fear/sad/run_away…)。原来那五个性格(乖巧/活泼/慵懒/黏人/高冷)是自己编的,
  换成游戏里的,名字与 `nature_id` 照抄,便于回表核对。31 条**只留七条**:
  按「脸 + 动静」两条轴挑到不重复 —— 五种脸各一个代表,默认脸那几条再留下差得最远的
  三个(平和 = 基线、调皮 = 最闲不住、冷静 = 最能睡)。剩下那 24 条要么脸重复、
  要么倍率折出来和已有的一模一样,摆进下拉框只是让人多滚两下。
  **五个行为倍率仍然是编的** —— 那两张表里没有「多久睡一次」这种量,只能按每条性格
  爱做的行为往旋钮上折,折算依据逐条写在 persona.rs 里。
- **表情池那组勾选去掉了**:表情跟着性格走。有默认表情的性格,一半时候做的就是它
  (「默认表情」在游戏里指的是平时那张脸,不是「只会做这一个」)。
  单独那行只读的「表情」也去掉了 —— 改不了的一行摆在一堆能改的中间,看着像是能点。
  结果直接写进性格自己的名字里:下拉框里换脸的那几条写成**性格 +「那双眼睛」**
  (`胆小「哭哭眼」`),挑性格的时候多半正是冲着那张脸去的,不该选完再回头看提示
  才知道选中了什么。默认脸的**不写**「默认眼」:那是「没有变化」的一档,
  三条各挂一个不说明任何事情的后缀,只会把真正带脸的那四条埋掉。
  下拉框得**显式撑开高度**(`ComboBox::height`)—— 默认上限 200px,七行要 232px,
  差这一点点就滚起来,而且正好挡住最后一条。
- **嗓音默认 0(原调)**,不再第一次上台随机掷 —— 那让同一个包的两只无端不同,
  而且「重掷」按钮本来就在那儿。想要不一样的嗓子自己按一下。那个数**后来改成能打字的
  数值框**(`DragValue`,范围 `VOICE_RANGE`):原来只能靠「重掷」碰运气,想要
  「就低一点点」得按到手酸。**只有框、没有滑杆** —— −37 与 −40 听感上差不多,
  滑杆给的那种「大概多大」的直觉在这儿没有对应物,反而占掉一整行。
  框子**定宽**,数字从 `+0` 变成 `−100` 时右边的「重掷」不跟着横跳;
  上下限交给 `DragValue::range` 自己夹,不另写提示 —— 打个 999 进去当场就看见了。
  落盘沿用「默认值一律不写」:0 存成不写这一行。
  拖动过程中不落盘,与大小那根滑杆同一条判断(「变了但没提交」= 正拖着)。
- **动作覆盖率那一格换成动作表**:16 个动作一格一个按钮,这个形态没有的置灰,
  **点一下当场在桌面上播一次**。配置窗口是另一个进程,所以走 `Control::Play(slot, clip)`:
  Linux 是 D-Bus 方法(带两个参数,`send_command` 那条只传方法名的路走不了),
  Windows 把两个参数塞进 `lparam` 的高低 16 位(wparam 已经装着命令编号)。
  运行时那边走的是**和受惊/摸头同一条路**(`React`),播完自己回待机。

**第六轮(表情跟着动作走,2026-08-02)**:

- **动作也换眼睛,不只是性格。** 游戏里一只「哭哭眼」的幽星光生气时是生气眼、
  睡着时是困倦眼 —— 性格给的那张脸只是它**平时**的样子。`face_uv()` 改成
  「**正在播的那段动作**说了算,它没意见才用性格那张」,按当前 clip 现算、不另存状态:
  换脸和换动作本来就是同一件事,存两份就会有对不上的时候。
- 这张「动作 → 表情」的对照表**是按语义挑的**,不像性格那张是查表查出来的。
  找过了:`NATURE_CONF.emotion_desc` 之外没有第二张表,`EMOTION_CONF` 里那 36 条是
  头顶飘的特效(happy/sad/问号/wait…)不是眼睛,`LLM_PET_BEHAVIOR_CONF` 每条只带
  `nature_id` 不带表情。游戏那边换脸是行为逻辑直接设材质参数,没落在配置表里。
  好在动作名本身把意思写清楚了:Anger→生气、Sad/Fear→哭哭、Shock→惊讶、
  Happy/Relax/Show→微笑、CallOut→大笑、Sleep 四段→困倦,
  Idle/Walk/Run/JumpFall/Alert 不改脸(否则性格那张脸没机会露面)。
  八格的名字优先用 `emotion_desc` 里出现过的,剩下两格(惊讶/大笑)只给意思实在对得上的。
- 顺带修掉一个**说一套做一套**的 bug:配置窗口的动作表说「睡着」能点(按 `has_clip` 算,
  幽星光只有 `SleepStand`,降级算有),点下去运行时却报「这只没有这段」
  (`play_clip` 拿 `model.clip` 直接找)。降级表原来在 `PetActor::new`、`has_clip`、
  `play_clip` 里各写了一份 —— 合成 `fallbacks()` 一处,`find_clip()` 统一查。
- 离屏渲染(`--render`)也按动作取表情了:它本来一律传默认脸,现在那张对比图
  顺带就是「换动作换眼睛」的验收图。

**第七轮(受惊时头被拉出画布,2026-08-02)**:

- 现象:点一下喵喵,受惊那一下**头瞬间蹿出画布**,能看见渲染区被切断的直边。
- 查出来是**动画数据里的坏帧**,不是渲染错了。喵喵 `Shock` 里脊椎的 X 缩放有一帧冲到
  **4.90**(前后两帧 1.26 与 2.91),脖子同段冲到 2.99。判定依据是:同一条通道**其余
  每一帧都满足 Y≡Z**(沿骨轴拉伸、径向等比挤压,卡通挤压就该长这样),偏偏这几帧不等 ——
  压缩动画解出来的坏值。导出器那边是 `track.GetBoneTransform` 直接取的局部缩放,
  中间没有分解矩阵这一步,所以不是我们算坏的。
- 取景盒本来就装不下这种:`animated_bounds` 按 [`MAX_POSE_GROWTH`] = 2.5 倍筛姿势,
  这一帧超出后被丢掉 —— 于是画布不覆盖它,而运行时照样会播。
- 修法是**在载入时夹掉过大的缩放关键帧**(`MAX_BONE_SCALE` = 2.0),不是放宽取景 ——
  放宽的代价是每只宠物都为一帧怪姿势缩小一圈。阈值是量出来的:本地全部包
  (24 个形态、约 63 万个缩放分量)**99.35%** 落在 [0.5, 2.0] 内,越界的集中在
  Shock / CallOut / Relax 的孤立帧;Idle 1.09、Alert 1.49、Sad 1.61、Anger 1.95
  这些正常挤压全在阈值内。
- **只夹上限**:往小了缩是有意义的(动画师拿它藏部件),夹下限会让藏起来的东西冒出来。
- 验收:本地 8 条链 **23 个形态 × 17 段动作 × 10 个时刻**离屏渲一遍,查每张图
  **四条边上有没有非透明像素**。修之前只有**喵喵一阶的 Shock** 真的顶出画布,修完没了。
  其余触边的都是**另一回事**:CallOut(喵喵/小夜/水蓝蓝/治愈兔/点点 —— 整只跳出画布)、
  幽星光二阶的 Relax(整体滑到一边),那是位移不是缩放,属于「整体平移该由程序挪画布」
  那条已知取舍(见上面 `MAX_POSE_CENTER_DRIFT`),夹缩放对它们没有影响。
- 坏帧**不止喵喵一处,全库过半的形态都有**。把 539 条链 835 个形态扫了一遍:
  1318 个(形态 × 动作)的缩放越过 2.0,被夹掉的分量占全部缩放关键帧的 **0.199%**
  (37815 / 1904 万)。
- 光看倍数会看错。螺旋帕帕的 `Shock` 里 `Bip001-L-Toe0` 冲到 **98 万倍**,渲出来
  却和没夹一模一样 —— 那根骨头没有蒙皮权重,缩多少都没有顶点跟着动。所以判「有多明显」
  要按**这根骨头连同子孙带着多少蒙皮权重**加权:`(倍数 − 1) × 网格占比`。
  按这个排,还剩 1095 个(形态 × 动作),涉及 405 个形态(48%)/ 292 个包(54%)。
- 用户报的喵喵 `Shock`(脊椎 4.9 倍、带着 74% 的网格)排第 **4**。比它更狠的两个:
  **斑斑** 的 `Alert`(脖子 13.8 倍、34% 网格,整个头被拉成一根尖刺戳出画布)、
  **电咩咩** 的 `CallOut`(脖子 13.1 倍、36% 网格,拉成一条对角线横穿整张画布)。
  两个都在夹完之后恢复正常 —— 也就是说这个修法救回来的远不止喵喵那一下。
- **这也是夹上限而不是逐帧修补的理由**:一条统一的规矩,不用为 405 个形态挑帧。
- **坏值是从哪儿来的?** 写了个探针直接问 CUE4Parse(scratch,没进仓库):喵喵 `Common_Shock`
  是 `FACLCompressedAnimData`(ACL 压缩,走 CUE4Parse-Natives 的原生解码),
  `AdditiveAnimType = AAT_None`(不是叠加动画,不经 `AccumulateWithAdditiveScale` 那条路),
  而 `KeyScale[5]` 解出来**就是** `(4.90, 0.43, 0.98)`。导出器对缩放只做了一次 Y/Z 轴交换,
  没有任何算术 —— 所以**不是导出器算坏的**,值在 ACL 解压那一步就是这样。
- 至于「ACL 解错了」还是「游戏本来就这么画」,能给出的是一条很干净的相关性:
  按骨骼带的蒙皮权重分桶,**带着半个身子以上的骨头没有一根超过 5 倍**,
  而超过 10 倍的 31 处里有 29 处长在「带不动网格(<1%)」或「小部件(1–10%)」上,
  最狠的 98 万倍那根压根没有蒙皮权重。这正是**有损压缩按蒙皮误差优化**的指纹:
  顶点多的骨头压得准,没顶点的骨头误差不设限。也就是说 >10 倍那批基本可以断定是解压噪声。
- 剩下 2–5 倍、长在脊椎/脖子上的那批(喵喵 4.9 就在其中)**更像动画师画的拖影帧**
  ——一帧极端拉伸、下一帧收回,是卡通动画的常规手法,游戏里多半也这么播。
  夹掉它是**一次取舍不是纯粹的修复**:放着不管就得把取景盒撑到装下 4.9 倍,
  代价是每只宠物常年缩小一半;夹掉只损失一帧拖影的幅度。
  真要确认游戏里到底长什么样,只能进游戏看那一下,或者换一个独立的 ACL 解码器对一遍。

**待做**:开机自启(KDE autostart 已有 `.desktop` / Windows 启动项)、
N 只宠物的性能与内存实测、Windows 安装包 / Linux AppImage、贴图 KTX2。
(`[profile.release]` 的 LTO 已把 Linux 产物从 31.7MB 压到 18.0MB。)

**配置窗口还没在 Windows 实机上验过**:交叉编译链接通过(rfd 的 COM 对话框、eframe/glow 的
WGL 导入符号都解析得了),wine 里跑到 GL 上下文为止(配置路径、单实例互斥量都真的走过),
但 wine 那台没有中文字体、GL 也起不来。要验的是:雅黑找不找得到、文件对话框弹不弹得出来。

**文件拖放整个删掉了**(2026-08-03,用户报「拖 `.rkpet` 进配置窗口没反应」)。
病因不在我们:winit 0.30.13 的 **Wayland 后端根本没实现文件拖放** ——
`platform_impl/linux/wayland/` 里一处 `DroppedFile`/`HoveredFile` 都没有,
而 `linux/x11/` 与 `windows/` 后端都有完整实现。于是 `dropped_files` / `hovered_files`
在 KDE Wayland 上恒为空,`drop_overlay()` 永远不画,导入那条路也永远不会被触发。
**代码是对的,只是收不到事件。**

三条路都看过了,选了最后一条:

- **升 winit** —— 不行:winit 0.31 还是 beta,而 egui 0.35 钉在 winit 0.30.x。
- **配置窗口走 XWayland**(`EventLoopBuilderExtX11::with_x11()`,eframe 的
  `event_loop_builder` 钩子几行就能挂上)—— 拖放立刻可用,但整个窗口在分数缩放
  (开发机 1.5×)下可能发虚,而且和「不做 X11 回退」那条拧着。
- **整个删掉**(采用):`drop_overlay()`、`dropped_files` 那段、空状态里那句提示,
  全部清掉。**不按平台分版** —— 一个功能在 Windows 上能用、在 Linux 上默默没反应,
  比两边都没有更难解释,文档和界面也得跟着分叉。导入统一走「导入包…」「导入目录…」
  那两个按钮,它们两个平台行为一致。

要是哪天 winit 补上了 Wayland 拖放,再加回来是件小事:导入路径(`SettingsApp::import`)
一直都在,当初拖放也只是往它前面接了一段取路径的代码。

### 横向待办(不属于某个阶段，随时可插)

按性价比排：

| 事项 | 为什么 | 代价 |
| --- | --- | --- |
| ~~**水体层:合成方式已读到,但第三次落地仍失败**~~ ~~遮罩追到了~~ **族认错了,已改正并落地(2026-08-29)**:PS 49966 是波波拉 `_By` 的 **`MI_P_Object_UVFlow_WPO_NoMetal`** 排列,不是 `Water_NoMetal` —— 那一层是根图 `M_P_Object` 上的**加性流动层**(`FlowTexture × FlowColor × FlowInt`,按顶点色 B 与基色 alpha 双重加权),火系 PS 41058 里逐指令相同。**顶点色 B 那条读对了**,只是它乘的是流动色不是 caustics。已实现:波波拉 调色板 **0.141 → 0.126**、对比 **1.33 → 1.18**,27 只中位一位没动 | `Water_NoMetal` 那条(水灵 `_Fx1` 才是)仍未实现,但它已不是波波拉的成因 | 见「~~水体那个遮罩追到了~~ 认错族了」与「那两层落地了」两节 |
| **实机私有数据:shader 归属从「猜」变成「查表」** | 安卓那条路一直卡在归属 —— APK 里有完整 shader library 却**没有宠物资产**,只能靠结构指纹猜。宠物资产在手机的应用私有目录(`adb root` 才拿得到,22 GiB),取到之后同平台 20 字节哈希精确命中,**外加**实机质量档给出四元组 `(Quality, FeatureLevel, LODUsed, DynamicSwitchId)` 唯一选出实机真跑的那份 resource,以及「求值 preshader 直接出 UB 数值、不需要参数名」这条绕过 `docs/shader.md` 那堵墙的路。见 [android-device.md](android-device.md) | 数据已导(2026-07-30)。**第一次产出是个负结果**:`docs/android-glsl.md` §4 那条 `#3119` 属于共享父材质 `MI_P_Object_Masked`,宠物全库零命中,§4 的槽位下标整体作废(不动运行时基线,没有代码依赖它) |
| ~~材质实例参数解析~~ **已完成** | 见 §1:材质能正常读,贴图改按 `TextureParameterValues` 接。全量 2043 个材质槽里修正了 **258 个**(246 个原来猜不到→退用本体色、12 个猜错)。幽星光一阶从「一坨黑」变成正确的粉色;水蓝蓝一族不再是噪声 | 已做 |
| ~~特效层通道(火焰/水壳/光晕)~~ **已完成(近似)** | 主色 × 遮罩 × 卷动噪声,加色/半透用预乘 alpha 统一;参数全部取自游戏材质。37 个形态带特效层。**是近似不是复刻**:没有折射、没有 MatCap 反射、菲涅尔只用 N·V 粗略代替 | 已做 |
| ~~玻璃/薄纱层与卷动色带~~ **已完成(近似)** | 半透族叠 MatCap 高光 + 边缘光混色,环带按 `FlowTexture` 卷动出渐变,星点按网格 UV0 贴在表面上(**「按屏幕位置贴」是当初的错判,已推翻**,见 §1)。暮星辰的裙子回到饱和蓝、环带有青↔粉渐变、幽星光那两颗球是红玻璃且不再闪 | 已做 |
| ~~球旋转时**仍有区域白闪**~~ **已修**:根因是上游把切线写进了 NORMAL(见 §1 法线那条),不是任何一层特效 —— 我原先排的两条嫌疑(屏幕空间星点遮罩、MatCap 高光)**都不是**:星点在球上只贡献 +0.5 亮度,matcap 只是放大器 | 摆幅 51.7 → 2.1。顺带把 matcap 按汇编改成「单通道 × MatCapColor」并让几层光用 `max` 合 | 已做 |
| 玻璃内部层继续对齐实机 | 机制已经照汇编实现了(折射 + 沿折射线三向投影采 `StarTex` + 时间卷动,见 §1),但**观感还没对上**:实机是一颗又大又干净、居中的四角星,我们这边是一团偏软的亮斑。差在 march 深度/平铺的归一化(实机用包围盒配 `GlobalDepth` = 100,我按最长边缩放后手挑了 0.7),以及固有色还没换成那个按高度的两色渐变 | 中:知道该调什么,但每一版都得对着截图看 |
| ~~**解 `FUniformExpressionSet` 的 FMemoryImage 布局** —— 名字这一关的总闸~~ **已打通(2026-07-28)** | 结构、cb 布局、opcode 编码、块 ↔ shader 配对全部查实(rocom-capture 的 `scripts/uniexpr.py`);**名字也通了**:`paramId` 是 uexp 里 shader map **自带**那张名字表的下标(不是包名字表 —— 实测 `paramId` 稠密取到 167 而包名字表只有 139 条)。锚点 13/13、根默认值复核 805/805。成品输出 `scripts/matparams.py`,一条命令出「名字 = 值」 | 已做。剩下的是**逐个把猜值换成读出来的**,不再是研究问题 |
| ~~补上曝光 / 显示编码这一环~~ **固有色链路已做** | 基色平方进线性 → 线性里做明暗 → 末尾 `sqrt(色 × 曝光)`,照汇编尾部那条链。于是两段明暗能用汇编原值(0.5/1.5 + 一个 `AMBIENT`),全库过曝 9 → 4。详见 §1.1 | 已做 |
| ~~把 `glow` 那几层也搬进线性~~ **已做** | 四个系数按 `旧² / EXPOSURE` 换算,几层光先在线性里相加再统一编码。于是 `MatCapColor` 的 HDR 值能原样用,`GLASS_MATCAP_GAIN` 撤成 1.0(这一层手挑参数清零)。全库过曝 4 → 2。详见 §1.1 | 已做 |
| **玻璃球:实机几乎没有明暗渐变** | 实机那颗球是一片平色 + 左上一小块白高光,我们有很强的斜向分界。汇编确认玻璃族**确实**吃两段明暗,所以差别在「环境项相对直接光更强」或取景 | 小:比对多个 yaw 可定 |
| ~~**特效通道(`fs_effect`)搬进线性**~~ **整包试过了,不做** | 三种编码 × 四档 `EFFECT_RIM_FLOOR` 全测过。按受影响的两只看(波波拉 + 水灵才有特效层,中位对它们不敏感):现状 0.337 + 0.097 = **0.434**;线性 + 下限 0.35 = 0.460、0.6 = 0.535、0.8 = 0.640、1.0 时水灵的颜色跑到贴近背景、直接触发抠图丢块检测。**每一档都更差** ⇒ 这层的显示空间标定是**自洽**的。真要做还得动 `glow`(来自 `Glow Intensity`,而它的根默认其实是 **0** —— 导出器兜底成 1,说明这一项对特效层根本不是 `Glow Intensity`)与 alpha 的耦合方式 | 已试,负结果入库 |
| ~~**特效通道(`fs_effect`)还整个留在显示空间 —— 但单改编码会更差**~~ | 主通道早就搬进「线性 + `sqrt(色 × 曝光)` 编码」了,`fs_effect` 至今直接返回裸 `tint`。量化上很像是这儿的问题:实机波波拉内部是 (11, **209**, **251**),而 `sqrt(MainColor)` = (108, **210**, **242**) —— G/B 几乎逐位对上。**但两个候选都试了、都更差**:`sqrt(tint)` 让水灵 0.097 → **0.168**、`sqrt(tint × 曝光)` → **0.145**(波波拉 0.337 → 0.345 / 0.316)。原因是特效层那几个系数(`glow`、`rim = mix(0.35, 1.0, facing)`、`strength`)当年都是**在显示空间对着截图标的** —— 和主通道当初一样,搬进线性是**打包活**:编码 + 重标一起做,单改一行只会打破自洽 | 中:要做就整包做,别单改编码 |
| **波波拉与火神的色差:两只都开着 `OpenCustomDepth`,依赖一条我们没有的通道** | 全库只有 **11 个材质**开了这个静态开关,正好是**水系与火系两族**(`Fir_JiZai3` / `Fir_XiaoHuoMiao1,2,3,Bo` / `Wat_DiMo2` / `Wat_ShuiLanLan1,2,3,3_011,Bo`),而 15 只对照里最大的两个色差项 —— 波波拉 0.337、火神 0.090 —— **都在这张表里**,都是那个 `_Fx`/`_Fx1` 材质。另外那片外壳是 `MSM_Unlit` + 半透 0.5,而运行时 `fs_effect` 把 matcap **只用来算 alpha**、颜色恒等于 `tint`;`MSM_Unlit` 材质里 `MatCapColor × matcap` 是**加进自发光**的(会提亮),这与「实机比壳与基色 0.5 混合更亮」对得上 | 中:**先弄清自定义深度那条通道在做什么**,比逐参数追更值得。改 matcap 那条要先拿到 composition(4 个块里都没有 `MainColor`) |
| ~~**水体层:参数已经通到宠物包,但合成方式还没读对**~~ **推翻:那三层实机一层都不画** | 汇编第 114~117 行 `r4 × (1 − r2.y)`,而 `r2.y` 在两个分支下都是 1(`OpenBlackMagicByIDMask` 全库零覆盖)⇒ `Color1`/caustics/菲涅尔三层全部归零。这是同一个坑第三次(球内星层 `FragmentsColor.w`=0、玻璃菲涅尔 `FresnelIntensity`=0)。**读出公式后必须再查链上每个乘法因子是不是 0** | 已解决:不要实现 | 导出器写出 `water_color1`(a = 增益 `Emitter Intensity`)/ `water_color2` / `water_main`(a = 混合系数)/ `water_caustics` / `water_shape`,caustics 贴图走 `noise_tex` 槽;值与属性 JSON 逐字对上。**运行时那层实现过一次、失败了并已撤回**:整层替换 → 调色板 0.337 崩到 0.631,加在着色结果上 → 0.618(水灵 0.097 → 0.335 / 0.403),渲图一片平色。根因是我只读了 shader 35663 的 50~150 行,`r4` 不是最终颜色 —— 基色与两段明暗在后面重新进来 | 中:**先把 35663 的 150~676 行读完**,参数侧不用再动 |
| ~~**水体预设:那三层实机一层都不画**~~ **推翻了(2026-08-30)** | 那条结论读的是 shader 35663,而实机默认那份是 PS 16335(`Num/lod=0/dsid=0`,`AC743E86…`),**里面没有那道 `1 − r2.y` 门**。同一个「挑错排列」的坑第四次。整条链已读全,见上面那节 | 已推翻 |
| **波波拉那 0.337 不是自发光,是水体预设没实现** | 做过决定性检验:**关掉自发光,波波拉一动不动仍是 0.337**(而火神从 0.090 恶化到 0.178)—— 所以「这两只离群 = 自发光」的因果**只对火神成立**。波波拉的水体材质(父 `MI_P_Object_Water_NoMetal`)覆盖的是 `Color1`/`Color2`/`Main Color`/`CausticsInt`/`FlowDistort`/`FresnelInt`/`FresnelPower`,是一整套 caustics + 菲涅尔的水体层,我们完全没画 | 中:**卡在「那个材质没有自己的内联 shader map」**,见 docs/shader.md。中途我按错配的块推出「实机偏青来自 `RedChannel`」并改了导出器 —— **那是错的,已整体撤回** |
| ~~**球内星光的颜色多半用错了**~~ **推翻了,不用改** | 「四对颜色槽全是标量广播」来自一次**错误的块配对**。正确配对(34529 → `Fx1` 块 10,`54+15+1 = 70` 精确相等)下 `cb5[36]` 就是 `StarColor`。**更重要的是:这一层整体被 `FragmentsColor.w` = 0 乘掉了** —— 实机不画它,实机球里那颗金星是附加特效精灵。见 §1.1 | 已解决 |
| 还卡着的具体条目 | **两段明暗、星点层那 4 个渐变色槽都已解决**。剩下的:球内星点的颜色(现在代的是根默认 `StarColor`)、球体固有色色相偏一档、水体那一族的色相 —— 这些现在**都是「去把槽位定位到名字」的执行活**,不再卡在方法上 | 小~中,逐条做 |
| 玻璃球缺一块**大面积白色高光** | 实机每颗球有一大块明确的白高光(见截图),我们的 matcap 只按单通道 × `MatCapColor` × 0.35 叠了一层很淡的 | 小:matcap 那条链路已按汇编改对(单通道 + `max` 合),缺的是它在实机里还乘着一个遮罩通道选出的高光区 —— 那张遮罩的语义还没查 |
| ~~那 11 个「真半透」材质做成真半透~~ **已做**:不透明度就是**基色贴图的 alpha**,判据就是 `Opacity or OpacityMask` 这个开关本身(见 §1)。两处独立测量对上(贴图值 0.55 / 截图水印衰减 0.50) | 顺带解决了暮星辰的「双层裙」观感 | 已做 |
| ~~**异色(玻璃)皮肤**~~ **已完成(2026-08-23)** | 当年这条把两件事混成了一件。**异色**是换整套材质(蓝图的 `DiffMaterials`),**炫彩**才是配置表 + `GlassySwitch` 那条 shader 分支;两条都做了,见「异色与炫彩:两件不同的事」那节。当年记下的两条死路仍然成立:① lua 里 `petbaseCfg.shining_model_conf` 在本版本的 `PETBASE_CONF` 里根本没有这个字段(122 个字段,查过),是跨版本残留;② `pet_glass_feature`(每只都有)的 id 落在 `SKILL_CONF` 里,是玻璃形态附带的**技能** id,不是配色 | 已做 |
| **取景包围盒的「坏姿势」守卫基准换成「按段中位数」** | 原来判据是「姿势中心偏离**绑定盒**中心超过绑定盒高度」,改对了两次:① 拿绑定盒当基准,对**浮游**宠物是灾难(叮叮卯绑定盒只有 13.8 cm 高、动作把它悬到 0.78 m,**每一帧都被丢**,盒子退回绑定盒 ⇒ 整只渲不出来);② 换成**全局**中位数仍不够 —— 某一整段动作整体偏在别处时会被整段丢掉,而运行时照样会播它。**守卫绝不能丢掉运行时真会显示的姿势。** 最终按**段内**中位数:整段一致的偏移不算异常,只剔段内离群帧。全库 `空白` **6 → 1** | 已做。剩下 1 个是 `Dem_JingJiLong2_001`,**资产本身坏**:绑定盒 y ∈ [−28.4, 3.2](31.6 米高,离群顶点),取景框住 31 米自然什么都看不见。要修得给绑定盒加分位数裁剪,那会动全库取景,没做 |
| 取景盒变大的代价 | 守卫放宽后,某些宠物的盒子从「只框住常见姿势」变成「框住全部姿势」(小雪人 2.45 倍:有段动作把它抬到 y = 2.23),宠物在画面里变小、边缘像素占比升高 —— 全库 `过曝` 2 → 3(小雪人 0.25,正好卡在 >0.25 的阈值上) | 这是「宁可小一点也不裁肢体」的既定取舍,不是回归 |
| ~~64 个形态渲不出任何一格~~ **已修** | 根因不是导出漏了:那些资产在**解包里根本没有 `Animation/` 目录**(实测 `Wat_ShuiLanLanBo_001` 只有 SKM + ABP + 材质,而同族的 `Wat_ShuiLanLan3_001` 有 62 段),游戏里多半是静态物件。离屏渲染改成**零动画时渲绑定姿势**(绑定姿势下 `世界变换 × 逆绑定 = I`,直接传单位阵即可)。渲出来是正经形态(带蝴蝶结的水生物、绿色植物系),不是废资产。全库 `失败` **64 → 0** | 已做。**运行时仍不能把它们当桌宠**(不会动),要不要在包里标出来是另一件事 |
| 附加特效组件(粒子 / socket 挂件) | 幽星光与暮星辰那两颗球里**单独转动的小星星**不在骨骼网格里(见 §1),同类的还有各种拖尾/光环。**新证据**:在实机截图里定位到那两个金黄形状(一颗球里是圆点、另一颗是四角星),**平涂、硬边、带白色描边**,而我们的渲图里完全没有 —— 既不在网格(有的话描边通道会画出来)也不在基色图集(球的 UV 处是平色圆盘)。要走蓝图 → 组件树 → 粒子系统/附加网格这条导出链 | 大:等于开第二条资产管线 |
| ~~直接读 `.rkpet`(zip)~~ **已完成** | 分发前必须补,也是体积优化的前提。做法见 §4.4:虚拟路径 + `assets::read` 一处判断,`Form` 里那二十多个 `PathBuf` 一个没动。喵喵链 14MB → 6.9MB,渲图与目录包一致 | 已做 |
| ~~**实机截图是图鉴 UI 的,未必和世界 base pass 同一条 shader**~~ **查了一半,够用了**:星贴层**只存在于带 `MobileDirectionalLight` 的排列**里(见 §1.1),而果冻那张截图属于「没有落地投影」的卡片场景 ⇒ 那张图里本来就不该有星点。**不再按那张截图调星点** | 没分开的是「这一帧没平行光」与「这是另一套静态排列」——但两种读法都指向同一个结论。**留下的真问题**:21 张截图里 11 张来自没投影的场景,凡是和光照有关的差异在那 11 只上不能直接当成我们画错 | 小(若要收尾):确认图鉴 UI 走哪条排列,给 `cmp_shots` 的输出按「有/没有投影」分两拨打印 |
| ~~**不透明族(`M_P_Object`)那道逐像素星点门没实现**~~ **量过了,现在做没有意义** | 半透族那道(`RampID ≥ 0.4`)落地之后,全库还带星贴层的只剩:果冻的 `_By`(半透族,已按 `RampID` 判)与星光族三只的 `_Fx`(假半透)。而假半透那三只的 `MaskTex`(= 法线图)alpha 在它们 `_Fx` 的 UV 上**中位 0.000、≥0.4 的只占 0.0~0.1%** ⇒ 实机那条门是**关的**,游戏根本不画那一段;我们给这三只画的是另一条(`NoiseTex` + `Color02`)近似,把它**整个关掉**实测只动 ±0.002(幽星光 0.088→0.090、曜星光 0.074→0.075、暮星辰 0.074→0.074)| 等到有材质**真的**需要它再说:要做得先把 `_M` 的 alpha 也传给运行时(现在只有 `mask_id_tex` 那一路传了) |
| ~~**`M_P_Object_Trans` 的不透明度**~~ **整条链已按汇编做完(2026-08-30)** | `α = max(高光×SpecInt, MatCap.r, 边缘光覆盖率, 基色a重映射)`,其中**边缘光那一路进 α 时不乘强度**(汇编里颜色与 α 是岔开的两条,见 findings.md「莫比乌乌」那节)。三对球那条老结论「实机是实心红球 = 背后的描边壳」**已被推翻**(曜星光一橙一蓝,而它两颗球挑的是同一档暗红描边);现在的做法是**先把球的远半球当不透明件画一遍**,见 findings.md「三对球」 | 已做。剩下的是球身颜色的残差(幽星光偏橙、暮星辰右球 G 偏高),归到「整条着色链」那条 |
| ~~**果冻的外壳圆顶还是太透**~~ **已按目标 ES3.1/Low resource 修复** | 旧记录误用了命中组里最大的 High shader 68869。实机真正选择的 Low PS 2109/55790 用 `OpenDepthDistance * saturate((sceneDepth-pixelDepth)/OpacityDepthDistance)` 补覆盖率；两遍深度管线、原始参数与正确的预乘顺序落地后，圆顶完整且仍能透出深绿色内胆，见 §1.1 | 已做；果冻很暗占比 **0.050** 对实机 **0.052** |
| **⚠ 上一行那条「源网格最多 2 套 UV」已被推翻** —— 全库 123 个网格有 3~4 套,glb 里也一直写着,是运行时只读了两套。见「「源网格最多两套 UV」是错的」那节 | 已修一部分(`Vertex.uv2`) |
| ~~**`M_P_Object` 缺的是那条逐顶点光照加项**~~ **查清了:是关卡的体积光照贴图,拿不到** | VS 39798 里 TEXCOORD1/2 是**算出来的**,来自两张 `texture3d` 的逐顶点采样 + 球谐归一化(0.886227548 = √π/2、0.318309873 = 1/π)—— UE 的 Volumetric Lightmap;`v4.y` 是同一批砖块里的方向光遮蔽量,没数据时恒 1。**这是关卡烘焙数据,不在宠物包里**。没有砖块时整条化简成`固有色 × 色带 × View[169]`,正是我们现在做的。见上面那节 | 已解决:不要实现。**顺带把 `MPC_S_Global` 整份解出来了**(14 标量 + 16 向量,布局是标量在前),以及「边缘光是 lerp 替换、不是加光」——后者别单独改,见那节末尾 |
| ~~**按 `MatID` 挑 `RampID` 查色带图**~~ **查了是关的,不做** | 3393 份宠物材质里 3387 份用根默认 `T_AllDebugRamp`,而那张图 **256 行没有一行沿明暗方向有梯度**(最大变化 0.102),行均色全在 0.947~1.000。整层顶多在暗部带 5% 粉色偏色,却要占掉第 16 张贴图绑定 | 负结果入库 |
| ~~**小灵面一家的身体整层没画对**~~ **已改一大半**(0.189 → **0.145**) | 读出来的差别在**自发光**那一层:卷动要采两次(两组速度、不同通道、相乘),遮罩里原来多了一个 `1.0 +`(从 Low 排列读的)——就是「身体像一层薄雾」的成因。固有色那条原实现本来就对。见「小灵面:卷动层要采两次」那节 | 剩下的:两层 twinkle 的完整式子 |
| ~~**半透那一族我们比实机透**~~ **莫比乌乌那半已解决;春兔那半查清了、但改不动** | 莫比乌乌是背板被当半透特效层画了(已修)。**春兔那半**:我们的 α 完全等于材质数据(基色 alpha 沿耳朵 0.675→0.067 的斜坡),而汇编 PS 68874 的 `max(基色a, 高光×SpecInt(=0), 边缘光(≤0.2)) + OpenDepthDistance(=0)×深度淡化` **也给不出实机那个数**;材质与根材质都没有 `TwoSided`,不是两面画。旧的「3.6 倍」是**姿势没对齐**量出来的假象(新加 `tools/posematch.py` 按轮廓 IoU 挑帧后差距明显变小) | 剩两个候选:① 我们按 `painted_opacity` 把这批排除在 `_Ol` 描边壳之外,而实机耳边确有一圈;② 图鉴场景可能先渲进带不透明底的 RT。**②只能靠一张平背景的新截图判**。见上面那节 |
| ~~**闪烁定位到了:是 `M_P_Object` 那层加性流动层**~~ **根因找到并修好了(2026-08-30)** | 上一条只到「是这一层」。再往下一层是 **UV 选错了套**:同一份 cooked resource(水灵 `_By` 的 `Num/lod=0/dsid=0`,`0F1003EB…`)里有两条像素着色器,选择器那一行不一样 —— **49966** 是 `lerp(v3, v4, saturate(UV Number))`(签名 v3=TEXCOORD0 / v4=TEXCOORD1),**37774** 是 `lerp(v3, **v5**, …)`(v3..v6 = TEXCOORD0..3)。这是 UE 按网格 `NumTexCoords` 编的两个变体:材质图里那个 TexCoord 节点写的是 **2**,编译时被 clamp 到 `NumTexCoords − 1`。**水灵的网格有 4 套 UV** ⇒ 实机取 **UV2**(98.7% 非零),我们取的是 UV1(**只有 0.6% 非零**,几乎全是 (0,0))—— 整层退化成「采贴图上同一个点、随时间滚过去」,于是全身一起亮一下。改成按「有没有第三套 UV」选之后:t∈[1.0,1.6] 平均亮度极差 **3.25% → 0.21%**,方波变成平滑漂移 | 已修。水灵 调色板 0.109 → **0.106**、对比比 0.68 → **0.72**;波波拉 对比比 1.18 → **1.15**;无投影那 14 只对比中位 1.12 → **1.10** |
| **水灵那圈水环:公式读全了,卡在身体幅度** | ① ~~亮度会瞬间闪烁~~ **已修**(UV 选错了套);② 缺白色流动高光 = 水体预设的 **caustics 层**,③ 那层浅蓝 = **两色菲涅尔层** —— 两层的完整公式都已从 PS 16335(实机默认排列)读出并实现,**但接上去水灵调色板 0.106 → 0.293、亮度 0.85 → 1.17**。原因量清楚了:层二在线性里 ≈ 0.3、身体 ≈ 0.43,同量级;而实机身体是 `固有色 × 色带`(色带实测 ≈ 恒 1),我们的 `shade` 最高 3.0 —— 身体先大了 2~3 倍。④ 波浪裙边与水鳍的亮白边都属于这两层没画 | **要动 `shade` 就得连这一层一起动**,和「`M_P_Object` 整包做」是同一件事。代码与参数都留着(`water_layer`),重开只要加回一行 |
| **矮脚爬爬的眼球:公式全解出来了,亮度那一格是运行时值** | ~~那 10 层是纯红球、遮罩槽指着占位图~~ **猜错了**:PS 65438 里两色渐变被 `TwoColorLerp = 0` 整层乘掉,球本来就是平的 `BaseColor`。真正的差别是亮度:`dim = clamp(FX_PostCC_Actor_HSL.w × DirLightIntAdjust(0.4), 0.2, 1)`,而 `FX_PostCC_*` 是**运行时由蓝图写**的(资产默认 0 ⇒ dim 0.2 ⇒ 0.276,太暗;dim 1 ⇒ 0.618,太亮;实机量到 0.457 ⇒ 反推 dim ≈ 0.55)。**和「逐顶点体积光照」同一种不可得**,不改系数 | **两条无歧义的还能做**:① 这一族 `o0.w = 1` 是不透明的,我们塞进了 `fs_effect`(混合通道、不写深度);② 汇编里没有 `fs_effect` 那个视角相关的 `EFFECT_RIM_FLOOR` 斜坡,所以我们的虹膜环边缘比中间亮。单做这两条数值上是平的(|误差和| 0.365 → 0.353)、G/B 反而更差,所以**没落地**,等亮度那格有着落一起做 |
| **「离远了变半透明」不是优化,是 LOD 换了 shader** | 用户实测(莫比乌乌 / 克莱因龙 / 暮星辰在手机上距离稍远时会变半透)。`_By` 有两套 cooked resource:`lod=0` 那份(PS 53987,**335 行**)算 `alpha = max(高光, MatCap.r, 边缘光, 重映射基色 alpha)`;`lod=-1` 那份(`435FE9BD…`,PS 37412,**218 行**)把前三项全砍了,只剩 `重映射基色 alpha` —— 砍掉的 117 行正好是让近处不透的那几项。**是既定行为,不是 bug**;桌宠恒定近距离,照 `lod=0` 做就对 | 已解释,不用实现。**又一次说明四元组后两项必须打出来** |
| **半透壳还是比实机透:剩下的是 `alpha = 基色 alpha` 这条本身** | 边缘光那一路修好之后(2026-08-30,α 取原始覆盖率、不乘 `Rim Intensity`),莫比乌乌管子边缘那圈由 2px、α=110 的虚边变成 α=255 的实白,整只不透明像素占比 74.2% → 77.6%、描边比 0.86 → 0.96。**但壳的主体仍偏透**:按材质数据算它就该是六成多的半透(基色 a 中位 0.620 ⇒ 重映射 0.644),而实机(图鉴与大世界两个场景都看过)是实的。四项里只剩基色 alpha 在起作用,`HighLight SpecInt` = 0、`Rim Intensity` = 0.2、`MatCap_glass06` 的 R 中位 0.000。**春兔的白耳朵是同一条** | 中:缺的那一项还没找到 —— `PixelDepthOffset` / `ForceUseDefOpacity` / 两个 `DepthDistance` 都按 `scalar-slot` 字节码核过槽位了,都是 0。下一个候选是 **WPO**(整条没实现)|
| **克莱因龙的气泡身体只回来一部分** | `tint.a` 那条修好之后身体从「完全没有」变成「有一块半透的壳」,但实机是一个包住整只的大气泡、里面盛着粉色液体。走 `M_P_FakeFulid`(全库仅此 1 个材质),我们只有「遮罩 × 卷动 × 边缘」这套通用近似 | 中:反汇编 `M_P_FakeFulid`;莫比乌乌那泡粉色液体(`M_Gra_Yutu_Ear_Lighting`,同春兔耳朵)大概率要一起看 |
| ~~**矮脚爬爬像蒙了一层膜,对比度偏低**~~ **那两个数早就过期了(2026-08-29,用户指出)** | 待办里记的是亮度比 **1.51**、对比比 **0.49**;**今天重跑同一张手机截图是 1.06 / 0.83**,调色板 0.020(27 只里第 24 名)—— 中间几轮修改早把它带回来了,只是这一行没跟着更新。换成高清全身图、按各自包围盒裁到同高之后两边几乎逐块对上,没有任何「膜」(`cmp-review/矮脚爬爬_win_裁剪.png`) | 已解决。**教训是待办表里的数字要跟着重跑**,过期的离群值会把后面的排查引到不存在的问题上 |
| ~~**水灵的水波纹理铺到了不该去的地方**~~ **已落地(2026-08-29)** | `MaskMode(1uv_0VertexColorB)` 这个名字**在实机那条排列的参数表里根本不存在**(56 个向量槽 / 65 个标量槽逐条查过)—— 真正管顶点色 B 反转的是 `InverVertexColor`,而顶点色 B 是**乘在流动层上的权重**,不是覆盖率。当年「按顶点色剔像素」之所以更差,就是把它当成了覆盖率 | 已实现,见上一行。`Vertex.uv1` 也顺带接上了(`UV Number` 是 UV 集选择器) |
| ~~**玻璃球:剩下的是底色**~~ **根因找到并修好了(2026-08-30)** | 实机那颗红球是**背后的描边壳**(通道比 1:0.115:0.128 与 `OutLineOtherColor` 的 1:0.092:0.097 一致;而两颗球在图集里的基色差很远、实机却几乎同色 ⇒ 球色不是基色给的)。我们把它挡住了:球的 alpha **100% 是 1.0**,因为边缘光覆盖率写成了 `saturate(pow(1−|N·V|, Rim Power))`,漏掉汇编里的 `gate` 与 `(x − 0.5) / Rim Soft Edge` 重映射 —— `Rim Power = 0.35` 那条曲线太平,整颗球都是 0.5~1。补齐后:G 0.510→**0.341**(实机 0.314)、B 0.420→**0.302**(实机 0.333),两颗球变同色,观感由橙转红 | 已修。**剩下 R 偏低**(0.79 vs 实机 0.93):纯描边壳编码是 0.688,实机比它还亮 ⇒ 壳之上还有一层加光,另算 |
| **春兔耳朵那泡液体只接了颜色** —— **气泡做过一版,整个撤了(2026-08-05)** | 三版都试了:① 按 `Bubbles Scale`(6.72)平铺网格 UV —— 那会在耳朵上铺 **6.5 × 6.4 个周期**,气泡碎成一层白雾;② 照汇编改成**屏幕空间** + 两次卷动相乘(`M_Gra_Yutu_Ear_Lighting` 的 shader 5445 第 215~234 行:`ndc×0.5+0.5` 起、硬写的 `(0.02,0.06)` 与 `(−0.04,−0.025)` 两路);③ 只取 G 通道(静态开关写着「R通道液体,G通道小球」)。**③ 与不画它逐像素完全相同(差 0 个像素)** —— 算得出来:G 均值 0.033 × `Bubble Color` 0.42 × 耳膜透过率 0.36 ≈ **0.005**,而 8 位量化是 0.0039,整层落在量化噪声里。**根因是它被耳膜挡着**:液体在里面、耳膜(近白、alpha ≈ 0.36)盖在外面,所以先要让耳膜别那么白,气泡才有可能看得见 | 中:~~先解决耳膜~~ **「实机耳膜是淡绿」是错的(2026-08-29,用户指出)** —— 实机耳膜就是**白色半透**,看着发绿是**截图背景**透过来的:图鉴截图的底色是宠物的**系别色**(草系绿 / 火系黄橙 / 水系蓝 / 鬼系紫 / 飞行系青),春兔是草系。实测把「非背景」像素按 G 分档,最亮那档中位 **(0.933, 0.914, 0.875)** —— 和基色 (0.93,0.91,0.89) 逐位对上,耳膜**没画错**。真正对不上的是**里面那泡液体**:实机是淡粉、随耳形收边、带小气泡,我们是一块**硬边高饱和品红矩形**,而且溢出耳廓。见 `cmp-review/春兔_win_裁剪.png` |
| ~~**果冻内胆的 glassy flow 只用了两色均值**~~ **已按 71636 完整实现** | 预蒙皮局部位置/法线、`1/(1+GlassyNoiseRefract)`、沿折射线 march、三平面 R/A 噪声、FlowColor02→01 与 FresnelColor 两次 lerp 均来自原 shader；没有补经验光照 | 已做。**对着 master 重量的配对结果**是调色板 **0.179 → 0.108**、对比比 **0.49 → 0.92**(0.130/0.70 是中途某一版的数,不是基线) |
| **`cmp_shots` 的选区判据 `alpha > 200` 对真半透不公平** | 真半透的部位在**我们这侧**整块落到选区外,而实机那侧看得见 —— 春花兔耳朵一变透,调色板就从 0.055「涨」到 0.069(同一次改动对比比反而 1.45 → 1.02)。**这不是画差了,是两边比的区域不一样** | 中:让我们这侧先合成到实机背景色再比。会动**全部**基线数字,得连着重标一轮 |
| ~~**星点层之外,rim/matcap 仍当「加光」处理**~~ **单独挪过来更差,已撤回(2026-08-05)** | 照汇编把玻璃族那两层从「加性光」改成「混进固有色、再乘光照」:**暮星辰 0.065 → 0.120**(几乎翻倍)、幽星光 0.092 → 0.094,**没有一只变好**,全部中位 0.070 → 0.076。原因和文档里那条「特效通道搬进线性」一样 —— 那几层的强度当年是**在「加性光」这个前提下标定**的,而且我们的 `shade`(toon 两段明暗)和实机的光照不是一回事:混进固有色之后边缘光会被暗面乘没,而实机那圈是要亮的 | 中~大:**要做就整包做** —— 混合位置 + 强度 + 遮罩形状一起改,连着 19 只重标;单挪一步必崩 |
| 贴图 KTX2 + 关键帧精简 | 全量 3.0GB，单形态 2.1–5.0MB 里动画通道占大头 | 中 |
| 64 个零动作形态 | 同族里也找不到带动画的资产，属素材本身不全；可能得放弃或用同阶段近亲代播 | 小(调查)，修不一定可行 |
| 真实时钟作息 / 心情影响表情 / 喂食 | Phase 3 的遗留，纯手感。**性格已经做了**(见 Phase 7):倍率乘在困倦/无聊/表情/起跑/社交五处 | 小 |
| damage 局部提交 | §3.3 的提交策略一条都没做(原来那条降频已取消) | 小 |
| 多显示器实测 | 代码按 per-output 写的，但手上只有单屏。**已知一条**:每个 output 上是各自独立的一只,tick 驱动的叫声(睡醒)两边会同时响 | 需要第二块屏 |

**推荐执行顺序：~~Phase 5 → Phase 6 → Phase 8 → Phase 7 的前半~~**(都已完成)。
Phase 7 还剩分发那一半:开机自启、安装包/AppImage、贴图 KTX2、N 只宠物的占用实测。
横向待办里的材质参数是纯导出器侧的活,和运行时不冲突,随时能单独插一轮。

## 10. 风险与未决问题

| 风险 | 缓解 |
| --- | --- |
| Windows 逐像素 alpha + GPU 交换链 | 必须走 `CreateSwapChainForComposition`；wgpu 可能要用 `SurfaceTargetUnsafe` 自建 surface，S1 定论 |
| KWin 对 wlr-layer-shell 的支持随 Plasma 版本变化(非正式协议) | 平台层抽象成 trait；S1 的验收项固化成回归清单，升级 Plasma 后重跑；实测的 KWin 版本写进 README 支持矩阵 |
| 全屏透明层的合成开销 | §3.3 的提交策略；S1 里就要量一次空闲/活动时的 CPU/GPU 占用 |
| 材质只能近似 | 明确目标是「像」；把 ramp/描边参数做成包内可调 |
| 游戏版本更新改路径/命名 | 导出器带版本适配与覆盖率报告，缺失动作降级而非报错 |
| 第三方包的脚本安全 | 自产包用 Lua；一旦开放第三方，换 WASM 沙箱 + 能力白名单 |

## 11. 法务与分发

- 素材版权属腾讯/发行方。仓库**只有代码、schema 与导出器**；原始解包数据、生成的宠物包
  都不入仓库、不随发布分发，用户用自己的游戏安装本地生成(沿用 rocom-capture / rocom-petvo 的约定)。
- 运行时不读游戏内存、不注入进程、不联网上报。
