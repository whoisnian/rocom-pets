//! 把**炫彩共享贴图**烘进二进制,这样装好的 exe 不必再单独摆一份素材目录。
//!
//! # 素材在仓库里(`assets/glassy`)
//!
//! 这 13 张(3.5MB)是**全库共用**的花纹与粒子图,不属于任何一只宠物,导出器在正常导包时
//! 也会往 `<out>/glassy` 写一份。它们随仓库走,于是 `cargo build` 在**任何**机器上都能编出
//! 带炫彩的二进制 —— 不必先备齐几十 GB 的游戏数据再导一次包。
//!
//! 这是本仓库「不含游戏素材」那条规矩的**唯一例外**,理由是它卡住了独立构建;宠物包
//! (201 个 / 1.7GB)仍然要自己导,见 README 末尾。
//!
//! 顺带提醒:烘进去之后,**那个 exe 里就带着这几张图了**,自己留着用没问题,往外发就是
//! 另一回事了。
//!
//! # wasm 不烘
//!
//! 网页预览是点开才下的一个 chunk,把 13 张图烘进去等于让每个点开预览的人都先付 3.5MB,
//! 而其中最大的一张自己就有 2MB、多数人一次也用不上。浏览器那边改成**按需 fetch**,
//! 见 `src/pet/glassy.rs` 的 `runtime_store`。
//!
//! # `rerun-if-changed` 登记的是目录
//!
//! cargo 对目录是递归看 mtime 的,而 `assets/glassy` 是仓库里的静态资源、平时不动,
//! 登记它既能认出增删也不会误触发。(**不能登记不存在的路径** —— cargo 会把那当成
//! 「永远是脏的」,构建脚本每次重跑、整个 crate 跟着重编;release 带 LTO 是两分多钟。
//! 这一条踩过,查证办法:`CARGO_LOG=cargo::core::compiler::fingerprint=trace cargo build`。)

use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo 会给 OUT_DIR"));
    let generated = out_dir.join("glassy_embed.rs");

    // 网页预览那份 wasm 不提供外观变异,烘进去纯粹是让 chunk 变大。
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        std::fs::write(&generated, "pub static EMBEDDED: &[(&str, &[u8])] = &[];\n")
            .expect("写得出 OUT_DIR 下的文件");
        return;
    }

    let manifest = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo 会给 CARGO_MANIFEST_DIR"),
    );
    let dir = manifest.join("assets/glassy");
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("读不到炫彩素材目录 {}:{e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")))
        .collect();
    // 排序:产物要可复现,`read_dir` 的顺序是文件系统给的。
    files.sort();
    // 素材是仓库的一部分,空了就是检出坏了 —— 静默编出一个「炫彩是灰的」二进制更糟。
    assert!(!files.is_empty(), "{} 里一张 png 都没有", dir.display());

    let mut body = String::from("pub static EMBEDDED: &[(&str, &[u8])] = &[\n");
    for path in &files {
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // `include_bytes!` 用绝对路径,字节不必再抄一份到 OUT_DIR。
        body.push_str(&format!(
            "    ({:?}, include_bytes!({:?})),\n",
            name,
            path.display().to_string()
        ));
    }
    body.push_str("];\n");
    std::fs::write(&generated, body).expect("写得出 OUT_DIR 下的文件");
    // **不用 `cargo:warning`**:cargo 会在**每次**构建时重放它(即使构建脚本没重跑),
    // 于是一条本来只想说一次的信息会永久挂在每次 `cargo build` 的输出顶上。
    // 这条走普通 println(`cargo build -vv` 看得到),运行时另有一条日志。
    println!("烘进炫彩素材:{} 张(来自 {})", files.len(), dir.display());
}
