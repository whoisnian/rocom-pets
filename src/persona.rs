//! 性格:**从游戏配置表里搬过来的**一组行为倾向 + 一张脸。
//!
//! 以前这里是自己编的五个(乖巧/活泼/慵懒/黏人/高冷)。解包数据里其实有现成的:
//!
//! - `NATURE_CONF` —— 31 条性格,每条一个 `emotion_desc`,那就是这只宠物的**默认表情**。
//!   31 条里只有 6 条不是「默认」:天真/开朗 → 微笑,懒散/悠闲 → 困倦,胆小 → 哭哭,
//!   急躁 → 生气。**表情落在眼睛(和嘴)上** —— 是脸那张图集里换一格,
//!   不是播一段动作,见 [`Expression`]。
//! - `LLM_PET_BEHAVIOR_CONF` —— 84 条宠物行为,每条标着 `nature_id`(哪几种性格会做它)。
//!   反过来读就是「这个性格爱做哪些动作」:调皮 → happy/happy_1/jump/run_to_player,
//!   冷静 → relax/nap/deep_sleep,悠闲 → fear/fear_1/sad/run_away …
//!
//! 两张表合起来正好是要的东西:**性格决定表情**,不用再让人手工勾表情池。
//!
//! 名字与 `nature_id` 都照抄游戏,便于回表核对。**倍率那五个数字是编的** ——
//! 游戏那两张表没有「多久睡一次」这种量,只能按每种性格爱做的行为往这五个旋钮上折:
//! 爱 nap/deep_sleep 的困得快,爱 jump/run_to_player 的闲不住,爱 run_away/turn_away
//! 的不搭理人。折算依据逐条写在下面。

use crate::stage::EMOTES;

/// 表情 = 脸那张贴图里的**一格**。
///
/// 眼睛和嘴各是一张 **2 列 × 4 行的图集**(`M_P_Eyes` 那一族材质),网格的 UV 落在
/// 左上那一格,换表情就是整格地偏一下 UV。八格的内容(逐格渲出来看的,以幽星光为例;
/// 抽查喵喵/火花/菊花梨,图集结构一致):
///
/// ```text
///   (0,0) 竖眼 + 弯月嘴     = 默认     (1,0) 眯眼笑 + 腮红 + 张嘴 = 微笑
///   (0,1) 圆睁眼 + 腮红     (惊讶)     (1,1) 尖角怒眼            = 生气
///   (0,2) 闭眼 + 水滴       = 困倦     (1,2) 八字垂眼 + 倒弯嘴    = 哭哭
///   (0,3) 眯眼 + 大张嘴     (大笑)     (1,3) 螺旋眼              (晕)
/// ```
///
/// 哪个性格用哪一格由游戏的 `NATURE_CONF.emotion_desc` 定;格子的**位置**是把八格
/// 逐个渲出来、和三方攻略里那张「幽星光不同性格的眼睛」逐张比对出来的 ——
/// 配置表里只有「微笑」这种名字,没有下标。五种表情与攻略图一一对上。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expression {
    /// `NATURE_CONF.emotion_desc` 里的名字。
    pub name: &'static str,
    /// 图集里的列、行。
    pub cell: (u32, u32),
}

/// 图集的格数:2 列 × 4 行。
pub const FACE_COLS: f32 = 2.0;
pub const FACE_ROWS: f32 = 4.0;

/// 默认那张脸 —— 31 条性格里有 24 条用它。
pub const DEFAULT_FACE: Expression = Expression {
    name: "默认",
    cell: (0, 0),
};

impl Expression {
    /// 贴图 UV 要偏多少(整格)。
    pub fn uv_offset(&self) -> [f32; 2] {
        [
            self.cell.0 as f32 / FACE_COLS,
            self.cell.1 as f32 / FACE_ROWS,
        ]
    }
}

/// 一个性格。字段里那五个倍率是**乘在 stage.rs 的手感常量上**的,1.0 = 照基线来。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Persona {
    /// 存档里写的值,也是配置的键。
    pub id: &'static str,
    /// 中文名,与游戏 `NATURE_CONF` 一致。
    pub name: &'static str,
    /// 游戏里的性格 id(`NATURE_CONF` 的行号)。留着是为了能回表核对。
    pub nature_id: u32,
    /// 配置窗口里的一句话说明。
    pub about: &'static str,
    /// 困倦攒得多快。大 = 更早去睡。
    pub sleepy: f32,
    /// 无聊攒得多快。大 = 更常起来走动。
    pub bored: f32,
    /// 无聊时「只做个表情」而不是走动的概率倍率。
    pub emote: f32,
    /// 起跑的门槛倍率。**小 = 更容易跑**(门槛是「目标点有多远」,见 `choose_next`)。
    pub run: f32,
    /// 注意到旁边那只的距离倍率。大 = 更爱搭理别人。
    pub social: f32,
    /// 这个性格的脸(`NATURE_CONF.emotion_desc`)—— **就是眼睛/嘴那张图集里的一格**。
    pub face: Expression,
    /// 待机时随手做的那个表情动作;None = 没有偏好。
    /// **和 `face` 不是一回事**:那是眼睛,这是动作。
    pub default_emote: Option<&'static str>,
    /// 这个性格爱做的表情(从 `LLM_PET_BEHAVIOR_CONF` 反查),动作名。
    /// 空 = 没有偏好,六个表情都做。
    pub likes: &'static [&'static str],
}

/// 全部性格。**第一个是默认**(见 [`Persona::default`]),顺序即下拉框顺序。
///
/// 游戏里有 31 条性格,这里只留七条。挑的标准是**两条轴上都不重复**:
///
/// - **脸**(`NATURE_CONF.emotion_desc`):五种各留一个代表。这是肉眼唯一看得出来的
///   区别,少一种就等于界面上少一档。
/// - **动静**(`LLM_PET_BEHAVIOR_CONF` 反查出来的行为):默认脸那几条里,
///   留下差得最远的三个 —— 平和(基线)、调皮(jump/run_to_player,最闲不住)、
///   冷静(nap/deep_sleep,最能睡)。
///
/// 按这个标准砍掉的例子:开朗与天真同是「微笑」脸,悠闲与懒散同是「困倦」脸;
/// 理性虽然有自己的行为(run_away/turn_away),但脸是默认的、「不搭理人」这一档
/// 已经有胆小占着 —— 多留一条只是让下拉框长一点。
pub const ALL: &[Persona] = &[
    Persona {
        id: "peaceful",
        name: "平和",
        nature_id: 28,
        about: "基线性格:不吵不闹,该睡就睡",
        sleepy: 1.0,
        bored: 1.0,
        emote: 1.0,
        run: 1.0,
        social: 1.0,
        // 游戏里 emotion_desc = 默认,行为表里也没给它单独的动作 —— 正好当基线
        face: Expression {
            name: "默认",
            cell: (0, 0),
        },
        default_emote: None,
        likes: &[],
    },
    Persona {
        id: "playful",
        name: "调皮",
        nature_id: 3,
        // call_out / jump / run_to_player:闲不住、爱凑过去、动不动就跑
        about: "闲不住,爱往人身边凑,稍远就跑起来",
        sleepy: 0.6,
        bored: 2.0,
        emote: 0.6,
        run: 0.6,
        social: 1.4,
        face: Expression {
            name: "默认",
            cell: (0, 0),
        },
        default_emote: None,
        likes: &["Happy"],
    },
    Persona {
        id: "naive",
        name: "天真",
        nature_id: 7,
        // show / show_1 / launch_player:爱显摆;emotion_desc = 微笑
        about: "爱显摆,笑得多",
        sleepy: 0.8,
        bored: 1.2,
        emote: 1.6,
        run: 1.0,
        social: 1.3,
        face: Expression {
            name: "微笑",
            cell: (1, 0),
        },
        default_emote: Some("Happy"),
        likes: &["Show", "Anger"],
    },
    Persona {
        id: "indolent",
        name: "懒散",
        nature_id: 8,
        // look_around / move_nearby:只在原地小动;emotion_desc = 困倦
        about: "很快就困,懒得走远,多半在原地待着",
        sleepy: 2.5,
        bored: 0.4,
        emote: 1.4,
        run: 1.8,
        social: 0.8,
        face: Expression {
            name: "困倦",
            cell: (0, 2),
        },
        default_emote: Some("Relax"),
        likes: &["Fear"],
    },
    Persona {
        id: "calm",
        name: "冷静",
        nature_id: 14,
        // relax / nap / deep_sleep / keep_turn_away:睡得最多,表情最少
        about: "睡得最多,很少做表情",
        sleepy: 2.8,
        bored: 0.6,
        emote: 0.5,
        run: 1.6,
        social: 0.6,
        face: Expression {
            name: "默认",
            cell: (0, 0),
        },
        default_emote: None,
        likes: &["Relax"],
    },
    Persona {
        id: "timid",
        name: "胆小",
        nature_id: 21,
        // 行为表里没给它动作;emotion_desc = 哭哭,那就往「怕生」上折
        about: "怕生,爱躲远点,一惊一乍",
        sleepy: 1.0,
        bored: 1.1,
        emote: 1.2,
        run: 0.7,
        social: 0.4,
        face: Expression {
            name: "哭哭",
            cell: (1, 2),
        },
        default_emote: Some("Sad"),
        likes: &["Fear"],
    },
    Persona {
        id: "impatient",
        name: "急躁",
        nature_id: 22,
        // 行为表里没给它动作;emotion_desc = 生气,那就往「坐不住」上折
        about: "坐不住,脾气也急",
        sleepy: 0.7,
        bored: 1.8,
        emote: 1.4,
        run: 0.5,
        social: 1.1,
        face: Expression {
            name: "生气",
            cell: (1, 1),
        },
        default_emote: Some("Anger"),
        likes: &["Anger"],
    },
];

impl Default for Persona {
    /// 平和 = stage.rs 的基线。**不写性格的存档必须落在这儿**,
    /// 否则升级一次运行时,所有人的宠物脾气都变了。
    fn default() -> Self {
        ALL[0]
    }
}

impl Persona {
    /// 按存档里的 id 找。找不到就退回默认并警告 —— 存档是机器写的,
    /// 出现不认识的 id 多半是降级运行(或者性格表换过一轮),不该拦住启动。
    pub fn by_id(id: &str) -> Self {
        match ALL.iter().find(|p| p.id == id || p.name == id) {
            Some(found) => *found,
            None => {
                log::warn!("不认识的性格 {id},按「{}」处理", Self::default().name);
                Self::default()
            }
        }
    }

    /// 存档里要不要写这一项。默认性格不写,存档保持干净。
    pub fn saved_id(&self) -> Option<String> {
        (self.id != Self::default().id).then(|| self.id.to_string())
    }

    /// 这个性格会做的表情(动作名),**默认表情排在最前**。
    ///
    /// 没有偏好也没有默认表情(平和)就是六个全做 —— 那是加性格之前的行为。
    pub fn emote_pool(&self) -> Vec<&'static str> {
        if self.default_emote.is_none() && self.likes.is_empty() {
            return EMOTES.iter().map(|(name, _)| *name).collect();
        }
        let mut pool: Vec<&'static str> = self.default_emote.into_iter().collect();
        for name in self.likes {
            if !pool.contains(name) {
                pool.push(name);
            }
        }
        pool
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_the_baseline_and_multiplies_to_one() {
        // 「平和」必须逐项等于 1.0,否则它就不是基线,而是又一个性格
        let p = Persona::default();
        assert_eq!(p.id, "peaceful");
        assert_eq!(
            (p.sleepy, p.bored, p.emote, p.run, p.social),
            (1.0, 1.0, 1.0, 1.0, 1.0)
        );
        assert_eq!(p.saved_id(), None, "默认性格不该写进存档");
        // 基线不挑表情:六个都做,与加性格之前一致
        assert_eq!(p.emote_pool().len(), EMOTES.len());
    }

    #[test]
    fn ids_are_unique_and_round_trip() {
        for persona in ALL {
            assert_eq!(Persona::by_id(persona.id), *persona);
            // 配置窗口里显示的是中文名,手改配置的人多半照着抄
            assert_eq!(Persona::by_id(persona.name), *persona);
        }
    }

    #[test]
    fn an_unknown_persona_falls_back_to_default() {
        // 降级运行、或者性格表换过一轮(以前那五个是自己编的)都不该报错
        assert_eq!(Persona::by_id("乖巧"), Persona::default());
    }

    /// 表情池只能出现 [`EMOTES`] 里有的动作 —— 写错一个字就是「这只永远不做表情」。
    #[test]
    fn every_emote_in_a_persona_is_a_real_one() {
        for persona in ALL {
            for name in persona.emote_pool() {
                assert!(
                    EMOTES.iter().any(|(known, _)| *known == name),
                    "{} 的表情 {name} 不在 EMOTES 里",
                    persona.name
                );
            }
            if persona.default_emote.is_some() {
                // 默认表情必须排第一:`pick_emote` 靠这个位置加权
                assert_eq!(persona.emote_pool().first().copied(), persona.default_emote);
            }
        }
    }

    /// 游戏里那五种脸(默认/微笑/困倦/哭哭/生气)在这份名单里都要有代表,
    /// 否则「性格换眼睛」这件事在界面上看不出来。
    #[test]
    fn the_five_game_faces_are_all_represented() {
        let faces: Vec<&str> = ALL.iter().map(|p| p.face.name).collect();
        for want in ["默认", "微笑", "困倦", "哭哭", "生气"] {
            assert!(faces.contains(&want), "没有性格用「{want}」那张脸");
        }
    }

    /// 同一张脸只留一个代表 —— 名单是按「脸 + 动静」两条轴挑的,重复了就是白占位置。
    #[test]
    fn no_two_personas_share_a_non_default_face() {
        let mut seen = Vec::new();
        for persona in ALL {
            let face = persona.face.name;
            if face == DEFAULT_FACE.name {
                continue;
            }
            assert!(!seen.contains(&face), "「{face}」这张脸有两个性格在用");
            seen.push(face);
        }
        // 默认脸那几条靠动静区分:必须真的差得开(最能睡的 ÷ 最不能睡的)
        let sleepy: Vec<f32> = ALL
            .iter()
            .filter(|p| p.face.name == DEFAULT_FACE.name)
            .map(|p| p.sleepy)
            .collect();
        let (lo, hi) = (
            sleepy.iter().cloned().fold(f32::MAX, f32::min),
            sleepy.iter().cloned().fold(0.0, f32::max),
        );
        assert!(hi / lo >= 3.0, "默认脸那几条不够分明: {sleepy:?}");
    }

    /// 格子必须落在图集里(2 列 × 4 行),越界就会采到别人的脸。
    #[test]
    fn every_face_cell_is_inside_the_atlas() {
        for persona in ALL {
            let (col, row) = persona.face.cell;
            assert!(
                (col as f32) < FACE_COLS && (row as f32) < FACE_ROWS,
                "{} 的格子 {:?} 越界",
                persona.name,
                persona.face.cell
            );
            let [u, v] = persona.face.uv_offset();
            assert!((0.0..1.0).contains(&u) && (0.0..1.0).contains(&v));
        }
        // 默认那张脸必须是左上角那一格 —— 网格 UV 本来就落在那儿,偏移 0 就是原样
        assert_eq!(DEFAULT_FACE.uv_offset(), [0.0, 0.0]);
    }
}
