//! 性格:一组行为倍率的预设。
//!
//! **游戏里没有这个概念**(游戏的「性格」改的是战斗属性成长,和桌宠行为无关),
//! 这一层完全是桌宠自己的:同一只喵喵,有人想要它到处跑,有人想要它趴着不动。
//!
//! 做成**倍率而不是另一套常量**:stage.rs 里那几个手感常量是调了很久的基线,
//! 性格只在它们上面乘一个系数,于是「乖巧」严格等于旧行为,不会因为加了这个功能
//! 就把所有人的宠物都改了 —— 这也是 [`Persona::default`] 必须是乖巧的原因。

/// 一个性格预设。字段都是**倍率**,1.0 = 照 stage.rs 的基线来。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Persona {
    /// 存档里写的值,也是命令与配置的键。
    pub id: &'static str,
    pub name: &'static str,
    /// 菜单/配置窗口里的一句话说明。
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
}

/// 全部预设。**第一个是默认**(见 [`Persona::default`]),顺序即菜单顺序。
pub const ALL: &[Persona] = &[
    Persona {
        id: "gentle",
        name: "乖巧",
        about: "基线性格:不吵不闹,该睡就睡",
        sleepy: 1.0,
        bored: 1.0,
        emote: 1.0,
        run: 1.0,
        social: 1.0,
    },
    Persona {
        id: "lively",
        name: "活泼",
        about: "闲不住,常换地方,稍远就跑起来",
        sleepy: 0.6,
        bored: 2.0,
        emote: 0.6,
        run: 0.6,
        social: 1.3,
    },
    Persona {
        id: "lazy",
        name: "慵懒",
        about: "很快就困,懒得走动,多半在原地做表情",
        sleepy: 2.5,
        bored: 0.4,
        emote: 1.8,
        run: 1.6,
        social: 0.8,
    },
    Persona {
        id: "clingy",
        name: "黏人",
        about: "老远就注意到同伴,爱打招呼",
        sleepy: 0.9,
        bored: 1.4,
        emote: 1.2,
        run: 0.8,
        social: 2.2,
    },
    Persona {
        id: "aloof",
        name: "高冷",
        about: "很少主动理人,动一下也是慢慢走",
        sleepy: 1.1,
        bored: 0.7,
        emote: 0.5,
        run: 2.0,
        social: 0.3,
    },
];

impl Default for Persona {
    /// 乖巧 = stage.rs 的基线。**不写性格的存档必须落在这儿**,
    /// 否则升级一次运行时,所有人的宠物脾气都变了。
    fn default() -> Self {
        ALL[0]
    }
}

impl Persona {
    /// 按存档里的 id 找。找不到就退回默认并警告 —— 存档是机器写的,
    /// 出现不认识的 id 多半是降级运行(新版本写的性格,旧版本读),不该拦住启动。
    pub fn by_id(id: &str) -> Self {
        match ALL.iter().find(|p| p.id == id || p.name == id) {
            Some(found) => *found,
            None => {
                log::warn!("不认识的性格 {id},按「{}」处理", Self::default().name);
                Self::default()
            }
        }
    }

    /// 在 [`ALL`] 里的下标(托盘菜单里选中项、命令里带的就是它)。
    pub fn index(&self) -> usize {
        ALL.iter().position(|p| p.id == self.id).unwrap_or(0)
    }

    /// 按下标取。越界就退默认(命令是跨进程/跨版本来的,不能信)。
    pub fn at(index: usize) -> Self {
        ALL.get(index).copied().unwrap_or_default()
    }

    /// 存档里要不要写这一项。默认性格不写,存档保持干净。
    pub fn saved_id(&self) -> Option<String> {
        (self.id != Self::default().id).then(|| self.id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_the_baseline_and_multiplies_to_one() {
        // 「乖巧」必须逐项等于 1.0,否则它就不是基线,而是又一个性格
        let p = Persona::default();
        assert_eq!(p.id, "gentle");
        assert_eq!((p.sleepy, p.bored, p.emote, p.run, p.social), (1.0, 1.0, 1.0, 1.0, 1.0));
        assert_eq!(p.saved_id(), None, "默认性格不该写进存档");
    }

    #[test]
    fn ids_are_unique_and_round_trip() {
        for (i, persona) in ALL.iter().enumerate() {
            assert_eq!(persona.index(), i, "{} 的下标对不上", persona.id);
            assert_eq!(Persona::at(i), *persona);
            assert_eq!(Persona::by_id(persona.id), *persona);
            // 配置窗口里显示的是中文名,手改配置的人多半照着抄
            assert_eq!(Persona::by_id(persona.name), *persona);
        }
    }

    #[test]
    fn unknown_and_out_of_range_fall_back_to_default() {
        // 降级运行(旧版本读新版本写的存档)不该报错
        assert_eq!(Persona::by_id("暴躁"), Persona::default());
        assert_eq!(Persona::at(999), Persona::default());
    }
}
