//! 把**炫彩共享贴图**烘进二进制,这样装好的 exe 不必再单独摆一份素材目录。
//!
//! # 为什么是构建期,而不是把图放进仓库
//!
//! 本仓库只有代码与导出器,**不包含也不分发任何游戏素材**(见 README 末尾)。所以这里烘的
//! 是**构建这台机器上、用自己的游戏 pak 导出来的那一份** —— 仓库仍然干净,而谁构建谁就得
//! 先有素材。这和「宠物包也要自己导」是同一条规矩。
//!
//! 顺带提醒:烘进去之后,**那个 exe 里就带着游戏素材了**,自己留着用没问题,往外发就是
//! 另一回事了。
//!
//! # 找素材的顺序
//!
//! 1. `$ROCOM_GLASSY_DIR` —— 显式指定;设成空字符串 = **明确不烘**(要一个小二进制时用)
//! 2. `<仓库>/packs/glassy` —— `--out packs` 是 README 里给的默认导出位置
//! 3. `<默认包目录>/glassy`(Linux `~/.local/share/rocom-pets/packs/glassy`)——
//!    导出器写的是 `<out>/glassy`,而 `--out` 通常就指着放包的那个目录
//! 4. 默认数据目录旁边的 `glassy/`(Linux `~/.local/share/rocom-pets/glassy`)
//!
//! 3 和 4 都要看:导出器把 glassy 写在 `--out` **里面**,而 `--out` 到底指包目录本身
//! 还是它的上一级,两种用法都有人用。只找 4 的话「导到自己放包的地方」就找不着 ——
//! 这条踩过。
//!
//! 一个都没有就烘 0 张,不会构建失败 —— **没有素材不是错误**:很多人先 `cargo build`
//! 再去导包。那时运行时会说一句「这个二进制没带炫彩素材」,界面上那几档是灰的。
//!
//! # `rerun-if-changed` 只登记**真实存在**的路径
//!
//! cargo 把指向**不存在**路径的 `rerun-if-changed` 当成「永远是脏的」
//! (fingerprint 里是 `StaleItem(MissingFile)`),于是构建脚本每次都重跑、整个 crate
//! 跟着重编 —— release 带 LTO 是**两分多钟**,`cargo run --release` 每次都要等。
//! 查证办法:`CARGO_LOG=cargo::core::compiler::fingerprint=trace cargo build --release`,
//! 它会直说 `stale: missing "<路径>"`。
//!
//! **也不能改成登记父目录**:三个候选的父目录不是包目录就是它的上一级,而 cargo 对
//! 目录是**递归**看 mtime 的(实测:改子目录深处的文件照样触发),于是每导一个宠物包
//! 都会赔上一次两分钟的重编。
//!
//! 代价是「素材从无到有」那一次叫不醒构建脚本 —— 那时一条候选都没登记。这一步用
//! **`ROCOM_GLASSY_DIR=<目录> cargo build`** 过去:那是 `rerun-if-env-changed` 跟踪的,
//! 一定叫得醒。找不到素材时的 warning 里就写着这条命令,而且每次构建都会重放。
//!
//! (更早还错过一版:只在「找到了」的分支里登记。那样空烘一次之后连
//! `ROCOM_GLASSY_DIR` 之外的触发器都没有,导完包再 `cargo build` 什么都不会发生。)

use std::path::{Path, PathBuf};

/// 单张贴图的上限。防的是「`ROCOM_GLASSY_DIR` 指错了地方」—— 指到一个装满宠物贴图的
/// 目录上,二进制会悄悄涨几百 MB。炫彩那几张最大的是 1024²(约 2MB)。
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// 全部贴图的上限,同上。当前实际用量约 3.5MB。
const MAX_TOTAL_BYTES: u64 = 16 * 1024 * 1024;

fn main() {
    println!("cargo:rerun-if-env-changed=ROCOM_GLASSY_DIR");
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo 会给 OUT_DIR"));
    let generated = out_dir.join("glassy_embed.rs");

    // 网页预览那份 wasm 不提供外观变异,烘进去纯粹是让 chunk 变大。
    let wasm = std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32");
    // **只登记存在的**:不存在的路径会让 cargo 认为构建脚本永远是脏的,
    // 于是每次构建都重跑、整个 crate 重编(见模块头)。
    let existing: Vec<PathBuf> = if wasm {
        Vec::new()
    } else {
        candidates().into_iter().filter(|p| p.is_dir()).collect()
    };
    for path in &existing {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let dir = existing.into_iter().next();

    let Some(dir) = dir else {
        std::fs::write(&generated, "pub static EMBEDDED: &[(&str, &[u8])] = &[];\n")
            .expect("写得出 OUT_DIR 下的文件");
        // 空烘一句要说出来,而且**要每次构建都说** —— cargo 会重放构建脚本的
        // warning,这里正好是想要的:二进制一天没有素材,这句就该挂一天。
        // 不想看见就 `ROCOM_GLASSY_DIR=`(空),那是明确表态,下面 locate 里不走这条。
        if wasm || opted_out() {
            return;
        }
        // 显式指了路却指空,多半是打错了 —— 这种要点名说,别混在通用那句里。
        if let Some(explicit) = std::env::var_os("ROCOM_GLASSY_DIR") {
            println!(
                "cargo:warning=ROCOM_GLASSY_DIR={} 不是目录,这个二进制里「炫彩」是灰的",
                PathBuf::from(explicit).display()
            );
            return;
        }
        // 这句里的命令**要照抄得能用**:一条候选都不存在时,光 `cargo build` 是叫不醒
        // 构建脚本的(见模块头),必须走 `ROCOM_GLASSY_DIR` 这条 env 触发器。
        println!(
            "cargo:warning=没找到炫彩素材,这个二进制里「炫彩」是灰的。先导一次包\
             (素材会写到 <out>/glassy),再 `ROCOM_GLASSY_DIR=<那个 glassy 目录> cargo build`;\
             确实不想要就设 ROCOM_GLASSY_DIR= (空)把这句关掉"
        );
        return;
    };

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|read| {
            read.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")))
                .collect()
        })
        .unwrap_or_default();
    // 排序:产物要可复现,`read_dir` 的顺序是文件系统给的。
    files.sort();

    let mut body = String::from("pub static EMBEDDED: &[(&str, &[u8])] = &[\n");
    let mut total = 0u64;
    let mut count = 0usize;
    for path in &files {
        let Ok(meta) = path.metadata() else { continue };
        if meta.len() > MAX_FILE_BYTES {
            println!(
                "cargo:warning=炫彩素材 {} 有 {}MB,超过单张上限,不烘进二进制",
                path.display(),
                meta.len() / 1024 / 1024
            );
            continue;
        }
        if total + meta.len() > MAX_TOTAL_BYTES {
            println!("cargo:warning=炫彩素材总量超过上限,{} 起不再烘", path.display());
            break;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // `include_bytes!` 用绝对路径,字节不必再抄一份到 OUT_DIR。
        body.push_str(&format!(
            "    ({:?}, include_bytes!({:?})),\n",
            name,
            path.display().to_string()
        ));
        total += meta.len();
        count += 1;
    }
    body.push_str("];\n");
    std::fs::write(&generated, body).expect("写得出 OUT_DIR 下的文件");
    // **不用 `cargo:warning`**:cargo 会在**每次**构建时重放它(即使构建脚本没重跑),
    // 于是一条本来只想说一次的信息会永久挂在每次 `cargo build` 的输出顶上。
    // 成功这条走普通 println(`cargo build -vv` 看得到),运行时另有一条日志。
    println!(
        "烘进炫彩素材:{count} 张,{:.1}MB(来自 {})",
        total as f64 / 1024.0 / 1024.0,
        dir.display()
    );
}

/// `ROCOM_GLASSY_DIR=`(空)= 明确不烘,和「没设」不是一回事。
fn opted_out() -> bool {
    std::env::var_os("ROCOM_GLASSY_DIR").is_some_and(|v| v.is_empty())
}

/// 见模块头的「找素材的顺序」。**返回的是候选清单,存不存在都返回** ——
/// 调用方要先把它们全部登记成 `rerun-if-changed`,再挑第一个真存在的。
fn candidates() -> Vec<PathBuf> {
    if opted_out() {
        return Vec::new();
    }
    if let Some(explicit) = std::env::var_os("ROCOM_GLASSY_DIR") {
        return vec![PathBuf::from(explicit)];
    }
    let mut out = Vec::new();
    if let Some(manifest) = std::env::var_os("CARGO_MANIFEST_DIR") {
        out.push(PathBuf::from(manifest).join("packs/glassy"));
    }
    out.extend(default_data_glassy());
    out
}

/// 默认数据目录那两处。**要和运行时 `pack::Pack::default_dir` 算的是同一处** ——
/// 那边是 `<数据目录>/rocom-pets/packs`;炫彩既可能在**它里面**(`--out <包目录>`),
/// 也可能在**它旁边**(`--out <上一级>`),两处都看。
fn default_data_glassy() -> Vec<PathBuf> {
    let data = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share")))
    };
    let Some(root) = data.map(|d| d.join("rocom-pets")) else {
        return Vec::new();
    };
    vec![root.join("packs").join("glassy"), root.join("glassy")]
}
