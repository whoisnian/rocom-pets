# rocom-pets

跨平台桌面宠物：把《洛克王国：世界》的宠物模型、动作与叫声做成本地生成的「宠物包」，
由一个原生运行时在桌面上播放与交互。宠物按需启用、可多只同时在场。

**当前状态:多只宠物可同时在场**——上桌待机/走动/奔跑/睡觉、鼠标交互(轮廓命中、受惊、摸头、
拖放,拎起的只是被点中的那只)、穿透开关、托盘里加一只/撤下/切形态且重启恢复阵容,
全量宠物包已导完(530 条进化链 / 831 个形态)。
宠物之间会互相注意到并打招呼、受惊会跑开,凑近了还会演一段跨宠互动
(珀尔鼬指挥捕尘长绒清扫)。叫声也接上了:摸头/受惊/召唤/睡醒各一段,
每只实体的嗓音随机(游戏里那个 −100~100 的 `voice` 属性),默认小声、托盘可静音。
**九条原始需求全部结掉**:Windows 后端也在实机上跑通了(2026-08-01)。
需求对照与后续计划见 [docs/design.md](docs/design.md) §9。

支持矩阵：**Windows 10+**(实机验过:上桌、置顶、点击穿透、拖放、托盘;
开发机是 Linux,靠交叉编译 + wine 冒烟 + 实机反馈来回磨)与
**KDE Plasma Wayland**(开发环境 Plasma 6.7.3 / kwin_wayland,日常在跑)。
GNOME 等不实现 wlr-layer-shell 的合成器不在支持范围，也不做 X11 回退。

### 编译 rocom-pets.exe

**在 Windows 上**(最省事):装 [rustup](https://rustup.rs) 与 Visual Studio Build Tools
的「使用 C++ 的桌面开发」(要 MSVC 链接器与 Windows SDK),然后

```sh
cargo build --release          # → target\release\rocom-pets.exe
```

**在 Linux 上交叉编译**(本仓库就是这么出的 exe,不需要 Windows 机器):

```sh
rustup target add x86_64-pc-windows-msvc
cargo install cargo-xwin                       # 自动取 MSVC 的 CRT/SDK 头与库
sudo pacman -S clang                           # 提供 lld-link(Arch;别的发行版装 lld)
PATH=/usr/lib/llvm*/bin:$PATH cargo xwin build --release --target x86_64-pc-windows-msvc
```

产物 `target/x86_64-pc-windows-msvc/release/rocom-pets.exe` **不需要 VC++ 运行库**
(`.cargo/config.toml` 里对这个目标开了 `+crt-static`),拷到 Windows 上双击即可;
除系统 DLL 外零依赖。只想验代码能不能过编译器的话,`cargo check --target
x86_64-pc-windows-msvc` 就够(只要 std,连链接器都不用)。

**双击不会有黑窗口**(release 版按 GUI 子系统链接),但**从 cmd/PowerShell 里跑仍然有
日志** —— 启动时会挂回父进程的控制台。要看日志就 `set RUST_LOG=info` 再从命令行启动;
挂回去时 shell 已经回到提示符,日志会和提示符交错着刷,这是这类程序的通病。
debug 版(`cargo build` 不带 `--release`)保持控制台子系统。

宠物包不随 exe 走:把 Linux 上导好的包目录拷到 `%LOCALAPPDATA%\rocom-pets\packs\`,
或者用 `--packs-dir` 指过去;不给包也能起(调试精灵模式,用来验平台层)。

- 运行时(`src/`)：Rust + wgpu，自写平台窗口层(wlr-layer-shell / Windows `WS_EX_NOREDIRECTIONBITMAP` + DirectComposition)。
  当前进度:KDE Wayland 后端已跑通(透明置顶、轮廓命中、穿透开关,[docs/spike-s1.md](docs/spike-s1.md));
  骨骼动画 + toon 着色已跑通([docs/spike-s2.md](docs/spike-s2.md));
  **宠物已经能站在桌面上待机、走动、睡觉、被摸头与拖放**(Phase 1–4,见 design.md §9)。
- 导出器(`exporter/`)：C# + CUE4Parse，从自己的游戏 pak 生成宠物包;
  一条进化链一个包(glb 含全部动作 + 贴图 + 叫声 + manifest.toml)，见 [docs/spike-s3.md](docs/spike-s3.md)。
  叫声要 `vgmstream-cli` 与 `ffmpeg`(缺了自动跳过,`--no-voice` 显式关);
  全库 835 个形态里 **499 个有叫声**,合计 31MB。
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
