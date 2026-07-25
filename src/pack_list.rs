//! `--list`:列出包目录里的宠物包。
//!
//! 「按需启用」目前就是这个:看有哪些包,然后 `--pack <名字>` 或写进配置。
//! 更花哨的启用/停用管理(GUI)等真有一堆包了再说。

use std::path::Path;

use anyhow::Result;

use crate::pack::Pack;

pub fn run(packs_dir: Option<&Path>) -> Result<()> {
    let Some(dir) = packs_dir else {
        println!("定不出包目录(HOME/XDG_DATA_HOME 都没有),用 --packs-dir 指定");
        return Ok(());
    };
    let packs = Pack::list(dir);
    if packs.is_empty() {
        println!("{} 里没有宠物包。", dir.display());
        println!(
            "用导出器生成一个:dotnet run --project exporter -- --species 3001 --out {}",
            dir.display()
        );
        return Ok(());
    }
    println!("包目录 {}({} 个包):", dir.display(), packs.len());
    for pack in &packs {
        let size = dir_size(&pack.dir);
        println!(
            "  {} (id {}) — {} 个形态,{:.1}MB",
            pack.species_name,
            pack.species_id,
            pack.forms.len(),
            size as f64 / 1024.0 / 1024.0
        );
        for form in &pack.forms {
            println!(
                "      stage {} {} ({})  高 {:.0}cm  {} 个动作",
                form.stage,
                form.name,
                form.asset,
                form.height_cm,
                form.clips.len()
            );
        }
    }
    println!("\n用法:rocom-pets --pack <物种名或目录名>,或写进配置文件的 pack =");
    Ok(())
}

/// 递归统计目录大小(只为在列表里给个量级,失败就算 0)。
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => dir_size(&entry.path()),
            Ok(_) => entry.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}
