#!/usr/bin/env python3
"""从**安卓**客户端的 UE shader library 里取出编译好的 shader —— 这边是 **GLSL 源码**。

Windows 客户端那份是 `PCD3D_ES31`(DXBC 字节码,反射段被剥,只能反汇编着读);
安卓客户端 cook 的是 `GLSL_ES3_1_ANDROID`,载荷是 **HLSLcc 生成的 GLSL 文本**,
公式直接能读、输入属性还带语义名。两边编译自同一份材质图,静态开关同样已定死。

取包见 [docs/android-glsl.md](../docs/android-glsl.md)(APK → OBB → pak → 库文件)。

    uv run python scripts/glsldump.py <库> --info 7           # 一条的概况
    uv run python scripts/glsldump.py <库> --extract 7 -o /tmp/s.glsl
    uv run python scripts/glsldump.py <库> --grep 'MatCap'    # 全量扫源码(建索引后很快)
    uv run python scripts/glsldump.py <库> --index            # 建/更新索引
    uv run python scripts/glsldump.py <库> --filter 'ps5,ps7' --freq 3   # 按声明筛

**容器格式与 Windows 那份完全一样**(`FShaderCodeArchive` version 2),所以直接复用
`shaderdump.ShaderArchive`;不同的只有解压后的载荷:

    'LSLGSP' + 头(输入属性名等) + GLSL 源码 + b'\\x00' + uint32

`--grep` 要解压全部 95814 条,纯 Python 的 LZ4 扛不住,这里用 `lz4` 包。
"""
import argparse
import json
import re
import sys
import time
from pathlib import Path

import lz4.block

sys.path.insert(0, str(Path(__file__).resolve().parent))
from shaderdump import FREQUENCY, ShaderArchive  # noqa: E402

# 魔数是 `LSLGS` + 频率字母:`P` 像素、`V` 顶点、`C` 计算…… 按整串 `LSLGSP` 判会把
# 顶点 shader 全判成"解不出"(实测 20577 条),而顶点 shader 里有蒙皮,是要读的。
MAGIC = b"LSLGS"


class GlslArchive(ShaderArchive):
    """`ShaderArchive` 的安卓版:同一个容器,载荷换成 GLSL。"""

    def payload(self, i: int) -> bytes:
        off, size, usize, _ = self.entry(i)
        raw = self.data[self.code_base + off : self.code_base + off + size]
        return lz4.block.decompress(raw, uncompressed_size=usize)

    def source(self, i: int) -> str:
        """第 i 条的 GLSL 源码。找不到就抛 —— 别静默返回空串,那会让 --grep 假阴性。"""
        p = self.payload(i)
        if not p.startswith(MAGIC):
            raise ValueError(f"第 {i} 条不是 GLSL 载荷(头 {p[:8]!r})")
        s = p.find(b"#version")
        if s < 0:
            raise ValueError(f"第 {i} 条里找不到 #version")
        e = p.rfind(b"}")
        return p[s : e + 1].decode("utf8", "replace")

    def attrs(self, i: int) -> list[str]:
        """载荷头里那串输入属性名(`in_texcoord0`/`in_primitive_id`…)。"""
        p = self.payload(i)
        s = p.find(b"#version")
        return [m.decode() for m in re.findall(rb"in_[a-z_0-9]+", p[: s if s > 0 else 800])]


# GLSL 里 uniform 是**按常量缓冲分组、再按精度分家**打包的:
# `pc6_m[15]` = 第 6 个 uniform buffer 的 mediump 区、15 个 vec4。
# 所以 DXBC 那边的 `cb6[i]` 与这里的 `pc6_m[j]` **不是同一套下标**(见 docs/android-glsl.md)。
RE_UNIFORM = re.compile(r"uniform\s+(?:highp\s+|mediump\s+|lowp\s+)?(\w+)\s+(\w+)(?:\[(\d+)\])?;")
RE_SAMPLER = re.compile(r"uniform\s+(?:highp\s+|mediump\s+|lowp\s+)?(\w*sampler\w*)\s+(ps\d+);")
RE_IN = re.compile(r"\bin\s+(?:highp\s+|mediump\s+|lowp\s+)?\w+\s+(in_\w+);")
RE_OUT = re.compile(r"\bout\s+(?:highp\s+|mediump\s+|lowp\s+)?\w+\s+(out_\w+);")


def summarize(src: str) -> dict:
    """一条 shader 的结构指纹 —— 跨平台找同一条 shader 就靠这个,见 docs/android-glsl.md。"""
    return {
        "samplers": sorted({m.group(2) for m in RE_SAMPLER.finditer(src)}),
        "sampler_types": sorted({m.group(1) for m in RE_SAMPLER.finditer(src)}),
        "packed": {
            m.group(2): int(m.group(3) or 1)
            for m in RE_UNIFORM.finditer(src)
            if m.group(2).startswith("pc")
        },
        "inputs": sorted(set(RE_IN.findall(src))),
        "outputs": sorted(set(RE_OUT.findall(src))),
        "lines": src.count("\n") + 1,
    }


def index_path(archive: Path) -> Path:
    return archive.with_suffix(archive.suffix + ".index.json")


def build_index(arc: GlslArchive, out: Path) -> list[dict]:
    """把每条的结构指纹存下来 —— 全量解压一遍大约一分钟,别每次都重做。"""
    rows, t0 = [], time.time()
    for i in range(arc.n_shaders):
        _, size, usize, freq = arc.entry(i)
        row = {"i": i, "freq": freq, "usize": usize}
        try:
            row |= summarize(arc.source(i))
        except Exception as e:  # noqa: BLE001 —— 解不出的要留痕,不能当成"没有"
            row["error"] = str(e)
        rows.append(row)
        if (i + 1) % 10000 == 0:
            print(f"  …{i + 1}/{arc.n_shaders}  {time.time() - t0:.0f}s", flush=True)
    out.write_text(json.dumps(rows, ensure_ascii=False))
    print(f"索引 → {out}({len(rows)} 条,{time.time() - t0:.0f}s)")
    return rows


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("archive", type=Path)
    ap.add_argument("--info", type=int, metavar="N", help="第 N 条的结构概况")
    ap.add_argument("--extract", type=int, metavar="N", help="导出第 N 条的 GLSL")
    ap.add_argument("-o", "--out", type=Path, help="配合 --extract")
    ap.add_argument("--grep", help="在全部 shader 源码里搜正则(忽略大小写)")
    ap.add_argument("--index", action="store_true", help="建/更新结构索引")
    ap.add_argument("--filter", help="按声明筛,逗号分隔全部命中(如 'ps5,pc6_m')")
    ap.add_argument("--freq", type=int, help="按 Frequency 筛(3=pixel、0=vertex)")
    ap.add_argument("--limit", type=int, default=20)
    args = ap.parse_args()

    arc = GlslArchive(args.archive)
    print(f"version {arc.version}:{arc.n_maps} 个 shader map、{arc.n_shaders} 条 shader")

    if args.index:
        build_index(arc, index_path(args.archive))
        return

    if args.extract is not None:
        src = arc.source(args.extract)
        out = args.out or Path(f"shader{args.extract}.glsl")
        out.write_text(src)
        freq = FREQUENCY[arc.entry(args.extract)[3]]
        print(f"第 {args.extract} 条 → {out}({len(src)} 字符,{freq})")
        return

    if args.info is not None:
        s = summarize(arc.source(args.info))
        print(f"  频率     {FREQUENCY[arc.entry(args.info)[3]]}")
        print(f"  行数     {s['lines']}")
        print(f"  采样器   {len(s['samplers'])} 个:{', '.join(s['samplers'])}")
        print(f"           类型 {', '.join(s['sampler_types'])}")
        print(f"  打包常量 {', '.join(f'{k}[{v}]' for k, v in sorted(s['packed'].items()))}")
        print(f"  输入     {', '.join(s['inputs'])}")
        print(f"  输出     {', '.join(s['outputs'])}")
        return

    if args.grep:
        pat = re.compile(args.grep, re.I)
        hits = 0
        for i in range(arc.n_shaders):
            if args.freq is not None and arc.entry(i)[3] != args.freq:
                continue
            try:
                src = arc.source(i)
            except Exception:  # noqa: BLE001 —— 扫描时跳过解不出的
                continue
            if pat.search(src):
                print(f"  #{i:<6} {FREQUENCY[arc.entry(i)[3]]:<7} {src.count(chr(10)) + 1} 行")
                hits += 1
                if hits >= args.limit:
                    break
        print(f"命中 {hits} 条")
        return

    if args.filter or args.freq is not None:
        ip = index_path(args.archive)
        if not ip.exists():
            sys.exit(f"先建索引:--index(会写 {ip})")
        rows = json.loads(ip.read_text())
        want = [w for w in (args.filter or "").split(",") if w]
        shown = 0
        for r in rows:
            if args.freq is not None and r["freq"] != args.freq:
                continue
            decls = set(r.get("samplers", [])) | set(r.get("packed", {}))
            if want and not all(w in decls for w in want):
                continue
            print(f"  #{r['i']:<6} {FREQUENCY[r['freq']]:<7} {r.get('lines', 0):<6} 行  "
                  f"采样器 {len(r.get('samplers', []))}  "
                  f"{','.join(f'{k}[{v}]' for k, v in sorted(r.get('packed', {}).items()))}")
            shown += 1
            if shown >= args.limit:
                break
        print(f"命中(前 {shown} 条已列)")


if __name__ == "__main__":
    main()
