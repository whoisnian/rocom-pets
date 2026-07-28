#!/usr/bin/env python3
"""从游戏的 UE shader library 里取出编译好的 shader(DXBC)。

**这是干什么用的。** 宠物渲染那套自研 toon(见 rocom-pets)只能拿到材质实例的**参数值**
与**静态开关**,拿不到材质图的公式——cooked 包里材质图是被剥掉的(editor-only 数据)。
公式的唯一离线来源就是编译产物:`ShaderArchive-NRC-PCD3D_ES31.ushaderbytecode`,
里头是 D3D 的 DXBC 字节码,**静态开关已经在编译期定死**,也就是「这个材质实际跑的是什么」。

用法(先用 unpack.sh 把 archive 导出来,它不在默认导出范围里):

    ./scripts/unpack.sh --out <dir> --no-exclude --no-post --filter "NRC/Content/ShaderArchive"
    uv run python scripts/shaderdump.py <dir>/NRC/Content/ShaderArchive-NRC-PCD3D_ES31.ushaderbytecode
    uv run python scripts/shaderdump.py <archive> --extract 0 -o /tmp/s0.dxbc
    uv run python scripts/shaderdump.py <archive> --find Material,MobileBasePass --freq 3 --limit 20

**已知的两处缺口**(见 rocom-pets/docs/design.md §1):
1. 归属:archive 里**没有任何材质名**(搜 `M_P_Object` 零命中),要知道「哪条 shader 属于
   哪个材质」得从材质资产的 cooked resource 里读 shader map 的 SHA1,再来这张表里查;
2. 反汇编:DXBC 的反射段(`RDEF`)被剥了,只剩 `ISGN`/`SHEX`。指令流是全的,但本机没有
   DXBC 反汇编器(RenderDoc / 3Dmigoto 的 cmd_Decompiler / DXVK 都能做,都要另外装)。
"""

import argparse
import re
import struct
from collections import Counter
from pathlib import Path

# UE 的 shader library 用裸 LZ4 块压每一条 shader。
# 格式极简,自己写比拉一个依赖省事;实测 72407 条全部解出的长度与表里声明的完全一致。
def lz4_block_decompress(src: bytes, out_size: int) -> bytes:
    out = bytearray()
    i, n = 0, len(src)
    while i < n:
        token = src[i]
        i += 1
        lit = token >> 4
        if lit == 15:
            while True:
                b = src[i]
                i += 1
                lit += b
                if b != 255:
                    break
        out += src[i : i + lit]
        i += lit
        if i >= n:
            break
        off = src[i] | (src[i + 1] << 8)
        i += 2
        ml = (token & 0xF) + 4
        if (token & 0xF) == 15:
            while True:
                b = src[i]
                i += 1
                ml += b
                if b != 255:
                    break
        start = len(out) - off
        if off >= ml:
            out += out[start : start + ml]
        else:
            # 重叠拷贝(off < ml)必须逐字节,不能整段切片
            for k in range(ml):
                out.append(out[start + k])
    if len(out) != out_size:
        raise ValueError(f"解压得到 {len(out)} 字节,表里写的是 {out_size}")
    return bytes(out)


# SF_Vertex=0 … SF_Compute=5(UE 的 EShaderFrequency)
FREQUENCY = {0: "vertex", 1: "hull", 2: "domain", 3: "pixel", 4: "geometry", 5: "compute"}


class ShaderArchive:
    """`FShaderCodeArchive`(version 2)。

    布局是 `FSerializedShaderArchive` 一串数组,紧跟着 shader 代码区:

        uint32                     Version(= 2)
        TArray<FSHAHash>           ShaderMapHashes      每条 20 字节
        TArray<FSHAHash>           ShaderHashes         每条 20 字节
        TArray<FShaderMapEntry>    ShaderMapEntries     16 字节:4 个 uint32
        TArray<FShaderCodeEntry>   ShaderEntries        **17 字节**:u64 Offset + u32 Size
                                                        + u32 UncompressedSize + u8 Frequency
        TArray<FFileCachePreloadEntry> PreloadEntries   16 字节
        TArray<uint32>             ShaderIndices
        ─── 以上结束处就是代码区基址 ───

    `FShaderCodeEntry` 是**紧凑写的、17 字节没有对齐填充**,按 20 字节读会把后面所有表
    的偏移全带偏(实测那样读出来 Frequency 是 534019 这种垃圾值)。校验办法:
    代码区各条是首尾相接的,`基址 + max(Offset + Size)` 应当正好等于文件长度。
    """

    def __init__(self, path: Path):
        self.data = data = path.read_bytes()
        off = 0

        def count(elem_size: int) -> tuple[int, int]:
            nonlocal off
            n = struct.unpack_from("<I", data, off)[0]
            off += 4
            start = off
            off += n * elem_size
            return n, start

        self.version = struct.unpack_from("<I", data, 0)[0]
        off = 4
        self.n_maps, self.off_map_hashes = count(20)
        self.n_shaders, self.off_shader_hashes = count(20)
        n_map_entries, self.off_map_entries = count(16)
        n_code_entries, self.off_code_entries = count(17)
        self.n_preload, self.off_preload = count(16)
        self.n_indices, self.off_indices = count(4)
        self.code_base = off

        if n_map_entries != self.n_maps or n_code_entries != self.n_shaders:
            raise ValueError("表长互不匹配,布局大概不是这个版本")
        end = max(o + s for o, s, _, _ in map(self.entry, range(self.n_shaders)))
        if self.code_base + end != len(data):
            raise ValueError(
                f"代码区末尾 {self.code_base + end} 与文件长度 {len(data)} 不符,布局对不上"
            )

    def entry(self, i: int) -> tuple[int, int, int, int]:
        """第 i 条:(Offset, Size, UncompressedSize, Frequency)。"""
        base = self.off_code_entries + i * 17
        off, size, usize = struct.unpack_from("<QII", self.data, base)
        return off, size, usize, self.data[base + 16]

    def shader_hash(self, i: int) -> str:
        o = self.off_shader_hashes + i * 20
        return self.data[o : o + 20].hex()

    def map_hash(self, i: int) -> str:
        o = self.off_map_hashes + i * 20
        return self.data[o : o + 20].hex()

    def shaders_of_map(self, i: int) -> list[int]:
        indices_off, num = struct.unpack_from("<II", self.data, self.off_map_entries + i * 16)
        base = self.off_indices + indices_off * 4
        return list(struct.unpack_from(f"<{num}I", self.data, base))

    def payload(self, i: int) -> bytes:
        """解压后的整条 shader 记录:资源表 + DXBC + 尾部(uniform buffer 名字)。"""
        off, size, usize, _ = self.entry(i)
        raw = self.data[self.code_base + off : self.code_base + off + size]
        return lz4_block_decompress(raw, usize)

    def dxbc(self, i: int) -> bytes:
        p = self.payload(i)
        start = p.find(b"DXBC")
        if start < 0:
            raise ValueError(f"第 {i} 条里找不到 DXBC")
        # DXBC 头第 24..28 字节是整个 blob 的长度
        return p[start : start + struct.unpack_from("<I", p, start + 24)[0]]

    def uniform_buffers(self, i: int) -> list[str]:
        """尾部那串 uniform buffer 名(`View`/`Primitive`/`Material`/`MobileBasePass…`)。

        这是**唯一还留着名字的东西** —— DXBC 的反射段被剥了,材质名也不在 archive 里,
        所以只能靠它给 shader 分类(哪些是 mobile base pass、哪些吃 Material 参数块)。
        """
        p = self.payload(i)
        start = p.find(b"DXBC")
        tail = start + struct.unpack_from("<I", p, start + 24)[0]
        return [s.decode() for s in re.findall(rb"[A-Za-z][A-Za-z0-9_]{3,}", p[tail:])]


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("archive", type=Path)
    ap.add_argument("--extract", type=int, metavar="N", help="导出第 N 条的 DXBC")
    ap.add_argument("-o", "--out", type=Path, help="配合 --extract")
    ap.add_argument("--ubs", type=int, metavar="N", help="打印第 N 条的 uniform buffer 名")
    ap.add_argument("--find", metavar="A,B", help="按 uniform buffer 名筛(逗号分隔,全部命中才算)")
    ap.add_argument("--freq", type=int, help="按 Frequency 筛(3=pixel、0=vertex)")
    ap.add_argument("--limit", type=int, default=10)
    ap.add_argument("--verify", type=int, metavar="N", help="抽 N 条验证解压")
    args = ap.parse_args()

    arc = ShaderArchive(args.archive)
    print(
        f"version {arc.version}:{arc.n_maps} 个 shader map、{arc.n_shaders} 条 shader,"
        f"代码区基址 0x{arc.code_base:x}"
    )

    if args.extract is not None:
        blob = arc.dxbc(args.extract)
        out = args.out or Path(f"shader{args.extract}.dxbc")
        out.write_bytes(blob)
        print(f"第 {args.extract} 条 → {out}({len(blob)} 字节,{FREQUENCY[arc.entry(args.extract)[3]]})")
        return

    if args.ubs is not None:
        print(f"第 {args.ubs} 条的 uniform buffer: {', '.join(arc.uniform_buffers(args.ubs))}")
        return

    if args.verify is not None:
        step = max(1, arc.n_shaders // args.verify)
        checked = failed = 0
        for i in range(0, arc.n_shaders, step):
            checked += 1
            try:
                arc.dxbc(i)
            except Exception as e:  # noqa: BLE001 —— 验证脚本要把失败原因原样报出来
                failed += 1
                print(f"  ✗ 第 {i} 条: {e}")
        print(f"抽查 {checked} 条:失败 {failed} 条")
        return

    if args.find or args.freq is not None:
        want = [s for s in (args.find or "").split(",") if s]
        shown = 0
        for i in range(arc.n_shaders):
            _, size, usize, freq = arc.entry(i)
            if args.freq is not None and freq != args.freq:
                continue
            if want:
                ubs = arc.uniform_buffers(i)
                if not all(w in ubs for w in want):
                    continue
                extra = f"  ub={','.join(ubs)}"
            else:
                extra = ""
            print(f"  #{i:<6} {FREQUENCY[freq]:<7} 压缩 {size:<7} 解压 {usize:<7}{extra}")
            shown += 1
            if shown >= args.limit:
                break
        return

    freqs = Counter(arc.entry(i)[3] for i in range(arc.n_shaders))
    print("Frequency 分布: " + ", ".join(f"{FREQUENCY[k]}={v}" for k, v in sorted(freqs.items())))
    sizes = [arc.entry(i)[2] for i in range(arc.n_shaders)]
    print(f"解压后大小:合计 {sum(sizes) / 1048576:.0f} MB,最大 {max(sizes)} 字节")
    print(f"shader map 每张平均 {arc.n_indices / arc.n_maps:.1f} 条 shader")


if __name__ == "__main__":
    main()
