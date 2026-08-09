//! 配置文件:`~/.config/rocom-pets/config.toml`。
//!
//! 命令行参数优先于配置文件(调试时不用改文件),文件不存在则写一份带注释的默认配置——
//! 桌宠是常驻程序,总得有个不用每次敲参数的地方。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// 首次运行写出去的模板。带注释,免得用户去翻文档。
const TEMPLATE: &str = r#"# rocom-pets 配置。命令行参数会覆盖这里的值。

# 宠物包目录(含 manifest.toml)。不填就只显示调试精灵。
# 注意:这里只管「还没用过托盘」时上哪一只。一旦在托盘里加/撤过宠物,
# 在场阵容就存到同目录的 roster.toml 里,启动时优先按它恢复。
# pack = "~/.local/share/rocom-pets/packs/喵喵"

# 用哪个形态(资产名或中文名)。不填 = 包里第一个(链首)。
# form = "Gra_MiaoMiao1_001"

# 每厘米多少逻辑像素:宠物屏幕高度 = manifest 里的 height_cm × 这个值。
# 喵喵 80cm:2.0 → 160px,3.0 → 240px。
px_per_cm = 2.0

# 启动时就开鼠标穿透(宠物只显示、不接鼠标)。
passthrough = false

# 没有全局热键这一项:抢组合键这件事交给系统更合适 —— 在 KDE「自定义快捷键」里
# 把任意键绑到 `rocom-pets --toggle-passthrough`(或 --recall / --reload)即可。

# 叫声音量 0~1。桌宠是常驻程序,默认小声;设成 0 就完全不开音频设备。
# 托盘里的「叫声」勾选是临时静音,不写回这里。
volume = 0.30

# 目标帧率:台上在干什么都按这个推进(没有「没动就降频」那回事)。
# 托盘里给 20 / 30 / 60 三档;手写别的值也认,10~240 之外会被拉回来。
fps = 30
"#;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub pack: Option<String>,
    #[serde(default)]
    pub form: Option<String>,
    #[serde(default = "default_px_per_cm")]
    pub px_per_cm: f32,
    #[serde(default)]
    pub passthrough: bool,
    #[serde(default = "default_volume")]
    pub volume: f32,
    /// 目标帧率。台上在干什么都按这个推进(见 `Stage::tick_interval`)。
    #[serde(default = "default_fps")]
    pub fps: u32,
}

/// 要写回配置文件的一个值。[`Config::write_back`] 的入参。
///
/// **没有「删掉这一项」**:删掉不等于关掉,而是「回到内置默认值」。
#[derive(Debug, Clone, Copy)]
pub enum Setting {
    Num(f32),
    /// 整数(帧率)。**和 `Num` 分开**:走 `Num` 会写成 `fps = 30.0`,再读回来时
    /// serde 要的是 u32,直接解析失败 —— 一次写回就把配置文件弄成读不了的。
    Int(u32),
    Flag(bool),
}

fn default_volume() -> f32 {
    crate::audio::DEFAULT_VOLUME
}

/// 默认目标帧率。30 就是原来写死在 stage.rs 里的那个值。
pub const DEFAULT_FPS: u32 = 30;

/// 手写的 `fps` 会被拉回这个区间。低于 10 就是在看幻灯片,高于 240 只是白烧 CPU。
pub const FPS_RANGE: std::ops::RangeInclusive<u32> = 10..=240;

fn default_fps() -> u32 {
    DEFAULT_FPS
}

fn default_px_per_cm() -> f32 {
    2.0
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pack: None,
            form: None,
            px_per_cm: default_px_per_cm(),
            passthrough: false,
            volume: default_volume(),
            fps: default_fps(),
        }
    }
}

impl Config {
    /// 默认路径:Linux 是 `$XDG_CONFIG_HOME/rocom-pets/config.toml`,
    /// Windows 是 `%APPDATA%\rocom-pets\config.toml`。
    ///
    /// **Windows 上没有 `HOME`/`XDG_*`**(是 `USERPROFILE`/`APPDATA`),照 XDG 那套找会
    /// 一个都找不到 —— 实测第一版在 wine 里直接「定不出配置文件位置」,配置与阵容全丢。
    pub fn default_path() -> Option<PathBuf> {
        Some(config_dir()?.join("rocom-pets").join("config.toml"))
    }

    /// 读配置;文件不存在就写一份模板并返回默认值。读失败(格式错)则报错——
    /// 配置写错了要让人看见,不该静默用默认值跑。
    pub fn load_or_create(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                // 认不得的键也算错(拼错了要让人看见)。**老版本留下的键会撞在这儿** ——
                // 删掉这个文件就会重新生成一份带注释的默认配置。
                let mut config: Config = toml::from_str(&text)
                    .with_context(|| format!("{path:?} 格式有误(删掉它会重新生成一份)"))?;
                // 帧率是手写得进去的,而 0 会让 `1.0 / fps` 变成无穷大的定时器间隔
                let clamped = config.fps.clamp(*FPS_RANGE.start(), *FPS_RANGE.end());
                if clamped != config.fps {
                    log::warn!("fps = {} 超出 {FPS_RANGE:?},按 {clamped} 用", config.fps);
                    config.fps = clamped;
                }
                log::info!("读配置 {}", path.display());
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                match std::fs::write(path, TEMPLATE) {
                    Ok(()) => log::info!("已生成默认配置 {}", path.display()),
                    Err(e) => log::warn!("写默认配置失败({e}),继续用内置默认值"),
                }
                Ok(Self::default())
            }
            Err(e) => Err(e).with_context(|| format!("读不了 {path:?}")),
        }
    }

    /// 把几项写回 config.toml,**保住注释与排版**。
    ///
    /// 这份文件是手写的、带一整篇说明,而托盘与配置窗口现在也要改它 ——
    /// 用 `toml::to_string` 重新序列化一遍会把注释全抹掉。所以走 `toml_edit`:
    /// 它保留原文,只替换被点名的那几个键。
    ///
    /// 文件不在就从模板起头(于是新生成的那份也带注释)。**失败只警告不报错**:
    /// 托盘里调个音量不该让桌宠崩掉,值在内存里已经生效了,顶多下次启动回到旧值。
    /// 只有桌面版会写回配置(浏览器里没有 config.toml,也没有 toml_edit)。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn write_back(path: &Path, updates: &[(&str, Setting)]) -> Result<()> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => TEMPLATE.to_string(),
            Err(e) => return Err(e).with_context(|| format!("读不了 {path:?}")),
        };
        let mut doc = text
            .parse::<toml_edit::DocumentMut>()
            .with_context(|| format!("{path:?} 格式有误,不敢改"))?;
        for (key, setting) in updates {
            match setting {
                // 老配置里可能压根没有这个键(比如 `fps` 是后加的),
                // toml_edit 的 `doc[key] = value` 正好就是「有则改,无则加」
                Setting::Num(v) => doc[*key] = toml_edit::value(*v as f64),
                Setting::Int(v) => doc[*key] = toml_edit::value(i64::from(*v)),
                Setting::Flag(v) => doc[*key] = toml_edit::value(*v),
            }
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(path, doc.to_string()).with_context(|| format!("写不了 {path:?}"))?;
        log::debug!("配置已写回 {}", path.display());
        Ok(())
    }

    /// 展开开头的 `~`(配置文件里手写路径时常用)。
    pub fn expand_path(value: &str) -> PathBuf {
        if let Some(rest) = value.strip_prefix("~/")
            && let Some(home) = std::env::var_os("HOME")
        {
            return PathBuf::from(home).join(rest);
        }
        PathBuf::from(value)
    }
}

/// 放配置的目录(不含 `rocom-pets` 那一层)。
#[cfg(not(target_os = "windows"))]
fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

#[cfg(target_os = "windows")]
fn config_dir() -> Option<PathBuf> {
    // 漫游配置(小、跟着用户走);拿不到就退回用户目录
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_into_defaults() {
        // 模板本身必须是合法配置,而且解析结果要与内置默认值一致,
        // 否则「照模板改」和「不写配置」两条路会给出不同行为
        let parsed: Config = toml::from_str(TEMPLATE).expect("模板该是合法 TOML");
        let default = Config::default();
        assert_eq!(parsed.px_per_cm, default.px_per_cm);
        assert_eq!(parsed.passthrough, default.passthrough);
        assert_eq!(parsed.volume, default.volume);
        assert_eq!(parsed.fps, default.fps);
        assert!(parsed.pack.is_none() && parsed.form.is_none());
    }

    #[test]
    fn the_frame_rate_survives_a_write_back() {
        // `Setting::Int` 存在的唯一理由:写成 30.0 的话下次 `load_or_create` 直接报格式错
        let dir = std::env::temp_dir().join(format!("rocom-fps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("该能建目录");
        let path = dir.join("config.toml");
        std::fs::write(&path, TEMPLATE).expect("该能写");

        Config::write_back(&path, &[("fps", Setting::Int(60))]).expect("该能写回");
        let text = std::fs::read_to_string(&path).expect("该能读回");
        assert!(text.contains("fps = 60"), "写成了别的样子:{text}");
        assert_eq!(Config::load_or_create(&path).expect("该还能读").fps, 60);

        // 手写的怪值要被拉回区间,而不是让定时器间隔变成无穷大
        std::fs::write(&path, "fps = 0\n").expect("该能写");
        assert_eq!(
            Config::load_or_create(&path).expect("该能读").fps,
            *FPS_RANGE.start()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        // 拼错的键要报错而不是被忽略,否则用户改了半天没生效还不知道为什么
        assert!(toml::from_str::<Config>("px_per_com = 2.0").is_err());
    }

    #[test]
    fn write_back_keeps_the_comments() {
        // 这是这条路存在的**唯一**理由:重新序列化一遍会把整篇说明抹掉
        let dir = std::env::temp_dir().join(format!("rocom-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("该能建目录");
        let path = dir.join("config.toml");
        std::fs::write(&path, TEMPLATE).expect("该能写");

        Config::write_back(
            &path,
            &[
                ("px_per_cm", Setting::Num(3.0)),
                ("volume", Setting::Num(0.6)),
                ("passthrough", Setting::Flag(true)),
            ],
        )
        .expect("该能写回");

        let text = std::fs::read_to_string(&path).expect("该能读回");
        assert!(text.contains("# rocom-pets 配置"), "注释头没了:{text}");
        assert!(text.contains("# 每厘米多少逻辑像素"), "键旁边的说明没了");
        let parsed = Config::load_or_create(&path).expect("写回的东西必须还能读");
        assert_eq!(parsed.px_per_cm, 3.0);
        assert_eq!(parsed.volume, 0.6);
        assert!(parsed.passthrough);
        // 没点名的键原样不动
        assert_eq!(parsed.fps, DEFAULT_FPS);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 没写过某个键的配置:读的时候用默认值,写回时把它**加出来**。
    /// (`fps` 就是后加的键,老配置里没有那一行。)
    #[test]
    fn a_missing_key_reads_as_default_and_gets_written_in() {
        let dir = std::env::temp_dir().join(format!("rocom-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("该能建目录");
        let path = dir.join("config.toml");
        std::fs::write(&path, "px_per_cm = 1.5\nvolume = 1.0\n").expect("该能写");

        let parsed = Config::load_or_create(&path).expect("该能读");
        assert_eq!(parsed.px_per_cm, 1.5);
        assert_eq!(parsed.fps, DEFAULT_FPS, "没写 fps 就用默认值");

        Config::write_back(&path, &[("fps", Setting::Int(60))]).expect("该能写回");
        let text = std::fs::read_to_string(&path).expect("该能读回");
        assert!(text.contains("fps = 60"), "新键该被加出来:{text}");
        assert!(text.contains("volume = 1.0"), "没点名的键不该被动");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tilde_is_expanded() {
        unsafe { std::env::set_var("HOME", "/home/tester") };
        assert_eq!(
            Config::expand_path("~/packs/x"),
            PathBuf::from("/home/tester/packs/x")
        );
        assert_eq!(Config::expand_path("/abs/x"), PathBuf::from("/abs/x"));
    }
}
