//! rocom-pets 运行时。
//!
//! 默认起 stage(每个显示器一个透明置顶表面,见 platform/);
//! `--render` 是离屏模式,不开窗口,把宠物渲成对比图用于验收与回归(见 offscreen.rs)。

// 双击不弹黑窗口:release 版在 Windows 上按 **GUI 子系统**链接。
// 从命令行跑时仍然要有日志 —— 见 `attach_parent_console`。
// debug 版保持控制台子系统:开发时日志比「没有黑窗口」重要得多。
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod act;
mod assets;
mod audio;
mod config;
mod control;
mod offscreen;
mod pack;
mod pack_list;
mod persona;
mod pet;
mod platform;
mod render;
mod roster;
mod settings;
mod sprite;
mod stage;

use std::path::PathBuf;

use anyhow::Context;

const USAGE: &str = "\
用法:
  rocom-pets --pack <包目录> [选项]        起 stage,把宠物放到桌面上
  rocom-pets                              起 stage,但用调试精灵(平台层验收模式)
  rocom-pets --render <包目录|glb> [选项]  离屏渲染宠物到 PNG

stage 模式(不给参数时读配置文件,首次运行会生成模板;
          Linux 在 ~/.config/rocom-pets/,Windows 在 %APPDATA%/rocom-pets/):
  --pack <目录>      宠物包目录(含 manifest.toml)
  --form <资产名>    选形态,默认包里第一个(链首)
  --px-per-cm <n>    每厘米多少逻辑像素(默认 2.0:80cm 的喵喵 → 160px 高)
  --config <文件>    换个配置文件
  --volume <0..1>    叫声音量(默认 0.35;0 = 不开音频)
  --no-tray          不起托盘图标
  --passthrough      启动就开鼠标穿透

配置窗口:
  --settings         打开配置窗口(宠物包管理 / 活跃宠物 / 常用配置)
                     托盘菜单里也能开;改完点「保存并应用」,在跑的桌宠会立刻跟着变

包管理:
  --list             列出包目录里的宠物包(默认 ~/.local/share/rocom-pets/packs)
  --packs-dir <目录> 换个包目录
  (--pack 接受包目录、`.rkpet` 文件,也接受包名/物种名,后者在包目录里找)

在场阵容(同时上几只):
  托盘菜单里「加一只」/「撤下」,阵容存在配置同目录的 roster.toml,
  下次启动自动恢复。给了 --pack 就只上这一只,不读也不动那份存档。
  (Windows 的托盘还没有加/撤菜单,只能手改 roster.toml 重启)

控制已在运行的实例(走 D-Bus,可绑到 KDE 自定义快捷键):
  --toggle-passthrough  切换鼠标穿透
  --recall              把宠物召回屏幕中间
  --reload              重读配置与阵容存档(手改完那两份文件后不必重启)
  --settings-window     让它开一个配置窗口
  --quit                让它退出

  --render <路径>    宠物包目录(含 forms/)或直接给 model.glb
  --form <资产名>    选形态,默认包里第一个
  --clips a,b,c      要渲的动作(默认 Idle,Walk,Happy,SleepLoop)
  --at <0..1>        采样时刻占动作时长的比例(默认 0.4)
  --time <秒>        喂给 shader 的时间(默认 0);看火焰流动、球内星点闪烁用
  --size <px>        每格边长(默认 320)
  --yaw <度>         观察角,0 = 正面(宠物朝 +Z)
  --no-fade          不额外渲「淡化中点」那一格
  --bench <帧数>     跑这么多帧测平均出帧耗时
  -o, --out <文件>   输出 PNG(默认 pet-render.png)
  -h, --help         本帮助
";

/// 从命令行启动时,把标准输出挂回父进程的控制台。
///
/// GUI 子系统的代价是标准句柄全是空的 —— 从 cmd 里跑也看不到任何日志,而这个后端
/// 还在磨合期,日志是唯一的排查手段。`AttachConsole(ATTACH_PARENT_PROCESS)` 能挂回去,
/// 但**必须自己把 `CONOUT$` 设成标准句柄**:AttachConsole 不替进程改这几个句柄。
///
/// 双击启动时没有父控制台,attach 会失败,那就什么都不做(也就不会凭空弹一个窗口)。
/// 已知小别扭:挂回去时 shell 早已回到提示符,日志会和提示符交错着刷 —— 这是所有
/// 「GUI 子系统 + 附着父控制台」的程序共有的,没法避免。
#[cfg(target_os = "windows")]
fn attach_parent_console() {
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    };
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };
    use windows::core::w;

    // SAFETY: 全是标准的「挂回父控制台」流程;失败一律当作「没有控制台」静默跳过。
    // **必须在任何输出之前调**:Rust 的 stdout 会缓存第一次拿到的句柄。
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            return;
        }
        let Ok(console) = CreateFileW(
            w!("CONOUT$"),
            (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            Default::default(),
            None,
        ) else {
            return;
        };
        let _ = SetStdHandle(STD_OUTPUT_HANDLE, console);
        let _ = SetStdHandle(STD_ERROR_HANDLE, console);
    }
}

fn main() -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    attach_parent_console();

    let args: Vec<String> = std::env::args().skip(1).collect();

    // zbus/tracing 的握手与派发日志是 INFO 级且极啰嗦(一次 D-Bus 调用刷十几行),
    // 一律压到 warn。注意不能只写进 default_filter:RUST_LOG 一设就把默认整条替换掉了,
    // 所以这里是在用户给的过滤器**后面**追加(用户显式点名 zbus/tracing 时不动)。
    //
    // 配置窗口那边再压一档到 error:它上会话总线**只为占一个名字**(单实例锁),
    // 而 zbus 一看见「要了名字却没挂对象」就警告一句 —— 对我们这个用法不成立,
    // 但每开一次窗口就刷一行,看着像出了事。
    let quiet_zbus = args.iter().any(|a| a == "--settings");
    let mut filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    for (noisy, level) in [
        ("zbus", if quiet_zbus { "error" } else { "warn" }),
        ("tracing", "warn"),
    ] {
        if !filter.contains(noisy) {
            filter.push_str(&format!(",{noisy}={level}"));
        }
    }
    env_logger::Builder::new().parse_filters(&filter).init();

    let mut args = args.into_iter();
    let mut request: Option<offscreen::Request> = None;
    // 命令行先收集成 Option,最后再与配置文件合并(命令行优先)
    let mut config_path: Option<PathBuf> = None;
    let mut cli_form: Option<String> = None;
    let mut cli_px_per_cm: Option<f32> = None;
    let mut cli_volume: Option<f32> = None;
    let mut cli_passthrough = false;
    let mut no_tray = false;
    let mut cli_pack_name: Option<String> = None;
    let mut cli_packs_dir: Option<PathBuf> = None;
    let mut list_packs = false;
    let mut open_settings = false;
    let next = |flag: &str, args: &mut dyn Iterator<Item = String>| -> anyhow::Result<String> {
        args.next()
            .ok_or_else(|| anyhow::anyhow!("{flag} 缺少参数值\n{USAGE}"))
    };

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            "--render" => {
                let pack = PathBuf::from(next("--render", &mut args)?);
                request = Some(offscreen::Request {
                    pack,
                    form: None,
                    clips: ["Idle", "Walk", "Happy", "SleepLoop"]
                        .map(String::from)
                        .to_vec(),
                    at: 0.4,
                    time: 0.0,
                    size: 320,
                    yaw_degrees: 0.0,
                    out: PathBuf::from("pet-render.png"),
                    fade_probe: true,
                    bench: 0,
                });
            }
            // --pack 可以是路径也可以是包名,到下面统一解析
            "--pack" => cli_pack_name = Some(next("--pack", &mut args)?),
            "--packs-dir" => cli_packs_dir = Some(PathBuf::from(next("--packs-dir", &mut args)?)),
            "--list" => list_packs = true,
            "--settings" => open_settings = true,
            "--px-per-cm" => cli_px_per_cm = Some(next("--px-per-cm", &mut args)?.parse()?),
            "--volume" => cli_volume = Some(next("--volume", &mut args)?.parse()?),
            "--config" => config_path = Some(PathBuf::from(next("--config", &mut args)?)),
            "--no-tray" => no_tray = true,
            "--passthrough" => cli_passthrough = true,
            "--toggle-passthrough" => {
                return control::send_command(control::Control::TogglePassthrough);
            }
            "--recall" => return control::send_command(control::Control::Recall),
            "--reload" => return control::send_command(control::Control::Reload),
            "--settings-window" => return control::send_command(control::Control::OpenSettings),
            "--quit" => return control::send_command(control::Control::Quit),
            // --form 两个模式都用得到
            "--form" if request.is_none() => cli_form = Some(next("--form", &mut args)?),
            other => {
                let Some(request) = request.as_mut() else {
                    anyhow::bail!("{other} 只在 --render 模式下有意义\n{USAGE}");
                };
                match other {
                    "--form" => request.form = Some(next("--form", &mut args)?),
                    "--clips" => {
                        request.clips = next("--clips", &mut args)?
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                    "--at" => request.at = next("--at", &mut args)?.parse()?,
                    "--size" => request.size = next("--size", &mut args)?.parse()?,
                    "--yaw" => request.yaw_degrees = next("--yaw", &mut args)?.parse()?,
                    "--time" => request.time = next("--time", &mut args)?.parse()?,
                    "--no-fade" => request.fade_probe = false,
                    "--bench" => request.bench = next("--bench", &mut args)?.parse()?,
                    "-o" | "--out" => request.out = PathBuf::from(next("--out", &mut args)?),
                    unknown => anyhow::bail!("未知参数 {unknown}\n{USAGE}"),
                }
            }
        }
    }

    if let Some(request) = request {
        return offscreen::render(&request);
    }

    let packs_dir = cli_packs_dir.or_else(pack::Pack::default_dir);

    if list_packs {
        return pack_list::run(packs_dir.as_deref());
    }

    // 配置文件位置:**配置窗口与桌宠必须按同一条规则定**,否则两边看的不是同一份文件
    let path = config_path.or_else(config::Config::default_path);

    if open_settings {
        return settings::run(path, packs_dir);
    }

    // stage 模式:配置文件打底,命令行覆盖
    let file = match &path {
        Some(path) => config::Config::load_or_create(path)?,
        None => {
            log::warn!("定不出配置文件位置(HOME/XDG_CONFIG_HOME 都没有),用内置默认值");
            config::Config::default()
        }
    };
    // 阵容来源,三选一(命令行 > 阵容存档 > 配置):
    // 命令行点名了就只上这一只(调试时要的就是「只看这只」);没点名才恢复上次的阵容;
    // 都没有就用配置里那只单宠 —— 这条是给还没碰过托盘的用户留的老路。
    let roster_path = path.as_deref().map(roster::Roster::path_beside);
    // `from_user`:这份名单是用户当场写的(命令行/配置)还是程序自己存的(阵容存档)。
    // 读不动时的处理完全不同,见下面。
    let (slots, from_user) = match cli_pack_name {
        Some(name) => (vec![roster::Slot::new(name, cli_form.clone())], true),
        None => match roster_path
            .as_deref()
            .and_then(roster::Roster::load)
            .filter(|r| !r.pets.is_empty())
        {
            Some(saved) => (saved.pets, false),
            None => (
                file.pack
                    .clone()
                    .map(|pack| {
                        vec![roster::Slot::new(
                            pack,
                            cli_form.clone().or_else(|| file.form.clone()),
                        )]
                    })
                    .unwrap_or_default(),
                true,
            ),
        },
    };

    // 包在起窗口前就读掉:manifest 有问题要立刻报错,而不是等到画第一帧。
    // **只有存档里那些**读不动才降级成警告 —— 它是机器写的,某个包被删了不该拦住启动;
    // 命令行/配置里点名的读不动就是硬错误,用户写的东西不生效必须让他看见。
    let mut pets = Vec::new();
    for slot in slots {
        match pack::Pack::resolve(&slot.pack, packs_dir.as_deref()) {
            Ok(pack) => pets.push(platform::StartupPet {
                pack,
                options: platform::PetOptions::from_slot(&slot),
                form: slot.form,
            }),
            Err(e) if from_user => {
                return Err(e).with_context(|| format!("解析宠物包 {} 失败", slot.pack));
            }
            Err(e) => log::warn!("阵容里的 {} 上不了台({e:#}),跳过", slot.pack),
        }
    }

    let options = platform::Options {
        pets,
        packs_dir,
        roster_path,
        config_path: path,
        px_per_cm: cli_px_per_cm.unwrap_or(file.px_per_cm),
        passthrough: cli_passthrough || file.passthrough,
        tray: !no_tray,
        hotkey: file.hotkey,
        volume: cli_volume.unwrap_or(file.volume).clamp(0.0, 1.0),
    };
    platform::run(options)
}
