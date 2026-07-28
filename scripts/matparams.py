#!/usr/bin/env python3
"""把材质冻结块里的参数记录读成「**名字 = 值**」—— 「名字这一关」已经通了。

## 怎么通的

补丁记录的头 4 字节(`paramId`)是 uexp 里 shader map **自带的那张名字表**的下标,
不是包名字表的。定表与判据见 `uniexpr.param_names()` 的文档字符串;
锚点校验 13/13,而且能用根材质默认值逐条复核(`--verify`)。

于是「这个图到底有哪些参数、按什么顺序」不再需要推:表本身就是答案,
`paramId` 是它的稠密下标,顺序就是表里的顺序(实测:先按参数**来源层**分段
——`__SubsurfaceProfile`/根图自身/各图层函数 `00FX_`…`06FX_`—— 段内字母序,
这也解释了此前「字母序秩对不上、差值一正一负」:全局字母序压根不是它的顺序)。

## 用途

拿到「名字 = 值」之后就能把 rocom-pets 里那批猜出来的常数换成读出来的:
`Stick_Intensity`、`StarTriPlannarBlendInt`、`StarTiling`/`StarUVScale`、
`Rim Intensity`、`StickRandomColor01..04`(星贴层缺的 4 个渐变色)等。

## 用法

    uv run python scripts/matparams.py <材质.uexp>                    # 全部块
    uv run python scripts/matparams.py <材质.uexp> --grep Star        # 只看名字匹配的
    uv run python scripts/matparams.py <材质.uexp> --block 5          # 只看某块
    uv run python scripts/matparams.py <材质.uexp> --table            # 打印整张名字表
    # 用根材质默认值复核名字表(探针的 根num/根col 行,来自 rocom-pets)
    uv run python scripts/matparams.py <材质.uexp> --verify 根默认值.txt
"""
import argparse
import re
import struct
from collections import defaultdict
from pathlib import Path

import uniexpr


def read_defaults(path: Path):
    """探针的 `根num/根col` 行 → ({名字: 标量值}, {名字: 向量元组})"""
    sca, vec = {}, {}
    for line in path.read_text().splitlines():
        m = re.match(r"根(num|col)\s+(\S.*?)\s*=\s*(.*)$", line.strip())
        if not m:
            continue
        kind, name, val = m.group(1), m.group(2).strip(), m.group(3).strip()
        if not val:
            continue                      # 探针对没有默认值的参数留空
        if kind == "num":
            sca[name] = round(float(val), 5)
        else:
            vec[name] = tuple(round(float(x), 4) for x in val.strip("()").split(","))
    return sca, vec


def close(a, b, tol: float = 6e-4) -> bool:
    """标量或等长向量的近似相等 —— 探针那侧只有 3 位小数。"""
    if isinstance(a, tuple) != isinstance(b, tuple):
        return False
    if isinstance(a, tuple):
        return len(a) == len(b) and all(abs(x - y) <= tol for x, y in zip(a, b))
    return abs(a - b) <= tol


def block_records(ue: bytes, start: int, end: int, patches: dict):
    """→ [(类型, 序号, paramId, 值)];**扫向量时必须排除标量段**,见 uniexpr.param_arrays"""
    out = []
    sas = uniexpr.param_arrays(ue, uniexpr.SCALAR_STRIDE, start, end)
    if not sas:
        return out
    soff, nsca = max(sas, key=lambda v: v[1])
    ex = [(soff, soff + nsca * uniexpr.SCALAR_STRIDE)]
    vas = uniexpr.param_arrays(ue, uniexpr.VEC_STRIDE, start, end, exclude=ex)
    for i in range(nsca):
        o = soff + uniexpr.SCALAR_STRIDE * i
        out.append(("sca", i, patches.get(o - start),
                    round(struct.unpack_from("<f", ue, o + uniexpr.VALUE_OFF)[0], 5)))
    if vas:
        voff, nvec = max(vas, key=lambda v: v[1])
        for i in range(nvec):
            o = voff + uniexpr.VEC_STRIDE * i
            out.append(("vec", i, patches.get(o - start),
                        tuple(round(x, 4) for x in
                              struct.unpack_from("<4f", ue, o + uniexpr.VALUE_OFF))))
    return out


def resolve(ue: bytes):
    """→ (名字表, [(块序号, 类型, 槽序号, 名字, 值)])"""
    names = uniexpr.param_names(ue)
    rows = []
    for bi, (start, end, patches) in enumerate(uniexpr.patch_tables(ue)):
        for kind, i, pid, v in block_records(ue, start, end, patches):
            nm = names[pid] if pid is not None and pid < len(names) else f"<paramId {pid}>"
            rows.append((bi, kind, i, nm, v))
    return names, rows


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("uexp", type=Path)
    ap.add_argument("--block", type=int, help="只看这一块")
    ap.add_argument("--grep", help="只看名字匹配这个正则的(忽略大小写)")
    ap.add_argument("--verify", type=Path, help="用探针的 根num/根col 行复核名字表")
    ap.add_argument("--table", action="store_true", help="打印整张名字表")
    args = ap.parse_args()

    ue = args.uexp.read_bytes()
    names, rows = resolve(ue)
    print(f"名字表 {len(names)} 条;冻结块 {len(set(r[0] for r in rows))} 个、"
          f"参数记录 {len(rows)} 条")

    if args.table:
        for i, n in enumerate(names):
            print(f"  {i:4d}  {n}")
        print()

    pat = re.compile(args.grep, re.I) if args.grep else None
    seen = set()
    for bi, kind, i, nm, v in rows:
        if args.block is not None and bi != args.block:
            continue
        if pat and not pat.search(nm):
            continue
        key = (kind, nm, v)
        if args.block is None:
            if key in seen:                # 同一参数在多块里重复,默认只报一次
                continue
            seen.add(key)
        tag = "标量" if kind == "sca" else "向量"
        where = f"块{bi} " if args.block is not None else ""
        print(f"  {where}{tag} #{i:<3} {nm:<36} = {v}")

    if args.verify:
        sca_names, vec_names = read_defaults(args.verify)
        ok = bad = miss = 0
        wrong = []
        for _bi, kind, _i, nm, v in rows:
            want = (sca_names if kind == "sca" else vec_names).get(nm)
            if want is None:
                miss += 1
            elif close(v, want):           # 探针只输出 3 位小数,不能按位全等比
                ok += 1
            else:
                bad += 1
                wrong.append((kind, nm, v, want))
        print(f"\n复核:与根默认值一致 {ok}、不一致 {bad}、根表里没这个名字 {miss}")
        agg = defaultdict(set)
        for kind, nm, v, want in wrong:
            agg[(kind, nm)].add((v, want))
        for (kind, nm), vs in sorted(agg.items())[:20]:
            v, want = next(iter(vs))
            print(f"  {'标量' if kind=='sca' else '向量'} {nm:<36} 块内 {v} ≠ 根默认 {want}"
                  f"{' (还有别的取值)' if len(vs) > 1 else ''}")
        print("  注:不一致多半是 MI 覆盖了默认值 —— 这正是要读的东西,不是错。")


if __name__ == "__main__":
    main()
