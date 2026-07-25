# Spike S1 — 平台层验证

验证桌宠赖以存在的两个平台集成点：**KDE Plasma Wayland 的 layer surface** 与
**Windows 的 layered 窗口 + DirectComposition**。方案见 [design.md](design.md) §3.2、§9。

程序目标：每个显示器一个透明置顶表面，中间画一张**软边测试精灵**(带 alpha 渐变的圆 +
内部棋盘格)，精灵可拖动，穿透可切换。精灵刻意做成软边 + 半透明格子，因为
预乘 alpha 错误、合成器不支持 per-pixel alpha、色彩空间错配都会在软边和半透格上一眼看出来。

## 跑法

```sh
RUST_LOG=info cargo run                # 所有显示器各开一个 stage
kill -USR1 $(pgrep -x rocom-pets)      # 切换全局穿透(S1 阶段的临时开关)
```

S1 不做全局热键：KDE Wayland 下没有全局按键抓取，正式实现要走 KGlobalAccel 的 D-Bus 注册
或 XDG GlobalShortcuts portal，那是 Phase 1 的事。这里只用 `SIGUSR1`
（`pgrep` 要用 `-x` 精确匹配进程名，`-f` 会把发信号的 shell 自己也匹配上）。

## 验收清单

实测后填结果（✅/❌/⏳ + 备注），失败项直接决定架构是否要改。
Wayland 一列的实测环境见 W6；带 👀 的项要人工肉眼/动手确认，程序侧无法自证。

### 通用（两平台都要过）

| # | 项 | KDE Wayland | Windows |
| --- | --- | --- | --- |
| 1 | 置顶于普通窗口之上（浏览器/终端/文件管理器） | ✅ 人工确认正常置顶（另有截图确认压在桌面内容之上） | ⏳ |
| 2 | 能把精灵放到指定屏幕坐标 | ✅ 表面铺满 output，精灵坐标是表面局部坐标，直接可控 | ⏳ |
| 3 | 逐像素 alpha 正确：软边无黑边/白边，半透格下能看到底下的窗口内容 | ✅ `alpha_modes=[Opaque, PreMultiplied]`，选中 `PreMultiplied`；截图里软边干净、半透格能看穿到底下内容 | ⏳ |
| 4 | 精灵内点击/拖动由自己接到 | ✅ 人工确认点击与拖动正常 | ⏳ |
| 5 | 精灵外的点击落到下层窗口（不吃掉） | ✅ 人工确认轮廓外可正常点到下层（输入区 = 轮廓的 24 个矩形近似） | ⏳ |
| 6 | 运行时切换全局穿透后，精灵内的点击也落到下层 | ✅ 人工确认穿透开启后点击与拖动均落到下层；机制侧 `SIGUSR1` 使输入区在 24 ↔ 0 个矩形间切换（日志确认） | ⏳ |
| 7 | 多显示器各自一个 stage，坐标互不串 | ⏳ 当前只接了一台 DP-3，无法验；代码按 output 逐个建 stage | ⏳ |
| 8 | 显示器热插拔/分辨率或缩放变更后不崩、自行重建 | ⏳ 同上，未验；`output_destroyed` 已会销毁对应 stage | ⏳ |
| 9 | HiDPI：分数缩放下精灵不模糊、命中判定不偏 | ❌ **见下方「分数缩放」**，要补 `wp_fractional_scale_v1` + `wp_viewporter` | ⏳ |
| 10 | 空闲（无动画）时不提交帧，CPU 占用 ≈0 | ✅ 5 秒内只涨 1 tick(10ms)≈0.2%；只在有事件时出帧，不挂 frame 回调 | ⏳ |
| 11 | 拖动中 60fps 时的 CPU/GPU 占用（记录数值） | ⏳ 👀 需要真拖动才能测 | ⏳ |
| 12 | 内存占用 | debug 构建 RSS ≈148MB（含 NVIDIA Vulkan 驱动）;release 待测 | ⏳ |

### 分数缩放（item 9 的细节）

实测这台机器：物理 3840×2160，layer surface 的 configure 给 2560×1440 逻辑 → 实际缩放
**1.5**，但 `wl_output` 只能报整数 scale，KWin 上报 **2**。当前实现按 scale=2 渲染
5120×2880 的 buffer 并 `set_buffer_scale(2)`，几何正确，但合成器要把它降采样到 1.5x
（截图里能看到轻微软化）。正确做法是绑 `wp_fractional_scale_v1` 拿到 120 分之几的精确
scale，再用 `wp_viewporter` 把 buffer 映射到逻辑尺寸，按 1.5 渲染。

代价小、收益明确，排进 Phase 1；不影响 S1 的架构结论。

### KDE Wayland 专项

| # | 项 | 结果 |
| --- | --- | --- |
| W1 | `layer=top` 与**全屏窗口**（全屏视频/游戏）的叠放次序 | ⏳ 👀 |
| W2 | 与**锁屏**的叠放次序（绝不能盖住锁屏） | ⏳ 👀 |
| W3 | 与**通知/OSD**（音量条、通知气泡）的叠放次序 | ⏳ 👀 |
| W4 | 与**Krunner / 任务切换器 / 桌面预览**的叠放次序 | ⏳ 👀 |
| W5 | `exclusive_zone=-1` 确认没有挤压其他窗口布局（最大化窗口仍占满屏） | ⏳ 👀 |
| W6 | 实测环境（升级 Plasma 后本清单需重跑） | Plasma 6.7.3 / kwin 6.7.3-1、NVIDIA RTX 3070 / Vulkan、单显示器 DP-3 3840×2160@1.5x、rustc 1.97.1、wgpu 30.0.0、sctk 0.21.1(`system` feature) |

### 已确认的实现细节

- `zwlr_layer_shell_v1` 在 KWin 6.7.3 可用，`Layer::Top` + 四边 anchor + `set_size(0,0)`
  拿到铺满 output 的表面，`exclusive_zone(-1)` 不参与布局。
- 表面能力：`formats=[Rgba8UnormSrgb, Bgra8UnormSrgb, Rgb10a2Unorm, Rgba8Unorm, Bgra8Unorm,
  Rgba16Float]`、`alpha_modes=[Opaque, PreMultiplied]`、`present_modes=[Mailbox, Fifo, Immediate]`。
  选 `Rgba8Unorm`（非 sRGB，精灵字节即最终颜色）+ `PreMultiplied`。
- wgpu 要真的 `wl_display*`/`wl_surface*` 指针，**必须开 sctk 的 `system` feature**
  （默认的纯 Rust 后端根本没有 C 指针，`display_ptr()` 不存在）。
- 输入区坐标是 surface-local 逻辑像素，与 pointer 事件坐标、`Stage` 内部坐标同一套，
  `set_buffer_scale` 只影响 buffer 尺寸，不影响这套坐标。

### Windows 专项

| # | 项 | 结果 |
| --- | --- | --- |
| N1 | `CreateSwapChainForComposition` + wgpu 能否直接用，还是要自建 DXGI swapchain | |
| N2 | 任务栏不出现图标（`WS_EX_TOOLWINDOW`）、Alt-Tab 不出现 | |
| N3 | 点击精灵不抢焦点（`WS_EX_NOACTIVATE`，前台窗口标题栏不失活） | |
| N4 | 与全屏独占游戏的叠放行为 | |

## 结论

**Wayland 半边：架构维持，核心项全部通过。** 四件关键事——layer-shell 拿到置顶铺满的表面、
`PreMultiplied` 逐像素 alpha、轮廓命中与轮廓外穿透、全局穿透可运行时切换——程序侧与人工
操作都已确认，没有需要改架构的发现。
唯一的实现级欠账是分数缩放（补 `wp_fractional_scale_v1` + `wp_viewporter`，排 Phase 1）。

**Windows 半边：未开始。** 路线见 `src/platform/windows.rs` 的注释；已确认 wgpu 30 提供
`SurfaceTargetUnsafe::CompositionVisual`，即 N1 那条「自建 DComp visual 交给 wgpu」的路可行。
