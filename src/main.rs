//! rocom-pets 运行时。
//!
//! 默认起 stage(每个显示器一个透明置顶表面,见 platform/);
//! `--render` 是离屏模式,不开窗口,把宠物渲成对比图用于验收与回归(见 offscreen.rs)。

mod offscreen;
mod pet;
mod platform;
mod render;
mod sprite;
mod stage;

use std::path::PathBuf;

const USAGE: &str = "\
用法:
  rocom-pets                              起 stage(KDE Plasma Wayland / Windows)
  rocom-pets --render <包目录|glb> [选项]  离屏渲染宠物到 PNG

  --render <路径>    宠物包目录(含 forms/)或直接给 model.glb
  --form <资产名>    选形态,默认包里第一个
  --clips a,b,c      要渲的动作(默认 Idle,Walk,Happy,SleepLoop)
  --at <0..1>        采样时刻占动作时长的比例(默认 0.4)
  --size <px>        每格边长(默认 320)
  --yaw <度>         观察角,0 = 正面(宠物朝 +Z)
  --no-fade          不额外渲「淡化中点」那一格
  --bench <帧数>     跑这么多帧测平均出帧耗时
  -o, --out <文件>   输出 PNG(默认 pet-render.png)
  -h, --help         本帮助
";

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut args = std::env::args().skip(1);
    let mut request: Option<offscreen::Request> = None;
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
                    size: 320,
                    yaw_degrees: 0.0,
                    out: PathBuf::from("pet-render.png"),
                    fade_probe: true,
                    bench: 0,
                });
            }
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
                    "--no-fade" => request.fade_probe = false,
                    "--bench" => request.bench = next("--bench", &mut args)?.parse()?,
                    "-o" | "--out" => request.out = PathBuf::from(next("--out", &mut args)?),
                    unknown => anyhow::bail!("未知参数 {unknown}\n{USAGE}"),
                }
            }
        }
    }

    match request {
        Some(request) => offscreen::render(&request),
        None => platform::run(),
    }
}
