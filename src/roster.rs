//! 在场阵容的存档:`~/.config/rocom-pets/roster.toml`。
//!
//! **为什么不塞进 config.toml**:那份是手写的、带注释的,而这份托盘每改一次就要机器重写
//! 一次 —— 序列化一遍下来注释全没了。两份文件各自单一来源:config.toml 归用户,
//! roster.toml 归程序。
//!
//! 坏了不报错只警告(这点和 config.rs 相反):用户没手写过它,它坏了不该拦住桌宠启动。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// 写在文件开头的说明。手改的人得知道下一次托盘操作会整份覆盖。
const HEADER: &str = "\
# rocom-pets 的在场阵容 —— 托盘或配置窗口里改动时自动重写。
# 手改也认,但下次改动会整份覆盖(注释保不住,这就是它没和 config.toml 放一起的原因)。
# pack 写包名、包目录名、`.rkpet` 文件名或路径;form 写形态资产名,不写 = 链首。
# scale 相对大小(1.0 = 按 config.toml 的 px_per_cm);persona 性格;emotes 表情池。
# 后三项不写就是默认(1.0 / 乖巧 / 全部表情)。

";

/// 阵容里的一只。
///
/// 除 `pack` 外全是 `Option`,而且**默认值一律不写出去**:存档要能一眼看出
/// 「哪几只是特意调过的」,而不是每只都糊上一整套等于默认的字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    pub pack: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<String>,
    /// 相对大小倍数(1.0 = 按配置里的 px_per_cm 算出来的尺寸)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f32>,
    /// 性格 id,见 persona.rs。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
    /// 允许的表情(stage.rs 的 `EMOTES` 里的动作名);不写 = 全部。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emotes: Option<Vec<String>>,
}

impl Slot {
    /// 只知道包名时的那一只(其余全默认)。
    pub fn new(pack: String, form: Option<String>) -> Self {
        Self {
            pack,
            form,
            scale: None,
            persona: None,
            emotes: None,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Roster {
    /// 文件里是 `[[pet]]`(一只一段,读起来比 `pets = [...]` 直观)。
    #[serde(default, rename = "pet")]
    pub pets: Vec<Slot>,
}

impl Roster {
    /// 阵容存档与配置文件同目录 —— `--config` 换到别处时阵容跟着走,
    /// 否则调试用的配置会去改真实阵容。
    pub fn path_beside(config_path: &Path) -> PathBuf {
        match config_path.parent() {
            Some(dir) => dir.join("roster.toml"),
            None => PathBuf::from("roster.toml"),
        }
    }

    /// 读阵容。文件不在 = 没存过(返回 None,由调用方退回配置里的单只);
    /// 读坏了也只警告 —— 见模块头。
    pub fn load(path: &Path) -> Option<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str::<Self>(&text) {
                Ok(roster) => {
                    log::info!("读阵容 {}({} 只)", path.display(), roster.pets.len());
                    Some(roster)
                }
                Err(e) => {
                    log::warn!("{} 解析失败({e}),这次按配置里的宠物起", path.display());
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                log::warn!("读不了 {}({e})", path.display());
                None
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let body = toml::to_string_pretty(self).context("阵容序列化失败")?;
        std::fs::write(path, format!("{HEADER}{body}"))
            .with_context(|| format!("写不了 {path:?}"))?;
        log::debug!("阵容已存({} 只)→ {}", self.pets.len(), path.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let roster = Roster {
            pets: vec![
                Slot {
                    pack: "喵喵".into(),
                    form: Some("Gra_MiaoMiao2_001".into()),
                    scale: Some(1.4),
                    persona: Some("lively".into()),
                    emotes: Some(vec!["Happy".into(), "Show".into()]),
                },
                Slot::new("/abs/path/波波拉".into(), None),
            ],
        };
        let text = toml::to_string_pretty(&roster).expect("该能序列化");
        assert_eq!(toml::from_str::<Roster>(&text).expect("该能读回"), roster);
        // 不写 form 的那只不该多出一行 `form = ""`,否则读回来就成了「找形态 ""」
        assert_eq!(text.matches("form").count(), 1, "{text}");
        // 全默认的那只除了 pack 什么都不该写:存档要能一眼看出谁被调过
        assert_eq!(text.matches("scale").count(), 1, "{text}");
        assert_eq!(text.matches("persona").count(), 1, "{text}");
    }

    #[test]
    fn old_rosters_still_load() {
        // 加选项之前存的档只有 pack/form,读回来必须是「全默认」而不是报错
        let roster: Roster = toml::from_str(
            "[[pet]]\npack = \"喵喵\"\n\n[[pet]]\npack = \"幽星光\"\nform = \"x\"\n",
        )
        .expect("旧存档该能读");
        assert_eq!(roster.pets.len(), 2);
        assert_eq!(roster.pets[0], Slot::new("喵喵".into(), None));
        assert_eq!(roster.pets[1].scale, None);
        assert_eq!(roster.pets[1].persona, None);
        assert_eq!(roster.pets[1].emotes, None);
    }

    #[test]
    fn empty_file_is_an_empty_roster() {
        // 撤掉最后一只之后存出来的就是空文件,读回来必须是「台上没有」而不是报错
        assert_eq!(
            toml::from_str::<Roster>("").expect("空文件合法"),
            Roster::default()
        );
    }

    #[test]
    fn header_survives_a_reread() {
        // 存档带注释头,读的时候不能被它噎住
        let dir = std::env::temp_dir().join(format!("rocom-roster-{}", std::process::id()));
        let path = dir.join("roster.toml");
        let roster = Roster {
            pets: vec![Slot::new("喵喵".into(), None)],
        };
        roster.save(&path).expect("该能写");
        assert_eq!(Roster::load(&path), Some(roster));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
