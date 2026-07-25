# rocom-pets

跨平台桌面宠物：把《洛克王国：世界》的宠物模型、动作与叫声做成本地生成的「宠物包」，
由一个原生运行时在桌面上播放与交互。宠物按需启用、可多只同时在场。

**当前状态：设计阶段，尚无代码。** 方案见 [docs/design.md](docs/design.md)。

支持矩阵：**Windows 10+** 与 **KDE Plasma Wayland**(开发环境 Plasma 6.7.3 / kwin_wayland)。
GNOME 等不实现 wlr-layer-shell 的合成器不在支持范围，也不做 X11 回退。

- 运行时：Rust + wgpu，自写平台窗口层(wlr-layer-shell / Windows layered + DirectComposition)。
- 宠物包：一条进化链一个 `.rkpet`(zip)，含 glb 模型、贴图、叫声与 manifest。
- 导出器：C#(CUE4Parse)，从用户自己的游戏 pak 生成宠物包。

资产提取链路在 [rocom-capture](../rocom-capture) 里验证；叫声提取管线复用 rocom-petvo。

素材版权属原发行方：**本仓库只有代码与导出器，不包含也不分发任何游戏素材或生成的宠物包**，
需自备游戏安装在本地生成。运行时不读游戏内存、不注入进程、不联网。
