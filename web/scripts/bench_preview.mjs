#!/usr/bin/env node
/**
 * 预览取包策略的对拍脚本。
 *
 * 量的是 `web/src/lib/rkpet.ts` + `preview.ts` 那条链路上四种取法的墙上时间:
 *
 *   A  现状      open 2 次(先读总长再取尾部)+ 每个条目 2 次(先本地头再数据)、串行
 *   B1 仅并发    open 同 A,条目改成「头和数据合成一次 Range」并发取
 *   B2 全套      open 用后缀 Range 一次拿到尾部与总长,条目同 B1
 *   C  整包      直接把整个 .rkpet 拉下来
 *
 * C 量的是「整包 vs 只取一个形态」的体量差,**不是** Worker 代取方案的时延预测 ——
 * 那个方案里拉整包的那一跳发生在 Cloudflare 内网、比这里快,但它最后仍然要把
 * 同样的那几 MB 推给浏览器,也就是 B2 已经在付的那一段。
 *
 * 每种都跑两个目标:直连 R2 自定义域、经 Worker 的 /api/preview 中转。
 *
 * 只用 Node 自带的东西(fetch / DecompressionStream),不装依赖。要 Node 20+。
 *
 *   node web/scripts/bench_preview.mjs
 *   node web/scripts/bench_preview.mjs --pack 291-厉毒小萝 --runs 5
 *   node web/scripts/bench_preview.mjs --site https://rkpet.whoisnian.com --no-whole
 *
 * 注意:本机若挂着代理,量到的是代理的往返,不是真实链路 —— 这脚本就是为了拿到干净环境的数。
 */

import { readFileSync } from "node:fs";

// ---------------------------------------------------------------- 参数

const argv = process.argv.slice(2);
const flag = (name, fallback) => {
  const i = argv.indexOf(`--${name}`);
  return i >= 0 && argv[i + 1] && !argv[i + 1].startsWith("--") ? argv[i + 1] : fallback;
};
const has = (name) => argv.includes(`--${name}`);

const SITE = (flag("site", "https://rkpet.whoisnian.com")).replace(/\/+$/, "");
const PACK = flag("pack", null); // 不给就用目录里第一个
const FORM = flag("form", null); // 不给就用该包第一个形态
const RUNS = Number(flag("runs", 3));
const SKIP_WHOLE = has("no-whole");
const ONLY = flag("only", null); // direct | worker

/** 本地头的 extra 字段长度可能和中央目录里的不一样,合并取的时候多留这么多余量。 */
const SLACK = 4096;
/** EOCD 至少 22 字节、注释最长 65535,取这么多尾部一次基本就够。 */
const TAIL = 66_000;

if (Number(process.versions.node.split(".")[0]) < 20) {
  console.error(`要 Node 20+,当前 ${process.version}`);
  process.exit(1);
}

// ---------------------------------------------------------------- 计量

/** 每个变体跑一次期间的账本:发了几次请求、下行多少字节、重试了几次。 */
let tally = { reqs: 0, bytes: 0, retries: 0 };

/**
 * `spec` 是 Range 头的值(不含 `bytes=`);给 null 表示不带 Range,整个拉。
 *
 * 链路不稳时 undici 会直接抛 `fetch failed`(连接被掐,不是 HTTP 错误)。整轮作废太浪费,
 * 所以网络层错误重试两次 —— 重试的耗时照算进这一轮,只是不让它把整个变体判死。
 * 重试次数单独记,数大了说明这批数本身不可信。
 */
async function get(url, spec, attempt = 0) {
  const headers = { Origin: SITE };
  if (spec !== null) headers.Range = `bytes=${spec}`;
  let res;
  try {
    res = await fetch(url, { headers });
  } catch (err) {
    if (attempt >= 2) throw new Error(`${err.message}(重试 ${attempt} 次仍失败)`);
    tally.retries++;
    return get(url, spec, attempt + 1);
  }
  if (!res.ok) throw new Error(`${url} → HTTP ${res.status}`);
  let buf;
  try {
    buf = new Uint8Array(await res.arrayBuffer());
  } catch (err) {
    if (attempt >= 2) throw new Error(`读 body 失败: ${err.message}`);
    tally.retries++;
    return get(url, spec, attempt + 1);
  }
  tally.reqs++;
  tally.bytes += buf.byteLength;
  return { buf, res };
}

const fetchRange = (url, from, to) => get(url, `${from}-${to}`);
/** 后缀 Range:`bytes=-N` 取最后 N 字节,总长照样能从 content-range 的分母上读。 */
const fetchSuffix = (url, n) => get(url, `-${n}`);

const totalOf = (res) => {
  const cr = res.headers.get("content-range");
  const n = cr && Number(cr.split("/")[1]);
  if (!n || !Number.isFinite(n)) throw new Error(`拿不到总长,content-range = ${cr}`);
  return n;
};

// ---------------------------------------------------------------- zip

const SIG_EOCD = 0x0605_4b50;
const SIG_CENTRAL = 0x0201_4b50;

function parseCentral(cd, count) {
  const v = new DataView(cd.buffer, cd.byteOffset, cd.byteLength);
  const dec = new TextDecoder();
  const out = [];
  for (let i = 0, p = 0; i < count && p + 46 <= cd.byteLength; i++) {
    if (v.getUint32(p, true) !== SIG_CENTRAL) break;
    const nameLen = v.getUint16(p + 28, true);
    const extraLen = v.getUint16(p + 30, true);
    const commentLen = v.getUint16(p + 32, true);
    out.push({
      name: dec.decode(cd.subarray(p + 46, p + 46 + nameLen)),
      nameLen,
      method: v.getUint16(p + 10, true),
      compressedSize: v.getUint32(p + 20, true),
      size: v.getUint32(p + 24, true),
      offset: v.getUint32(p + 42, true),
    });
    p += 46 + nameLen + extraLen + commentLen;
  }
  return out;
}

/** 从已取到的尾部里定位并解出中央目录。 */
function centralFromTail(tail, tailFrom, total) {
  const v = new DataView(tail.buffer, tail.byteOffset, tail.byteLength);
  let eocd = -1;
  for (let i = tail.byteLength - 22; i >= 0; i--) {
    if (v.getUint32(i, true) === SIG_EOCD) { eocd = i; break; }
  }
  if (eocd < 0) throw new Error("找不到 EOCD");
  const count = v.getUint16(eocd + 10, true);
  const cdSize = v.getUint32(eocd + 12, true);
  const cdOffset = v.getUint32(eocd + 16, true);
  if (cdOffset < tailFrom || cdOffset + cdSize > total) {
    return { count, cdSize, cdOffset, entries: null }; // 不在尾部里,得再取一次
  }
  return {
    count, cdSize, cdOffset,
    entries: parseCentral(tail.subarray(cdOffset - tailFrom, cdOffset - tailFrom + cdSize), count),
  };
}

/** open 的 A 版:两次请求(先读总长,再取尾部)。 */
async function openTwoStep(url) {
  const probe = await fetchRange(url, 0, 0);
  const total = totalOf(probe.res);
  const tailFrom = Math.max(0, total - TAIL);
  const { buf: tail } = await fetchRange(url, tailFrom, total - 1);
  let cd = centralFromTail(tail, tailFrom, total);
  if (!cd.entries) {
    const { buf } = await fetchRange(url, cd.cdOffset, cd.cdOffset + cd.cdSize - 1);
    cd = { ...cd, entries: parseCentral(buf, cd.count) };
  }
  return cd.entries;
}

/** open 的 B 版:后缀 Range,一次同时拿到尾部与总长。 */
async function openSuffix(url) {
  const { buf: tail, res } = await fetchSuffix(url, TAIL);
  const total = totalOf(res);
  const tailFrom = total - tail.byteLength;
  let cd = centralFromTail(tail, tailFrom, total);
  if (!cd.entries) {
    const { buf } = await fetchRange(url, cd.cdOffset, cd.cdOffset + cd.cdSize - 1);
    cd = { ...cd, entries: parseCentral(buf, cd.count) };
  }
  return cd.entries;
}

async function inflateRaw(data) {
  const s = new Response(data).body.pipeThrough(new DecompressionStream("deflate-raw"));
  return new Uint8Array(await new Response(s).arrayBuffer());
}

async function decode(entry, raw) {
  if (entry.method === 0) return raw;
  if (entry.method === 8) return inflateRaw(raw);
  throw new Error(`${entry.name}: 不认得的压缩方式 ${entry.method}`);
}

/** 读条目的 A 版:先取 30 字节本地头,再取数据。 */
async function readTwoStep(url, e) {
  const { buf: head } = await fetchRange(url, e.offset, e.offset + 29);
  const hv = new DataView(head.buffer, head.byteOffset, head.byteLength);
  const from = e.offset + 30 + hv.getUint16(26, true) + hv.getUint16(28, true);
  const { buf: raw } = await fetchRange(url, from, from + e.compressedSize - 1);
  return decode(e, raw);
}

/** 读条目的 B 版:头和数据合成一次 Range,多取 SLACK 字节余量。 */
async function readCoalesced(url, e) {
  const pad = 30 + e.nameLen + SLACK;
  const { buf } = await fetchRange(url, e.offset, e.offset + pad + e.compressedSize - 1);
  const hv = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  const skip = 30 + hv.getUint16(26, true) + hv.getUint16(28, true);
  if (skip + e.compressedSize > buf.byteLength) return readTwoStep(url, e); // 余量不够,回落
  return decode(e, buf.subarray(skip, skip + e.compressedSize));
}

/**
 * 把条目按「在包里首尾相接」分组。
 *
 * 导出器是一个形态的 glb 写一段、`tex/` 写一段,所以一个形态的渲染文件实测正好落成
 * **2 段**(12 个本地包 / 35 个形态,最大也是 2),而且段内零空隙 —— 于是可以一段一次
 * Range 取完,既不多下字节,又把请求换成了长连续流。
 *
 * 段末位置只能估(本地头的 extra 长度可能和中央目录里的不一样),所以用 SLACK 当容差。
 * 真空隙都是几百 KB 到几 MB 级别,不会被这 4KB 误并;万一某个包排布不同,它自然退化成
 * 更多段,行为等价于 B2,不会出错。
 */
function groupRuns(entries) {
  const sorted = [...entries].sort((a, b) => a.offset - b.offset);
  const endOf = (e) => e.offset + 30 + e.nameLen + SLACK + e.compressedSize;
  const runs = [];
  for (const e of sorted) {
    const last = runs[runs.length - 1];
    if (last && e.offset <= endOf(last.items[last.items.length - 1])) last.items.push(e);
    else runs.push({ items: [e] });
  }
  return runs;
}

/** 一段一次 Range 取完,再在缓冲区里按各条目的绝对偏移切出来。 */
async function readRun(url, run) {
  const first = run.items[0];
  const last = run.items[run.items.length - 1];
  const from = first.offset;
  const to = last.offset + 30 + last.nameLen + SLACK + last.compressedSize - 1;
  const { buf } = await get(url, `${from}-${to}`);
  const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  return Promise.all(run.items.map((e) => {
    const at = e.offset - from;                       // 中央目录给了绝对偏移,段内定位是精确的
    const skip = at + 30 + view.getUint16(at + 26, true) + view.getUint16(at + 28, true);
    if (skip + e.compressedSize > buf.byteLength) return readTwoStep(url, e); // 只可能发生在最后一个
    return decode(e, buf.subarray(skip, skip + e.compressedSize));
  }));
}

// ---------------------------------------------------------------- 选形态

/** 渲染要用的条目 —— 叫声与动作音效跳过,和 preview.ts 的 keepForRender 一致。 */
const keepForRender = (e) => !/\/(voice|sfx)\//.test(e.name);
const pickForm = (entries, asset) =>
  entries.filter((e) => e.name.startsWith(`forms/${asset}/`) && !e.name.endsWith("/")).filter(keepForRender);

// ---------------------------------------------------------------- 变体

const VARIANTS = [
  {
    key: "A", label: "A 现状  open2 + 逐条目2次串行",
    run: async (url, asset) => {
      const entries = await openTwoStep(url);
      for (const e of pickForm(entries, asset)) await readTwoStep(url, e);
    },
  },
  {
    key: "B1", label: "B1 并发  open2 + 合并头数据并发",
    run: async (url, asset) => {
      const entries = await openTwoStep(url);
      await Promise.all(pickForm(entries, asset).map((e) => readCoalesced(url, e)));
    },
  },
  {
    key: "B2", label: "B2 全套  后缀Range + 合并并发",
    run: async (url, asset) => {
      const entries = await openSuffix(url);
      await Promise.all(pickForm(entries, asset).map((e) => readCoalesced(url, e)));
    },
  },
  {
    key: "B3", label: "B3 连续段 后缀Range + 按段取",
    run: async (url, asset) => {
      const entries = await openSuffix(url);
      await Promise.all(groupRuns(pickForm(entries, asset)).map((r) => readRun(url, r)));
    },
  },
];

const WHOLE = {
  key: "C", label: "C 整包  一次拉完整个 .rkpet",
  run: async (url) => { await get(url, null); },
};

// ---------------------------------------------------------------- 杂项

/**
 * 从同仓库的 `web/wrangler.jsonc` 里读 `PUBLIC_BASE`。
 *
 * 那是 jsonc(带注释),不能直接 JSON.parse;这里只要一个值,正则够用 ——
 * 顺手跳过行注释,免得把注释里举例的域名当真。脚本被单独拷出去跑时读不到,
 * 那就只测 Worker 那条,或者用 --base 指过来。
 */
function publicBaseFromWrangler() {
  try {
    const path = new URL("../wrangler.jsonc", import.meta.url);
    const text = readFileSync(path, "utf8").replace(/^\s*\/\/.*$/gm, "");
    const m = text.match(/"PUBLIC_BASE"\s*:\s*"([^"]+)"/);
    return m?.[1] || null;
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------- 跑

const ms = (n) => `${n.toFixed(0)}`.padStart(6);
const mb = (n) => `${(n / 1048576).toFixed(2)}MB`;

function summarize(runs) {
  const s = [...runs].sort((a, b) => a - b);
  return { min: s[0], med: s[(s.length - 1) >> 1], max: s[s.length - 1] };
}

async function measure(variant, url, asset) {
  const times = [];
  let cost = null;
  let retries = 0;
  for (let i = 0; i < RUNS; i++) {
    tally = { reqs: 0, bytes: 0, retries: 0 };
    const t0 = performance.now();
    await variant.run(url, asset);
    times.push(performance.now() - t0);
    cost ??= tally;
    retries += tally.retries;
  }
  return { ...summarize(times), ...cost, retries };
}

async function main() {
  console.log(`站点     ${SITE}`);

  const catRes = await fetch(`${SITE}/catalog.json`);
  if (!catRes.ok) throw new Error(`catalog.json 取不到(HTTP ${catRes.status})`);
  const catalog = await catRes.json();

  const pack = PACK ? catalog.packs.find((p) => p.id === PACK) : catalog.packs[0];
  if (!pack) throw new Error(`目录里没有 ${PACK}`);
  const asset = FORM ?? pack.forms[0].asset;
  if (!pack.forms.some((f) => f.asset === asset)) throw new Error(`${pack.id} 没有形态 ${asset}`);

  // R2 自定义域:命令行优先,否则从同仓库的 wrangler.jsonc 里抠 PUBLIC_BASE。
  // /api/config 只回了布尔的 direct,拿不到域名本身。
  const base = flag("base", publicBaseFromWrangler());

  const targets = [];
  if (base) {
    targets.push({ name: "直连 R2", url: new URL(encodeURI(pack.key), base.replace(/\/?$/, "/")).toString() });
  }
  targets.push({ name: "经 Worker", url: `${SITE}/api/preview/${encodeURIComponent(pack.id)}` });
  const chosen = targets.filter((t) => !ONLY || t.name.includes(ONLY === "direct" ? "直连" : "Worker"));
  if (!chosen.length) throw new Error("没有可测的目标");

  if (!base) {
    console.log("提示     没读到 wrangler.jsonc 里的 PUBLIC_BASE,只测 Worker 那条;");
    console.log("         要一起测就加 --base https://你的-r2-域名");
  }

  // 基准 RTT:先发一发把 DNS/TCP/TLS 建好(否则量到的是建连,不是往返),再量三次取最快
  await fetch(chosen[0].url, { headers: { Origin: SITE, Range: "bytes=0-0" } }).then((r) => r.arrayBuffer());
  const probes = [];
  for (let i = 0; i < 3; i++) {
    const t0 = performance.now();
    await fetch(chosen[0].url, { headers: { Origin: SITE, Range: "bytes=0-0" } }).then((r) => r.arrayBuffer());
    probes.push(performance.now() - t0);
  }
  const rtt = Math.min(...probes);

  // 直连那条顺带把 CORS 验一遍
  const direct = chosen.find((t) => t.name === "直连 R2");
  if (direct) {
    const r = await fetch(direct.url, { headers: { Origin: SITE, Range: "bytes=0-0" } });
    await r.arrayBuffer();
    const acao = r.headers.get("access-control-allow-origin");
    const expose = r.headers.get("access-control-expose-headers") ?? "";
    console.log(`CORS     allow-origin=${acao ?? "缺失!"}  content-range 已暴露=${expose.includes("content-range") ? "是" : "否!"}`);
  }

  // 预热:建连接、把形态的文件数和体积报出来
  const probeUrl = chosen[0].url;
  const entries = pickForm(await openSuffix(probeUrl), asset);
  const bytes = entries.reduce((n, e) => n + e.compressedSize, 0);

  console.log(`包       ${pack.id}  ${mb(pack.size)}`);
  console.log(`形态     ${asset}  ${entries.length} 个文件 / ${mb(bytes)}`);
  console.log(`基准RTT  ${rtt.toFixed(0)} ms   跑 ${RUNS} 轮取中位\n`);

  const variants = SKIP_WHOLE ? VARIANTS : [...VARIANTS, WHOLE];
  for (const target of chosen) {
    console.log(`── ${target.name}`);
    console.log(`   ${"变体".padEnd(30)}${"最快".padStart(7)}${"中位".padStart(7)}${"最慢".padStart(7)}   请求  下行      重试`);
    for (const v of variants) {
      if (v.key === "C" && target.name !== "直连 R2" && chosen.length > 1) continue; // 整包量一次就够
      try {
        const r = await measure(v, target.url, asset);
        console.log(`   ${v.label.padEnd(30)}${ms(r.min)}${ms(r.med)}${ms(r.max)}   ${String(r.reqs).padStart(4)}  ${mb(r.bytes).padEnd(8)}${String(r.retries).padStart(4)}`);
      } catch (err) {
        console.log(`   ${v.label.padEnd(30)}  失败: ${err.message}`);
      }
    }
    console.log("");
  }
  console.log("链路抖的时候「中位」「最慢」会被单次卡顿带飞,以「最快」列为准;");
  console.log("「重试」不为 0 说明这批数期间链路掉过连接,参考价值打折。");
}

main().catch((err) => {
  console.error(`\n出错了: ${err.message}`);
  process.exit(1);
});
