//! 读宠物包的 manifest.toml。
//!
//! manifest 是导出器与运行时之间唯一的契约(schema 见 docs/design.md §4.3),
//! 运行时只认里面的**逻辑动作名**与形态元数据,不关心资产原名。
//! 缺字段就按默认值降级——包是本地生成物,宁可少个动作也不该整只加载不出来。
//!
//! 包可以是**解开的目录**,也可以是一个 `.rkpet`(zip)。这一层不关心是哪种:
//! 位置一律叫 `path`,内容一律走 [`crate::assets`] 读(见那个模块的「虚拟路径」说明)。

mod material;

pub use material::*;
use material::{RawMaterial, material_table};

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
    #[serde(default = "material::one")]
    scale: f32,
    #[serde(default)]
    height_cm: f32,
    #[serde(default)]
    locomotion: String,
    #[serde(default)]
    clips: HashMap<String, RawClip>,
    /// `[forms.face]`:眼神曲线,键与 `[forms.clips]` 同一套。**旧包没有这一节**
    /// ⇒ 空表,运行时退回 `persona::face_for_clip` 那张兜底表。
    #[serde(default)]
    face: HashMap<String, RawFace>,
    /// `[forms.morph]`:形变目标(脸的 blendshape)。位移与权重曲线都在 glb 里
    /// (标准的 glTF morph target + `weights` 通道),这儿只留名字用来回表核对。
    #[serde(default)]
    morph: Option<RawMorph>,
    #[serde(default)]
    materials: HashMap<String, RawMaterial>,
    /// `[forms.shiny_materials]`:异色那一套。**旧包没有这一节** ⇒ 空表 = 这只没有异色。
    #[serde(default)]
    shiny_materials: HashMap<String, RawMaterial>,
    #[serde(default)]
    voice: Option<RawVoice>,
    /// `[forms.sfx]`:动作音效层,键与 `[forms.voice]`、`[forms.clips]` 同一套。
    #[serde(default)]
    sfx: HashMap<String, RawVoiceClip>,
}

/// `[forms.voice]`:叫声。`cents_low/high` 是游戏里 `voice` 属性拉到 ±100 时的音分
/// (「粗嗓门」「婉转声」),运行时按 `2^(音分/1200)` 调播放速率复刻。
#[derive(Deserialize)]
struct RawVoice {
    #[serde(default)]
    cents_low: f32,
    #[serde(default)]
    cents_high: f32,
    /// 其余键都是「动作逻辑名 → 音频文件」。
    #[serde(flatten)]
    clips: HashMap<String, RawVoiceClip>,
}

#[derive(Deserialize)]
struct RawVoiceClip {
    path: String,
    #[serde(default)]
    ms: u32,
}

#[derive(Deserialize)]
struct RawClip {
    #[serde(default)]
    ms: u32,
    /// 走跑类动作:动画自带的位移换算出的速度(cm/s);0 表示原地循环。
    #[serde(default)]
    speed_cm_s: f32,
}

/// `[forms.morph]`:形变目标的名字,顺序即 glb 里 morph target 的顺序。
#[derive(Deserialize)]
struct RawMorph {
    #[serde(default)]
    targets: Vec<String>,
}

/// `[forms.face]` 里的一段:**一个脸槽一条**曲线,键是槽名(`eye` / `eye_1` /
/// `mouth` / `dynamic1`…),值是 `[[毫秒, 格号], …]`。
type RawFace = HashMap<String, Vec<[i64; 2]>>;

/// 一条眼神曲线:**阶梯**的 (秒, 图集格号 1..8),按时间升序。
///
/// 来源是动画自带的 `EC_*` 曲线(见导出器的 `FaceCurves.cs`)。空 = 这段动作没给这个槽
/// 值,那时用性格那张脸。**一个脸槽一条**,同一段里可以完全不同 —— 幽星光的 `Shock`
/// 就是眼第 3 格、嘴第 7 格。
pub type FaceTrack = Vec<(f32, u32)>;

/// 一个形态最多认几个脸槽。编号是**定死的**(见 [`face_slot`]),
/// 不是按包里出现的顺序分配 —— 这样材质与曲线两边各自查表就能对上,不用再传一张映射。
///
/// 全库实测的上限:`_Es` 最多 2 个(一窝蜂二/三阶、加油海葵异形、里奥三阶异形)、
/// `_Mh` 最多 2 个(加油海葵异形)、`_Dynamic*` 最多 3 个(卡波二阶)。
pub const MAX_FACE_SLOTS: usize = 8;

/// 槽名 → 编号。`None` = 不认得的名字(新导出器加了槽而运行时还没跟上),按「不是脸」处理。
///
/// 顺序就是 [`crate::pet::gpu::FrameParams::face_uv`] 那个数组的下标,也是着色器里
/// `camera.face_uv[]` 的下标。**加新槽只能往后加**:旧包里写的是名字、不是编号,
/// 但材质那份 uniform 里存的是编号,前面插一个会把已经导好的包整体串位。
pub fn face_slot(key: &str) -> Option<usize> {
    Some(match key {
        "eye" => 0,
        "eye_1" => 1,
        "mouth" => 2,
        "mouth_1" => 3,
        "dynamic1" => 4,
        "dynamic2" => 5,
        "dynamic3" => 6,
        "dynamic4" => 7,
        _ => return None,
    })
}

/// 阶梯查值:返回 `time` 时刻生效的那一格。空曲线或时刻在第一帧之前都返回 None
/// (= 用性格那张脸)。
pub fn face_at(track: &FaceTrack, time: f32) -> Option<u32> {
    let mut card = None;
    for &(t, c) in track {
        if t > time {
            break;
        }
        card = Some(c);
    }
    card
}

fn face_track(raw: Vec<[i64; 2]>) -> FaceTrack {
    raw.into_iter()
        .map(|[ms, card]| (ms as f32 / 1000.0, card.clamp(1, 8) as u32))
        .collect()
}

/// `[forms.face]` 的一段 → 按槽号排好的曲线表。不认得的键直接丢
/// (往前兼容:新导出器多写一个槽,旧运行时照常跑,只是那个槽不动)。
fn face_tracks(raw: RawFace) -> [FaceTrack; MAX_FACE_SLOTS] {
    let mut tracks: [FaceTrack; MAX_FACE_SLOTS] = Default::default();
    for (key, steps) in raw {
        if let Some(slot) = face_slot(&key) {
            tracks[slot] = face_track(steps);
        }
    }
    tracks
}

// manifest 是契约的一部分:这些字段现在还没人读(形态切换/行为要用),但照着 schema
// 解出来放着,比等到要用时再补解析更省事
#[allow(dead_code)]
#[derive(Clone, Default)]
pub struct Clip {
    pub seconds: f32,
    pub speed_cm_s: f32,
    /// 各脸槽的眼神曲线,下标见 [`face_slot`]。空 = 这段动作没给这个槽值,
    /// 那时那个槽用性格那张脸 —— 和游戏一致:动画上那个通知只覆盖它配置过的槽。
    pub faces: [FaceTrack; MAX_FACE_SLOTS],
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
    /// 声音;None = 这个形态没导出(两族 Wwise 库都没有,或者导出时缺 vgmstream/ffmpeg)。
    pub voice: Option<Voice>,
    /// glb 里的材质名 → 该画什么。**载入模型必需**,空的话 `Model::load` 直接报错
    /// (旧版导出的包没有这一节,重导即可)。
    pub materials: HashMap<String, Material>,
    /// 异色那一套材质,**键与 `materials` 完全相同**(glb 里的材质名是默认那套)。
    /// 空 = 这个形态没有异色 —— 全库只有一小部分有(游戏里也是),见 `Form::has_shiny`。
    pub shiny_materials: HashMap<String, Material>,
    /// 形变目标(脸的 blendshape)的名字,顺序即 glb 里 morph target 的顺序。
    /// **渲染不读它** —— 位移与权重曲线都在 glb 里;留着是为了报告与排查
    /// (「这只的嘴是几何还是贴图」一眼能看出来)。全库 1000 个资产里只有 37 个非空。
    pub morph_targets: Vec<String>,
}

impl Form {
    /// 这个形态有没有异色。**多数没有** —— 游戏里也是:异色要美术另做一套材质与贴图,
    /// 全库 `MODEL_CONF` 3297 行里只有 177 行的 `shiny_icon` 与普通图不同。
    /// 界面上据此决定「异色」这一档给不给点。
    pub fn has_shiny(&self) -> bool {
        !self.shiny_materials.is_empty()
    }

    /// 该按哪一张材质表画。异色那档在包里就是另一套材质,`Model::load` 拿它当唯一来源。
    pub fn materials_for(&self, shiny: bool) -> &HashMap<String, Material> {
        if shiny && self.has_shiny() {
            &self.shiny_materials
        } else {
            &self.materials
        }
    }
}

/// 一个形态的声音。**两层**:嗓子发出来的叫声,和身体动静的音效 ——
/// 游戏里同一段情绪就是这两条叠着放的,分别来自 `Pet_Vo_*` 与 `Pet_Action_*` 两族库。
#[derive(Clone)]
pub struct Voice {
    /// `voice` 属性拉到 ±100 时的音分(粗嗓门 / 婉转声),运行时按
    /// `2^(音分/1200)` 调播放速率 —— Wwise 的 pitch 本来就是重采样。
    /// **只管叫声那层**:动作音效不跟着变调(`Pet_Action_*` 库压根没挂这条曲线)。
    pub cents_low: f32,
    pub cents_high: f32,
    /// 动作逻辑名(与 `clips` 同一把键)→ 叫声文件。
    pub clips: HashMap<String, VoiceClip>,
    /// 同一把键 → 动作音效文件。
    pub sfx: HashMap<String, VoiceClip>,
}

#[derive(Clone)]
pub struct VoiceClip {
    pub path: PathBuf,
    #[allow(dead_code)] // 时长目前只用于排查;播放不需要预先知道长度
    pub seconds: f32,
}

/// manifest 里那一节音频表 → 包内绝对路径。两层各调一次。
/// `MI_P_Object_SeasonMutation*` 那族的赛季外观。三个 float4 各把一个颜色与一个标量
/// 打包在一起(`.w` 是标量),与 manifest 里的写法一致。
#[derive(Clone, Debug)]
pub struct SeasonMutation {
    /// 花纹图(每宠物一张),顶替玻璃层里那张全库共享的 `MainTex`。
    /// **可以没有** —— 机幕方舟的 `_By1` 就没写,那时用共享的那张。
    pub flow_noise: Option<PathBuf>,
    /// **区域遮罩**(每宠物一张):`.b` 走幂曲线混向 `blue`,`.a ≥ 0.79` 的地方换成 `metal`。
    /// 「只在翅膀」「只在身体与肩顶」就是它划的。
    pub mix_mask: Option<PathBuf>,
    /// 金属区那层金属光泽的 matcap。**只有 `MetalSpecInt > 0` 的材质导得到** ——
    /// 机幕方舟有(实机是带高光的银),龙息帕尔没有(实机是平白)。见导出器那条说明。
    pub matcap: Option<PathBuf>,
    /// [RedChannel.rgb, GlobalRefraction]
    pub red: [f32; 4],
    /// [GreenChannel.rgb, GlobalDepth]
    pub green: [f32; 4],
    /// [BlueChannel.rgb, FlowMaskInt]
    pub blue: [f32; 4],
    /// [MetalColor.rgb, FlowMaskPow]
    pub metal: [f32; 4],
    /// [MetalColor02.rgb, 0]
    pub metal2: [f32; 4],
    /// [FlowSpeedX, FlowSpeedY, MainTexTiling, NormalEffectAmount]。
    /// **两个流速轴都要**:机幕方舟给 Y、龙息帕尔给 X,只取一个会让另一只静止。
    pub flow: [f32; 4],
}

fn sound_files(root: &Path, raw: HashMap<String, RawVoiceClip>) -> HashMap<String, VoiceClip> {
    raw.into_iter()
        .map(|(key, clip)| {
            (
                key,
                VoiceClip {
                    path: root.join(clip.path),
                    seconds: clip.ms as f32 / 1000.0,
                },
            )
        })
        .collect()
}

impl Form {
    pub fn clip(&self, logical: &str) -> Option<&Clip> {
        self.clips.get(logical)
    }

    /// 只带一张动作表的形态,给动作覆盖率的单测用。
    #[cfg(test)]
    pub fn for_test(clips: HashMap<String, Clip>) -> Self {
        Self {
            id: 0,
            name: "测试".into(),
            stage: 1,
            asset: "Test_001".into(),
            model: PathBuf::from("<test>"),
            scale: 1.0,
            height_cm: 80.0,
            locomotion: "ground".into(),
            clips,
            voice: None,
            materials: HashMap::new(),
            shiny_materials: HashMap::new(),
            morph_targets: Vec::new(),
        }
    }
}

/// 包目录里的一项:够列一行表格,但**不含动作表与材质表**(见 [`Pack::list_entries`])。
pub struct PackEntry {
    /// 物种名(链首的名字)。
    pub name: String,
    /// 整条进化链的形态名。列表里直接写成「喵喵 → 喵呜 → 魔力猫」——
    /// 比只写链名好搜:想找魔力猫的人不一定记得它的链首叫喵喵。
    pub forms: Vec<String>,
    /// 包的位置:目录,或者 `.rkpet` 文件。
    pub path: PathBuf,
    /// 占多少字节。
    pub size: u64,
}

impl PackEntry {
    /// 「喵喵 → 喵呜 → 魔力猫」。单形态的包就只有一个名字。
    pub fn chain(&self) -> String {
        if self.forms.is_empty() {
            return self.name.clone();
        }
        self.forms.join(" → ")
    }

    /// 是 `.rkpet` 归档还是解开的目录。列表里要标出来(对应 `--list` 的 `[rkpet]`)。
    pub fn archived(&self) -> bool {
        self.path.is_file()
    }
}

pub struct Pack {
    pub species_id: i64,
    pub species_name: String,
    pub forms: Vec<Form>,
    /// 包的位置(目录或 `.rkpet` 文件)。列表显示与包内相对路径都要用。
    pub path: PathBuf,
}

/// 放大件数据的目录(不含 `rocom-pets` 那一层)。
#[cfg(not(target_os = "windows"))]
fn data_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
}

#[cfg(target_os = "windows")]
fn data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

impl Pack {
    /// 默认包目录:Linux 是 `$XDG_DATA_HOME/rocom-pets/packs`,
    /// Windows 是 `%LOCALAPPDATA%\rocom-pets\packs`(包有几 GB,不该跟着漫游配置走)。
    /// 见 config.rs 的 `config_dir`:Windows 上没有 `HOME`/`XDG_*`。
    pub fn default_dir() -> Option<PathBuf> {
        Some(data_dir()?.join("rocom-pets").join("packs"))
    }

    /// 包目录里所有**看着像包**的位置(目录含 manifest,或 `.rkpet` 文件),按路径排序。
    ///
    /// 以下几个「扫包目录」的方法**只有桌面版有** —— 浏览器里的包是 JS 喂进来的字节,
    /// 没有目录可扫(见 assets.rs 的 `memory`)。
    #[cfg(not(target_arch = "wasm32"))]
    fn candidates(dir: &Path) -> Vec<PathBuf> {
        let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
            Ok(read) => read
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| crate::assets::is_pack(p))
                .collect(),
            Err(e) => {
                log::debug!("包目录 {dir:?} 读不了: {e}");
                return Vec::new();
            }
        };
        entries.sort();
        entries
    }

    /// 列出包目录下所有能读的包(按名字排序)。读不动的只警告,不让一个坏包挡住其他的。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn list(dir: &Path) -> Vec<Pack> {
        Self::candidates(dir)
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

    /// 只列**名字**,不读形态。托盘的「加一只」菜单要把包目录整个列出来
    /// (手上就有 539 个),而 [`Pack::list`] 会把每个包的动作表与材质表全解析出来 ——
    /// 菜单只需要一行字,真选中了再 [`Pack::load`]。
    ///
    /// 解析不了的包退用目录名:它多半仍能加载(名字这一节坏了不代表形态坏了),
    /// 真加载失败时再报错也不迟。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn list_entries(dir: &Path) -> Vec<PackEntry> {
        let mut packs: Vec<PackEntry> = Self::candidates(dir)
            .into_iter()
            .map(|path| {
                let (name, forms) = Self::peek(&path);
                let size = crate::assets::size(&path);
                PackEntry {
                    name,
                    forms,
                    path,
                    size,
                }
            })
            .collect();
        packs.sort_by(|a, b| a.name.cmp(&b.name));
        packs
    }

    /// 只把 manifest 里的物种名与形态名抠出来。读不动就退用文件名(去掉 `.rkpet` 后缀)。
    ///
    /// **这一趟已经把 manifest 读进内存了**,顺手多解一层 `[[forms]].name` 是白捡的 ——
    /// 比起单独为「列表要显示进化链」再读一遍全库五百多个 manifest 划算得多。
    /// 动作表与材质表仍然不解:那才是 `Pack::load` 慢的地方。
    pub fn peek(path: &Path) -> (String, Vec<String>) {
        #[derive(Deserialize)]
        struct NamesOnly {
            species: RawSpecies,
            #[serde(default)]
            forms: Vec<FormName>,
        }
        #[derive(Deserialize)]
        struct FormName {
            name: String,
        }

        let parsed = crate::assets::read_manifest(path)
            .ok()
            .and_then(|text| toml::from_str::<NamesOnly>(&text).ok());
        match parsed {
            Some(raw) => (
                raw.species.name,
                raw.forms.into_iter().map(|f| f.name).collect(),
            ),
            None => {
                let stem = if path.is_file() {
                    path.file_stem()
                } else {
                    path.file_name()
                };
                let name = stem
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "?".to_string());
                (name, Vec::new())
            }
        }
    }

    /// 只要名字那一半。
    pub fn peek_name(path: &Path) -> String {
        Self::peek(path).0
    }

    /// 按「路径」或「包名」定位一个包:优先当路径用,否则在包目录里按物种名/文件名找。
    ///
    /// 文件名那一条要**连去掉后缀的也认**:阵容存的是 `喵喵.rkpet`,而用户在配置里
    /// 多半只写 `喵喵` —— 同一个包换成目录形态之后名字还得对得上。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn resolve(value: &str, packs_dir: Option<&Path>) -> Result<Pack> {
        let scanned;
        let entries = match packs_dir {
            Some(dir) => {
                scanned = Pack::list_entries(dir);
                scanned.as_slice()
            }
            None => &[],
        };
        // 包目录写进上下文里,但**别用 Debug 格式** —— `{:?}` 出来是 `Some("/home/…")`,
        // 这句话是要摆到报错窗口里给人看的
        Pack::resolve_in(value, entries).with_context(|| match packs_dir {
            Some(dir) => format!("包目录是 {}", dir.display()),
            None => "而且定不出包目录".to_string(),
        })
    }

    /// 同上,但**目录已经扫好了**。
    ///
    /// 一次要认好几个名字时必须走这条:`resolve` 自己会把整个包目录的 manifest 读一遍,
    /// 每只宠物调一次就是把整库读上几遍 —— 实测六只在场时,`reload` 有 242ms 花在这儿
    /// (库里 201 个包),而且随在场只数线性涨。用户看到的「加一只宠物就像把所有包
    /// 重新加载一遍,卡一两秒」就是它。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn resolve_in(value: &str, entries: &[PackEntry]) -> Result<Pack> {
        // 存档里也可能直接写着路径(`--pack /some/where` 存下来的那种)
        let as_path = crate::config::Config::expand_path(value);
        if crate::assets::is_pack(&as_path) {
            return Pack::load(&as_path);
        }
        for entry in entries {
            let file_name = entry.path.file_name().map(|n| n.to_string_lossy());
            let stem = entry.path.file_stem().map(|n| n.to_string_lossy());
            let hit = entry.name == value
                || file_name.as_deref() == Some(value)
                || stem.as_deref() == Some(value);
            if hit {
                return Pack::load(&entry.path);
            }
        }
        bail!("找不到宠物包 {value}(既不是包目录/`.rkpet`,也不在包目录里)")
    }

    /// `root` 是包目录(含 manifest.toml)或 `.rkpet` 文件。
    pub fn load(root: &Path) -> Result<Self> {
        let path = crate::assets::manifest_path(root);
        let text = crate::assets::read_manifest(root)
            .with_context(|| format!("读不到 {path:?}(不是宠物包?)"))?;
        Self::parse(&text, root)
    }

    /// manifest 正文 → `Pack`。**从读盘那一步拆出来**是为了能拿一段内联 manifest 做单测:
    /// 包是本地生成物、不入仓库,不拆的话「这个字段解没解出来」只能靠跑全库肉眼看。
    fn parse(text: &str, root: &Path) -> Result<Self> {
        let path = crate::assets::manifest_path(root);
        let raw: RawManifest =
            toml::from_str(text).with_context(|| format!("{path:?} 解析失败"))?;
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
                model: root.join(form.model),
                scale: form.scale,
                // 没给高度就按一只猫的量级兜底,免得算出 0 像素
                height_cm: if form.height_cm > 1.0 {
                    form.height_cm
                } else {
                    80.0
                },
                locomotion: form.locomotion,
                // 两族库各自可能缺席(叫声 621 个 bnk、音效 650 个),**任一有就算有声音**
                voice: (form.voice.is_some() || !form.sfx.is_empty()).then(|| {
                    let (cents_low, cents_high, clips) = match form.voice {
                        Some(v) => (v.cents_low, v.cents_high, v.clips),
                        None => (0.0, 0.0, HashMap::new()),
                    };
                    Voice {
                        cents_low,
                        cents_high,
                        clips: sound_files(root, clips),
                        sfx: sound_files(root, form.sfx),
                    }
                }),
                clips: {
                    let mut face = form.face;
                    form.clips
                        .into_iter()
                        .map(|(name, clip)| {
                            let f = face.remove(&name).unwrap_or_default();
                            (
                                name,
                                Clip {
                                    seconds: clip.ms as f32 / 1000.0,
                                    speed_cm_s: clip.speed_cm_s,
                                    faces: face_tracks(f),
                                },
                            )
                        })
                        .collect()
                },
                materials: material_table(root, form.materials),
                shiny_materials: material_table(root, form.shiny_materials),
                morph_targets: form.morph.map(|m| m.targets).unwrap_or_default(),
            })
            .collect::<Vec<_>>();
        if forms.is_empty() {
            bail!("{path:?} 里没有任何形态");
        }
        Ok(Self {
            species_id,
            species_name,
            forms,
            path: root.to_path_buf(),
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


#[cfg(test)]
mod tests {
    use super::*;

    /// 一段真实 manifest 的节选(导出器的产物,只删了无关的表)。
    const MANIFEST: &str = r#"
schema = 1
[species]
id = 3001
name = "喵喵"

[[forms]]
id = 3001
name = "喵喵"
stage = 1
asset = "Gra_MiaoMiao1_001"
model = "forms/Gra_MiaoMiao1_001/model.glb"
height_cm = 55.6

  [forms.clips]
  Happy = { clip = "Happy", ms = 1500, frames = 46 }
  Alert = { clip = "Alert", ms = 4000, frames = 121 }

  [forms.voice]
  cents_low = -300
  cents_high = 300
  Happy = { path = "forms/Gra_MiaoMiao1_001/voice/Happy.ogg", ms = 2366 }
  Alert = { path = "forms/Gra_MiaoMiao1_001/voice/Alert.ogg", ms = 4499 }

  [forms.sfx]
  Happy = { path = "forms/Gra_MiaoMiao1_001/sfx/Happy.ogg", ms = 2354 }
"#;

    /// 声音是**两层**,而且两层与 `[forms.clips]` 同一把键 —— 运行时按动作名取声音就靠这个。
    #[test]
    fn both_sound_layers_are_read_and_keyed_by_clip_name() {
        let pack = Pack::parse(MANIFEST, Path::new("/packs/喵喵")).expect("该解得开");
        let form = &pack.forms[0];
        let voice = form.voice.as_ref().expect("该有声音");

        let mut keys: Vec<&str> = voice.clips.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["Alert", "Happy"], "叫声的键就是动作逻辑名");
        assert!(form.clip("Happy").is_some(), "同一把键在动作表里查得到");

        assert_eq!(voice.sfx.len(), 1, "音效层单独一节,少几段是常事");
        assert_eq!(
            voice.sfx["Happy"].path,
            Path::new("/packs/喵喵/forms/Gra_MiaoMiao1_001/sfx/Happy.ogg"),
            "路径要拼到包根上"
        );
        assert_eq!(voice.cents_low, -300.0);
    }

    /// 只有 `Pet_Action_*` 库、没有 `Pet_Vo_*` 的形态(全库 22 个)照样算有声音 ——
    /// 原来 `[forms.voice]` 一缺就整只哑掉。
    #[test]
    fn a_form_with_only_sfx_still_has_sound() {
        let text = MANIFEST
            .split("  [forms.voice]")
            .next()
            .expect("前半段")
            .to_string()
            + "  [forms.sfx]\n  Happy = { path = \"forms/x/sfx/Happy.ogg\", ms = 1 }\n";
        let pack = Pack::parse(&text, Path::new("/packs/x")).expect("该解得开");
        let voice = pack.forms[0].voice.as_ref().expect("只有音效也算有声音");
        assert!(voice.clips.is_empty());
        assert_eq!(voice.sfx.len(), 1);
        // 没有叫声那层就没有曲线,按原调走
        assert_eq!((voice.cents_low, voice.cents_high), (0.0, 0.0));
    }
}
