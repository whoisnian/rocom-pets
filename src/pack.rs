//! 读宠物包的 manifest.toml。
//!
//! manifest 是导出器与运行时之间唯一的契约(schema 见 docs/design.md §4.3),
//! 运行时只认里面的**逻辑动作名**与形态元数据,不关心资产原名。
//! 缺字段就按默认值降级——包是本地生成物,宁可少个动作也不该整只加载不出来。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// 运行时能读的 manifest 版本;更高的版本直接拒。
const SUPPORTED_SCHEMA: u32 = 1;

#[derive(Deserialize)]
struct RawManifest {
    schema: u32,
    /// 导出时的 pak 指纹;只用于日志/排查,不参与逻辑。
    #[serde(default)]
    source_version: Option<String>,
    species: RawSpecies,
    #[serde(default)]
    forms: Vec<RawForm>,
}

#[derive(Deserialize)]
struct RawSpecies {
    id: i64,
    name: String,
}

#[derive(Deserialize)]
struct RawForm {
    id: i64,
    name: String,
    #[serde(default)]
    stage: i64,
    asset: String,
    model: String,
    #[serde(default = "one")]
    scale: f32,
    #[serde(default)]
    height_cm: f32,
    #[serde(default)]
    locomotion: String,
    #[serde(default)]
    clips: HashMap<String, RawClip>,
    #[serde(default)]
    materials: HashMap<String, RawMaterial>,
}

/// `[forms.materials]` 一条:导出器从游戏材质实例里解出来的「这个槽该画什么」。
#[derive(Deserialize)]
struct RawMaterial {
    /// 基色贴图的包内相对路径。**缺失 = 纯特效层**(火焰/水壳/光晕:材质里没有
    /// BaseTex/EyeTex,固有色是 shader 算的),运行时整片跳过。
    #[serde(default)]
    base_color: Option<String>,
    /// 贴图 alpha 是不是真遮罩。眼/嘴的表情图集是(不剔就是一块方糊),
    /// 本体贴图不是(它的 alpha 是美术塞的遮罩通道,拿来剔会把身体啃掉)。
    #[serde(default)]
    mask_alpha: bool,
}

#[derive(Deserialize)]
struct RawClip {
    #[serde(default)]
    ms: u32,
    /// 走跑类动作:动画自带的位移换算出的速度(cm/s);0 表示原地循环。
    #[serde(default)]
    speed_cm_s: f32,
}

fn one() -> f32 {
    1.0
}

// manifest 是契约的一部分:这些字段现在还没人读(形态切换/行为要用),但照着 schema
// 解出来放着,比等到要用时再补解析更省事
#[allow(dead_code)]
#[derive(Clone)]
pub struct Clip {
    pub seconds: f32,
    pub speed_cm_s: f32,
}

/// 一个材质槽该怎么画。由导出器解析游戏材质实例得出,取代原来按贴图命名约定的猜法。
#[derive(Clone)]
pub struct Material {
    /// 基色贴图的绝对路径;None = 纯特效层,不画。
    pub base_color: Option<PathBuf>,
    pub mask_alpha: bool,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct Form {
    pub id: i64,
    pub name: String,
    pub stage: i64,
    pub asset: String,
    /// glb 的绝对路径。
    pub model: PathBuf,
    pub scale: f32,
    pub height_cm: f32,
    pub locomotion: String,
    pub clips: HashMap<String, Clip>,
    /// glb 里的材质名 → 该画什么。空表示这个包是旧版导的,载入时退回命名约定。
    pub materials: HashMap<String, Material>,
}

impl Form {
    pub fn clip(&self, logical: &str) -> Option<&Clip> {
        self.clips.get(logical)
    }
}

pub struct Pack {
    pub species_id: i64,
    pub species_name: String,
    pub forms: Vec<Form>,
    /// 包目录,列表显示与相对路径都要用。
    pub dir: PathBuf,
}

impl Pack {
    /// 默认包目录:`$XDG_DATA_HOME/rocom-pets/packs`。
    pub fn default_dir() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
        Some(base.join("rocom-pets").join("packs"))
    }

    /// 列出包目录下所有能读的包(按名字排序)。读不动的只警告,不让一个坏包挡住其他的。
    pub fn list(dir: &Path) -> Vec<Pack> {
        let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
            Ok(read) => read
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.join("manifest.toml").is_file())
                .collect(),
            Err(e) => {
                log::debug!("包目录 {dir:?} 读不了: {e}");
                return Vec::new();
            }
        };
        entries.sort();
        entries
            .iter()
            .filter_map(|path| match Pack::load(path) {
                Ok(pack) => Some(pack),
                Err(e) => {
                    log::warn!("跳过 {path:?}: {e:#}");
                    None
                }
            })
            .collect()
    }

    /// 按「路径」或「包名」定位一个包:优先当路径用,否则在包目录里按物种名/目录名找。
    pub fn resolve(value: &str, packs_dir: Option<&Path>) -> Result<Pack> {
        let as_path = crate::config::Config::expand_path(value);
        if as_path.join("manifest.toml").is_file() {
            return Pack::load(&as_path);
        }
        if let Some(dir) = packs_dir {
            for pack in Pack::list(dir) {
                if pack.species_name == value || pack.dir.file_name().is_some_and(|n| n == value) {
                    return Ok(pack);
                }
            }
        }
        bail!("找不到宠物包 {value}(既不是包目录,也不在 {packs_dir:?} 里)")
    }

    /// `dir` 是包目录(含 manifest.toml)。
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join("manifest.toml");
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("读不到 {path:?}(不是宠物包目录?)"))?;
        let raw: RawManifest =
            toml::from_str(&text).with_context(|| format!("{path:?} 解析失败"))?;
        if raw.schema > SUPPORTED_SCHEMA {
            bail!(
                "{path:?} 的 schema 是 {},本运行时只支持到 {SUPPORTED_SCHEMA}",
                raw.schema
            );
        }

        if let Some(version) = &raw.source_version {
            log::debug!("{path:?} 由源 {version} 导出");
        }
        let species_id = raw.species.id;
        let species_name = raw.species.name;
        let forms = raw
            .forms
            .into_iter()
            .map(|form| Form {
                id: form.id,
                name: form.name,
                stage: form.stage,
                asset: form.asset,
                model: dir.join(form.model),
                scale: form.scale,
                // 没给高度就按一只猫的量级兜底,免得算出 0 像素
                height_cm: if form.height_cm > 1.0 {
                    form.height_cm
                } else {
                    80.0
                },
                locomotion: form.locomotion,
                clips: form
                    .clips
                    .into_iter()
                    .map(|(name, clip)| {
                        (
                            name,
                            Clip {
                                seconds: clip.ms as f32 / 1000.0,
                                speed_cm_s: clip.speed_cm_s,
                            },
                        )
                    })
                    .collect(),
                materials: form
                    .materials
                    .into_iter()
                    .map(|(name, mat)| {
                        (
                            name,
                            Material {
                                base_color: mat.base_color.map(|rel| dir.join(rel)),
                                mask_alpha: mat.mask_alpha,
                            },
                        )
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        if forms.is_empty() {
            bail!("{path:?} 里没有任何形态");
        }
        Ok(Self {
            species_id,
            species_name,
            forms,
            dir: dir.to_path_buf(),
        })
    }

    /// 形态在 `forms` 里的下标(按资产名或中文名);给 None 就是 0。
    pub fn form_index(&self, asset: Option<&str>) -> Result<usize> {
        match asset {
            None => Ok(0),
            Some(want) => self
                .forms
                .iter()
                .position(|f| f.asset == want || f.name == want)
                .with_context(|| {
                    format!(
                        "包里没有形态 {want};有的是: {}",
                        self.forms
                            .iter()
                            .map(|f| format!("{}({})", f.asset, f.name))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }),
        }
    }
}
