/* 把 DXBC 反汇编成 d3d asm(`ps_5_0` 那种汇编)。
 *
 * 配 scripts/shaderdump.py 用:那个从游戏的 shader library 里取出 DXBC,这个把 DXBC 变成
 * 能读的东西。用途见 AGENTS.md —— cooked 包里材质图被剥掉了,公式只能从编译产物读。
 *
 * **不需要 Windows,也不需要下任何二进制。** 走 wine 自带 d3dcompiler_47.dll 的
 * `D3DDisassemble`(wine 11 里由 vkd3d-shader 实现),编译也只用 wine 自己的 winegcc:
 *
 *     winegcc -o dxbcdis.exe scripts/dxbcdis.c
 *     wine ./dxbcdis.exe out/*.dxbc          # 每个 x.dxbc 出一个 x.dxbc.asm
 *
 * 实测抽查 31 条(覆盖整个 archive 的索引区间)全部成功,最长 891 行。
 *
 * **wine 那边有两个坑**:① 一次跑一个文件、反复起进程会偶发 `rc=3 直接不跑` 甚至挂死,
 * 所以这个工具支持一次给一批文件(一次 wine 调用处理完);② 真撞上 rc=3 就
 * `wineserver -k && wineserver -w` 让它重来。加 `WINEDEBUG=-all
 * WINEDLLOVERRIDES="mscoree,mshtml="` 能把无关输出压掉。 */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>

typedef struct IBlob IBlob;
typedef struct {
    HRESULT (WINAPI *QueryInterface)(IBlob*, const GUID*, void**);
    ULONG   (WINAPI *AddRef)(IBlob*);
    ULONG   (WINAPI *Release)(IBlob*);
    void*   (WINAPI *GetBufferPointer)(IBlob*);
    SIZE_T  (WINAPI *GetBufferSize)(IBlob*);
} IBlobVtbl;
struct IBlob { const IBlobVtbl *vt; };

typedef HRESULT (WINAPI *PFN)(const void*, SIZE_T, UINT, const char*, IBlob**);

/* 一次处理多个文件:每个 <x.dxbc> 出一个 <x.dxbc.asm>。
 * 之所以支持批量:wine 起一次进程要一秒上下,而且频繁反复起 wineserver 会出现
 * 「rc=3 直接不跑」和偶发挂死,一次调用处理完一批最省事。 */
int main(int argc, char **argv)
{
    if (argc < 2) { fprintf(stderr, "用法: dxbcdis <文件.dxbc> [更多…]\n"); return 2; }

    HMODULE h = LoadLibraryA("d3dcompiler_47.dll");
    if (!h) { fprintf(stderr, "加载 d3dcompiler_47.dll 失败\n"); return 2; }
    PFN dis = (PFN)GetProcAddress(h, "D3DDisassemble");
    if (!dis) { fprintf(stderr, "找不到 D3DDisassemble\n"); return 2; }

    int failed = 0;
    for (int i = 1; i < argc; i++) {
        FILE *f = fopen(argv[i], "rb");
        if (!f) { fprintf(stderr, "%s: 打不开\n", argv[i]); failed++; continue; }
        fseek(f, 0, SEEK_END); long n = ftell(f); fseek(f, 0, SEEK_SET);
        void *buf = malloc(n);
        if (fread(buf, 1, n, f) != (size_t)n) {
            fprintf(stderr, "%s: 读不全\n", argv[i]); fclose(f); failed++; continue;
        }
        fclose(f);

        IBlob *out = NULL;
        HRESULT hr = dis(buf, n, 0, NULL, &out);
        if (hr != 0 || !out) {
            fprintf(stderr, "%s: D3DDisassemble 返回 0x%08lx\n", argv[i], (unsigned long)hr);
            failed++; free(buf); continue;
        }
        char path[1024];
        snprintf(path, sizeof(path), "%s.asm", argv[i]);
        FILE *o = fopen(path, "wb");
        if (!o) { fprintf(stderr, "%s: 写不出\n", path); failed++; }
        else {
            fwrite(out->vt->GetBufferPointer(out), 1, out->vt->GetBufferSize(out), o);
            fclose(o);
            printf("%s -> %s\n", argv[i], path);
        }
        out->vt->Release(out);
        free(buf);
    }
    return failed ? 1 : 0;
}
