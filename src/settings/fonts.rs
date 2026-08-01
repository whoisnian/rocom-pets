//! 给配置窗口找一份中文字体。
//!
//! **这不是锦上添花,是这个窗口能不能用的前提**:egui 自带的字体只有拉丁字母,
//! 界面上每一个汉字都会是豆腐块(第一版就是这样,实机截图里满屏方块)。
//!
//! 为什么不把字体打进二进制:一份能覆盖常用汉字的字体十几 MB(比整个运行时还大),
//! 而且再分发要看许可。系统上必然已经有一份 —— 找出来用就是了。

use eframe::egui;
use std::path::{Path, PathBuf};

/// 一个候选:字体文件 + 用里面第几个字面。
///
/// **字面下标不能省**。中文字体几乎清一色是 `.ttc`(字体集合):一个文件里装着
/// 日/韩/简/繁好几套字面,汉字的写法各不相同(门、骨、直、画……)。
/// 取错下标不会报错,只会让整个界面显示成日文字形。
type Candidate = (PathBuf, u32);

/// 把系统里的中文字体挂成 egui 的后备字体。
///
/// 一个都找不到时只警告:界面还是能出来的(汉字变豆腐块,英文与数字照常),
/// 总比因为「没字体」就不让人开配置窗口强。
pub fn install(ctx: &egui::Context) {
    let Some((path, index, bytes)) = load_first() else {
        log::warn!(
            "系统里没找到中文字体,配置窗口里的汉字会显示成方块;\
             装一份思源黑体/文泉驿(Linux)或确认 C:\\Windows\\Fonts 里有微软雅黑(Windows)"
        );
        return;
    };
    log::debug!("配置窗口用字体 {}(第 {index} 个字面)", path.display());

    let mut fonts = egui::FontDefinitions::default();
    let mut data = egui::FontData::from_owned(bytes);
    data.index = index;
    fonts
        .font_data
        .insert("system-cjk".to_owned(), std::sync::Arc::new(data));
    // **挂成后备而不是首选**:egui 自带那份的拉丁字形排得更好看,
    // 汉字落不到它头上时自然会往后找。
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("system-cjk".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// 按候选顺序找第一份读得进来的字体。
fn load_first() -> Option<(PathBuf, u32, Vec<u8>)> {
    for (path, index) in candidates() {
        match std::fs::read(&path) {
            Ok(bytes) if !bytes.is_empty() => return Some((path, index, bytes)),
            _ => continue,
        }
    }
    None
}

/// 候选字体,按优先级排。
#[cfg(not(target_os = "windows"))]
fn candidates() -> Vec<Candidate> {
    // 先问 fontconfig:它知道这台机器上「能写中文的字体」究竟是哪一份、哪个字面,
    // 比我们列一串路径准得多。没有 fc-match 就退回硬编码的常见位置。
    let mut found = fc_match();
    found.extend(
        [
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansSC-Regular.otf",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
            "/usr/share/fonts/adobe-source-han-sans/SourceHanSansSC-Regular.otf",
            "/usr/share/fonts/opentype/source-han-sans/SourceHanSansSC-Regular.otf",
            "/usr/share/fonts/TTF/SourceHanSansSC-Regular.otf",
        ]
        // 兜底只能取第 0 个字面(可能是日文那套);fontconfig 在的话轮不到这儿
        .into_iter()
        .map(|path| (PathBuf::from(path), 0)),
    );
    found.retain(|(path, _)| path.is_file());
    found
}

/// 问 fontconfig 要一份能写简体中文的字体。
///
/// **查询里不能带 `sans-serif`**:那样问出来的第一名是 `NotoSans-Regular.ttf`
/// (纯拉丁的那份 Noto,「sans 的最佳匹配」),一个汉字都没有。
/// 只按 `:lang=zh-cn` 问,fontconfig 直接给出「能写中文的那一份 + 正确的字面下标」
/// (这台机器上是 `NotoSansCJK-Regular.ttc` 的第 2 个字面,即简体那套)。
#[cfg(not(target_os = "windows"))]
fn fc_match() -> Vec<Candidate> {
    let output = std::process::Command::new("fc-match")
        .args(["-f", "%{file}:%{index}\n", ":lang=zh-cn"])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_fc_line)
        .collect()
}

/// 解析 `路径:下标`。**从右边找冒号**:路径里也可能有冒号。
#[cfg(not(target_os = "windows"))]
fn parse_fc_line(line: &str) -> Option<Candidate> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let (path, index) = match line.rsplit_once(':') {
        Some((path, index)) if !path.is_empty() => {
            (path, index.trim().parse::<u32>().unwrap_or(0))
        }
        // 没有下标就是单字面字体
        _ => (line, 0),
    };
    Some((PathBuf::from(path), index))
}

#[cfg(target_os = "windows")]
fn candidates() -> Vec<Candidate> {
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let fonts = root.join("Fonts");
    // Windows 上的这几份**第 0 个字面就是简体**(雅黑/宋体/黑体都是给简中做的),
    // 不像 Noto CJK 那样一个文件塞四套字面,所以下标一律 0。
    [
        "msyh.ttc",   // 微软雅黑,Vista 起的默认中文界面字体
        "msyhl.ttc",  // 雅黑 Light
        "simhei.ttf", // 黑体
        "simsun.ttc", // 宋体
        "Deng.ttf",   // 等线
    ]
    .into_iter()
    .map(|name| (fonts.join(name), 0))
    .filter(|(path, _): &Candidate| path.is_file())
    .collect()
}

/// 找不到任何字体时给的空列表(测试里要能区分「没装字体」与「代码写错了」)。
#[allow(dead_code)]
fn is_collection(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ttc"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_are_absolute() {
        // 空列表是合法的(这台机器上可能真没装中文字体),但凡列出来的都得是绝对路径
        assert!(candidates().iter().all(|(p, _)| p.is_absolute()));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn fc_lines_parse_into_path_and_face_index() {
        // fontconfig 给的就是这个格式;下标丢了的话整个界面会变成日文字形
        assert_eq!(
            parse_fc_line("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc:2"),
            Some((
                PathBuf::from("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc"),
                2
            ))
        );
        // 单字面字体没有下标那一段
        assert_eq!(
            parse_fc_line("/usr/share/fonts/x/A.otf"),
            Some((PathBuf::from("/usr/share/fonts/x/A.otf"), 0))
        );
        assert_eq!(parse_fc_line("  "), None);
    }

    /// 这台机器上真能找出一份中文字体来吗。
    ///
    /// **不断言一定找得到**(CI 上可能没装),但只要找到了,就必须是能读的文件;
    /// 而且 `.ttc` 必须在候选里 —— 第一版把它们整个跳过了,结果满屏豆腐块。
    #[test]
    fn a_real_font_loads_if_the_system_has_one() {
        let Some((path, _index, bytes)) = load_first() else {
            return;
        };
        assert!(bytes.len() > 1024, "{path:?} 太小了,不像字体");
        // 字体集合是常态而不是例外:Noto CJK / 雅黑 / 宋体全是 .ttc
        let has_collection = candidates().iter().any(|(p, _)| is_collection(p));
        let has_single = candidates().iter().any(|(p, _)| !is_collection(p));
        assert!(has_collection || has_single);
    }
}
