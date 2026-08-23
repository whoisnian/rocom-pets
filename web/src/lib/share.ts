/**
 * 预览的分享链接:把「看的是哪只、哪个形态、什么表情、穿哪一身」放进 query。
 *
 * 带参数打开页面会**自动滚到那只并打开预览**;预览开着时地址栏跟着当前选择走,
 * 于是直接复制地址栏就是一条能还原现场的链接。
 *
 * **只用 `replaceState`,不 push。** 预览里改个配色就压一条历史记录的话,
 * 想退回列表得按十几次「后退」。代价是「后退」不会关弹窗 —— 那有右上角的叉、
 * Esc、点遮罩三条路,不缺这一条。
 */

/** 一条分享链接里的东西。除 `pet` 外都可省,省了就是默认。 */
export interface PreviewLink {
  /** 包 id,就是 `catalog.json` 里那个(如 `011-鸭吉吉`)。 */
  pet: string;
  /** 形态资产名(如 `Ar_YaJiJi1_001`);不写 = 链首。 */
  form?: string;
  /** 表情名(如 `生气眼`);不写 = 默认。 */
  face?: string;
  /** 外观,写法同 `roster.toml` 的 `mutation`(`异色+炫彩:3/33`);不写 = 原样。 */
  look?: string;
}

const KEYS = ["pet", "form", "face", "look"] as const;

/** 地址栏里有没有一条分享链接。没有 `pet` 就当没有。 */
export function readLink(): PreviewLink | null {
  const q = new URLSearchParams(location.search);
  const pet = q.get("pet");
  if (!pet) return null;
  const pick = (k: string) => q.get(k) || undefined;
  return { pet, form: pick("form"), face: pick("face"), look: pick("look") };
}

/**
 * 把当前现场写回地址栏;`null` = 抹掉这几个参数(关掉预览时)。
 *
 * **只动这四个键**,别的(以后可能有的 `q=`、`sort=`)原样留着。
 */
export function writeLink(link: PreviewLink | null): void {
  const url = new URL(location.href);
  for (const k of KEYS) url.searchParams.delete(k);
  if (link) {
    url.searchParams.set("pet", link.pet);
    if (link.form) url.searchParams.set("form", link.form);
    if (link.face) url.searchParams.set("face", link.face);
    if (link.look) url.searchParams.set("look", link.look);
  }
  // 和现在一样就别写:`replaceState` 每帧调一次不便宜,而选项一改就会来一趟
  if (url.href !== location.href) history.replaceState(history.state, "", url);
}
