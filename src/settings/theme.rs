//! 配置窗口的排版规格:字号、行高、间距、那几个语义色。
//!
//! 全部来自设计稿的「中文排版与字号规格」那一页。收在一处的理由很实际:
//! **这些数字散在三个页面文件里的话,改一次要翻三遍,而它们必须一致** ——
//! 设计稿两栏并排的意义就是「同一份信息在两个平台上只差观感,不差结构」。
//!
//! 只做一套(不按平台分叉)。egui 不是原生控件,再怎么调也变不成 Breeze 或 Fluent;
//! 与其做两套四不像,不如做一套**结构与文案严格照设计**、观感中性的。
//! 设计稿里那些真正分平台的东西(KDE 要解释 portal 授权、Windows 不用)另说,
//! 那是内容差异,不是外观差异。

use eframe::egui;

/// 主字号:表单标签与正文。
pub const BODY: f32 = 13.0;
/// 次要说明、状态栏补充。
pub const SMALL: f32 = 12.0;
/// 分组标题(侧栏的「活跃宠物 · 3」、菜单里的分组行)。
pub const GROUP: f32 = 11.0;
/// 详情页标题。
pub const TITLE: f32 = 19.0;
/// 数值一律等宽:124% · 6.9 MB · Meta+Shift+P。
pub const MONO: f32 = 12.5;

/// 控件高度 28 / 行高 30 —— 设计稿 KDE 栏的规格。
pub const CONTROL_H: f32 = 28.0;
pub const ROW_H: f32 = 30.0;
/// 下拉选项左右要留的余量(选中标记 + 内边距 + 可能出现的滚动条位置)。
/// 量出文字宽度之后加上它,才是 popup 该有的宽度。
pub const COMBO_ITEM_PAD: f32 = 34.0;
/// 表单标签列宽(设计稿写的是 78–82)。
pub const LABEL_W: f32 = 78.0;
/// 侧栏宽度。
pub const SIDEBAR_W: f32 = 236.0;
/// 窗口初始大小。
pub const WINDOW: [f32; 2] = [900.0, 620.0];

/// 把字号规格装进 egui 的 `TextStyle`。
///
/// egui 的 `Body/Small/Monospace/Heading/Button` 五档正好够用,于是不引入自定义
/// `TextStyle` —— 那样每处都要写 `RichText::new(..).text_style(..)`,噪音大过收益。
pub fn install(ctx: &egui::Context) {
    use egui::{FontId, TextStyle};
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::proportional(TITLE)),
            (TextStyle::Body, FontId::proportional(BODY)),
            (TextStyle::Button, FontId::proportional(BODY)),
            (TextStyle::Small, FontId::proportional(SMALL)),
            (TextStyle::Monospace, FontId::monospace(MONO)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.interact_size.y = CONTROL_H;
        // 侧栏那些行要能整行点中,不是只有文字可点
        style.spacing.menu_margin = egui::Margin::same(4);
        // 可编辑的数值框也算「数值」,跟着等宽走(见 `value`)
        style.drag_value_text_style = TextStyle::Monospace;
    });
}

/// 分组标题那一行(侧栏的「活跃宠物 · 3」)。比 `Small` 再淡一档、带点字距。
pub fn group_label(ui: &mut egui::Ui, text: &str) {
    let color = ui.visuals().weak_text_color();
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(text)
            .size(GROUP)
            .color(color)
            .extra_letter_spacing(0.4),
    );
}

/// 数值:等宽、右对齐用。百分比无小数,体积一位小数 + 空格 + MB。
pub fn value(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).monospace()
}

/// 百分比 30%。**大小也用它** —— 「1.24×」要在脑子里换算一次才知道是「大了两成半」,
/// 而托盘里那三档本来就写着 50% / 100% / 150%,两边对不上更糟。
pub fn percent(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

/// 体积 6.9 MB。**带空格**,与中文之间不加空格 —— 见设计稿的中文标点一节。
pub fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
}

/// 滚动条按 KDE Breeze Dark 取色。
///
/// egui 默认拿 `fg_stroke.color` 画手柄 —— 深色主题下那是近白色,一条白杠杵在
/// 深灰表格旁边比内容本身还显眼。改成取 `bg_fill`,并按本机 Breeze Dark 实测的
/// 取色与宽度来:手柄 rgb(42,84,107)、7px 宽,**槽与背景同色**(Breeze 根本不画槽)。
///
/// **只在滚动区那一层改**:`bg_fill` 同时是滑杆轨道的颜色,全局改掉会把滑杆一起染蓝。
/// 浅色主题不动 —— 那边默认的手柄本来就是深色的,不刺眼。
pub fn scrollbar(ui: &mut egui::Ui) {
    // `solid()` 这一套正好是要的:占位不浮动(浮动条会盖住最右边那一列)、
    // 手柄取 `bg_fill` 而不是 `fg_stroke`。egui 默认那套是浮动的 10px + 近白手柄。
    let scroll = &mut ui.style_mut().spacing.scroll;
    *scroll = egui::style::ScrollStyle::solid();
    // Breeze Dark 上量到的手柄是 7 逻辑像素宽,照抄
    scroll.bar_width = 7.0;
    if !ui.visuals().dark_mode {
        return;
    }
    let visuals = &mut ui.style_mut().visuals;
    visuals.extreme_bg_color = egui::Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(42, 84, 107);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(54, 106, 133);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(61, 130, 165);
}

/// 次要说明文字(灰的一行小字)。
pub fn hint(ui: &mut egui::Ui, text: impl Into<String>) {
    let color = ui.visuals().weak_text_color();
    ui.label(egui::RichText::new(text).size(SMALL).color(color));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_follow_the_spec() {
        // 设计稿的数值规范:百分比无小数,体积一位小数 + 空格 + MB
        assert_eq!(percent(1.2384), "124%");
        assert_eq!(percent(1.0), "100%");
        assert_eq!(percent(0.3), "30%");
        assert_eq!(percent(0.0), "0%");
        assert_eq!(megabytes(6_900_000), "6.6 MB");
        assert_eq!(megabytes(0), "0.0 MB");
    }
}
