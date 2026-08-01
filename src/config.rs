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
                let config: Config =
                    toml::from_str(&text).with_context(|| format!("{path:?} 格式有误"))?;
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
    fn tilde_is_expanded() {
        unsafe { std::env::set_var("HOME", "/home/tester") };
        assert_eq!(
            Config::expand_path("~/packs/x"),
            PathBuf::from("/home/tester/packs/x")
        );
        assert_eq!(Config::expand_path("/abs/x"), PathBuf::from("/abs/x"));
    }
}
