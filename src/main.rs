//! rocom-pets 运行时。
//!
//! 默认起 stage(每个显示器一个透明置顶表面,见 platform/);
//! `--render` 是离屏模式,不开窗口,把宠物渲成对比图用于验收与回归(见 offscreen.rs)。

mod act;
mod config;
mod control;
mod offscreen;
mod pack;
mod pack_list;
mod pet;
mod platform;
mod render;
mod roster;
mod sprite;
mod stage;

use std::path::PathBuf;

use anyhow::Context;

const USAGE: &str = "\
用法:
  rocom-pets --pack <包目录> [选项]        起 stage,把宠物放到桌面上
  rocom-pets                              起 stage,但用调试精灵(平台层验收模式)
  rocom-pets --render <包目录|glb> [选项]  离屏渲染宠物到 PNG

stage 模式(不给参数时读 ~/.config/rocom-pets/config.toml,首次运行会生成模板):
  --pack <目录>      宠物包目录(含 manifest.toml)
  --form <资产名>    选形态,默认包里第一个(链首)
  --px-per-cm <n>    每厘米多少逻辑像素(默认 2.0:80cm 的喵喵 → 160px 高)
  --config <文件>    换个配置文件
  --no-tray          不起托盘图标
  --passthrough      启动就开鼠标穿透

包管理:
  --list             列出包目录里的宠物包(默认 ~/.local/share/rocom-pets/packs)
  --packs-dir <目录> 换个包目录
  (--pack 既接受目录路径,也接受包名/物种名,后者在包目录里找)

在场阵容(同时上几只):
  托盘菜单里「加一只」/「撤下」,阵容存在 ~/.config/rocom-pets/roster.toml,
  下次启动自动恢复。给了 --pack 就只上这一只,不读也不动那份存档。

控制已在运行的实例(走 D-Bus,可绑到 KDE 自定义快捷键):
  --toggle-passthrough  切换鼠标穿透
  --recall              把宠物召回屏幕中间
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

fn main() -> anyhow::Result<()> {
    // zbus/tracing 的握手与派发日志是 INFO 级且极啰嗦(一次 D-Bus 调用刷十几行),
    // 一律压到 warn。注意不能只写进 default_filter:RUST_LOG 一设就把默认整条替换掉了,
    // 所以这里是在用户给的过滤器**后面**追加(用户显式点名 zbus/tracing 时不动)。
    let mut filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    for noisy in ["zbus", "tracing"] {
        if !filter.contains(noisy) {
            filter.push_str(&format!(",{noisy}=warn"));
        }
    }
    env_logger::Builder::new().parse_filters(&filter).init();

    let mut args = std::env::args().skip(1);
    let mut request: Option<offscreen::Request> = None;
    // 命令行先收集成 Option,最后再与配置文件合并(命令行优先)
    let mut config_path: Option<PathBuf> = None;
    let mut cli_form: Option<String> = None;
    let mut cli_px_per_cm: Option<f32> = None;
    let mut cli_passthrough = false;
    let mut no_tray = false;
    let mut cli_pack_name: Option<String> = None;
    let mut cli_packs_dir: Option<PathBuf> = None;
    let mut list_packs = false;
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
            "--px-per-cm" => cli_px_per_cm = Some(next("--px-per-cm", &mut args)?.parse()?),
            "--config" => config_path = Some(PathBuf::from(next("--config", &mut args)?)),
            "--no-tray" => no_tray = true,
            "--passthrough" => cli_passthrough = true,
            "--toggle-passthrough" => {
                return control::send_dbus_command(control::Control::TogglePassthrough);
            }
            "--recall" => return control::send_dbus_command(control::Control::Recall),
            "--quit" => return control::send_dbus_command(control::Control::Quit),
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

    // stage 模式:配置文件打底,命令行覆盖
    let path = config_path.or_else(config::Config::default_path);
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
        Some(name) => (
            vec![roster::Slot {
                pack: name,
                form: cli_form.clone(),
            }],
            true,
        ),
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
                        vec![roster::Slot {
                            pack,
                            form: cli_form.clone().or_else(|| file.form.clone()),
                        }]
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
        px_per_cm: cli_px_per_cm.unwrap_or(file.px_per_cm),
        passthrough: cli_passthrough || file.passthrough,
        tray: !no_tray,
        hotkey: file.hotkey,
    };
    platform::run(options)
}
