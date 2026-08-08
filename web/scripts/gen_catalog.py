#!/usr/bin/env python3
"""生成下载站的目录数据 public/catalog.json,以及头像精灵图 public/sprite.webp。

两种数据源,按需选一个:

  --packs DIR   真实模式。扫目录下的 *.rkpet,算 size/sha256,读包内 manifest.toml
                取形态构成。出来的目录和你要上传到 R2 的那批文件逐字对应。

  --index PATH  演示模式。读 docs/petindex.md 的「## 清单」一节,只有包名与形态名,
                size 填 0、sha256 留空。用来在没有 1.4GB 包的机器上把页面跑起来,
                前端会把这类条目标成「待上传」。

头像来自隔壁 rocom-petvo 的 sprite.webp + data.js(--petvo,默认 ../rocom-petvo):
按形态中文名去 data.js 里查精灵图序号。petvo 只收录了有图鉴号且有 Common_Happy
事件的 425 只,所以「000」那批包和少数形态查不到 —— 记 null,前端退回文字头像。

用法:
  scripts/gen_catalog.py --packs ~/Downloads/rocom/packs-all-v2 \
                         --apps dist-bin --version 0.1.0
  scripts/gen_catalog.py --index ../docs/petindex.md          # 无包时的演示目录
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import re
import shutil
import sys
import tomllib
import zipfile
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
WEB = HERE.parent
REPO = WEB.parent

# 应用本体的文件名 → (id, 平台, 展示名)。认不出来的文件会被跳过并打一行警告。
APP_PATTERNS: list[tuple[re.Pattern[str], str, str, str]] = [
    (re.compile(r"\.exe$", re.I), "app-windows-x64", "windows", "Windows 10+ (x64)"),
    (re.compile(r"\.(AppImage|tar\.(gz|xz|zst))$", re.I), "app-linux-x64", "linux", "Linux (x64)"),
]


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        # 1.4GB / 200 个文件,一次读 4MB 比默认的 8KB 快一个量级
        for chunk in iter(lambda: f.read(4 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


# ---------------------------------------------------------------- 头像精灵图


def load_sprite_index(petvo: Path) -> tuple[dict[str, int], dict]:
    """从 rocom-petvo/data.js 解出 中文名 → 精灵图序号,以及精灵图的几何参数。

    data.js 是「注释 + const COLS=… + const PETS=[…]」的手写 JS,不是 JSON,
    但 PETS 那个数组本身是严格 JSON,截出来直接 loads 即可。
    """
    js = (petvo / "data.js").read_text("utf-8")
    head = js[: js.index("const PETS=")]
    cols = int(re.search(r"COLS\s*=\s*(\d+)", head).group(1))
    cell = int(re.search(r"\bD\s*=\s*(\d+)", head).group(1))
    pets = json.loads(js[js.index("[", js.index("const PETS=")) : js.rindex("];") + 1])

    # 同名的取序号最小的那个(data.js 里 419 个唯一名 / 425 条,重名的是不同图鉴号
    # 的同名宠,头像本就该各归各的 —— 但按名字查只能给一个,取先出现的最稳定)。
    index: dict[str, int] = {}
    for p in pets:
        index.setdefault(p["name"], p["i"])
    return index, {"cols": cols, "cell": cell, "count": len(pets)}


def copy_sprite(petvo: Path, out_dir: Path) -> None:
    src = petvo / "sprite.webp"
    dst = out_dir / "sprite.webp"
    if not src.exists():
        sys.exit(f"找不到 {src} —— 用 --petvo 指到 rocom-petvo 目录")
    shutil.copyfile(src, dst)
    print(f"  头像精灵图 {src} → {dst} ({dst.stat().st_size / 1e6:.1f}MB)")


# ---------------------------------------------------------------- 真实模式


def stage_of(form: dict) -> int:
    """王者形态排最后。资产名以 Bo_001 结尾的是王者,配置里的 stage 不可靠
    (它是相对本条链的,见 docs/petindex.md 规则 4)。"""
    if re.search(r"Bo_\d+$", form.get("asset", "")):
        return 99
    return int(form.get("stage", 1) or 1)


def read_pack(path: Path, sprite_index: dict[str, int]) -> dict:
    stem = path.stem  # 002-喵喵
    book, _, name = stem.partition("-")
    with zipfile.ZipFile(path) as z:
        manifest = tomllib.loads(z.read("manifest.toml").decode("utf-8"))

    # 同名多条 = 同一形态的多种外观(晶石蜗x6),合成一条并记 skins
    grouped: dict[str, dict] = {}
    for raw in manifest.get("forms", []):
        fname = raw["name"]
        if fname in grouped:
            grouped[fname]["skins"] += 1
            continue
        grouped[fname] = {
            "name": fname,
            "asset": raw.get("asset", ""),
            "stage": stage_of(raw),
            "skins": 1,
            "sprite": sprite_index.get(fname),
        }
    forms = sorted(grouped.values(), key=lambda f: (f["stage"], f["name"]))

    return {
        "id": stem,
        "book": book if book.isdigit() else "000",
        "name": name or stem,
        "key": f"packs/{path.name}",
        "size": path.stat().st_size,
        "sha256": sha256_of(path),
        "forms": forms,
        "sprite": sprite_index.get(manifest.get("species", {}).get("name", name)),
        "_source_version": manifest.get("source_version", ""),
    }


def scan_packs(packs_dir: Path, sprite_index: dict[str, int]) -> tuple[list[dict], str]:
    files = sorted(packs_dir.glob("*.rkpet"))
    if not files:
        sys.exit(f"{packs_dir} 里没有 .rkpet —— 先用导出器出包:\n"
                 f"  dotnet run --project exporter -- --all --zip-only --out packs")
    out: list[dict] = []
    versions: collections.Counter[str] = collections.Counter()
    for i, f in enumerate(files, 1):
        pack = read_pack(f, sprite_index)
        versions[pack.pop("_source_version")] += 1
        out.append(pack)
        print(f"\r  [{i}/{len(files)}] {f.name}".ljust(60), end="", file=sys.stderr)
    print(file=sys.stderr)
    if len(versions) > 1:
        print(f"  [警告] 这批包来自 {len(versions)} 个不同的 source_version:"
              f"{dict(versions)} —— 混版本会让「宠物包内包含宠物」和实际对不上")
    return out, (versions.most_common(1)[0][0] if versions else "")


# ---------------------------------------------------------------- 演示模式

# 000-三叶草龙.rkpet:包含 三叶草龙/花叶龙 两种形态;
LINE = re.compile(r"^(?P<file>[^:]+\.rkpet):包含 (?P<forms>\S+) ")


def scan_index(index_md: Path, sprite_index: dict[str, int]) -> list[dict]:
    packs: list[dict] = []
    body = index_md.read_text("utf-8")
    body = body[body.index("## 清单"):]
    for line in body.splitlines():
        m = LINE.match(line.strip())
        if not m:
            continue
        stem = Path(m.group("file")).stem
        book, _, name = stem.partition("-")
        forms = []
        for i, token in enumerate(m.group("forms").split("/")):
            fname, _, n = token.partition("x")
            forms.append({
                "name": fname,
                "asset": "",
                "stage": i + 1,
                "skins": int(n) if n.isdigit() else 1,
                "sprite": sprite_index.get(fname),
            })
        packs.append({
            "id": stem,
            "book": book if book.isdigit() else "000",
            "name": name or stem,
            "key": f"packs/{stem}.rkpet",
            "size": 0,
            "sha256": "",
            "forms": forms,
            "sprite": sprite_index.get(name),
        })
    if not packs:
        sys.exit(f"{index_md} 里没解出条目 —— 「## 清单」一节的格式变了?")
    return packs


# ---------------------------------------------------------------- 应用本体


def scan_apps(apps_dir: Path, version: str) -> list[dict]:
    apps: list[dict] = []
    for f in sorted(apps_dir.iterdir()):
        if not f.is_file():
            continue
        for pattern, app_id, platform, label in APP_PATTERNS:
            if pattern.search(f.name):
                apps.append({
                    "id": app_id,
                    "platform": platform,
                    "label": label,
                    "key": f"app/{version}/{f.name}",
                    "filename": f.name,
                    "size": f.stat().st_size,
                    "sha256": sha256_of(f),
                    "version": version,
                })
                print(f"  应用本体 {f.name} ({f.stat().st_size / 1e6:.1f}MB)")
                break
        else:
            print(f"  [跳过] 认不出的文件 {f.name}")
    return apps


# ----------------------------------------------------------------


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--packs", type=Path, help="宠物包目录(*.rkpet)")
    src.add_argument("--index", type=Path, nargs="?", const=REPO / "docs/petindex.md",
                     help="演示模式:从 docs/petindex.md 的清单造目录")
    ap.add_argument("--apps", type=Path, help="应用本体所在目录(.exe / .AppImage / .tar.*)")
    ap.add_argument("--version", default="0.0.0", help="应用本体版本号,进 R2 key")
    ap.add_argument("--petvo", type=Path, default=REPO.parent / "rocom-petvo",
                    help="rocom-petvo 目录,取 sprite.webp 与 data.js")
    ap.add_argument("--out", type=Path, default=WEB / "public",
                    help="输出目录,默认 web/public")
    args = ap.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)

    print("头像:")
    sprite_index, sprite_geom = load_sprite_index(args.petvo)
    copy_sprite(args.petvo, args.out)

    source_version = ""
    if args.packs:
        print("宠物包(算 sha256,1.4GB 大约要一分钟):")
        packs, source_version = scan_packs(args.packs, sprite_index)
    else:
        print(f"宠物包(演示模式,读 {args.index}):")
        packs = scan_index(args.index, sprite_index)

    apps: list[dict] = []
    if args.apps:
        print("应用本体:")
        apps = scan_apps(args.apps, args.version)

    # 排序只影响没有统计数据时的兜底顺序 —— 页面默认按下载次数排,那份数据来自 D1。
    packs.sort(key=lambda p: (p["book"] == "000", p["book"], p["name"]))

    catalog = {
        "generated_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "source_version": source_version,
        "sprite": {"url": "/sprite.webp", **sprite_geom},
        "apps": apps,
        "packs": packs,
    }
    out_file = args.out / "catalog.json"
    out_file.write_text(json.dumps(catalog, ensure_ascii=False, separators=(",", ":")), "utf-8")

    total_forms = sum(len(p["forms"]) for p in packs)
    with_avatar = sum(1 for p in packs if p["sprite"] is not None)
    total_bytes = sum(p["size"] for p in packs)
    print(f"\n{out_file} ({out_file.stat().st_size / 1024:.0f}KB)")
    print(f"  {len(packs)} 个包 / {total_forms} 个形态 / {total_bytes / 1e9:.2f}GB")
    print(f"  有头像的包 {with_avatar}/{len(packs)};"
          f"「000」那批在 petvo 里没有条目,页面退回文字头像")
    if apps:
        print(f"  应用本体 {len(apps)} 个,版本 {args.version}")

    print("\n下一步:")
    print(f"  rclone copy {args.packs or '<包目录>'} r2:rocom-pets/packs/ --progress")
    if args.apps:
        print(f"  rclone copy {args.apps} r2:rocom-pets/app/{args.version}/ --progress")
    print("  npm run build && wrangler deploy")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
