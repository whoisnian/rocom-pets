import { Hono } from "hono";
import type { Catalog, SiteConfig, StatsResponse } from "../shared/types.ts";
import { REPORT_REASONS } from "../shared/types.ts";

export interface Env {
  ASSETS: Fetcher;
  FILES: R2Bucket;
  DB: D1Database;
  DEDUPE: KVNamespace;
  /** R2 自定义域,如 https://files.example.com。留空则由 Worker 代理字节 */
  PUBLIC_BASE?: string;
  TURNSTILE_SITEKEY?: string;
  TURNSTILE_SECRET?: string;
  DEDUPE_SALT?: string;
}

type Kind = "pack" | "app";

const app = new Hono<{ Bindings: Env }>();

/** 同一 IP 每天最多这么多次异常标记(跨全站,不是每个包) */
const REPORTS_PER_IP_PER_DAY = 20;
/** 备注截断长度,防止有人往 D1 里灌正文 */
const NOTE_MAX = 200;

const VALID_REASONS = new Set<string>(REPORT_REASONS.map((r) => r.value));

// ---------------------------------------------------------------- 目录

// 目录是静态资源,但 /api/dl/:id 必须自己解析 id → R2 key:让客户端传 key 等于
// 把整个桶开放给任意路径。isolate 存活期间缓存一份,冷启动才回源一次。
let catalogCache: { at: number; value: Catalog } | null = null;
const CATALOG_TTL_MS = 5 * 60 * 1000;

async function getCatalog(env: Env, base: string): Promise<Catalog> {
  const now = Date.now();
  if (catalogCache && now - catalogCache.at < CATALOG_TTL_MS) return catalogCache.value;
  const res = await env.ASSETS.fetch(new URL("/catalog.json", base));
  if (!res.ok) throw new Error(`catalog.json 取不到: ${res.status}`);
  const value = (await res.json()) as Catalog;
  catalogCache = { at: now, value };
  return value;
}

interface Target {
  id: string;
  kind: Kind;
  key: string;
  filename: string;
  size: number;
  sha256: string;
}

async function resolve(env: Env, base: string, id: string): Promise<Target | null> {
  const catalog = await getCatalog(env, base);
  const pack = catalog.packs.find((p) => p.id === id);
  if (pack) {
    return {
      id: pack.id, kind: "pack", key: pack.key,
      filename: `${pack.id}.rkpet`, size: pack.size, sha256: pack.sha256,
    };
  }
  const build = catalog.apps.find((a) => a.id === id);
  if (build) {
    return {
      id: build.id, kind: "app", key: build.key,
      filename: build.filename, size: build.size, sha256: build.sha256,
    };
  }
  return null;
}

// ---------------------------------------------------------------- 去重

function clientIp(req: Request): string {
  return req.headers.get("CF-Connecting-IP") ?? req.headers.get("X-Forwarded-For") ?? "0.0.0.0";
}

function today(): string {
  return new Date().toISOString().slice(0, 10);
}

/** 到今天 UTC 结束还剩几秒。KV 的 expirationTtl 最小 60s,不足就垫到 60。 */
function ttlToMidnight(): number {
  const now = new Date();
  const midnight = Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate() + 1);
  return Math.max(60, Math.ceil((midnight - now.getTime()) / 1000));
}

async function hash(...parts: string[]): Promise<string> {
  const data = new TextEncoder().encode(parts.join("\0"));
  const digest = await crypto.subtle.digest("SHA-256", data);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/**
 * 「今天这个 IP 是不是已经在这个 id 上记过一次了」。
 *
 * 存进 KV 的只有哈希,原始 IP 不落盘。返回 true 表示这次该计数(并已占位)。
 * 读写之间有竞态,但撞上的代价只是偶尔多记一次 —— 比为它上 Durable Object 划算。
 */
async function claim(env: Env, action: "dl" | "rp", id: string, ip: string): Promise<boolean> {
  const tag = await hash(env.DEDUPE_SALT ?? "rocom-pets", ip, today());
  const key = `${action}:${today()}:${id}:${tag.slice(0, 32)}`;
  if (await env.DEDUPE.get(key)) return false;
  await env.DEDUPE.put(key, "1", { expirationTtl: ttlToMidnight() });
  return true;
}

/** 异常标记的 IP 日配额。和去重是两码事:去重管「同一个包」,这个管「全站」。 */
async function underReportQuota(env: Env, ip: string): Promise<{ ok: boolean; tag: string }> {
  const tag = (await hash(env.DEDUPE_SALT ?? "rocom-pets", ip, today())).slice(0, 32);
  const key = `rq:${today()}:${tag}`;
  const used = Number((await env.DEDUPE.get(key)) ?? "0");
  if (used >= REPORTS_PER_IP_PER_DAY) return { ok: false, tag };
  await env.DEDUPE.put(key, String(used + 1), { expirationTtl: ttlToMidnight() });
  return { ok: true, tag };
}

// ---------------------------------------------------------------- 计数

/**
 * 摘要里那个 id 的 JSON 路径,`$."002-喵喵"`。**只有一层** —— 父节点永远是 `$`,
 * 于是新 id 首次出现时 `json_set` 能直接把它建出来(SQLite 不会替你补中间层)。
 *
 * SQLite 的路径解析在双引号键里不认反斜杠转义,所以 id 里带 `"` 就没法安全表达。
 * 那种 id 不会从 `catalog.json` 里来,真碰上了回 null,让调用方**只写明细不写摘要** ——
 * 宁可摘要少这一条(`db:init` 重算能补回来),也不能拼出一条歪路径去改别的键。
 */
function summaryPath(id: string): string | null {
  return id.includes('"') ? null : `$."${id}"`;
}

/**
 * 一次计数要写的语句:**明细一条 + 摘要一条**,永远一起 batch。
 *
 * - 明细(`asset_stats`)是真相。UPSERT 里的 `列 = 列 + 1` 单语句原子,读出来加一
 *   再写回会在并发下丢计数。
 * - 摘要(`stats_summary`)是给 `/api/stats` 单行读的派生物。这里也是**单语句原子**:
 *   `json_set(payload, 路径, json_object(旧值 + 增量))`,增量在 SQL 里算完,不经过
 *   Worker 的读-改-写,所以并发下不会互相盖掉。
 *
 * 两条放一个 `batch()` 里是**一个事务**:要么都成、要么都不成。宁可这次不计数,
 * 也不要出现「明细加了、摘要没加」那种谁也说不清的漂移。
 */
function bumpWrites(
  env: Env,
  id: string,
  kind: Kind,
  column: "downloads" | "reports",
): D1PreparedStatement[] {
  const at = Date.now();
  const dl = column === "downloads" ? 1 : 0;
  const rp = column === "reports" ? 1 : 0;

  const writes = [
    env.DB.prepare(
      `INSERT INTO asset_stats (id, kind, downloads, reports, updated_at)
       VALUES (?1, ?2, ?3, ?4, ?5)
       ON CONFLICT(id) DO UPDATE SET ${column} = ${column} + 1, updated_at = excluded.updated_at`,
    ).bind(id, kind, dl, rp, at),
  ];

  const path = summaryPath(id);
  if (path === null) {
    console.error("id 里有双引号,摘要跳过这一条,等 db:init 重算", id);
    return writes;
  }

  writes.push(
    env.DB.prepare(
      `UPDATE stats_summary
       SET payload = json_set(payload, ?1, json_object(
             -- CAST 不能省:D1 把 JS 的数字当 REAL 绑进来,不铸回整数的话摘要里会
             -- 一路攒出 "downloads":1.0 这种值 —— 前端 JSON.parse 出来虽然还是 1,
             -- 但数字白白变长,而且看一眼库就觉得哪里不对
             'downloads', CAST(COALESCE(json_extract(payload, ?1 || '.downloads'), 0) + ?2 AS INTEGER),
             'reports',   CAST(COALESCE(json_extract(payload, ?1 || '.reports'),   0) + ?3 AS INTEGER)
           )),
           updated_at = ?4
       WHERE id = 1`,
    ).bind(path, dl, rp, at),
  );
  return writes;
}

// ---------------------------------------------------------------- 路由

/**
 * `/api/stats` 读的是 `stats_summary` 那**一行**。以前它是全站唯一一个全表扫,而 D1
 * 按「读了多少行」计费 —— 200 多个包,每回源一次就吃掉 200 多行,免费额度(500 万行/天)
 * 两万多次首屏就见底。改成摘要之后每次回源只读 1 行,并且从此与包的数量脱钩。
 *
 * 摘要由计数那条路顺手维护(见 `bumpWrites`),真相仍在 `asset_stats`,`db:init` 会
 * 从明细重算一遍摘要。
 *
 * 缓存还是要的:1 行也是行,而且省掉的是一整个 D1 往返(十几毫秒)。
 * **只在响应头上写 `s-maxage` 没有用**:Cloudflare 的边缘缓存在 Worker 的*下游*
 * (它缓存的是 Worker 里 `fetch()` 发出去的子请求),Worker 自己 `return` 的响应不会
 * 被自动收进去 —— 那个头只管到浏览器。要让它进边缘缓存,得自己 `caches.default.put()`。
 *
 * 于是这里摞两层,都是 60 秒:
 *
 * - **isolate 内存** —— 同一个 isolate 连着来的请求连 Cache API 那次 await 都省了;
 *   也是本地 `wrangler dev` 唯一生效的一层(miniflare 那边的 Cache API 是个永远 miss 的空壳)。
 * - **Cache API** —— 跨 isolate、同一节点共享,isolate 被回收了也还在。
 *
 * 两层**都只由「真查了 D1」那条路填**,谁也不回填谁:所以最坏就是差 60 秒,不会两层
 * 叠成 120 秒。计数本来也不是实时数据(点了下载是前端本地乐观 +1,见 `App.tsx` 里
 * `stats` 与 `orderStats` 的分工),晚一分钟没人看得出来。
 */
const STATS_TTL_MS = 60 * 1000;
let statsCache: { at: number; body: string } | null = null;

/** 摘要行缺席时的回落。类型标注顺带把「payload 就是 StatsResponse」这件事钉在编译期。 */
const EMPTY_STATS: StatsResponse = {};
const EMPTY_STATS_BODY = JSON.stringify(EMPTY_STATS);

const STATS_HEADERS: Record<string, string> = {
  "content-type": "application/json",
  // max-age 给浏览器;s-maxage 是下面 cache.put() 存多久的依据 —— Cache API 认这一条
  "cache-control": "public, max-age=30, s-maxage=60",
};

/**
 * 缓存键**把 query 丢掉**。`/api/stats?t=1` 和 `/api/stats` 是同一份数据,不归一的话
 * 随手带个随机参数就能一路穿透到 D1,这层缓存等于没加。
 */
function statsCacheKey(url: string): Request {
  const key = new URL(url);
  key.search = "";
  return new Request(key.toString(), { method: "GET" });
}

/**
 * `caches.default` 就是边缘缓存本身,Workers 独有。tsconfig 的 lib 里同时开着 DOM,
 * 而 DOM 那个 `CacheStorage` 没有 `default` 成员又正好压过 workers-types 的声明 ——
 * 运行时是有的,所以这里断言一次,别把它扩散成到处 any。
 */
function edgeCache(): Cache {
  return (caches as unknown as { default: Cache }).default;
}

app.get("/api/stats", async (c) => {
  const now = Date.now();
  if (statsCache && now - statsCache.at < STATS_TTL_MS) {
    return new Response(statsCache.body, { headers: STATS_HEADERS });
  }

  const cache = edgeCache();
  const cacheKey = statsCacheKey(c.req.url);
  const hit = await cache.match(cacheKey);
  if (hit) return hit;

  // payload 本身就是一份 StatsResponse 的 JSON —— 原样当响应体发出去,不解析也不重新序列化
  const row = await c.env.DB.prepare(
    `SELECT payload FROM stats_summary WHERE id = 1`,
  ).first<{ payload: string }>();

  // 摘要行不见了(库还没跑过 db:init)只回空表:前端拿到空对象就是「都还没有数字」,
  // 页面照常出。**别在这儿回落去扫 asset_stats** —— 那正是要躲开的那次全表扫,
  // 真回落了反而是「越缺越贵」。
  const body = row?.payload ?? EMPTY_STATS_BODY;
  statsCache = { at: now, body };

  const res = new Response(body, { headers: STATS_HEADERS });
  // 写缓存别挡在响应前面;本地 dev 那个空壳 Cache 也可能直接抛,别让它冒到请求上
  c.executionCtx.waitUntil(
    cache
      .put(cacheKey, res.clone())
      .catch((err: unknown) => console.error("stats 缓存写入失败", err)),
  );
  return res;
});

app.get("/api/config", (c) => {
  const config: SiteConfig = {
    turnstileSitekey: c.env.TURNSTILE_SITEKEY || null,
    // 预览据此决定直连 R2 还是走下面那条代理。这是个公开域名(下载的 302 早就跳过去了),
    // 发给前端不泄露什么。
    publicBase: c.env.PUBLIC_BASE || null,
  };
  return c.json(config);
});

app.get("/api/dl/:id", async (c) => {
  const id = c.req.param("id");
  const target = await resolve(c.env, c.req.url, id);
  if (!target) return c.json({ error: "没有这个文件" }, 404);

  // 有自定义域就 302 过去:Range、断点续传、边缘缓存全部由 R2 原生处理,
  // Worker 不碰字节。没配就自己代理 —— 能跑,但每次下载都算一次 Worker 请求。
  let res: Response;
  if (c.env.PUBLIC_BASE) {
    const to = new URL(target.key, c.env.PUBLIC_BASE.replace(/\/?$/, "/"));
    res = c.redirect(to.toString(), 302);
  } else {
    res = await streamFromR2(c.env, c.req.raw, target);
  }

  // 只有真发出了字节(或跳走了)才计数 —— 桶里没有这个对象时别把数字加上去。
  // 断点续传的每个 Range 分片都会走一遍这里,靠 IP+日期去重把它收敛成一次。
  if (res.ok || res.status === 206 || res.status === 302) {
    // 计数不能挡在下载前面:放 waitUntil 里,响应已经在返回路上了。
    c.executionCtx.waitUntil(
      (async () => {
        try {
          if (await claim(c.env, "dl", id, clientIp(c.req.raw))) {
            await c.env.DB.batch(bumpWrites(c.env, id, target.kind, "downloads"));
          }
        } catch (err) {
          console.error("下载计数失败", id, err);
        }
      })(),
    );
  }
  return res;
});

/**
 * 预览取包的**回落路径**。配了 `PUBLIC_BASE` 时前端直连 R2(见 `src/lib/api.ts` 的
 * `previewUrl`),不会走到这儿;没配的话(本地 `wrangler dev`)由这里代理字节。
 *
 * 和 `/api/dl/:id` 的差别有两处,都要紧:
 *
 * - **不计数**。预览是按 Range 一片片取的(zip 尾部 → 中央目录 → 那一个形态),
 *   走下载那条路会把「看一眼」记成「装了一个」。
 * - **不 302**。它由 `fetch` 发起,跳到 R2 自定义域就是跨源 —— 直连那条是前端一开始
 *   就把地址指过去、由桶的 CORS 放行,而不是从同源被 302 甩过去。
 */
app.get("/api/preview/:id", async (c) => {
  const target = await resolve(c.env, c.req.url, c.req.param("id"));
  if (!target) return c.json({ error: "没有这个文件" }, 404);
  if (target.kind !== "pack") return c.json({ error: "只有宠物包能预览" }, 400);
  const res = await streamFromR2(c.env, c.req.raw, target);
  // 下载那条路上的 `attachment` 会让浏览器把它当文件存;预览是 fetch 读字节,
  // 留着没害处但语义不对,去掉更干净
  const headers = new Headers(res.headers);
  headers.delete("content-disposition");
  return new Response(res.body, { status: res.status, headers });
});

/**
 * 炫彩共享贴图的**回落路径**,和 `/api/preview/:id` 同一个道理:配了 `PUBLIC_BASE`
 * 时前端直连 R2,没配(本地 `wrangler dev`)由这里代理。
 *
 * 这几张图不进目录(`catalog.json` 只列包与应用),所以 key 是**按名字直接拼**的 ——
 * 那就必须自己把名字关死:`[A-Za-z0-9_]+`,不许有点也不许有斜杠。放开一个 `.`
 * 就等于把整个桶交给客户端遍历。
 *
 * 整取,不支持 Range:最大的一张 2MB,分片取反而多几个往返。
 */
app.get("/api/glassy/:name", async (c) => {
  const name = c.req.param("name");
  if (!/^[A-Za-z0-9_]{1,64}$/.test(name)) {
    return c.json({ error: "名字不合法" }, 400);
  }
  const object = await c.env.FILES.get(`glassy/${name}.png`);
  // 404 是**正常情况**:部署时没传 glassy/ 就没有。前端据此把炫彩那几档禁掉,
  // 和桌面版「这个二进制没烘炫彩素材」是同一句话
  if (!object) return c.json({ error: `R2 里没有 glassy/${name}.png` }, 404);
  const headers = new Headers();
  object.writeHttpMetadata(headers);
  headers.set("etag", object.httpEtag);
  headers.set("content-type", "image/png");
  // 内容按名字定死(导出器出的是同一份),放心让浏览器长期缓存
  headers.set("cache-control", "public, max-age=31536000, immutable");
  return new Response(object.body, { headers });
});

async function streamFromR2(env: Env, req: Request, target: Target): Promise<Response> {
  // 「这次请求要不要按 Range 回」只能看请求头 —— R2 返回的对象上 `range` 字段
  // 即使没请求分片也会被填成 {offset:0, length:size},拿它当判据会让整文件下载
  // 也回 206 + Content-Range,下载器那边就当成分片了。
  const wantsRange = req.headers.has("Range");
  const object = await env.FILES.get(target.key, {
    range: wantsRange ? req.headers : undefined,
    onlyIf: req.headers,
  });

  if (object === null) {
    return Response.json({ error: `R2 里没有 ${target.key}` }, { status: 404 });
  }

  const headers = new Headers();
  object.writeHttpMetadata(headers);
  headers.set("etag", object.httpEtag);
  headers.set("accept-ranges", "bytes");
  headers.set("cache-control", "public, max-age=31536000, immutable");
  headers.set(
    "content-disposition",
    `attachment; filename*=UTF-8''${encodeURIComponent(target.filename)}`,
  );
  if (target.sha256) headers.set("x-rocom-sha256", target.sha256);

  // onlyIf 不满足时 R2 只回元数据、没有 body。是「缓存还新鲜」(304)还是
  // 「前置条件没过」(412),取决于客户端发的是哪一类条件头。
  const body = "body" in object ? object.body : null;
  if (body === null) {
    const revalidating =
      req.headers.has("If-None-Match") || req.headers.has("If-Modified-Since");
    return new Response(null, { status: revalidating ? 304 : 412, headers });
  }

  // R2Range 有三种形状:{offset,length} / {offset} / {length} / {suffix}。预览开包用的是
  // 后缀式(`bytes=-66000`),实测 R2 把它规范化成了带 offset 的那种,但别赌这个 ——
  // 认漏一种就会走进下面的 200 分支,配着整个对象的 content-length 只回一小段,静默截断。
  const span = object.range ? spanOf(object.range, object.size) : null;
  if (wantsRange && span) {
    headers.set("content-range", `bytes ${span.offset}-${span.offset + span.length - 1}/${object.size}`);
    headers.set("content-length", String(span.length));
    return new Response(body, { status: 206, headers });
  }

  headers.set("content-length", String(object.size));
  return new Response(body, { status: 200, headers });
}

/**
 * 把 R2 回的 range 归一成绝对的 {offset, length}。算不出来回 null(交给整取那条路)。
 *
 * **判「值有没有」,不能判「键在不在」** —— R2 回来的对象三个键都在,只是其中几个是
 * `undefined`。用 `"suffix" in range` 的话后缀 Range(`bytes=-66000`)会走进 suffix 分支,
 * `Math.min(undefined, size)` 得 NaN,响应就带上一个 `content-range: bytes NaN-NaN/…`
 * ——— 身子是对的、头在撒谎,比整个报错还难查。踩过一次。
 */
function spanOf(range: R2Range, size: number): { offset: number; length: number } | null {
  const r = range as { offset?: number; length?: number; suffix?: number };
  if (r.suffix !== undefined) {
    const length = Math.min(r.suffix, size);
    return { offset: size - length, length };
  }
  const offset = r.offset ?? 0;
  const length = Math.min(r.length ?? size - offset, size - offset);
  return Number.isFinite(offset) && Number.isFinite(length) && length >= 0
    ? { offset, length }
    : null;
}

// ---------------------------------------------------------------- 外部链接

/**
 * 隐藏/赛季炫彩 `HIDDEN_GLASS_CONF.id` → 款名。分享链接里的 `look` 写的是**名字**
 * (`炫彩:黑白`):wasm 那边虽然也认 id,但前端 `parseLook` 只按名字回填下拉,
 * 拿 id 进去选框会是空的。名字与 id 同 `src/pet/glassy_table.rs` 的 `HIDDEN`。
 */
const HIDDEN_GLASS: Record<number, string> = {
  1: "暗夜拾光",
  2: "狂欢怪谈",
  3: "铅字幻梦",
  1000: "黑白",
};

/** 常规炫彩色号的打包位宽:`glass_value = (粒子id << 20) | 配色id`(同游戏客户端)。 */
const GLASS_PARTICLE_SHIFT = 20;

/**
 * 把 `glass_type:glass_value`(游戏 `GlassInfo` 原样)译成 `look` 里的炫彩那一段。
 * 认不得就回 null —— 调用方当作「没有炫彩」,链接照发,人还是能看到这只。
 */
function glassPart(spec: string | undefined): string | null {
  if (!spec) return null;
  const [t, v] = spec.split(":");
  const type = Number(t);
  const value = Number(v);
  if (!Number.isInteger(type) || !Number.isInteger(value) || value <= 0) return null;
  if (type === 2) {
    const name = HIDDEN_GLASS[value];
    return name ? `炫彩:${name}` : null;
  }
  if (type === 1) {
    const particle = value >>> GLASS_PARTICLE_SHIFT;
    const color = value & ((1 << GLASS_PARTICLE_SHIFT) - 1);
    return particle && color ? `炫彩:${particle}/${color}` : null;
  }
  return null;
}

/**
 * **开放接口**:拿游戏侧的编号换一条预览链接。给外部工具用(抓包统计
 * [rocom-capture](https://github.com/whoisnian/rocom-capture) 的宠物详情页点色卡就走这里),
 * 免得每个调用方都去抄一遍「形态编号 → 包名/资产名」和 `look` 的写法 —— 那两样都是
 * 本仓库自己的实现细节,换个版本就可能变。
 *
 *     GET /api/link?petbase=3007&shiny=1&glass=2:1
 *
 * | 参数 | 是什么 |
 * | --- | --- |
 * | `petbase` | **必填**,形态编号(`PETBASE_CONF` 行 id,即 `PetData.base_conf_id`) |
 * | `shiny`   | 异色,`1`/`true` 为是(即 `mutation_type & 1`) |
 * | `glass`   | 炫彩,`<glass_type>:<glass_value>`,`GlassInfo` 原样送过来 |
 * | `face`    | 表情名,原样透传 |
 * | `format`  | `json` 则回 JSON,默认回 302 |
 *
 * 默认**回 302**,所以可以直接当 `<a href>` 用:不需要 JS、也不吃 CORS。
 * `format=json` 回 `{ url, pet, form, look }`,并放开 CORS 供别的站点取用。
 *
 * **查不到那只形态**(还没出包)时:302 那条跳站点首页并在 query 里留个
 * `missing=<petbase>`(前端只读 pet/form/face/look,多的参数不碍事),让人至少落在
 * 下载站而不是一张 404;`format=json` 则老老实实回 404。
 */
app.get("/api/link", async (c) => {
  const q = c.req.query();
  const petbase = Number(q.petbase);
  const wantJson = q.format === "json";

  if (!Number.isInteger(petbase) || petbase <= 0) {
    return c.json({ error: "petbase 必填,且要是形态编号(PETBASE_CONF 行 id)" }, 400);
  }

  const catalog = await getCatalog(c.env, c.req.url);
  const pack = catalog.packs.find((p) => p.forms.some((f) => f.conf === petbase));
  const form = pack?.forms.find((f) => f.conf === petbase);
  if (!pack || !form) {
    if (wantJson) return c.json({ error: "还没有这个形态的宠物包", petbase }, 404);
    const home = new URL("/", c.req.url);
    home.searchParams.set("missing", String(petbase));
    return c.redirect(home.toString(), 302);
  }

  const parts: string[] = [];
  if (q.shiny === "1" || q.shiny === "true") parts.push("异色");
  const glass = glassPart(q.glass);
  if (glass) parts.push(glass);
  const look = parts.join("+");

  const url = new URL("/", c.req.url);
  url.searchParams.set("pet", pack.id);
  if (form.asset) url.searchParams.set("form", form.asset);
  if (q.face) url.searchParams.set("face", q.face);
  if (look) url.searchParams.set("look", look);

  if (wantJson) {
    return c.json(
      { url: url.toString(), pet: pack.id, form: form.asset, look: look || null },
      200,
      { "Access-Control-Allow-Origin": "*", "Cache-Control": "public, max-age=300" },
    );
  }
  return c.redirect(url.toString(), 302);
});

app.post("/api/report", async (c) => {
  const body = await c.req.json<{
    id?: string; reason?: string; note?: string; token?: string;
  }>().catch(() => null);

  if (!body?.id || !body.reason) return c.json({ error: "缺 id 或 reason" }, 400);
  if (!VALID_REASONS.has(body.reason)) return c.json({ error: "reason 不认识" }, 400);

  const target = await resolve(c.env, c.req.url, body.id);
  if (!target) return c.json({ error: "没有这个文件" }, 404);

  if (c.env.TURNSTILE_SECRET) {
    if (!(await verifyTurnstile(c.env.TURNSTILE_SECRET, body.token, clientIp(c.req.raw)))) {
      return c.json({ error: "人机校验没过,刷新页面重试" }, 403);
    }
  }

  const ip = clientIp(c.req.raw);
  const quota = await underReportQuota(c.env, ip);
  if (!quota.ok) {
    return c.json({ error: `今天标记得有点多(上限 ${REPORTS_PER_IP_PER_DAY} 次),明天再来` }, 429);
  }

  // 同一个 IP 当天对同一个包只算一次;重复提交仍然收下明细,只是不再加数字。
  const counted = await claim(c.env, "rp", body.id, ip);
  const note = body.note?.trim().slice(0, NOTE_MAX) || null;

  const writes: D1PreparedStatement[] = [
    c.env.DB.prepare(
      `INSERT INTO report_log (asset_id, reason, note, ip_tag, created_at)
       VALUES (?1, ?2, ?3, ?4, ?5)`,
    ).bind(body.id, body.reason, note, quota.tag.slice(0, 12), Date.now()),
  ];
  // 明细 + 摘要跟着一起进这个 batch:和下载那条走同一套写法,别在这儿手抄第二份 UPSERT
  if (counted) writes.push(...bumpWrites(c.env, body.id, target.kind, "reports"));
  await c.env.DB.batch(writes);

  return c.json({ ok: true, counted });
});

async function verifyTurnstile(secret: string, token: string | undefined, ip: string) {
  if (!token) return false;
  const form = new FormData();
  form.append("secret", secret);
  form.append("response", token);
  form.append("remoteip", ip);
  const res = await fetch("https://challenges.cloudflare.com/turnstile/v0/siteverify", {
    method: "POST",
    body: form,
  });
  const data = (await res.json()) as { success?: boolean };
  return data.success === true;
}

app.all("/api/*", (c) => c.json({ error: "没有这个接口" }, 404));

// run_worker_first 只把 /api/* 交给 Worker,这条是本地 dev 与配置漂移时的兜底。
app.all("*", (c) => c.env.ASSETS.fetch(c.req.raw));

export default app;
