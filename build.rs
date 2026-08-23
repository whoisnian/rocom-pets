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
//! 3. 默认数据目录旁边的 `glassy/`(Linux `~/.local/share/rocom-pets/glassy`)
//!
//! 一个都没有就烘 0 张,运行时自动退回「读目录」那条路 —— 行为和加这个文件之前一样,
//! 不会构建失败。**没有素材不是错误**:很多人先 `cargo build` 再去导包。
//!
//! # 磁盘上的目录优先级更高
//!
//! 运行时**先看目录、再看烘进来的**(见 `pet::glassy::asset_bytes`)。这样新出一款赛季炫彩
//! 时,把新贴图丢进 `glassy/` 就能用上,不必重新编译。

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
    let dir = if wasm { None } else { locate() };

    let Some(dir) = dir else {
        std::fs::write(&generated, "pub static EMBEDDED: &[(&str, &[u8])] = &[];\n")
            .expect("写得出 OUT_DIR 下的文件");
        return;
    };
    println!("cargo:rerun-if-changed={}", dir.display());

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

/// 见模块头的「找素材的顺序」。
fn locate() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("ROCOM_GLASSY_DIR") {
        // 空字符串是**明确不烘**,不是「没设」。
        let explicit = PathBuf::from(explicit);
        if explicit.as_os_str().is_empty() {
            return None;
        }
        if !explicit.is_dir() {
            println!(
                "cargo:warning=ROCOM_GLASSY_DIR={} 不是目录,这次不烘炫彩素材",
                explicit.display()
            );
            return None;
        }
        return Some(explicit);
    }
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);
    let candidates = [Some(manifest.join("packs/glassy")), default_data_glassy()];
    candidates.into_iter().flatten().find(|p| p.is_dir())
}

/// 默认数据目录旁边那份。**要和运行时 `pack::Pack::default_dir` 算的是同一处** ——
/// 那边是 `<数据目录>/rocom-pets/packs`,炫彩在它旁边。
fn default_data_glassy() -> Option<PathBuf> {
    let data = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share")))
    };
    Some(data?.join("rocom-pets").join("glassy"))
}
