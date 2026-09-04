//! 性格:**从游戏配置表里搬过来的**一组行为倾向 + 一张脸。
//!
//! 以前这里是自己编的五个(乖巧/活泼/慵懒/黏人/高冷)。解包数据里其实有现成的:
//!
//! - `NATURE_CONF` —— 31 条性格,每条一个 `emotion_desc`,那就是这只宠物的**默认眼神**。
//!   31 条里只有 6 条不是「默认」:天真/开朗 → 微笑,懒散/悠闲 → 困倦,胆小 → 哭哭,
//!   急躁 → 生气。**眼神落在眼睛(和嘴)上** —— 是脸那张图集里换一格,
//!   不是播一段动作,见 [`Expression`]。
//! - `LLM_PET_BEHAVIOR_CONF` —— 84 条宠物行为,每条标着 `nature_id`(哪几种性格会做它)。
//!   反过来读就是「这个性格爱做哪些动作」:调皮 → happy/happy_1/jump/run_to_player,
//!   冷静 → relax/nap/deep_sleep,悠闲 → fear/fear_1/sad/run_away …
//!
//! 两张表合起来正好是要的东西:**性格决定眼神**,顺带定了它爱做哪几个表情,
//! 不用再让人手工勾表情池。
//!
//! **「眼神」与「表情」是两套词,别混**(这条口径贯穿两端的界面与文档):
//! *眼神* = 脸那张图集里的一格(默认/微笑/惊讶/生气/困倦/哭哭/闭紧/晕眩),跟着性格
//! 与动画曲线走,人只在下载站的下拉框里直接挑;*表情* = Happy/Sad 那几段**动作**
//! (开心/放松/炫耀/生气/伤心/惊恐),配置窗口里那个「表情池」说的是这一套。
//!
//! 名字与 `nature_id` 都照抄游戏,便于回表核对。**倍率那五个数字是编的** ——
//! 游戏那两张表没有「多久睡一次」这种量,只能按每种性格爱做的行为往这五个旋钮上折:
//! 爱 nap/deep_sleep 的困得快,爱 jump/run_to_player 的闲不住,爱 run_away/turn_away
//! 的不搭理人。折算依据逐条写在下面。

use crate::stage::EMOTES;

/// 眼神 = 脸那张贴图里的**一格**。**不是**「表情」那一套 —— 那是 Happy/Sad
/// 那几段动作(见 [`crate::stage::EMOTES`]),这里说的是眼睛与嘴上换的那张图。
///
/// 眼睛和嘴**各是一张** 2 列 × 4 行的图集(`M_P_Eyes` 那一族材质,材质名后缀 `_Es`/`_Mh`),
/// 网格的 UV 落在左上那一格,换眼神就是整格地偏一下 UV。八格的内容(逐格渲出来看的,
/// 以幽星光为例;抽查喵喵/火花/菊花梨/加尔/里奥,图集结构一致):
///
/// ```text
///   (0,0) 竖眼 + 弯月嘴     = 默认     (1,0) 眯眼笑 + 腮红 + 张嘴 = 微笑
///   (0,1) 圆睁眼 + 腮红     = 惊讶     (1,1) 尖角怒眼            = 生气
///   (0,2) 闭眼 + 水滴       = 困倦     (1,2) 八字垂眼 + 倒弯嘴    = 哭哭
///   (0,3) 「><」紧闭眼+大张嘴 = 闭紧     (1,3) 螺旋眼              = 晕眩
/// ```
///
/// **格号就是游戏的编号**:`col + 2·row + 1` ∈ 1..8,见 [`Expression::card`]。原来这条
/// 只是从网格脸族的顶点色推出来的,现在被动画里的 `EC_Eye` 曲线独立坐实了 ——
/// 那条曲线的值就是 `格号 × 100`,而 Happy=200、Anger=400、Sad=600、Shock=300、
/// Sleep=500 与这里的 (1,0)/(1,1)/(1,2)/(0,1)/(0,2) 逐个对上。
///
/// 哪个性格用哪一格由游戏的 `NATURE_CONF.emotion_desc` 定;格子的**位置**是把八格
/// 逐个渲出来、和三方攻略里那张「幽星光不同性格的眼睛」逐张比对出来的 ——
/// 配置表里只有「微笑」这种名字,没有下标。五种眼神与攻略图一一对上。
///
/// 另外三格(惊讶/闭紧/晕眩)配置表里没有名字,但**美术自己给形变目标起的名**能当第二证人:
/// 那套 blendshape 叫 `Zheng/Xi/Jing/Nu/Shui/Ai/Shou/Yun`(正/喜/惊/怒/睡/哀/收/晕),
/// 正好八个。把全库「只有一个非默认 `EC_Eye` 值」的动画拿来投票、看那段同时驱动了哪条
/// 同名曲线,第一名逐格对上自己的格:2→xi 62 次、3→jing 10、4→nu 96、6→ai 15、
/// 7→shou 22、8→yun 15(5 号那格 shui 21 次,`Shui` 按用法是「睡」不是「水」)。
///
/// **(0,3) 原来叫「大笑」,是误读**:全库 `EC_Eye` 里这一格 76% 出现在 Fear 段上
/// (加尔的 `Common_Fear` 是 `0=100 0.167=700 1.267=100`),其余落在落地与入睡起手。
/// 三只对照(幽星光/加尔/里奥)画的都是「><」那种用力闭紧的眼,配的嘴是大张。
/// 美术给这一格的形变目标起的名是 `Shou`(收),所以这里就叫**闭紧** ——
/// 描述的是眼睛的样子,不猜它在表达什么情绪。
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

/// 图集里另外那几格。名字取自 `NATURE_CONF.emotion_desc` 的用词;配置表里没名字的那三格
/// 里,「晕眩」用游戏行为表 `dizzy` 的官方词,「惊讶」「闭紧」照着格子里画的东西 +
/// 美术给形变目标起的名(`Jing` 惊 / `Shou` 收)起。
pub const SMILE: Expression = Expression {
    name: "微笑",
    cell: (1, 0),
};
pub const SURPRISED: Expression = Expression {
    name: "惊讶",
    cell: (0, 1),
};
pub const ANGRY: Expression = Expression {
    name: "生气",
    cell: (1, 1),
};
pub const SLEEPY: Expression = Expression {
    name: "困倦",
    cell: (0, 2),
};
pub const CRYING: Expression = Expression {
    name: "哭哭",
    cell: (1, 2),
};
/// 「><」紧闭眼 + 大张嘴。**原来叫「大笑」是误读**,见 [`Expression`] 的说明:
/// 游戏的 `EC_Eye` 曲线把这一格用在 Fear 上,美术给它起的名是 `Shou`(收)。
pub const CLENCHED: Expression = Expression {
    name: "闭紧",
    cell: (0, 3),
};
/// 螺旋眼。战斗里的那一格,`emotion_desc` 里没有它,也不在给人挑的那七格里 ——
/// 但技能循环段与 `Common_Stun` 的 `EC_Eye` 会用到(幽星光 `Skill3Loop` 的嘴就是 800),
/// 所以 [`Expression::from_card`] 要认得它。名字用游戏行为表 `dizzy` 的官方词「晕眩」。
pub const DIZZY: Expression = Expression {
    name: "晕眩",
    cell: (1, 3),
};

/// 有名字的那七格,给「让人自己挑一双眼神」的界面用(下载站的预览)。
///
/// **第八格 (1,3) 螺旋眼不在里面**:那是战斗里的「晕眩」,游戏的 `emotion_desc` 里没有它,
/// 桌宠也没有哪段动作会用到 —— 列出来只会让人点一个不属于这只宠物的眼神。
pub const EXPRESSIONS: &[Expression] = &[
    DEFAULT_FACE,
    SMILE,
    SURPRISED,
    ANGRY,
    SLEEPY,
    CRYING,
    CLENCHED,
];

/// 这段动作自带的眼神 —— **只在包里没有 `[forms.face]` 时用的兜底**。
///
/// 正经的来源是**动画自带的 `EC_Eye`/`EC_Mouth` 曲线**(见导出器的 `FaceCurves.cs`
/// 与 `pack::FaceTrack`):它逐帧、分眼与嘴、而且**每个形态自己一份**。
/// 这张表是全库投票压扁出来的一个平均值,只够给旧包兜底:
///
/// - 压扁掉了眨眼(待机段里 `EC_Eye` 100↔500 来回跳的那一路);
/// - 压扁掉了眼嘴不一致(全库 8636 段两条曲线都有的动画里约三成对不上);
/// - 压扁掉了形态差异(加灵一阶 Fear 用第 6 格,二/三阶用第 7 格 —— 同一条进化链都不同)。
///
/// 表里每一档都换成了全库投票的结果(每段动画各取自己的非默认主值,再跨全库投票),
/// 原来那版是按动作名的意思猜的,其中三档是错的:
/// Fear 猜「哭哭」实际是第 7 格「闭紧」(76%)、CallOut 猜第 7 格实际是「微笑」(48%)、
/// Alert 猜「不改脸」实际是「生气」(40%)。
pub fn face_for_clip(clip: &str) -> Option<Expression> {
    Some(match clip {
        "Anger" => ANGRY,
        "Sad" => CRYING,
        // 惊恐是「><」那格,不是八字垂眼 —— 全库 Fear 段 76% 落在第 7 格
        "Fear" => CLENCHED,
        "Shock" => SURPRISED,
        // CallOut 也是微笑那格(48%),不是张大嘴那格
        "Happy" | "Relax" | "Show" | "CallOut" => SMILE,
        // 警觉(Alert):全库 40% 是尖角怒眼(31% 是闭眼,那是段里的眨眼)
        "Alert" => ANGRY,
        "SleepStart" | "SleepLoop" | "SleepStand" | "SleepEnd" => SLEEPY,
        // 待机/走/跑/落地不改脸:平时什么样就什么样
        _ => return None,
    })
}

impl Expression {
    /// 贴图 UV 要偏多少(整格)。
    pub fn uv_offset(&self) -> [f32; 2] {
        [
            self.cell.0 as f32 / FACE_COLS,
            self.cell.1 as f32 / FACE_ROWS,
        ]
    }

    /// **网格脸**要画第几张卡(1–8),见 `pack.rs` 的 `Material::face_cards`。
    ///
    /// 那一族(`M_P_Eyes_Mesh`,全库 20 个形态、21 片网格)把整套眼神做成**八张重叠的几何**,
    /// 每张的 UV 早就钉在图集的某一格上,顶点色的 **G 通道**写着它是第几张
    /// (`floor(G × 10)`,实测取值 0.149/0.247/…/0.847,正好落在 1..8 每一档的中间)。
    /// 所以这一族不偏 UV,改成**只画一张**。
    ///
    /// 编号与格子的对应是 `col + 2·row + 1`(21 片里 16 片逐格对得上,
    /// 其余 5 片是美术摆位的出入)。**只有 1 号是例外:它不是「默认」。**
    /// 证据:① 觅觅蝠一/三阶压根没有 1 号卡,碎晶蝎与觅觅蝠二阶的 1 号卡只有十几个顶点
    /// (占位);② 翠顶夫人/黑羽夫人的 1 号卡与 5 号卡(困倦)是同一份美术;
    /// ③ 实机图鉴里这两只的待机脸是 **2 号卡**那张(尖睫毛 + 腮红)。
    /// 图鉴用的就是默认那张脸 —— 拿单卡族的点点对照过:图鉴里是圆睁的绿眼,
    /// 正是 (0,0) 那格,不是微笑那格。所以 1 号是眨眼/占位一类,默认落在 2 号。
    /// 于是「默认」与「微笑」在这一族里是同一张脸(它们只有七张)。
    pub fn card(&self) -> u32 {
        (self.cell.0 + self.cell.1 * 2 + 1).max(2)
    }

    /// 格号(1..8)→ 眼神。`card` 的逆,给 `EC_Eye`/`EC_Mouth` 曲线用
    /// (曲线的值就是 `格号 × 100`,见 `pack::FaceTrack`)。
    ///
    /// **1 号返回 None**:那是「默认」,而「默认」是哪张脸由性格说了算
    /// (`NATURE_CONF.emotion_desc`)—— 一只「哭哭眼」的幽星光在待机段里
    /// 该是哭哭眼,不是 (0,0) 那格。**这一条是推的**:游戏那边是把性格那张脸设成材质的
    /// 基础 `Number`、再让曲线在段内覆盖,数据里没有第二处能直接对照的地方;
    /// 但按「1 = 用性格那张」解释,待机眨眼(1↔5)与性格脸两件事同时成立,
    /// 换任何别的解释都会丢掉其中一件。
    ///
    /// 越界(0 或 >8)也当默认:曲线是浮点的,四舍五入之外不该再信它。
    pub fn from_card(card: u32) -> Option<Expression> {
        Some(match card {
            2 => SMILE,
            3 => SURPRISED,
            4 => ANGRY,
            5 => SLEEPY,
            6 => CRYING,
            7 => CLENCHED,
            8 => DIZZY,
            _ => return None,
        })
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
    /// 这个性格的眼神(`NATURE_CONF.emotion_desc`)—— **就是眼睛/嘴那张图集里的一格**。
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
        face: DEFAULT_FACE,
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
        face: DEFAULT_FACE,
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
        face: SMILE,
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
        face: SLEEPY,
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
        face: DEFAULT_FACE,
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
        face: CRYING,
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
        face: ANGRY,
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

    /// 游戏里那五种眼神(默认/微笑/困倦/哭哭/生气)在这份名单里都要有代表,
    /// 否则「性格换眼睛」这件事在界面上看不出来。
    #[test]
    fn the_five_game_faces_are_all_represented() {
        let faces: Vec<&str> = ALL.iter().map(|p| p.face.name).collect();
        for want in ["默认", "微笑", "困倦", "哭哭", "生气"] {
            assert!(faces.contains(&want), "没有性格用「{want}」那双眼神");
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
        // 动作带来的那几张也一样(它们和性格用的是同一批常量)
        for face in [SMILE, SURPRISED, ANGRY, SLEEPY, CRYING, CLENCHED, DIZZY] {
            let (col, row) = face.cell;
            assert!(
                (col as f32) < FACE_COLS && (row as f32) < FACE_ROWS,
                "「{}」的格子 {:?} 越界",
                face.name,
                face.cell
            );
        }
    }

    /// 网格脸的卡号:1..8 之内,而且 1 号永远不选(它不是「默认」,见 `Expression::card`)。
    /// 每格一号、号不重复 —— 除了「默认」与「微笑」共用 2 号,那一族只有七张脸。
    #[test]
    fn face_cards_are_one_per_cell_and_never_number_one() {
        let faces = [
            DEFAULT_FACE,
            SMILE,
            SURPRISED,
            ANGRY,
            SLEEPY,
            CRYING,
            CLENCHED,
            DIZZY,
        ];
        for face in faces {
            let card = face.card();
            assert!(
                (2..=8).contains(&card),
                "「{}」的卡号 {card} 不在 2..8 里",
                face.name
            );
        }
        // 网格脸只有七张:「默认」与「微笑」共用 2 号
        assert_eq!(DEFAULT_FACE.card(), SMILE.card());
        // 除那一对外,别的格子不许撞号 —— 撞了就是有张脸永远轮不到
        let others = [SURPRISED, ANGRY, SLEEPY, CRYING, CLENCHED, DIZZY];
        let mut seen = vec![DEFAULT_FACE.card()];
        for face in others {
            assert!(
                !seen.contains(&face.card()),
                "「{}」的卡号 {} 和别人撞了",
                face.name,
                face.card()
            );
            seen.push(face.card());
        }
        // 卡号是**按行读**图集:(列, 行) → 列 + 行×2 + 1。抽两格钉住这个换算。
        assert_eq!(ANGRY.card(), 4, "生气在 (1,1)");
        assert_eq!(CLENCHED.card(), 7, "闭紧在 (0,3)");
    }

    /// 格号 ↔ 眼神的来回。**`EC_Eye`/`EC_Mouth` 曲线就是靠这一对认格子的**,
    /// 反过来错一格就是全库眼神整体串位。
    #[test]
    fn cards_round_trip_through_from_card() {
        for face in [SMILE, SURPRISED, ANGRY, SLEEPY, CRYING, CLENCHED, DIZZY] {
            assert_eq!(
                Expression::from_card(face.card()),
                Some(face),
                "第 {} 格该是「{}」",
                face.card(),
                face.name
            );
        }
        // 1 号是「默认」——那是性格那张脸,不是图集左上角那一格,所以不给具体眼神
        assert_eq!(Expression::from_card(1), None);
        // 越界一律当默认:曲线是浮点的,四舍五入之外不该再信它
        assert_eq!(Expression::from_card(0), None);
        assert_eq!(Expression::from_card(9), None);
        // 游戏曲线的值就是格号 ×100,这几档是全库实测最常见的
        assert_eq!(Expression::from_card(200 / 100), Some(SMILE), "Happy=200");
        assert_eq!(Expression::from_card(400 / 100), Some(ANGRY), "Anger=400");
        assert_eq!(Expression::from_card(600 / 100), Some(CRYING), "Sad=600");
        assert_eq!(Expression::from_card(700 / 100), Some(CLENCHED), "Fear=700");
    }

    /// 兜底表:会换脸的动作与不换脸的动作,两边都点名核一遍。
    /// **这张表只给没有 `[forms.face]` 的旧包用**,值来自全库 `EC_Eye` 投票
    /// (见 `face_for_clip` 的说明),不是猜的 —— 改动前先回那份统计。
    #[test]
    fn actions_map_to_the_faces_they_say_they_do() {
        for (clip, want) in [
            ("Anger", ANGRY),
            ("Sad", CRYING),
            // Fear 是第 7 格「闭紧」,不是哭哭 —— 全库 EC_Eye 投票 76%
            ("Fear", CLENCHED),
            ("Shock", SURPRISED),
            ("Happy", SMILE),
            ("Relax", SMILE),
            ("Show", SMILE),
            // CallOut 也是微笑那格(48%),不是第 7 格那张大张嘴的
            ("CallOut", SMILE),
            // 警觉(Alert)是尖角怒眼(40%)
            ("Alert", ANGRY),
            ("SleepStart", SLEEPY),
            ("SleepLoop", SLEEPY),
            ("SleepEnd", SLEEPY),
            // 降级用的那段也得认:幽星光那批只有它
            ("SleepStand", SLEEPY),
        ] {
            let got = face_for_clip(clip);
            assert_eq!(got, Some(want), "{clip} 该是「{}」", want.name);
        }
        // 日常那几段不改脸,否则性格给的那张脸基本没机会露面
        for clip in ["Idle", "Walk", "Run", "JumpFall"] {
            assert_eq!(face_for_clip(clip), None, "{clip} 不该改脸");
        }
    }
}
