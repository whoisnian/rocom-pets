//! 包内资产的读取。包有两种形态:**解开的目录**,或者一个 `.rkpet`(zip 归档)。
//!
//! 做法是「虚拟路径」:manifest 里的相对路径照旧拼在包的位置后面,于是
//! `…/packs/喵喵.rkpet/forms/Gra_MiaoMiao1_001/model.glb` 这样的路径能一路传到
//! model.rs / audio.rs。真读的时候由 [`read`] 看路径上有没有一段是 `.rkpet` ——
//! 有就开归档读余下那段,没有就是普通的 `std::fs::read`。
//!
//! **为什么不把 `Form` 里的 `PathBuf` 换成「(来源, 包内相对路径)」**:那要改材质表里
//! 二十多个字段、三张资产缓存的键、日志与阵容存档 —— 而收益只是把这一处判断挪个位置。
//! 代价是路径不再一定能 `open`,所以**包内资产一律走这个模块读**,别再直接 `fs::read`。
//!
//! 浏览器里还有第三种形态:**根本没有文件系统**。下载站的预览把包内文件逐个喂进
//! [`memory`] 那张表,虚拟路径当键 —— 上面那条「一律走这个模块读」的规矩,
//! 正好让整条加载链(`Pack::load` / `Model::load`)在 wasm 上一行不改就能跑。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// 归档包的后缀(见 docs/design.md §4.2)。
pub const PACK_EXT: &str = "rkpet";

/// manifest 在包里的位置。两种形态同名。
pub const MANIFEST: &str = "manifest.toml";

/// 路径穿过某个 `.rkpet` 吗?是的话拆成(归档文件, 包内相对路径)。
///
/// 只认**真是文件**的那一段:目录也可以叫 `喵喵.rkpet`(解开时忘了改名就会这样),
/// 那种照目录读才对。
#[cfg(not(target_arch = "wasm32"))]
fn split_archive(path: &Path) -> Option<(PathBuf, String)> {
    let mut archive = PathBuf::new();
    let mut rest = path.components();
    for component in rest.by_ref() {
        archive.push(component);
        let is_archive = archive
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case(PACK_EXT));
        if is_archive && archive.is_file() {
            // zip 的条目名恒用 `/`,不能拿 `Path::join` 拼(Windows 上会变成 `\`)
            let inner: Vec<String> = rest
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            if inner.is_empty() {
                return None; // 路径就是归档本身,不是「读它里面的东西」
            }
            return Some((archive, inner.join("/")));
        }
    }
    None
}

/// 读一份包内资产。目录包就是 `fs::read`,归档包就从 zip 里取。
#[cfg(not(target_arch = "wasm32"))]
pub fn read(path: &Path) -> Result<Vec<u8>> {
    match split_archive(path) {
        Some((archive, inner)) => read_from_archive(&archive, &inner),
        None => std::fs::read(path).with_context(|| format!("读不到 {path:?}")),
    }
}

/// 同上,浏览器版:没有文件系统,只有 [`memory`] 那张表。
#[cfg(target_arch = "wasm32")]
pub fn read(path: &Path) -> Result<Vec<u8>> {
    memory::read(path)
}

#[cfg(not(target_arch = "wasm32"))]
fn read_from_archive(archive: &Path, inner: &str) -> Result<Vec<u8>> {
    use std::io::Read;

    let file = std::fs::File::open(archive).with_context(|| format!("打不开 {archive:?}"))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .with_context(|| format!("{archive:?} 不是合法的 zip"))?;
    let mut entry = zip
        .by_name(inner)
        .with_context(|| format!("{archive:?} 里没有 {inner}"))?;
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut bytes)
        .with_context(|| format!("{archive:?} 里的 {inner} 读坏了"))?;
    Ok(bytes)
}

/// 这个位置看着像个宠物包吗?
///
/// 只有桌面版会问 —— 浏览器那边的「包」是 JS 喂进来的字节,没有目录可扫。
#[cfg(not(target_arch = "wasm32"))]
///
/// 只做**便宜的判断**(后缀 + manifest 在不在),真读得动要等 `Pack::load`。
/// 列包目录时对每一项都会问一次,不能在这儿解压。
pub fn is_pack(path: &Path) -> bool {
    if path.is_dir() {
        return path.join(MANIFEST).is_file();
    }
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(PACK_EXT))
        && path.is_file()
}

/// 读包的 manifest 文本。`root` 是包目录或 `.rkpet` 文件。
pub fn read_manifest(root: &Path) -> Result<String> {
    let bytes = read(&manifest_path(root))?;
    String::from_utf8(bytes).with_context(|| format!("{root:?} 的 {MANIFEST} 不是 UTF-8"))
}

/// manifest 的虚拟路径(报错信息里要显示,所以单独给一个)。
pub fn manifest_path(root: &Path) -> PathBuf {
    root.join(MANIFEST)
}

/// 包占多少字节。目录就递归相加,归档就是文件本身的大小。
/// 读不动算 0 —— 这个数只用来在列表里给个量级。
#[cfg(not(target_arch = "wasm32"))]
pub fn size(root: &Path) -> u64 {
    if root.is_file() {
        return std::fs::metadata(root).map(|m| m.len()).unwrap_or(0);
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => size(&entry.path()),
            Ok(_) => entry.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

/// 浏览器里的「文件系统」:一张 虚拟路径 → 字节 的表,由 JS 喂。
///
/// **按虚拟路径存**,而不是包内相对路径:这样 `Pack::load` 拼出来的
/// `<根>/forms/…/model.glb` 直接就是键,加载链一个字都不用改。
#[cfg(target_arch = "wasm32")]
pub mod memory {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static FILES: RefCell<HashMap<PathBuf, Vec<u8>>> = RefCell::new(HashMap::new());
    }

    pub fn insert(path: PathBuf, bytes: Vec<u8>) {
        FILES.with(|f| f.borrow_mut().insert(path, bytes));
    }

    /// 换一个包之前清一次 —— 不清的话上一只的贴图会一直占着内存。
    pub fn clear() {
        FILES.with(|f| f.borrow_mut().clear());
    }

    pub fn read(path: &Path) -> Result<Vec<u8>> {
        FILES.with(|f| {
            f.borrow()
                .get(path)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("没喂过 {path:?}"))
        })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    /// 造一个只有 manifest 的最小 .rkpet,用来验「虚拟路径」这条链。
    fn write_archive(path: &Path, entries: &[(&str, &[u8])]) {
        use std::io::Write;
        let file = std::fs::File::create(path).expect("该能建文件");
        let mut zip = zip::ZipWriter::new(file);
        for (name, bytes) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .expect("该能开条目");
            zip.write_all(bytes).expect("该能写");
        }
        zip.finish().expect("该能收尾");
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rocom-assets-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("该能建目录");
        dir
    }

    #[test]
    fn reads_through_a_virtual_path() {
        let dir = scratch("read");
        let archive = dir.join("喵喵.rkpet");
        write_archive(
            &archive,
            &[
                (MANIFEST, b"schema = 1"),
                ("forms/a/model.glb", b"glTF-not-really"),
            ],
        );
        // 关键:拼出来的路径在文件系统里根本不存在,但照样读得到
        let virtual_path = archive.join("forms/a/model.glb");
        assert!(!virtual_path.exists());
        assert_eq!(read(&virtual_path).expect("该读得到"), b"glTF-not-really");
        assert_eq!(read_manifest(&archive).expect("该读得到"), "schema = 1");
        assert!(is_pack(&archive));
        assert_eq!(size(&archive), std::fs::metadata(&archive).unwrap().len());
        // 归档里没有的条目要报错,而不是当成空文件
        assert!(read(&archive.join("forms/a/tex/none.png")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plain_directories_still_work() {
        let dir = scratch("plain");
        let pack = dir.join("波波拉");
        std::fs::create_dir_all(pack.join("forms/a")).expect("该能建");
        std::fs::write(pack.join(MANIFEST), "schema = 1").expect("该能写");
        std::fs::write(pack.join("forms/a/model.glb"), b"xyz").expect("该能写");
        assert!(is_pack(&pack));
        assert_eq!(read(&pack.join("forms/a/model.glb")).expect("该读得到"), b"xyz");
        assert!(size(&pack) > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_named_rkpet_is_read_as_a_directory() {
        // 解开归档时忘了改名就会长这样。判据是「那一段是不是**文件**」,不是后缀
        let dir = scratch("dirext");
        let pack = dir.join("喵喵.rkpet");
        std::fs::create_dir_all(&pack).expect("该能建");
        std::fs::write(pack.join(MANIFEST), "schema = 1").expect("该能写");
        assert_eq!(split_archive(&pack.join(MANIFEST)), None);
        assert!(is_pack(&pack));
        assert_eq!(read_manifest(&pack).expect("该读得到"), "schema = 1");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
