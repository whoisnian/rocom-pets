//! `--list`:列出包目录里的宠物包。
//!
//! 命令行这条路留着给脚本与排查用;点着改的那套在 `rocom-pets --settings`(见 settings/)。

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
        let size = crate::assets::size(&pack.path);
        let kind = if pack.path.is_file() { " [rkpet]" } else { "" };
        println!(
            "  {} (id {}){kind} — {} 个形态,{:.1}MB",
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
