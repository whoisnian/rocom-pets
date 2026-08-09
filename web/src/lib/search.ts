import type { Pack, StatsResponse } from "../../shared/types.ts";

export interface PackHit {
  pack: Pack;
  /** 命中的形态名。命中的是包名或图鉴号时为空集 */
  formHits: Set<string>;
}

/**
 * 前端搜索。三种输入都要认:
 *   图鉴号   `2` / `002` / `00`(前缀)
 *   宠物名   `喵喵`(= 包名,也就是链首名)
 *   形态名   `魔力猫` —— 它在包名里根本不出现,这条是本页搜索存在的主要理由
 *
 * 201 个包 / 607 个形态,每次输入全量扫一遍是几十微秒的事,不上索引。
 */
export function searchPacks(packs: Pack[], raw: string): PackHit[] {
  const q = raw.trim().toLowerCase();
  if (!q) return packs.map((pack) => ({ pack, formHits: new Set<string>() }));

  const numeric = /^\d+$/.test(q);
  const padded = numeric ? q.padStart(3, "0") : "";

  const hits: PackHit[] = [];
  for (const pack of packs) {
    const formHits = new Set<string>();
    for (const form of pack.forms) {
      if (form.name.toLowerCase().includes(q)) formHits.add(form.name);
    }

    const byName = pack.name.toLowerCase().includes(q) || pack.id.toLowerCase().includes(q);
    // 「000」是「没有图鉴号」的占位,不该被搜 `0` 搜出来一大片
    const byBook =
      numeric && pack.book !== "000" && (pack.book === padded || pack.book.startsWith(q));

    if (byName || byBook || formHits.size) hits.push({ pack, formHits });
  }
  return hits;
}

// ---------------------------------------------------------------- 排序

/** 没有图鉴号的占位。搜索里要防它被 `0` 搜出来,排序里要让它殿后。 */
const NO_BOOK = "000";

export const SORTS = [
  { value: "downloads", label: "下载次数" },
  { value: "book", label: "图鉴号" },
  { value: "name", label: "名称" },
  { value: "size", label: "文件大小" },
  { value: "reports", label: "异常标记" },
] as const;

export type SortKey = (typeof SORTS)[number]["value"];

/**
 * 中文排序器。**别用 `a.localeCompare(b)`** —— 那个每调一次都要现建一个 `Intl.Collator`,
 * 而排 200 个包是一千多次比较。复用一个实例,同样的结果,代价小一个量级(见下面的实测)。
 */
const zh = new Intl.Collator("zh");

/** 图鉴号补足三位,直接比字符串就是比数字,不必过 collator。 */
const byBook = (a: Pack, b: Pack) => (a.book < b.book ? -1 : a.book > b.book ? 1 : 0);

const COMPARATORS: Record<SortKey, (a: Pack, b: Pack, stats: StatsResponse) => number> = {
  downloads: (a, b, s) => (s[b.id]?.downloads ?? 0) - (s[a.id]?.downloads ?? 0) || byBook(a, b),
  reports: (a, b, s) => (s[b.id]?.reports ?? 0) - (s[a.id]?.reports ?? 0) || byBook(a, b),
  book: (a, b) => byBook(a, b) || zh.compare(a.name, b.name),
  name: (a, b) => zh.compare(a.name, b.name),
  size: (a, b) => b.size - a.size || byBook(a, b),
};

/**
 * 排序。**无图鉴号的一律沉到最后**,不管当前按哪一列排 —— 它们是「游戏里查不到号」的
 * 边角料(当前 22/201),混在中间会让人以为列表漏了一段;而按图鉴号排时它们的占位
 * 「000」还会正好顶到第一页最前面。组内再按选中的规则排。
 *
 * 入参不改动:`hits` 是上游 memo 的结果,原地排会让它下次比较时看见的顺序已经变了。
 */
export function sortHits(hits: PackHit[], sort: SortKey, stats: StatsResponse): PackHit[] {
  const cmp = COMPARATORS[sort];
  return [...hits].sort(
    (x, y) =>
      Number(x.pack.book === NO_BOOK) - Number(y.pack.book === NO_BOOK) ||
      cmp(x.pack, y.pack, stats),
  );
}
