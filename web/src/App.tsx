import { useCallback, useEffect, useMemo, useState } from "react";
import { Download, Moon, PackageSearch, Search, Sun, X } from "lucide-react";
import { Toaster, toast } from "sonner";
import type { AppBuild, Catalog, Pack, StatsResponse } from "../shared/types.ts";
import { fetchCatalog, fetchConfig, fetchStats, startDownload } from "@/lib/api.ts";
import { searchPacks } from "@/lib/search.ts";
import { AppSection } from "@/components/AppSection.tsx";
import { PackCard } from "@/components/PackCard.tsx";
import { PackDialog } from "@/components/PackDialog.tsx";
import { ReportDialog } from "@/components/ReportDialog.tsx";
import { Button } from "@/components/ui/button.tsx";
import {
  Input,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Skeleton,
  TooltipProvider,
} from "@/components/ui/primitives.tsx";
import { formatBytes } from "@/lib/utils.ts";

const SORTS = [
  { value: "downloads", label: "下载次数" },
  { value: "book", label: "图鉴号" },
  { value: "name", label: "名称" },
  { value: "size", label: "文件大小" },
  { value: "reports", label: "异常标记" },
] as const;
type SortKey = (typeof SORTS)[number]["value"];

export default function App() {
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [stats, setStats] = useState<StatsResponse>({});
  const [sitekey, setSitekey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortKey>("downloads");
  const [detail, setDetail] = useState<Pack | null>(null);
  const [reportTarget, setReportTarget] = useState<{ id: string; label: string } | null>(null);
  const [dark, setDark] = useState(() => document.documentElement.classList.contains("dark"));

  useEffect(() => {
    fetchCatalog().then(setCatalog).catch((e: Error) => setError(e.message));
    // 统计挂了不该拖垮整页 —— 计数当 0 显示,下载照常。
    fetchStats().then(setStats).catch(() => {});
    fetchConfig().then((c) => setSitekey(c.turnstileSitekey)).catch(() => {});
  }, []);

  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
    localStorage.setItem("theme", dark ? "dark" : "light");
  }, [dark]);

  // 「/」聚焦搜索框,和大多数文档站一致
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "/" || e.metaKey || e.ctrlKey) return;
      const tag = (e.target as HTMLElement | null)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA") return;
      e.preventDefault();
      document.getElementById("q")?.focus();
    };
    addEventListener("keydown", onKey);
    return () => removeEventListener("keydown", onKey);
  }, []);

  const bumpLocal = useCallback((id: string, column: "downloads" | "reports") => {
    setStats((prev) => {
      const cur = prev[id] ?? { downloads: 0, reports: 0 };
      return { ...prev, [id]: { ...cur, [column]: cur[column] + 1 } };
    });
  }, []);

  const download = useCallback(
    (item: { id: string; name?: string; label?: string }) => {
      // 服务端每 IP 每天只计一次,这里的乐观 +1 只是让点击有反馈;
      // 下次拉 /api/stats 时会被真实数字覆盖。
      bumpLocal(item.id, "downloads");
      toast.success("开始下载", { description: item.name ?? item.label });
      startDownload(item.id);
    },
    [bumpLocal],
  );

  const hits = useMemo(() => {
    if (!catalog) return [];
    const list = searchPacks(catalog.packs, query);
    const cmp: Record<SortKey, (a: Pack, b: Pack) => number> = {
      downloads: (a, b) =>
        (stats[b.id]?.downloads ?? 0) - (stats[a.id]?.downloads ?? 0) ||
        a.book.localeCompare(b.book),
      reports: (a, b) =>
        (stats[b.id]?.reports ?? 0) - (stats[a.id]?.reports ?? 0) || a.book.localeCompare(b.book),
      book: (a, b) => a.book.localeCompare(b.book) || a.name.localeCompare(b.name, "zh"),
      name: (a, b) => a.name.localeCompare(b.name, "zh"),
      size: (a, b) => b.size - a.size,
    };
    return [...list].sort((x, y) => cmp[sort](x.pack, y.pack));
  }, [catalog, query, sort, stats]);

  const totals = useMemo(() => {
    if (!catalog) return null;
    return {
      packs: catalog.packs.length,
      forms: catalog.packs.reduce((n, p) => n + p.forms.length, 0),
      bytes: catalog.packs.reduce((n, p) => n + p.size, 0),
    };
  }, [catalog]);

  if (error) {
    return (
      <div className="mx-auto max-w-2xl p-8">
        <div className="rounded-xl border border-destructive/40 bg-destructive/5 p-5">
          <h1 className="mb-1 font-semibold text-destructive">目录加载失败</h1>
          <p className="text-sm text-muted-foreground">{error}</p>
        </div>
      </div>
    );
  }

  return (
    <TooltipProvider delayDuration={200}>
      <Toaster position="top-center" richColors closeButton theme={dark ? "dark" : "light"} />

      <header className="sticky top-0 z-30 border-b bg-background/85 backdrop-blur-md">
        <div className="mx-auto flex max-w-6xl flex-wrap items-center gap-3 px-4 py-3">
          <div className="mr-auto flex items-center gap-2">
            <PackageSearch className="size-5 text-primary" />
            <span className="text-[15px] font-semibold">rocom-pets</span>
            <span className="hidden text-sm text-muted-foreground sm:inline">下载</span>
          </div>

          <div className="relative order-3 w-full sm:order-none sm:w-auto sm:min-w-72 sm:flex-1">
            <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              id="q"
              type="search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => e.key === "Escape" && setQuery("")}
              placeholder="搜图鉴号 / 宠物名 / 形态名,如 076、喵喵、魔力猫"
              autoComplete="off"
              className="pr-9 pl-9"
              aria-label="搜索宠物包"
            />
            {query && (
              <button
                type="button"
                onClick={() => setQuery("")}
                aria-label="清空搜索"
                className="absolute top-1/2 right-2 -translate-y-1/2 rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground"
              >
                <X className="size-3.5" />
              </button>
            )}
          </div>

          <Select value={sort} onValueChange={(v) => setSort(v as SortKey)}>
            <SelectTrigger className="w-36" aria-label="排序方式">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {SORTS.map((s) => (
                <SelectItem key={s.value} value={s.value}>
                  {s.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          <Button
            variant="ghost"
            size="icon"
            onClick={() => setDark((d) => !d)}
            aria-label="切换深色模式"
          >
            {dark ? <Sun /> : <Moon />}
          </Button>
        </div>
      </header>

      <main className="mx-auto max-w-6xl px-4 pb-16">
        <section className="py-6">
          <h1 className="text-xl font-semibold">应用本体</h1>
          <p className="mt-1 mb-4 text-sm text-muted-foreground">
            运行时不联网、不读游戏内存、不注入进程。宠物包放进 packs 目录即可,不用解压。
          </p>
          {catalog ? (
            <AppSection
              apps={catalog.apps}
              stats={stats}
              onDownload={download}
              onReport={(a: AppBuild) => setReportTarget({ id: a.id, label: a.label })}
            />
          ) : (
            <div className="grid gap-3 sm:grid-cols-2">
              <Skeleton className="h-44" />
              <Skeleton className="h-44" />
            </div>
          )}
        </section>

        <section>
          <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
            <h2 className="text-xl font-semibold">宠物包</h2>
            {totals && (
              <p className="text-sm text-muted-foreground">
                {query ? (
                  <>
                    命中 {hits.length} / {totals.packs} 个包
                  </>
                ) : (
                  <>
                    {totals.packs} 个包 · {totals.forms} 个形态
                    {totals.bytes > 0 && <> · {formatBytes(totals.bytes)}</>}
                  </>
                )}
              </p>
            )}
          </div>
          <p className="mt-1 mb-4 text-sm text-muted-foreground">
            一个图鉴号一个包,同一条进化链的形态都在里面。搜索认图鉴号、链首名,也认包里任何一个形态名
            —— 按 <kbd className="rounded border px-1 font-mono text-[11px]">/</kbd> 直接聚焦。
          </p>

          {!catalog ? (
            <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
              {Array.from({ length: 9 }, (_, i) => (
                <Skeleton key={i} className="h-44" />
              ))}
            </div>
          ) : hits.length === 0 ? (
            <div className="rounded-xl border border-dashed py-16 text-center text-sm text-muted-foreground">
              没有匹配「{query}」的宠物包
            </div>
          ) : (
            <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
              {hits.map((hit) => (
                <PackCard
                  key={hit.pack.id}
                  hit={hit}
                  sheet={catalog.sprite}
                  stat={stats[hit.pack.id]}
                  onOpen={setDetail}
                  onDownload={download}
                  onReport={(p) => setReportTarget({ id: p.id, label: p.name })}
                />
              ))}
            </div>
          )}
        </section>
      </main>

      <footer className="border-t">
        <div className="mx-auto max-w-6xl px-4 py-6 text-xs leading-relaxed text-muted-foreground">
          <p>
            素材版权属原发行方。宠物包由导出器从游戏安装包本地生成,模型、贴图、叫声均为提取物。
          </p>
          {catalog && (
            <p className="mt-1">
              目录生成于 {new Date(catalog.generated_at).toLocaleString("zh-CN")}
              {catalog.source_version && <> · 游戏数据版本 {catalog.source_version}</>}
              {" · "}
              <a
                className="underline underline-offset-2 hover:text-foreground"
                href="https://github.com/whoisnian/rocom-pets"
                target="_blank"
                rel="noopener"
              >
                源码与导出器
              </a>
            </p>
          )}
          <p className="mt-1 flex items-center gap-1">
            <Download className="size-3" />
            下载次数与异常标记按 IP + 日期去重,原始 IP 不入库,只存当天的哈希。
          </p>
        </div>
      </footer>

      <PackDialog
        pack={detail}
        sheet={catalog?.sprite ?? { url: "/sprite.webp", cols: 21, cell: 128, count: 0 }}
        stat={detail ? stats[detail.id] : undefined}
        onOpenChange={(open) => !open && setDetail(null)}
        onDownload={download}
        onReport={(p) => setReportTarget({ id: p.id, label: p.name })}
      />

      <ReportDialog
        target={reportTarget}
        sitekey={sitekey}
        onOpenChange={(open) => !open && setReportTarget(null)}
        onCounted={(id) => bumpLocal(id, "reports")}
      />
    </TooltipProvider>
  );
}
