import type { Pack } from "../../shared/types.ts";

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
 * 265 个包 / 658 个形态,每次输入全量扫一遍是几十微秒的事,不上索引。
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
