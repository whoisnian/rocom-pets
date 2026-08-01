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

# 全局热键(切换鼠标穿透)。走 XDG GlobalShortcuts portal 申请,
# KDE 会弹窗让你确认/改键;不支持的桌面上会自动跳过,用托盘菜单即可。
# 写空串 "" = 不要热键;**整行删掉不等于不要**,那是「用默认值」。
hotkey = "CTRL+ALT+p"

# 叫声音量 0~1。桌宠是常驻程序,默认小声;设成 0 就完全不开音频设备。
# 托盘里的「叫声」勾选是临时静音,不写回这里。
volume = 0.35
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
    #[serde(default = "default_hotkey")]
    pub hotkey: Option<String>,
    #[serde(default = "default_volume")]
    pub volume: f32,
}

/// 要写回配置文件的一个值。[`Config::write_back`] 的入参。
///
/// **没有「删掉这一项」**:删掉不等于关掉,而是「回到内置默认值」——
/// 想表达「不要热键」得写空串,见 [`Config::load_or_create`]。
#[derive(Debug, Clone, Copy)]
pub enum Setting<'a> {
    Num(f32),
    Flag(bool),
    Text(&'a str),
}

fn default_volume() -> f32 {
    crate::audio::DEFAULT_VOLUME
}

fn default_px_per_cm() -> f32 {
    2.0
}

fn default_hotkey() -> Option<String> {
    Some("CTRL+ALT+p".to_string())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pack: None,
            form: None,
            px_per_cm: default_px_per_cm(),
            passthrough: false,
            hotkey: default_hotkey(),
            volume: default_volume(),
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
                let mut config: Config =
                    toml::from_str(&text).with_context(|| format!("{path:?} 格式有误"))?;
                // **空串 = 明确不要热键**,与「这一项没写 = 用内置默认」区分开。
                // 少了这一条,配置窗口里把热键清空就没法保存 —— 删掉那一行只会让它
                // 落回默认的 CTRL+ALT+p(单元测试就是这么发现的)。
                if config.hotkey.as_deref().is_some_and(str::is_empty) {
                    config.hotkey = None;
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
    pub fn write_back(path: &Path, updates: &[(&str, Setting<'_>)]) -> Result<()> {
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
                // 模板里这些键多半是**注释掉的**(pack/form),写的时候要真加一行;
                // toml_edit 的 `doc[key] = value` 正好就是「有则改,无则加」
                Setting::Num(v) => doc[*key] = toml_edit::value(*v as f64),
                Setting::Flag(v) => doc[*key] = toml_edit::value(*v),
                Setting::Text(v) => doc[*key] = toml_edit::value(*v),
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
        if let Some(rest) = value.strip_prefix("~/") {
            if let Some(home) = std::env::var_os("HOME") {
                return PathBuf::from(home).join(rest);
            }
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
        assert_eq!(parsed.hotkey, default.hotkey);
        assert_eq!(parsed.volume, default.volume);
        assert!(parsed.pack.is_none() && parsed.form.is_none());
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
                // 模板里 pack 是注释掉的,写回时要真长出一行来
                ("pack", Setting::Text("喵喵")),
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
        assert_eq!(parsed.pack.as_deref(), Some("喵喵"));
        // 没点名的键原样不动
        assert_eq!(parsed.hotkey, default_hotkey());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_hotkey_means_none_but_a_missing_one_means_default() {
        // 这两件事不一样,而且**必须**不一样:配置窗口里把热键清空要存得住,
        // 而「这一项没写」的老配置该照常拿到默认键
        let dir = std::env::temp_dir().join(format!("rocom-hotkey-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("该能建目录");

        let cleared = dir.join("cleared.toml");
        std::fs::write(&cleared, "hotkey = \"\"\n").expect("该能写");
        assert_eq!(Config::load_or_create(&cleared).expect("该能读").hotkey, None);

        let missing = dir.join("missing.toml");
        std::fs::write(&missing, "px_per_cm = 2.0\n").expect("该能写");
        assert_eq!(
            Config::load_or_create(&missing).expect("该能读").hotkey,
            default_hotkey()
        );
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
