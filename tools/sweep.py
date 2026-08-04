#!/usr/bin/env python3
"""全库扫一遍:每个形态渲一格,统计**失败 / 空白 / 过曝**三类。

素材不入仓库:宠物包放 `~/Downloads/rocom/packs-all/`(`exporter --all` 的产物)。

    uv run --with numpy --with pillow python tools/sweep.py            # 全量
    uv run --with numpy --with pillow python tools/sweep.py --limit 40 # 试跑
    SWEEP_OUT=/tmp/sweep uv run … tools/sweep.py --keep                # 留下渲图

这是这个项目的**回归闸门**:改着色、改导出器之后跑一遍,三个数字都不许变差。
它抓到过的真问题:64 个形态动画为零(渲出全空)、6 个形态被取景判据全拒(空白)、
提亮改动把一批宠物冲到过曝。

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
            if kind != "ok":
                bad[kind].append((name, asset, f"不透明 {cover:.3f} 过曝 {hot:.3f}"))
            if not args.keep:
                png.unlink()
        if i % 100 == 0:
            print(f"  …{i}/{len(todo)}", flush=True)

    print(f"\n{len(todo)} 个形态:失败 {len(bad['fail'])}、空白 {len(bad['blank'])}、"
          f"过曝 {len(bad['overexposed'])}")
    for kind, label in (("fail", "失败"), ("blank", "空白"), ("overexposed", "过曝")):
        for name, asset, why in bad[kind]:
            print(f"  [{label}] {name} ({asset}) {why}")
    if args.keep:
        print(f"\n渲图留在 {out}")


if __name__ == "__main__":
    main()
