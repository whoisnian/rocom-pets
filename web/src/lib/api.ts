import type { Catalog, Pack, SiteConfig, StatsResponse } from "../../shared/types.ts";

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

/**
 * 站点配置。**只取一次**,结果记住 —— 首屏为了 Turnstile 已经取过一遍,
 * 预览要拿 `publicBase` 拼直连地址时就不必再等一个往返了。
 *
 * 取不到不算错:退回「没有 sitekey、没有自定义域」,于是不显示人机校验、预览走
 * Worker 代理。都是能跑的路径,没必要为此让页面炸掉。
 */
let configOnce: Promise<SiteConfig> | null = null;

export function fetchConfig(): Promise<SiteConfig> {
  configOnce ??= (async () => {
    try {
      const res = await fetch("/api/config");
      if (!res.ok) throw new Error(String(res.status));
      return (await res.json()) as SiteConfig;
    } catch {
      return { turnstileSitekey: null, publicBase: null };
    }
  })();
  return configOnce;
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

/**
 * 预览取包用的地址。**和下载分两条路** —— 预览是按 Range 分片取的,走 `/api/dl` 会把
 * 「一次预览」记成一次下载(那个计数是给「有多少人装了这个包」看的)。
 *
 * 配了 R2 自定义域就**直连**:一次预览要发好几个 Range 请求,经 Worker 中转的话每一个
 * 都算一次 Worker 请求(免费额度 10 万/天里唯一真会被刷到的地方),还平白多一跳。
 * 桶上的 CORS 要放行本站来源、并且**把 `content-range` 列进 expose-headers** ——
 * `rkpet.ts` 是从它的分母上读对象总长的,漏了那条预览一开就报「不支持 Range」。
 *
 * 没配自定义域(本地 `wrangler dev` 就是)回落到 Worker 代理那条,同源、不需要 CORS。
 */
export async function previewUrl(pack: Pack): Promise<string> {
  const { publicBase } = await fetchConfig();
  // key 里有中文,交给 URL 去百分号编码 —— 和 Worker 那边拼 302 目标用的是同一套
  if (publicBase) return new URL(pack.key, publicBase.replace(/\/?$/, "/")).toString();
  return `/api/preview/${encodeURIComponent(pack.id)}`;
}
