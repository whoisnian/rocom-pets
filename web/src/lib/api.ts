import type { Catalog, StatsResponse } from "../../shared/types.ts";

export async function fetchCatalog(): Promise<Catalog> {
  const res = await fetch("/catalog.json");
  if (!res.ok) throw new Error(`目录取不到(${res.status})—— 先跑 npm run catalog`);
  return res.json();
}

export async function fetchStats(): Promise<StatsResponse> {
  const res = await fetch("/api/stats");
  if (!res.ok) throw new Error(`统计取不到(${res.status})`);
  return res.json();
}

export async function fetchConfig(): Promise<{ turnstileSitekey: string | null; direct: boolean }> {
  const res = await fetch("/api/config");
  if (!res.ok) return { turnstileSitekey: null, direct: false };
  return res.json();
}

export async function submitReport(input: {
  id: string;
  reason: string;
  note?: string;
  token?: string;
}): Promise<{ counted: boolean }> {
  const res = await fetch("/api/report", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  const data = (await res.json().catch(() => ({}))) as { error?: string; counted?: boolean };
  if (!res.ok) throw new Error(data.error ?? `提交失败(${res.status})`);
  return { counted: data.counted ?? false };
}

/** 下载走整页跳转:交给浏览器的下载器,大文件才有进度条与断点续传。 */
export function startDownload(id: string) {
  window.location.href = `/api/dl/${encodeURIComponent(id)}`;
}
