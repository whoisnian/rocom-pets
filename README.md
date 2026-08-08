# rocom-pets

跨平台桌面宠物：把《洛克王国：世界》的宠物模型、动作与叫声做成本地生成的「宠物包」，
由一个原生运行时在桌面上播放与交互。宠物按需启用、可多只同时在场。

**当前状态:多只宠物可同时在场**——上桌待机/走动/奔跑/睡觉、鼠标交互(轮廓命中、受惊、摸头、
拖放,拎起的只是被点中的那只)、穿透开关、托盘里加一只/撤下/切形态且重启恢复阵容,
全量宠物包已导完(**201 个包 / 607 个形态**,1.6GB;按图鉴号归并,见
[docs/petindex.md](docs/petindex.md))。
宠物之间会互相注意到并打招呼、受惊会跑开,凑近了还会演一段跨宠互动
(珀尔鼬指挥捕尘长绒清扫)。声音**两层**:嗓子发出来的叫声(开心/受惊/害怕/难过/生气/
展示/放松/警觉/召唤九种情绪)加上身体动静的动作音效,受惊、摸头、睡醒、待机做表情、
配置窗口点动作时一起响;嗓音可调(游戏里那个 −100~100 的 `voice` 属性,只作用在叫声那层),
默认小声、托盘可静音,自己叫的那些一分钟至多一次。
**九条原始需求全部结掉**:Windows 后端也在实机上跑通了(2026-08-01)。
现在还有**独立的配置窗口**(`--settings`,托盘里也能开):管理宠物包(导入/查找/删除)、
管理在场宠物(加/撤,以及每只的形态/大小/性格/叫声/落脚点),改什么都即时生效;
运行时也**直接读 `.rkpet`**(zip)。
需求对照与后续计划见 [docs/design.md](docs/design.md) §9。

支持矩阵：**Windows 10+**(实机验过:上桌、置顶、点击穿透、拖放、托盘;
开发机是 Linux,靠交叉编译 + wine 冒烟 + 实机反馈来回磨)与
**KDE Plasma Wayland**(开发环境 Plasma 6.7.3 / kwin_wayland,日常在跑)。
GNOME 等不实现 wlr-layer-shell 的合成器不在支持范围，也不做 X11 回退。

### 编译 rocom-pets(Linux)

要 [rustup](https://rustup.rs) 与 wgpu 跑 Vulkan 要的驱动(Mesa 或厂商驱动)。
配置窗口的文件对话框走 XDG portal,KDE 上由 `xdg-desktop-portal-kde` 提供 —— Plasma 自带。

```sh
cargo build --release          # → target/release/rocom-pets(18.0MB)
```

`[profile.release]` 开了 fat LTO + `codegen-units = 1` + `strip`:产物 31.7MB → **18.0MB**,
代价是编译从 1m09s 涨到 2m46s。**去掉符号后崩溃回溯只剩地址** —— 要排查就用不带
`--release` 的 debug 档,那一档不受影响。

### 编译 rocom-pets.exe(Windows)

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
(`.cargo/config.toml` 里对这个目标开了 `+crt-static`),拷到 Windows 上双击即可,
除系统 DLL 外零依赖。体积在开 `[profile.release]` 的 LTO 之前量到约 19MB
(配置窗口那套 egui/winit 占了一多半,但换来的是不必再单独分发一个配置程序);
开了之后没在这台机器上重量过 —— 同一份改动让 Linux 的产物从 31.7MB 降到 18.0MB。
只想验代码能不能过编译器的话,`cargo check --target
x86_64-pc-windows-msvc` 就够(只要 std,连链接器都不用)。

**双击不会有黑窗口**(release 版按 GUI 子系统链接),但**从 cmd/PowerShell 里跑仍然有
日志** —— 启动时会挂回父进程的控制台。要看日志就 `set RUST_LOG=info` 再从命令行启动;
挂回去时 shell 已经回到提示符,日志会和提示符交错着刷,这是这类程序的通病。
debug 版(`cargo build` 不带 `--release`)保持控制台子系统。

宠物包不随 exe 走:把 Linux 上导好的包目录拷到 `%LOCALAPPDATA%\rocom-pets\packs\`,
或者用 `--packs-dir` 指过去;不给包也能起(调试精灵模式,用来验平台层)。

- 运行时(`src/`)：Rust + wgpu，自写平台窗口层(wlr-layer-shell / Windows `WS_EX_NOREDIRECTIONBITMAP` + DirectComposition)。
  两个后端都已跑通:平台层(透明置顶、轮廓命中、穿透开关)见 [docs/spike-s1.md](docs/spike-s1.md),
  骨骼动画 + toon 着色见 [docs/spike-s2.md](docs/spike-s2.md);
  行为、多实体、音频与配置窗口见 design.md §9 的 Phase 1–8。
- 导出器(`exporter/`)：C# + CUE4Parse，从自己的游戏 pak 生成宠物包;
  **一个图鉴号一个包**(`076-海盔虫.rkpet`,glb 含全部动作 + 贴图 + 叫声 + manifest.toml),
  归并规则与全量清单见 [docs/petindex.md](docs/petindex.md),结构见 [docs/spike-s3.md](docs/spike-s3.md)。
  `--index` 只列包名不碰 pak(和 `tools/petindex.py` 对账用);
  `--zip` 额外打一个 `.rkpet`、`--zip-only` 打完就删掉包目录,运行时两种都直接读。
  动画按**场景类别前缀**挑(`World_` 大世界 > `Common_` > … > `Ride_` 骑乘),
  `--probe-anim <资产>` 打印某只的重定向模式与各段动画的异常平移、`--probe-anim ALL`
  全库普查撞名 —— 待机取到骑乘那一版就是这么查出来的(design.md「待机取到了骑乘那一版动作」)。
  音频要 `vgmstream-cli` 与 `ffmpeg`(缺了自动跳过,`--no-audio` 显式关):
  叫声取 `Pet_Vo_<拼音>.bnk`、动作音效取 `Pet_Action_<拼音>.bnk`,两族库对同一批情绪
  各有一套且**内容不同**(包络相关只有 0.11~0.42,见 design.md §1)。
  全库 607 个形态里 **533 个有声音**(叫声 511、音效 529),音频合计 141MB。
- 下载站(`web/`)：应用本体与宠物包的下载页,整站在 Cloudflare 上 ——
  Workers 出页面并接管 `/api/*`、R2 存文件、D1 记下载与异常标记次数、KV 按 IP + 日期去重防刷。
  头像自己从解包数据拼(游戏自带的 `Icon/HeadIcon/<conf_id>.png`,按 id 直接对上
  manifest 里的形态,不引外部仓库的成品图),搜索认图鉴号、链首名与**包里任何一个形态名**。
  目录(`catalog.json`)由 `web/scripts/gen_catalog.py` 扫包目录生成 —— 算 sha256、
  读 manifest 取形态构成,和素材一样是生成物、不入仓库。部署见 [web/README.md](web/README.md)。
- 验证工具(`tools/`)：`verify_glb.py` 按 glTF 规范自采样 + 蒙皮 + 光栅化，渲图肉眼核对动画正确性;
  `sweep.py` 是**回归闸门** —— 全库每个形态渲一格,统计「失败 / 空白 / 过曝」三个数,
  改着色或改导出器之后跑一遍,三个数都不许变差;`cmp_shots.py` 拿实机截图对照渲图给出差距数字
  (抠图那步在 `gamemask.py`,取最大连通块)。三个都要素材,而素材不入仓库,
  路径见各自文件开头的说明。
- shader 逆向(`scripts/`)：cooked 包里材质图被剥了、只剩参数值与静态开关,而编译产物里公式是全的、
  静态开关也已定死。这批脚本把公式从 shader library 里读出来 ——
  Windows 端走 DXBC(`shaderdump.py` 取码、`dxbcdis.c` 反汇编、`dxbcsig.py` 对语义、
  `matshader.py` 认归属、`uniexpr.py` + `matparams.py` 把 cb 槽位对回参数名),
  安卓端走 GLSL 源码(`glsldump.py`,好读得多)。
  流水线与结论见 [docs/shader.md](docs/shader.md) 与 [docs/android-glsl.md](docs/android-glsl.md)。
  安卓那条路原本卡在**归属**(APK 里有 shader 却没有宠物资产,只能靠结构指纹猜);
  宠物资产在手机的**应用私有目录**里,`adb root` 取到之后归属变成精确哈希查表,
  见 [docs/android-device.md](docs/android-device.md)。

### 打包:目录或 `.rkpet`

包可以是**解开的目录**,也可以是导出器打出来的 `.rkpet`(zip;喵喵链 13.6MB → 7.1MB)。
运行时两种都直接读,不解压到临时目录 —— 包内相对路径拼在包的位置后面当「虚拟路径」用,
真读的时候由 `src/assets.rs` 判断要不要开归档(见那个模块的说明)。
包目录里两种可以混着放,`--list` 会在归档那几行标 `[rkpet]`。

```sh
dotnet run --project exporter -- --species 3001 --out packs --zip       # 目录 + .rkpet
dotnet run --project exporter -- --all --zip-only --skip-existing --out packs  # 全量,只留归档
```

**全量导出用 `--zip-only`**:`--zip` 会把包目录和归档**两份都留着**,全库就是
3.3GB + 2.0GB;`--zip-only` 打完即删源目录,只剩 2.0GB(25 条链抽样量的压缩比 0.61)。
`--skip-existing` **认得 `.rkpet`**,所以只留归档照样能分批续跑。

压缩级别用的是默认档,量过之后**没有调**:换 `SmallestSize` 只小 0.3% 而耗时多 57%;
把已经压过的 png/ogg 改成仅存储反而更大(deflate 还能从 PNG 里再挤出一点)。
体积的大头是 glb(全库 2008MB,占 63%),png 1121MB、ogg 31MB —— 真要再小得换
KTX2 贴图,那是另一件事(见 design.md 横向待办)。
归档必须是 **deflate 或 store**:运行时的 `zip` crate 只链了 `deflate-flate2`。

### 配置

配置在 `~/.config/rocom-pets/config.toml`(首次运行生成带注释模板),
**在场阵容存在同目录的 `roster.toml` 里**(每改一次就整份重写,所以没和手写的 config.toml
混在一起),下次启动自动恢复;给了 `--pack` 则只上这一只、不动存档。

**托盘菜单**只放菜单表达得了的东西 —— 文字、勾选、单选、子菜单、分隔线:

```
✓ 点击穿透 / ✓ 静音叫声 / 召回宠物
─────
帧率设置 ▸   20 / 30 / 60 帧每秒
大小倍率 ▸   50% / 100% / 150% / 自定义…
叫声音量 ▸   静音 / 30% / 60% / 100%
─────
首选项       ← 开配置窗口(落在「常用配置」页)
重新载入
退出
```

**菜单里没有滑块**,所以连续量(124%、37%)在这里降级成几个档位,精确值只在配置窗口里
存在;不在任何一档上时菜单**一个都不勾**,而不是硬勾一个最近的。加/撤宠物、切形态那些
要先列阵容再逐只展开的操作也不在托盘里 —— 菜单一深就没法用,顶层留一条「首选项」。
那三组档位**各自摆在顶层**而不是收进一个「常用配置」里:套一层的话调个音量要点两次
才看得见选项,而这三样正是最常调的。
在场只数不占菜单里的一行(那是条点不动的字),它在图标的悬停提示里。

「帧率设置」是**目标帧率**:台上在干什么都按它推进。这里曾经按姿势变化速度自动降频
(睡着的宠物落到 10Hz),取消了 —— 省下的那点 CPU 换来的是「什么时候降、降到多少」
全凭它自己判断,而帧率是用户看得见、也说得出偏好的东西。

**配置窗口**(`rocom-pets --settings`,或托盘里那两条)是一个独立进程,900×620,
左边是导航、右边是内容:

- **宠物包**:表格(名称写成整条进化链「喵喵 → 喵呜 → 魔力猫」、形态数、体积、
  `rkpet`/目录)、搜索、导入、上桌、删除。导入是**两个按钮**(「导入包…」选 `.rkpet`、
  「导入目录…」选解开的包目录)—— 原生文件对话框没有「文件和目录都行」这个模式。
  **没有文件拖放**:winit 0.30 的 Wayland 后端没实现它(x11 与 windows 后端有),
  与其在一个平台上能用、另一个平台上默默没反应,不如两边都只留这两个按钮;
- **活跃宠物**:侧栏逐只展开,每只可改形态、大小、性格、参与叫声(嗓音是个能打字的
  数值框,−100~100,旁边一个「重掷」)、记住上次落脚点;底下是这只的**动作表** —— 一格一个动作,
  这个形态没有的置灰,**点一下就在桌面上当场播一次**;
- **常用配置**:目标帧率、整体大小、叫声音量、启动就穿透。

大小与音量都是**滑杆 + 右边一个能直接打字的数值框**,两边盯着同一个值;
嗓音只有数值框(它没有「大概多大」这种直觉,滑杆帮不上忙)。
打进去超范围的数会自动夹回上下限。大小一律写成百分比(150% 而不是 1.50×)——
「1.50×」要在脑子里换算一次才知道是「大了五成」,而托盘里那三档本来就写着百分比。

**性格决定表情,不用手工勾**。规则是从解包数据里搬的:游戏的 `NATURE_CONF` 里每条性格
带一个 `emotion_desc`,31 条里只有 6 条不是「默认」—— 天真/开朗→微笑、懒散/悠闲→困倦、
胆小→哭哭、急躁→生气。**表情落在眼睛上**:眼睛与嘴各是一张 2×4 的表情图集
(`M_P_Eyes` 那族材质),网格 UV 落在左上那格,换表情就是整格地偏一下 UV。
八格逐个渲出来和三方攻略里那张「幽星光不同性格的眼睛」比对过,五种脸一一对上。
少数几只(21 片脸网格,乖乖鹄一家在内)是**另一种做法**:八种表情各做一份几何叠在一起,
卡号写在顶点色里,换表情 = 只画其中一张(见 design.md「网格脸」那节)。
**做动作的时候眼睛也跟着换**:生气时是生气眼、睡着是困倦眼、受惊是圆睁眼 ——
性格给的那张脸是它平时的样子。这张「动作 → 表情」的对照表是按语义挑的
(配置表里只有性格那一张,游戏那边换脸是行为逻辑直接设材质参数)。
性格还顺带定了它爱做哪几个表情动作(`LLM_PET_BEHAVIOR_CONF` 里 84 条行为各自标着
「哪几种性格会做」,反过来读)。游戏里 31 条性格,桌宠只留**七条**,按「脸 + 动静」
两条轴挑到不重复:五种脸各一个代表,默认脸那几条再留下差得最远的三个
(平和 = 基线、调皮 = 最闲不住、冷静 = 最能睡)。名字与 `nature_id` 都是游戏里的,
见 `src/persona.rs`。配置窗口的下拉框里**换脸的那几条连眼睛一起写**
(`胆小「哭哭眼」`),七条一屏排开、不用滚 —— 挑性格多半正是冲着那张脸去的。
默认脸的不写后缀:那是「没有变化」的一档,标出来反而看不见真正带脸的是哪几条。

**改什么都即时生效**,没有「保存」按钮:桌宠是看得见的,盯着屏幕就知道对不对。
顶上那条常驻:没改动时说「改动即时生效,不需要手动保存」,改过之后说「已修改 N 项」并提供**撤销**
(回到打开窗口时那一份)。**它一直在那儿、高度也不变** —— 改动出现时才冒出来的话,
底下整页会往下跳一截,而正在拖的那根滑杆就在这页上,手还按着。
唯一的例外是滑杆与那个数值框 —— 拖的时候只动数字,松手(或输入框提交)才落盘,
否则每帧都在重建宠物。

两个进程之间**只靠 `config.toml` + `roster.toml`** 对话,改完发一条 `Reload`;
桌宠没在跑的话改动下次启动照样生效。手改完文件想立刻生效就 `rocom-pets --reload`。
反方向只有一句话:托盘点「退出」(或 `rocom-pets --quit`)时,配置窗口也跟着关 ——
桌宠都没了,剩一个窗口对着不存在的宠物调大小没有意义。喊话用的就是配置窗口
占单实例的那个凭据(Linux 是 D-Bus 名字、Windows 是具名内核对象),不另起一套。

窗口里的中文字体是**从系统里找的**(Linux 问 fontconfig 要能写简中的那一份**与字面下标**,
Windows 找雅黑/黑体),不打进二进制:一份中文字体比整个运行时还大。

**没有内置的全局热键**:要快捷键就在系统里把自定义快捷键绑到
`rocom-pets --toggle-passthrough`(还有 `--recall` / `--reload` / `--quit`)。
键位归系统管,桌宠一个组合键都不抢,也就不会和别的程序打架 ——
原来那条 XDG GlobalShortcuts portal 的路(要桌面实现 portal、要用户点授权弹窗)去掉了。
配置里认不得的键(包括老版本留下的 `hotkey` / `hotkey_recall`)会**直接报错**而不是
被忽略 —— 拼错了要让人看见;删掉 config.toml 就会重新生成一份带注释的。

```sh
cargo run --release -- --pack packs/喵喵                    # 把宠物放到桌面上
rocom-pets --settings --page pets                          # 打开配置窗口(pets / packs / common)
rocom-pets --list                                          # 列出 ~/.local/share/rocom-pets/packs 里的包
rocom-pets --pack 喵喵                                      # 按包名启动(目录、.rkpet 路径也行)
rocom-pets --toggle-passthrough                            # 通知已在跑的实例(可绑快捷键)
rocom-pets --reload                                        # 手改完 config/roster 后让它重读
cargo run                                                  # 同上但用调试精灵(平台层验收模式)
cargo run --release -- --render packs/喵喵 --bench 600      # 离屏渲宠物 + 测出帧耗时
git -C "$CUE4PARSE_DIR" apply exporter/patches/*.patch      # 导出前必做:修上游法线与顶点色导出 bug
dotnet run --project exporter -- --species 3001 --out packs # 导一条进化链
dotnet run --project exporter -- --all --zip-only --skip-existing --out packs  # 全量导(可分批续跑)
python tools/verify_glb.py packs/喵喵 --clips Idle,Walk     # 渲图验证
uv run --with numpy --with pillow python tools/sweep.py    # 回归闸门:全库三个数不许变差
uv run --with lz4 python scripts/glsldump.py <安卓 shader 库> --index   # shader 逆向(见 docs/shader.md)
```

资产提取链路在 [rocom-capture](../rocom-capture) 里验证;音频那条(bnk → 事件 → wem)的
原理由 [rocom-petvo](../rocom-petvo) 先跑通,这里是照着原理自己实现的一份 ——
**不引它的代码,也不用它的成品资源**。

素材版权属原发行方：**本仓库只有代码与导出器，不包含也不分发任何游戏素材或生成的宠物包**，
需自备游戏安装在本地生成。运行时不读游戏内存、不注入进程、不联网。
