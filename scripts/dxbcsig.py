"""解 DXBC 的 ISGN/OSGN 签名段:把汇编里的 `vN`/`oN` 寄存器对回语义名(`TEXCOORD3` 之类)。

配 shaderdump.py + dxbcdis 用 —— 那两个能把 shader 取出来、反汇编成汇编,但汇编里只有
`v2.zw` 这种寄存器号,**要接回资产就得知道它是哪个语义**。签名段里有,而反射段 `RDEF`
被剥了、`D3DDisassemble` 也不打印语义名,所以自己解。

    uv run python scripts/dxbcsig.py out/34529.dxbc

实例:幽星光那两颗球的片元着色器里,内部星光的采样起点是 `r4.xy = v2.zw; r4.z = v3.x`;
这个工具给出 `v2` = TEXCOORD0、`v3` = TEXCOORD1,再配 UE 的 UV 打包规则
(TEXCOORD0 = UV0.xy + UV1.xy、TEXCOORD1 = UV2.xy + UV3.xy),就定死了起点是
`(UV1.x, UV1.y, UV2.x)` —— 不用猜。
"""
import struct, sys

def chunks(d):
    n = struct.unpack_from('<I', d, 28)[0]
    for i in range(n):
        off = struct.unpack_from('<I', d, 32 + i*4)[0]
        yield d[off:off+4].decode(), off + 8, struct.unpack_from('<I', d, off+4)[0]

def signature(d, want=('ISGN','OSGN','OSG5')):
    out = {}
    for name, data_off, size in chunks(d):
        if name not in want: continue
        count, _ = struct.unpack_from('<II', d, data_off)
        elems = []
        stride = 32 if name == 'OSG5' else 24
        base = data_off + 8
        for i in range(count):
            e = base + i*stride
            if name == 'OSG5':
                stream, nameoff, semidx, sysval, comptype, reg = struct.unpack_from('<IIIIII', d, e)
                mask, rwmask = d[e+24], d[e+25]
            else:
                nameoff, semidx, sysval, comptype, reg = struct.unpack_from('<IIIII', d, e)
                mask, rwmask = d[e+20], d[e+21]
            s = data_off + nameoff
            sem = d[s:d.index(b'\0', s)].decode()
            elems.append((reg, sem, semidx, mask, rwmask))
        out[name] = elems
    return out

MASK = lambda m: ''.join(c for c, b in zip('xyzw', (1,2,4,8)) if m & b)

if __name__ == '__main__':
    d = open(sys.argv[1], 'rb').read()
    for kind, elems in signature(d).items():
        print(f'--- {kind}')
        for reg, sem, idx, mask, rw in elems:
            print('  %s%-2d .%-4s %s%d' % ('vo'[kind[0]=='O'], reg, MASK(mask) or MASK(rw), sem, idx))
