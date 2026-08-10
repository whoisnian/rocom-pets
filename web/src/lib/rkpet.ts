/**
 * 从远端 `.rkpet` 里**只取要的那几个文件**,不下整包。
 *
 * 一个包中位 7.0MB、最大 33.1MB,而预览一次只看一个形态 —— 那是 2.9MB 左右
 * (glb + 它自己的贴图)。zip 的中央目录在文件末尾,所以:
 *
 *   1. 一次**后缀 Range**(`bytes=-66000`)同时拿到尾部与总长 → 找 EOCD → 中央目录;
 *   2. 把要的条目按「在包里首尾相接」分组,**一段一次 Range 取完**,段之间并发;
 *   3. 在段的缓冲区里按各条目的绝对偏移切片,deflate 的用 `DecompressionStream` 解。
 *
 * **不用现成的 zip 库**:它们要么要整个 ArrayBuffer,要么要一个 Blob ——
 * 两条路都得先把整包下下来,那这一整套就白做了。
 *
 * 只支持 zip 的两种存法:0 = 原样存、8 = deflate。导出器就写这两种
 * (`--zip` 走 deflate,已经压过的 png/ogg 常被存成 0)。
 *
 * 为什么是「按段取」而不是「每个文件一次」:实测(`scripts/bench_preview.mjs`)在
 * 高延迟链路上,请求数才是墙上时间的大头 —— 短 Range 请求全程待在 TCP 慢启动里,
 * 一个卡住整批就得等它。导出器写包时一个形态的 glb 一段、`tex/` 一段,所以渲染要用的
 * 文件天然就落成 **2 段**(12 个包 / 35 个形态实测,最大也是 2),段内零空隙 ——
 * 于是 5 次请求变 2 次,**一个字节都不多下**。哪天导出器换了排布,分段会自然退化成
 * 更多段,行为等价于「每个文件一次」,不会出错。
 */

/** EOCD 至少 22 字节;注释最长 65535。倒着找的时候先取这么多,一次基本就够。 */
const TAIL_BYTES = 66_000;

/**
 * 本地文件头里 extra 字段的长度**可能和中央目录里记的不一样**,所以按中央目录估算的
 * 条目末尾要留这么多余量。真空隙都是几百 KB 到几 MB 级别,不会被这 4KB 误并。
 */
const SLACK = 4096;

const SIG_EOCD = 0x0605_4b50;
const SIG_EOCD64_LOCATOR = 0x0706_4b50;
const SIG_EOCD64 = 0x0606_4b50;
const SIG_CENTRAL = 0x0201_4b50;

export interface ZipEntry {
  name: string;
  /** 本地文件头的偏移(不是数据本身:头长度还得现读)。 */
  offset: number;
  /** 中央目录里记的文件名长度,用来估算条目末尾。 */
  nameLen: number;
  compressedSize: number;
  size: number;
  /** 0 = 原样存,8 = deflate。 */
  method: number;
}

/** 一段首尾相接的条目,取的时候合成一次 Range。 */
interface Run {
  entries: ZipEntry[];
  from: number;
  /** 闭区间的末字节。按中央目录估算 + SLACK,超出文件末尾由服务端夹住。 */
  to: number;
}

export interface ReadAllOptions {
  signal?: AbortSignal;
  /** 每解出一个条目调一次。段内的条目会在这一段下完之后连着回调。 */
  onData: (name: string, data: Uint8Array) => void;
  /** 字节到达就调,给进度条用。`total` 是所有段加起来要传的字节数。 */
  onProgress?: (done: number, total: number) => void;
}

export class RemoteZip {
  private constructor(
    readonly url: string,
    readonly entries: Map<string, ZipEntry>,
  ) {}

  /** 读中央目录。顺利的话**只花一次请求**。 */
  static async open(url: string, signal?: AbortSignal): Promise<RemoteZip> {
    // 后缀 Range 一次搞定「取尾部」和「问总长」两件事:总长从 content-range 的分母上读。
    // 以前是先发一个 `bytes=0-0` 问长度再取尾部,白花一个往返。
    const { data: tail, total } = await fetchRange(url, `-${TAIL_BYTES}`, signal);
    if (total === null) throw new Error("这个地址不支持 Range 请求");
    const tailFrom = total - tail.byteLength;
    const view = new DataView(tail.buffer, tail.byteOffset, tail.byteLength);

    // EOCD 从后往前找:注释里可能混着相同的四个字节,而**最后一个**才是真的
    let eocd = -1;
    for (let i = tail.byteLength - 22; i >= 0; i--) {
      if (view.getUint32(i, true) === SIG_EOCD) {
        eocd = i;
        break;
      }
    }
    if (eocd < 0) throw new Error("不是 zip(找不到 EOCD)");

    let count = view.getUint16(eocd + 10, true);
    let cdSize = view.getUint32(eocd + 12, true);
    let cdOffset = view.getUint32(eocd + 16, true);

    // zip64:条目数或偏移到顶(0xFFFF/0xFFFFFFFF)时真值在 zip64 那份 EOCD 里。
    // 现在的包还到不了 4GB,但留着这段比日后突然读不出来强。
    if (count === 0xffff || cdOffset === 0xffff_ffff) {
      const locator = eocd - 20;
      if (locator < 0 || view.getUint32(locator, true) !== SIG_EOCD64_LOCATOR) {
        throw new Error("zip64 定位器缺失");
      }
      const at = Number(view.getBigUint64(locator + 8, true));
      const { data: rec } = await fetchRange(url, `${at}-${at + 55}`, signal);
      const rv = new DataView(rec.buffer, rec.byteOffset, rec.byteLength);
      if (rv.getUint32(0, true) !== SIG_EOCD64) throw new Error("zip64 EOCD 对不上");
      count = Number(rv.getBigUint64(32, true));
      cdSize = Number(rv.getBigUint64(40, true));
      cdOffset = Number(rv.getBigUint64(48, true));
    }

    // 中央目录多半就在刚取的那一段里,能省一次请求
    const cd =
      cdOffset >= tailFrom && cdOffset + cdSize <= total
        ? tail.subarray(cdOffset - tailFrom, cdOffset - tailFrom + cdSize)
        : (await fetchRange(url, `${cdOffset}-${cdOffset + cdSize - 1}`, signal)).data;

    return new RemoteZip(url, parseCentral(cd, count));
  }

  /** 名字以 `prefix` 开头的条目(目录项 —— 以 `/` 结尾的 —— 不算)。 */
  under(prefix: string): ZipEntry[] {
    return [...this.entries.values()].filter(
      (e) => e.name.startsWith(prefix) && !e.name.endsWith("/"),
    );
  }

  /** 取一个条目的内容。 */
  async read(entry: ZipEntry, signal?: AbortSignal): Promise<Uint8Array> {
    // 本地文件头的长度只有读了才知道(名字与 extra 的长度可能和中央目录里的不一样),
    // 但没必要为此单发一个 30 字节的请求 —— 连着数据一起多取 SLACK 字节,回来再按
    // 头里写的长度往后跳。余量不够时(extra 特别长)才回落到分两次那条路。
    const to = entry.offset + 30 + entry.nameLen + SLACK + entry.compressedSize - 1;
    const { data } = await fetchRange(this.url, `${entry.offset}-${to}`, signal);
    const skip = localHeaderSize(data, 0);
    if (skip + entry.compressedSize > data.byteLength) return this.readTwoStep(entry, signal);
    return decode(entry, data.subarray(skip, skip + entry.compressedSize));
  }

  /**
   * 一次把一批条目取回来:先按「在包里首尾相接」分组,一段一次 Range,段之间并发。
   *
   * 进度按**传输字节**算(不是解压后的),边下边报,所以只有两段也不会一跳一跳的。
   */
  async readAll(entries: ZipEntry[], opts: ReadAllOptions): Promise<void> {
    const runs = groupRuns(entries);
    const total = runs.reduce((n, r) => n + (r.to - r.from + 1), 0);
    let done = 0;
    opts.onProgress?.(0, total);

    await Promise.all(
      runs.map(async (run) => {
        const { data } = await fetchRange(this.url, `${run.from}-${run.to}`, opts.signal, (n) => {
          done += n;
          opts.onProgress?.(Math.min(done, total), total);
        });
        for (const entry of run.entries) {
          const at = entry.offset - run.from;
          // 中央目录给的是绝对偏移,所以段内定位是精确的,不用扫签名
          const skip = at + localHeaderSize(data, at);
          const raw =
            skip + entry.compressedSize <= data.byteLength
              ? data.subarray(skip, skip + entry.compressedSize)
              : null;
          // 只可能发生在段末且 extra 超过 SLACK —— 单独补一次
          opts.onData(entry.name, raw ? await decode(entry, raw) : await this.read(entry, opts.signal));
        }
      }),
    );

    opts.onProgress?.(total, total);
  }

  /** 分两次取(先 30 字节本地头,再数据)。只在合并取的余量不够时用。 */
  private async readTwoStep(entry: ZipEntry, signal?: AbortSignal): Promise<Uint8Array> {
    const { data: head } = await fetchRange(this.url, `${entry.offset}-${entry.offset + 29}`, signal);
    const from = entry.offset + localHeaderSize(head, 0);
    const { data: raw } = await fetchRange(
      this.url,
      `${from}-${from + entry.compressedSize - 1}`,
      signal,
    );
    return decode(entry, raw);
  }
}

// ---------------------------------------------------------------- 中央目录

function parseCentral(cd: Uint8Array, count: number): Map<string, ZipEntry> {
  const entries = new Map<string, ZipEntry>();
  const cv = new DataView(cd.buffer, cd.byteOffset, cd.byteLength);
  const decoder = new TextDecoder();
  let p = 0;
  for (let i = 0; i < count && p + 46 <= cd.byteLength; i++) {
    if (cv.getUint32(p, true) !== SIG_CENTRAL) break;
    const nameLen = cv.getUint16(p + 28, true);
    const extraLen = cv.getUint16(p + 30, true);
    const commentLen = cv.getUint16(p + 32, true);
    const name = decoder.decode(cd.subarray(p + 46, p + 46 + nameLen));
    entries.set(name, {
      name,
      nameLen,
      method: cv.getUint16(p + 10, true),
      compressedSize: cv.getUint32(p + 20, true),
      size: cv.getUint32(p + 24, true),
      offset: cv.getUint32(p + 42, true),
    });
    p += 46 + nameLen + extraLen + commentLen;
  }
  return entries;
}

/** 本地文件头占多少字节(定长 30 + 名字 + extra)。`at` 是头在 `data` 里的位置。 */
function localHeaderSize(data: Uint8Array, at: number): number {
  const v = new DataView(data.buffer, data.byteOffset, data.byteLength);
  return 30 + v.getUint16(at + 26, true) + v.getUint16(at + 28, true);
}

/** 按中央目录估算的条目末尾(闭区间),含 SLACK 余量。 */
function endOf(e: ZipEntry): number {
  return e.offset + 30 + e.nameLen + SLACK + e.compressedSize - 1;
}

/** 把条目按「首尾相接」分组;组内按偏移排好,取回来才能直接切片。 */
function groupRuns(entries: ZipEntry[]): Run[] {
  const sorted = [...entries].sort((a, b) => a.offset - b.offset);
  const runs: Run[] = [];
  for (const e of sorted) {
    const last = runs[runs.length - 1];
    if (last && e.offset <= last.to + 1) {
      last.entries.push(e);
      last.to = Math.max(last.to, endOf(e));
    } else {
      runs.push({ entries: [e], from: e.offset, to: endOf(e) });
    }
  }
  return runs;
}

// ---------------------------------------------------------------- 传输

interface Fetched {
  data: Uint8Array;
  /** 对象总长,从 content-range 的分母上读;不是分片响应时为 null。 */
  total: number | null;
}

/**
 * 取一段字节。`spec` 是 Range 头的值(不含 `bytes=`),支持 `a-b` 与后缀式 `-n`。
 *
 * 给了 `onBytes` 就边读边报,进度条因此不必等整段下完才动。
 */
async function fetchRange(
  url: string,
  spec: string,
  signal?: AbortSignal,
  onBytes?: (n: number) => void,
): Promise<Fetched> {
  const res = await fetch(url, { headers: { Range: `bytes=${spec}` }, signal });
  if (!res.ok) throw new Error(`取 ${spec} 失败(HTTP ${res.status})`);

  const cr = res.headers.get("content-range");
  const parsed = cr ? Number(cr.split("/")[1]) : NaN;
  const total = Number.isFinite(parsed) ? parsed : null;

  if (!onBytes || !res.body) return { data: new Uint8Array(await res.arrayBuffer()), total };

  const chunks: Uint8Array[] = [];
  let len = 0;
  const reader = res.body.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    len += value.byteLength;
    onBytes(value.byteLength);
  }
  const data = new Uint8Array(len);
  let at = 0;
  for (const c of chunks) {
    data.set(c, at);
    at += c.byteLength;
  }
  return { data, total };
}

async function decode(entry: ZipEntry, raw: Uint8Array): Promise<Uint8Array> {
  if (entry.method === 0) return raw;
  if (entry.method !== 8) throw new Error(`${entry.name}: 不认得的压缩方式 ${entry.method}`);
  return inflateRaw(raw);
}

/** deflate(裸流,没有 zlib 头)→ 原文。浏览器自带,不必带一份 inflate 进来。 */
async function inflateRaw(data: Uint8Array): Promise<Uint8Array> {
  const stream = new Blob([data as BlobPart]).stream().pipeThrough(
    new DecompressionStream("deflate-raw"),
  );
  return new Uint8Array(await new Response(stream).arrayBuffer());
}
