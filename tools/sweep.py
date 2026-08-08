#!/usr/bin/env python3
"""全库扫一遍:每个形态渲一格,统计**失败 / 空白 / 过曝**三类。

素材不入仓库:宠物包放 `~/Downloads/rocom/packs-all/`(`exporter --all` 的产物)。

    uv run --with numpy --with pillow python tools/sweep.py            # 全量
    uv run --with numpy --with pillow python tools/sweep.py --limit 40 # 试跑
    SWEEP_OUT=/tmp/sweep uv run … tools/sweep.py --keep                # 留下渲图

这是这个项目的**回归闸门**:改着色、改导出器之后跑一遍,三个数字都不许变差。
它抓到过的真问题:64 个形态动画为零(渲出全空)、6 个形态被取景判据全拒(空白)、
提亮改动把一批宠物冲到过曝。

**已知未实装的形态从三个数字里排除**(见 `KNOWN_UNRELEASED`),但仍然照常渲、
照常打印 —— 只是单独一行,不计进闸门。它们的资产还在做,拿它们当回归信号只会长期
盖着一个不会变的数。

## 判据

- **失败**:渲染进程非零退出或没出 PNG。
- **空白**:不透明像素占比 < 0.5% —— 模型没画出来,或者取景把它框在画外。
- **过曝**:主体里 `min(r,g,b) > 0.96` 的像素占比 > 15% —— 大片糊白。
  阈值定在 0.96/15% 是照「肉眼明显糊白」标的;换阈值会让绝对数字整体平移,
  **只和同一阈值下的历史数字比**(踩过一次:重写脚本时换了阈值,过曝从 2 变 9,
  当时误判成回归)。

**这个脚本不入渲图缓存** —— 每次都重渲。cmp_shots.py 那边的缓存曾经让改动前后的
数字一行都不变,见那个文件的坑 3。
"""
import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image

# 同 `cmp_shots.py`:`SWEEP_PACKS` 指到别处就能扫另一批包(改完导出器重导一份再扫)
PACKS = Path(os.environ.get("SWEEP_PACKS", Path.home() / "Downloads/rocom/packs-all"))
BIN = Path(__file__).resolve().parent.parent / "target/release/rocom-pets"

BLANK_COVER = 0.005
OVEREXP_LEVEL = 0.96
OVEREXP_SHARE = 0.15

# **已知未实装、不计进闸门的形态**:asset → 为什么。
#
# 进这张表的门槛是「问题出在资产本身,而且这只宠物游戏里还没上」,不是「我们画不好」。
# 每条都要写清楚查到了什么,免得日后被当成挡箭牌。导出器那边已经挡掉了「材质资产全部
# 悬空」的 13 个形态(见 Program.cs),这里只收它挡不住的。
KNOWN_UNRELEASED = {
    # 5753 个顶点里有 98 个停在 y ≈ -28.39 m(身体本身只有 3 m 高),权重绑在
    # `Bone001` 上,而那根骨骼的绑定姿势局部平移就是 (0, -28.394, 0)。包围盒因此高
    # 31.6 m、`height_cm` 写成 3162 —— 全库第二高的圣羽翼王才 426,是 7.4 倍的孤点。
    # 取景按包围盒算,于是整只被缩成画面顶上一条(不透明占比 0.001)。
    # **这不是着色/取景的 bug,是未实装资产里挂着一块没归位的几何**;
    # 为它去改包围盒规则会改写全部 827 个形态的 `height_cm`,连带改掉桌面上的显示尺寸。
    "Dem_JingJiLong2_001": "惩戒巨笼:游戏内尚未正式出现;98 个顶点挂在地下 28m 的骨骼上,撑爆包围盒",
}


def forms(manifest: Path):
    """从 manifest.toml 里摘出 (形态名, asset)。只要这两个字段,不引 toml 依赖。"""
    out, name = [], None
    for line in manifest.read_text().splitlines():
        s = line.strip()
        if s.startswith("name = "):
            name = s.split("=", 1)[1].strip().strip('"')
        elif s.startswith("asset = "):
            asset = s.split("=", 1)[1].strip().strip('"')
            out.append((name or asset, asset))
    return out


def classify(png: Path):
    """→ ('ok'|'blank'|'overexposed', 不透明占比, 过曝占比)"""
    a = np.asarray(Image.open(png).convert("RGBA"), dtype=np.float32) / 255.0
    alpha = a[..., 3]
    cover = float((alpha > 0.5).mean())
    if cover < BLANK_COVER:
        return "blank", cover, 0.0
    body = a[..., :3][alpha > 0.5]
    hot = float((body.min(axis=1) > OVEREXP_LEVEL).mean())
    return ("overexposed" if hot > OVEREXP_SHARE else "ok"), cover, hot


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--limit", type=int, help="只扫前 n 个形态(试跑)")
    ap.add_argument("--keep", action="store_true", help="留下渲图(配合 SWEEP_OUT)")
    ap.add_argument("--time", default="0.7", help="喂给 shader 的时间,默认 0.7")
    args = ap.parse_args()

    if not BIN.exists():
        sys.exit(f"先 cargo build --release:{BIN} 不存在")
    out = Path(os.environ.get("SWEEP_OUT", tempfile.mkdtemp(prefix="sweep-")))
    out.mkdir(parents=True, exist_ok=True)

    todo = [(pack, name, asset)
            for pack in sorted(PACKS.iterdir()) if (pack / "manifest.toml").exists()
            for name, asset in forms(pack / "manifest.toml")]
    if args.limit:
        todo = todo[: args.limit]

    bad = {"fail": [], "blank": [], "overexposed": []}
    skipped = []
    for i, (pack, name, asset) in enumerate(todo, 1):
        png = out / f"{asset}.png"
        r = subprocess.run([str(BIN), "--render", str(pack), "--form", asset,
                            "--clips", "Idle", "--yaw", "25", "--size", "320",
                            "--time", args.time, "--no-fade", "-o", str(png)],
                           capture_output=True)
        if r.returncode != 0 or not png.exists():
            bad["fail"].append((name, asset, (r.stderr or b"").decode()[-160:]))
        else:
            kind, cover, hot = classify(png)
            if kind != "ok" and asset in KNOWN_UNRELEASED:
                skipped.append((name, asset, kind, f"不透明 {cover:.3f} 过曝 {hot:.3f}"))
            elif kind != "ok":
                bad[kind].append((name, asset, f"不透明 {cover:.3f} 过曝 {hot:.3f}"))
            if not args.keep:
                png.unlink()
        if i % 100 == 0:
            print(f"  …{i}/{len(todo)}", flush=True)

    label = {"fail": "失败", "blank": "空白", "overexposed": "过曝"}
    print(f"\n{len(todo)} 个形态(不计已知未实装 {len(skipped)} 个):"
          f"失败 {len(bad['fail'])}、空白 {len(bad['blank'])}、过曝 {len(bad['overexposed'])}")
    for kind in ("fail", "blank", "overexposed"):
        for name, asset, why in bad[kind]:
            print(f"  [{label[kind]}] {name} ({asset}) {why}")
    for name, asset, kind, why in skipped:
        print(f"  [不计:{label[kind]}] {name} ({asset}) {why} —— {KNOWN_UNRELEASED[asset]}")
    if args.keep:
        print(f"\n渲图留在 {out}")


if __name__ == "__main__":
    main()
