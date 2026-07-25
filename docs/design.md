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

## 1. 已验证的数据事实

以下都在解包数据上实测过，是方案的地基(2026-07-25，客户端 pak 对应 `GAME_RocoKingdomWorld`)。

| 事实 | 结论对方案的影响 |
| --- | --- |
| 宠物资产在 `NRC/Content/ArtRes/AnimSequence/Pets/<Asset>/`：1001 个目录、683 个带 `SKM_*`(蒙皮网格)，Pets 下 AnimSequence 共 2.9 万个 | 导出器的输入根；`ArtRes` 默认被 unpack.sh 排除，要 `--filter` 单独导 |
| 喵喵 `SKM_Gra_MiaoMiao1_001_Skin`：4 级 LOD、LOD0 3095 顶点/4826 三角、44 骨骼、3 材质槽、8 套 UV + 顶点色 | 单只宠物的量级极小，多实体常驻可行；LOD1 可作为省内存档 |
| 每只宠物 `Animation/` 约 60 个序列，含 `World_Idle/Walk/Run/Jump_Fall/Hide_*`、`Common_Happy/Sad/Anger/Fear/Relax/Show/Sleep_{Start,Loop,End}`、`Fight_*`、`Ride_Hug`、LookAt BlendSpace | 桌宠要的动作全都有，不需要自己做动画 |
| 动作是**配置驱动**的：`MODEL_CONF.anim_conf_id` → `ANIM_CONF` 给出逻辑动作名 + 毫秒时长，`ANIM_ID_CONF` 给 id→名。喵喵 30+ 条 | 包 manifest 的「逻辑动作 → clip」可自动生成，并能出每只宠物的动作覆盖率报告 |
| 逻辑名到资产名有规律：`Idle→World_Idle`、`Walk→World_Walk`、`SleepLoop→Common_Sleep_Loop` | 去前缀 + 忽略下划线大小写即可对齐，对不上的进报告人工兜 |
| `anim_conf_id` 可以不等于 `model_conf` 的 id(珀尔鼬 model 14765 / anim_conf 14641) | 必须从 MODEL_CONF 读，不能拿 model id 当动作表 id |
| 动画是 **ACL 压缩**的 | 导出器依赖 CUE4Parse-Natives 带 ACL 编译，见 §8 |
| 进化链可从配置归组：`PETBASE_CONF.stage / evolution_pet_id`，且资源目录名数字后缀 = 阶段。喵喵链 = 3001 喵喵(`Gra_MiaoMiao1_001`) → 3025 喵呜(`…2_001`) → 3007 魔力猫(`…3_001`) | 「一条链一个包」可完全自动切分 |
| `PETBASE_CONF` 含测试行与重复行(`9901 测试喵喵1`、`32000001 喵喵`) | 导出器要过滤：名称含「测试」、id 段异常、`legal_petbase` 等字段 |
| `INTERACTIONTREE_CONF` 有「摸头」「亲昵」「查看信息」并带动作键 | 交互动作有官方对应关系可循 |
| `NRC_AI_BEHAVIOR_CONF`(3077 行)只是指向 `Modules/AI/BehaviorTree/MFBT/…` 的自研 Dots 行为树资产 | **不移植**；但 `editor_name` 是中文可读的(如「【毛头小蛛】主动清扫」)，可作为「这只宠物该有什么行为」的选型参考 |
| 捕尘长绒的资产家族是 `Wor_MaoTouXiaoZhu2_001`(毛头小蛛)，AI 表里正有「【毛头小蛛】主动清扫」 | §6 的第一个互动样例(珀尔鼬 × 捕尘长绒)有据可依 |
| 叫声在 `WwiseAudio` 的 `Pet_Vo_*.bnk` + 流式 wem，`PetData.voice` 选组；粗嗓门/婉转声是运行时 Wwise pitch RTPC，wem 本身中性 | 复用 rocom-petvo 的提取管线；变调用播放速率复刻 |
| CUE4Parse 的 BC7 解码有 R/B 通道对调的上游 bug | 导出贴图时必须换回，参照 rocom-capture 的 `FixBc7ChannelOrder` |
| `UMaterialInstance.Deserialize` 在该版本抛 OverflowException，贴图槽拿到父材质默认值 | 贴图按命名约定接：材质名后缀 `_By/_Es/_Mh` ↔ `T_<Asset>_<槽>_D` |
| **CUE4Parse 的 glTF 骨骼旋转约定是错的**：Y/Z 交换是反射，正确四元数是 `(-x,-z,-y,w)`，上游写的 `(x,z,y,w)` 是它的共轭。绑定姿势下 world × IBM = I 掩盖了它，上游又不导 glTF 动画，故从未暴露 | 导出器必须改写骨骼旋转**并重算 inverseBindMatrices**，见 [spike-s3.md](spike-s3.md) §1；这是动画正确性的单点故障，也是回归重点 |
| 走跑动画的 root motion 方向恒为 glTF +Z(= UE +Y，这些骨架朝 +Y)，但**逐 clip 不一致**：同一条链里有的带位移、有的原地 | manifest 逐 clip 给 `in_place`/`speed_cm_s`，运行时两种都要能处理 |
| CUE4Parse 只有 `FRocoBinData` 解码器，**不解 `.non` schema**；全仓唯一实现是 rocom-capture 的 `scripts/bin2json.py` | 导出器读 rocom-capture 产出的配置 JSON，不重复实现，见 §8 |

已验证的端到端结果：喵喵的 LOD0 网格 + 骨架 + `World_Idle/World_Walk/Common_Happy/Common_Sleep_Loop`
经 CPU 蒙皮 + 软件光栅化渲出正确形体、贴图与姿态，说明**网格、骨架、蒙皮权重、动画关键帧、贴图全部可用**。

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
| 命中/穿透 | `wl_surface.set_input_region` = 宠物轮廓并集；全局穿透 = 置空区域 | `WM_NCHITTEST` 返回 `HTTRANSPARENT`；全局穿透 = 加 `WS_EX_TRANSPARENT` |
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
  游戏是自研 shader，含 RampTex/MatCap/描边/StarStick/Fragments 等几十个参数，且材质实例参数还解不全。
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

### 4.2 结构

```
<链名>.rkpet                     # zip 归档
├── manifest.toml
├── forms/<asset>/model.glb      # mesh + skin + 已合并的全部 clip
├── forms/<asset>/tex/*.ktx2     # 基色/遮罩，已修正 BC7 通道序
├── voice/*.opus
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
voice     = "voice/vo_3001"
tags      = []        # 互动能力标签，如 ["cleaner"]/["commander"]

  [forms.clips]              # 由 ANIM_CONF 自动生成
  idle   = { clip = "World_Idle",   ms = 1333, loop = true }
  walk   = { clip = "Walk", ms = 1133, frames = 35, in_place = false, root_motion_cm = 53.06, speed_cm_s = 46.8 }
  run    = { clip = "Run",  ms =  600, frames = 19, in_place = false, root_motion_cm = 180,   speed_cm_s = 300 }
  happy  = { clip = "Common_Happy", ms = 1500 }
  anger  = { clip = "Common_Anger", ms = 1500 }
  shock  = { clip = "Common_Shock", ms = 1500 }
  sleep  = { start = "Common_Sleep_Start", loop = "Common_Sleep_Loop", end = "Common_Sleep_End" }
  callout= { clip = "Common_Show",  ms = 1500, voice = "callout" }

[report]              # 导出覆盖率,缺失动作让运行时降级而不是报错
missing_clips = ["hide"]
```

实际产物比这份草案更细(每 clip 带 `frames`/`root_motion_cm`/`speed_cm_s`，贴图带槽位与尺寸)，
见 [spike-s3.md](spike-s3.md) 与导出器的 `Manifest.cs`。

### 4.4 加载与体积

- 发现路径 `~/.local/share/rocom-pets/packs/`、`%APPDATA%\rocom-pets\packs\`；
  启动只读各包 manifest(轻)，**启用某形态时**才流式读该形态的 glb 与贴图。
- 体积(S3 实测，喵喵链 16 个动作 + 1024 贴图)：每形态 **2.1–5.0MB** glb + 贴图，
  一条链目录 13MB、`.rkpet` 6.9MB。比原估的 2MB/形态高一倍,动画通道是主要占比
  (骨骼数 × clip 数 × 帧数)。
- 压体积手段(已做)：只导桌宠动作白名单、恒定轨道不写通道/只写单帧。
  (待做，Phase 4)：关键帧精简、贴图降到 512、KTX2/BasisU、只导当前启用的形态。

## 5. 动作与行为

- **逻辑动作层**：运行时只认 `idle/walk/run/happy/anger/sad/fear/shock/show/relax/sleep/callout/…`，
  具体 clip 由 manifest 映射，缺失则降级(如无 `run` 就用 `walk` 提速)。
- **三段式(Start/Loop/End)是一等公民**：睡觉、隐藏、技能都是这个结构；Loop 时长由状态机需求决定。
- 每实体一个状态机 + 需求值(困倦/心情/无聊) + 作息时钟；转移由事件驱动：鼠标、邻近实体、
  屏幕边界、定时器、脚本 Intent。
- 进阶：LookAt BlendSpace → 视线跟随鼠标。
- **已由 S3 定论**(详见 [spike-s3.md](spike-s3.md))：走跑动画**逐 clip 不一致**——同一条链里
  有的带 root motion 有的原地，方向恒为 glTF +Z(= UE +Y)。故 manifest 逐 clip 给
  `in_place`/`speed_cm_s`；运行时有速度就用它推进位置并原地循环播放，没有就按 locomotion
  取默认值，并对离谱值钳制(魔力猫 Run 反推出 7.5m/s)。单位：glb 米制，`height_cm` 取
  `ImportedBounds` 全高(喵喵链 80/104/204cm)。
- **待验证**：
  - `MODEL_CONF.SMR`、`PET_SHOW_SPEED_CONF` 各自的含义(现在速度直接从 root motion 反推，
    够用；要与游戏内手感对齐再查)；
  - `INTERACTIONTREE_CONF` 的 `anim_key*` 到动作表的确切映射(「摸头」指向的 id 20 在
    `ANIM_ID_CONF` 里叫 `Sad`，字面对不上，需实机核对)；
  - stage 0 目录(如 `Gra_MiaoMiao0_001`)只有 Mat/Tex 没有 SKM，是蛋还是共享皮，包里怎么表达。

## 6. 多实体与跨宠物互动

- 同一 stage 内多实体：同物种可多开(一个包创建多个实体)，不同包可同时启用。
- **事件总线**：`Intent{from, kind, target}` + `Perception{邻近实体, 鼠标, 屏幕边界}`。
- **互动包(interaction pack)**声明依赖，双方都在场且距离够近才可触发：

  ```toml
  [interaction]
  id = "peel_commands_cleaner"
  requires = [{ species = 3758 }, { species = 3604 }]   # 珀尔鼬 × 捕尘长绒
  trigger  = { kind = "proximity", max_distance = 200, cooldown = "3m" }
  ```

- 编排用**演出脚本(时间轴)**而非让两个状态机自发协商：谁在第几秒播哪个 clip、走到哪、
  何时出声，可靠且可调。脚本用 Lua(自产包)，若将来接受第三方包则换 WASM 沙箱。
- 诚实的限制：「清扫」这类游戏里由行为树驱动、没有独立 clip 的行为，只能用现成动作拼近似
  (`walk` 往返 + `show`/`attack1` 当动作)。

## 7. 音频

- 来源：复用 rocom-petvo 已跑通的 `Pet_Vo_*.bnk` + wem → vgmstream 管线，转 opus 进包；
  `PetData.voice` 决定用哪一组。
- 粗嗓门/婉转声是运行时 pitch RTPC，用播放速率/变调复刻，不需要额外音频文件。
- 触发点：启用召唤、受惊、摸头满意、睡醒。默认低音量、可静音、可全局关。
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
- CUE4Parse-Natives **必须带 ACL 编译**，否则动画解压报 `nAllocate` 找不到：
  `git submodule update --init --recursive CUE4Parse-Natives/ACL/external/acl`，
  再 `cmake -B builddir -DCMAKE_BUILD_TYPE=RelWithDebInfo . && cmake --build builddir`。
  build type **必须避开 Debug/Release**——那两个会命中 `install(TARGETS … RUNTIME DESTINATION)`，
  Linux 上 SHARED 库属 LIBRARY 产物无 destination，cmake 报错会让 `dotnet build` 挂在 MSB3073。
- 语言：导出器留在 C#(CUE4Parse 在那边)，运行时是 Rust；两者只通过包格式耦合。

## 9. 实施阶段

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
全局热键、自己的 D-Bus 控制接口。

全局热键有两条路,都实测通过:

1. **XDG GlobalShortcuts portal**(`org.freedesktop.portal.GlobalShortcuts`)。
   应用只能*建议*按键,KDE 会**弹窗让用户确认**——在用户点之前 portal 不回应,
   所以代码里放了看门狗提示去看弹窗(一开始误判成「KDE 丢弃了请求」,实机确认是等确认)。
2. **`org.rocom.Pets` D-Bus 接口** + `rocom-pets --toggle-passthrough|--recall|--quit`。
   在 KDE「自定义快捷键」里把任意键绑到这条命令即可,不依赖 portal,顺带让宠物可脚本化。

**待做**:落地用 `JumpFall` 动作、damage 局部提交、开机自启(`packaging/rocom-pets.desktop`
复制到 `~/.config/autostart/`)。

### Phase 2 — 鼠标交互

**已完成**:轮廓命中与轮廓输入区(离屏画布 alpha 异步回读成 8 物理像素的格子掩码,
腿与尾之间的空隙能点穿,实测输入区 60–87 个矩形随动画变化)、点击受惊(`Shock`)、
摸头(指针在头部区域来回蹭够 3 次换向 → `Happy`)、拎起来害怕(`Fear`)/放下落地、
**按姿势变化速度**自适应降频。行为逻辑有 10 个单测(用 `Model::for_test` 的合成模型,
不碰 GPU 也不需要宠物包)。
实测:CPU **1.3% 单核**、RSS 219MB。

降频这条踩过一次:一开始按状态硬分档(「待机」→ 12Hz),实机反馈**明显发顿**——
待机动画本身带起伏,实测关节最大速度约 6m/s(行走 4.7m/s),根本不算静止。
改成用关节速度连续映射成帧率(1m/s 以上跑满 30Hz,越接近静止越省,下限 10Hz):
待机/行走都稳稳跑满,睡觉那类真正近乎静止的动作会自动落到下限,不需要给每段动作手工标注。

**待做**:多显示器(手上只有单屏,没法验)、HiDPI 分数缩放(见下)、
掩码回读的内存开销(比 Phase 1 多 ~65MB,疑似 wgpu 的可映射缓冲内存池;
若要抠可以改成渲一张 64×64 的专用掩码附件而不是回读整张画布)。

### Phase 3 — 行为引擎

**已完成**:需求值(困倦/无聊)驱动的状态机、睡觉三段式(入睡 `SleepStart` → 睡着 `SleepLoop`
循环到睡饱 → 醒来 `SleepEnd`)、被戳会醒(而不是原地受惊)、待机时随手做表情
(`Happy/Sad/Anger/Show/Relax/Alert` 里随机)、指针悬在身上时侧身「瞥一眼」。
时间尺度是手感常量(困倦 8 分钟攒满、睡 90 秒睡饱、无聊 6 秒攒满),
`ROCOM_PETS_NEEDS_SPEED=20` 可整体加速,几十秒看完一轮作息。
睡着时姿势几乎不动 → Phase 2 的自适应帧率自动把它降到 10Hz,不需要额外标注。
新增 5 个行为单测(作息三段、戳醒不受惊、无聊消涨、瞥视方向、睡着降频)。

**不做**:真正的视线跟随。它要 LookAt BlendSpace(没导出),而且 Wayland 下**输入区之外
根本收不到指针事件**——要追全屏光标就得把输入区扩大到吃掉点击,代价不划算。
现在只在指针落在身上时侧身,读起来已经像在瞥。

**待做**:按真实时钟的作息(std 没有时区,要引依赖)、心情影响表情选择、饥饿/喂食。

### Phase 4 — 包格式定稿与导出器成品
manifest schema 落地、进化链打包、形态切换、按需启用/停用、包管理(先 CLI，再 egui GUI)。

### Phase 5 — 多实体与跨宠物互动
事件总线、proximity 判定、演出脚本、第一个互动包(珀尔鼬 × 捕尘长绒)。

### Phase 6 — 音频
叫声 + 变调；成本低，可提前插到 Phase 2 之后。

### Phase 7 — 打磨与分发
配置 GUI、开机自启(KDE autostart / Windows 启动项)、N 只宠物的性能与内存、
Windows 安装包 / Linux AppImage。

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
