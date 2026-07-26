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
    /// BaseTex/EyeTex,固有色是 shader 算的),运行时整片跳过;
    /// 将来做特效通道时改成按 blend 走半透/加色,见 design.md 横向待办。
    #[serde(default)]
    base_color: Option<String>,
    /// 贴图 alpha 是不是真遮罩。眼/嘴的表情图集是(不剔就是一块方糊),
    /// 本体贴图不是(它的 alpha 是美术塞的遮罩通道,拿来剔会把身体啃掉)。
    #[serde(default)]
    mask_alpha: bool,
    /// 以下都只对特效层有意义(`base_color` 缺失时)。
    #[serde(default)]
    tint: Option<[f32; 4]>,
    #[serde(default = "one")]
    opacity: f32,
    #[serde(default = "one")]
    glow: f32,
    #[serde(default)]
    flow: Option<[f32; 4]>,
    #[serde(default)]
    mask_tex: Option<String>,
    #[serde(default)]
    noise_tex: Option<String>,
    #[serde(default)]
    mask_matcap: bool,
    /// 以下对所有材质都可能有(有基色的也一样)。
    #[serde(default)]
    translucent: bool,
    #[serde(default)]
    star_tex: Option<String>,
    #[serde(default)]
    star_tiling: Option<[f32; 2]>,
    #[serde(default)]
    star_color: Option<[f32; 3]>,
    #[serde(default)]
    matcap_tex: Option<String>,
    #[serde(default)]
    matcap_color: Option<[f32; 3]>,
    #[serde(default)]
    rim_color: Option<[f32; 3]>,
    #[serde(default)]
    rim_intensity: f32,
    #[serde(default = "three")]
    rim_power: f32,
    #[serde(default)]
    main_color: Option<[f32; 3]>,
    #[serde(default)]
    flow_tex: Option<String>,
    #[serde(default = "one")]
    flow_power: f32,
    #[serde(default)]
    interior_tex: Option<String>,
    #[serde(default)]
    interior_color: Option<[f32; 3]>,
    #[serde(default = "one")]
    refraction: f32,
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

/// 边缘光衰减次数的缺省:pow(1-N·V, 3) 是我们原来写死的那圈细边。
fn three() -> f32 {
    3.0
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
    /// 基色贴图的绝对路径;None = 纯特效层,走 `effect` 那套画法。
    pub base_color: Option<PathBuf>,
    pub mask_alpha: bool,
    /// 只在 `base_color` 为 None 时有效。
    pub effect: Effect,
    /// 半透。**有基色的材质也可能是半透**:暮星辰的裙子与那两个球都是,
    /// 当不透明画就是死板的实心块。
    pub translucent: bool,
    pub opacity: f32,
    /// 身上那些细碎星光。
    pub star: Option<PathBuf>,
    pub star_tiling: [f32; 2],
    pub star_color: [f32; 3],
    /// 球面反射查找表:玻璃/金属高光。
    pub matcap: Option<PathBuf>,
    pub matcap_color: [f32; 3],
    pub rim_color: [f32; 3],
    pub rim_intensity: f32,
    /// 边缘光的衰减次数。**小于 1 = 整片泛色**(幽星光的球 0.35),不是一圈细边。
    pub rim_power: f32,
    /// 半透材质的整体着色;None = 不改色。
    pub main_color: [f32; 3],
    /// 卷动色带:一张渐变图沿 UV 滚过表面,叠在固有色上(暮星辰环带的青↔粉渐变)。
    pub flow: Option<PathBuf>,
    /// [u 速度, v 速度, u 平铺, v 平铺] + 混入强度。
    pub flow_uv: [f32; 4],
    pub flow_power: f32,
    /// **玻璃内部那颗星**:四角星场贴图(`StarTex` = `T_EMeng003`),沿折射光线在物体空间
    /// march、三向投影采样、按时间卷动。读 shader 汇编得来,见 docs/design.md §1。
    pub interior: Option<PathBuf>,
    pub interior_color: [f32; 3],
    /// 折射率(材质里的 `GlobalRefraction` = 1.3)。
    ///
    /// manifest 里还有个 `refract_depth`(= `GlobalDepth` = 100),**这里故意不读**:
    /// 实机那个 100 是配着它自己那套归一化用的,我们按包围盒最长边缩放,深度是对着截图
    /// 手挑的(见 gpu.rs)。留在 manifest 里是当材质记录,别当成运行时参数。
    pub refraction: f32,
}

/// 特效层(火焰/水壳/光晕)的画法参数,全部来自游戏材质。
#[derive(Clone)]
pub struct Effect {
    /// 主色,**可能是 HDR**:火花的 `Color01` = (6, 0.8, 0)。任一通道 >1 就当加色发光。
    pub tint: [f32; 4],
    pub opacity: f32,
    pub glow: f32,
    /// [u 速度, v 速度, u 平铺, v 平铺]
    pub flow: [f32; 4],
    pub mask: Option<PathBuf>,
    pub noise: Option<PathBuf>,
    /// 遮罩是 MatCap:要按**视空间法线**采样(球面反射查找表),不是网格 UV。
    pub mask_matcap: bool,
}

impl Effect {
    /// 主色任一通道 >1 说明美术是当**加色发光**用的(黑=加零),此时不该按半透混合。
    pub fn additive(&self) -> bool {
        self.tint[0] > 1.0 || self.tint[1] > 1.0 || self.tint[2] > 1.0
    }
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
    /// glb 里的材质名 → 该画什么。**载入模型必需**,空的话 `Model::load` 直接报错
    /// (旧版导出的包没有这一节,重导即可)。
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
                            // 键统一小写:材质名在「资产文件名」与「对象名」之间大小写会漂
                            // (喵呜是 MiaoMiao/Miaomiao、魔力猫反过来),查表必须不区分大小写
                            name.to_ascii_lowercase(),
                            Material {
                                base_color: mat.base_color.map(|rel| dir.join(rel)),
                                mask_alpha: mat.mask_alpha,
                                effect: Effect {
                                    // 没给主色就用白,至少形体在
                                    tint: mat.tint.unwrap_or([1.0; 4]),
                                    opacity: mat.opacity,
                                    glow: mat.glow,
                                    flow: mat.flow.unwrap_or([0.0, 0.0, 1.0, 1.0]),
                                    mask: mat.mask_tex.map(|rel| dir.join(rel)),
                                    noise: mat.noise_tex.map(|rel| dir.join(rel)),
                                    mask_matcap: mat.mask_matcap,
                                },
                                translucent: mat.translucent,
                                opacity: mat.opacity,
                                star: mat.star_tex.map(|rel| dir.join(rel)),
                                star_tiling: mat.star_tiling.unwrap_or([1.0, 1.0]),
                                star_color: mat.star_color.unwrap_or([1.0; 3]),
                                matcap: mat.matcap_tex.map(|rel| dir.join(rel)),
                                matcap_color: mat.matcap_color.unwrap_or([1.0; 3]),
                                rim_color: mat.rim_color.unwrap_or([1.0; 3]),
                                rim_intensity: mat.rim_intensity,
                                rim_power: mat.rim_power,
                                main_color: mat.main_color.unwrap_or([1.0; 3]),
                                flow: mat.flow_tex.map(|rel| dir.join(rel)),
                                flow_uv: mat.flow.unwrap_or([0.0, 0.0, 1.0, 1.0]),
                                flow_power: mat.flow_power,
                                interior: mat.interior_tex.map(|rel| dir.join(rel)),
                                interior_color: mat.interior_color.unwrap_or([1.0; 3]),
                                refraction: mat.refraction,
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
