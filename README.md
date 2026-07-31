# rocom-pets

跨平台桌面宠物：把《洛克王国：世界》的宠物模型、动作与叫声做成本地生成的「宠物包」，
由一个原生运行时在桌面上播放与交互。宠物按需启用、可多只同时在场。

**当前状态:多只宠物可同时在场**——上桌待机/走动/奔跑/睡觉、鼠标交互(轮廓命中、受惊、摸头、
拖放,拎起的只是被点中的那只)、穿透开关、托盘里加一只/撤下/切形态且重启恢复阵容,
全量宠物包已导完(530 条进化链 / 831 个形态)。
宠物之间会互相注意到并打招呼、受惊会跑开,凑近了还会演一段跨宠互动
(珀尔鼬指挥捕尘长绒清扫)。
**九条原始需求只剩叫声与 Windows 后端两条没结。**
需求对照与后续计划见 [docs/design.md](docs/design.md) §9。

支持矩阵：**Windows 10+** 与 **KDE Plasma Wayland**(开发环境 Plasma 6.7.3 / kwin_wayland)。
GNOME 等不实现 wlr-layer-shell 的合成器不在支持范围，也不做 X11 回退。

- 运行时(`src/`)：Rust + wgpu，自写平台窗口层(wlr-layer-shell / Windows layered + DirectComposition)。
  当前进度:KDE Wayland 后端已跑通(透明置顶、轮廓命中、穿透开关,[docs/spike-s1.md](docs/spike-s1.md));
  骨骼动画 + toon 着色已跑通([docs/spike-s2.md](docs/spike-s2.md));
  **宠物已经能站在桌面上待机、走动、睡觉、被摸头与拖放**(Phase 1–4,见 design.md §9)。
- 导出器(`exporter/`)：C# + CUE4Parse，从自己的游戏 pak 生成宠物包;
  一条进化链一个包(glb 含全部动作 + 贴图 + manifest.toml)，见 [docs/spike-s3.md](docs/spike-s3.md)。
- 验证工具(`tools/verify_glb.py`)：按 glTF 规范自采样 + 蒙皮 + 光栅化，渲图肉眼核对动画正确性。
- shader 逆向(`scripts/`)：cooked 包里材质图被剥了、只剩参数值与静态开关,而编译产物里公式是全的、
  静态开关也已定死。这批脚本把公式从 shader library 里读出来 ——
  Windows 端走 DXBC(`shaderdump.py` 取码、`dxbcdis.c` 反汇编、`dxbcsig.py` 对语义、
  `matshader.py` 认归属、`uniexpr.py` + `matparams.py` 把 cb 槽位对回参数名),
  安卓端走 GLSL 源码(`glsldump.py`,好读得多)。
  流水线与结论见 [docs/shader.md](docs/shader.md) 与 [docs/android-glsl.md](docs/android-glsl.md)。
  安卓那条路原本卡在**归属**(APK 里有 shader 却没有宠物资产,只能靠结构指纹猜);
  宠物资产在手机的**应用私有目录**里,`adb root` 取到之后归属变成精确哈希查表,
  见 [docs/android-device.md](docs/android-device.md)。

配置在 `~/.config/rocom-pets/config.toml`(首次运行生成带注释模板);托盘菜单可加一只/撤下、
切形态、切穿透、召回、退出。**在场阵容存在同目录的 `roster.toml` 里**(托盘改一次就重写一次,
所以没和手写的 config.toml 混在一起),下次启动自动恢复;给了 `--pack` 则只上这一只、不动存档。
全局热键走 XDG GlobalShortcuts portal(KDE 会弹窗确认),或把 KDE 自定义快捷键
绑到 `rocom-pets --toggle-passthrough`。

```sh
cargo run --release -- --pack packs/喵喵                    # 把宠物放到桌面上
rocom-pets --list                                          # 列出 ~/.local/share/rocom-pets/packs 里的包
rocom-pets --pack 喵喵                                      # 按包名启动(也可给目录路径)
rocom-pets --toggle-passthrough                            # 通知已在跑的实例(可绑快捷键)
cargo run                                                  # 同上但用调试精灵(平台层验收模式)
cargo run --release -- --render packs/喵喵 --bench 600      # 离屏渲宠物 + 测出帧耗时
git -C "$CUE4PARSE_DIR" apply exporter/patches/*.patch      # 导出前必做:修上游把法线写成切线的 bug
dotnet run --project exporter -- --species 3001 --out packs # 导一条进化链
dotnet run --project exporter -- --all --skip-existing --out packs  # 全量导(可分批续跑)
python tools/verify_glb.py packs/喵喵 --clips Idle,Walk     # 渲图验证
uv run --with lz4 python scripts/glsldump.py <安卓 shader 库> --index   # shader 逆向(见 docs/shader.md)
```

资产提取链路在 [rocom-capture](../rocom-capture) 里验证；叫声提取管线复用 rocom-petvo。

素材版权属原发行方：**本仓库只有代码与导出器，不包含也不分发任何游戏素材或生成的宠物包**，
需自备游戏安装在本地生成。运行时不读游戏内存、不注入进程、不联网。
