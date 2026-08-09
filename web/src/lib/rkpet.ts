/**
 * 从远端 `.rkpet` 里**只取要的那几个文件**,不下整包。
 *
 * 一个包中位 6.8MB、最大 31.6MB,而预览一次只看一个形态 —— 那是 2.9MB 左右
 * (glb + 它自己的贴图)。zip 的中央目录在文件末尾,所以三步就够:
 *
 *   1. Range 取尾部一小段 → 找 EOCD → 拿到中央目录的位置与大小;
 *   2. Range 取中央目录 → 每个条目的偏移、压缩大小、压缩方式;
 *   3. 按需 Range 取条目的字节 → deflate 的用 `DecompressionStream` 解。
 *
 * **不用现成的 zip 库**:它们要么要整个 ArrayBuffer,要么要一个 Blob ——
 * 两条路都得先把整包下下来,那这一整套就白做了。
 *
 * 只支持 zip 的两种存法:0 = 原样存、8 = deflate。导出器就写这两种
 * (`--zip` 走 deflate,已经压过的 png/ogg 常被存成 0)。
 */

/** EOCD 至少 22 字节;注释最长 65535。倒着找的时候先取这么多,一次基本就够。 */
const TAIL_BYTES = 66_000;

const SIG_EOCD = 0x0605_4b50;
const SIG_EOCD64_LOCATOR = 0x0706_4b50;
const SIG_EOCD64 = 0x0606_4b50;
const SIG_CENTRAL = 0x0201_4b50;

export interface ZipEntry {
  name: string;
  /** 本地文件头的偏移(不是数据本身:头长度还得现读)。 */
  offset: number;
  compressedSize: number;
  size: number;
  /** 0 = 原样存,8 = deflate。 */
  method: number;
}

export class RemoteZip {
  private constructor(
    readonly url: string,
    readonly entries: Map<string, ZipEntry>,
  ) {}

  /** 读中央目录。这一步只花两次 Range 请求。 */
  static async open(url: string, signal?: AbortSignal): Promise<RemoteZip> {
    const total = await contentLength(url, signal);
    const tailFrom = Math.max(0, total - TAIL_BYTES);
    const tail = await range(url, tailFrom, total - 1, signal);
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
      const rec = await range(url, at, at + 55, signal);
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
        : await range(url, cdOffset, cdOffset + cdSize - 1, signal);

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
        method: cv.getUint16(p + 10, true),
        compressedSize: cv.getUint32(p + 20, true),
        size: cv.getUint32(p + 24, true),
        offset: cv.getUint32(p + 42, true),
      });
      p += 46 + nameLen + extraLen + commentLen;
    }
    return new RemoteZip(url, entries);
  }

  /** 名字以 `prefix` 开头的条目(目录项 —— 以 `/` 结尾的 —— 不算)。 */
  under(prefix: string): ZipEntry[] {
    return [...this.entries.values()].filter(
      (e) => e.name.startsWith(prefix) && !e.name.endsWith("/"),
    );
  }

  /** 这些条目一共要下多少字节,给进度条用。 */
  static bytesOf(entries: ZipEntry[]): number {
    return entries.reduce((n, e) => n + e.compressedSize, 0);
  }

  /** 取一个条目的内容。 */
  async read(entry: ZipEntry, signal?: AbortSignal): Promise<Uint8Array> {
    // 本地文件头的长度只有读了才知道(名字与 extra 的长度可能和中央目录里的不一样),
    // 所以先取那 30 字节的定长头,再按头里写的长度往后跳
    const head = await range(this.url, entry.offset, entry.offset + 29, signal);
    const hv = new DataView(head.buffer, head.byteOffset, head.byteLength);
    const skip = 30 + hv.getUint16(26, true) + hv.getUint16(28, true);
    const from = entry.offset + skip;
    const raw = await range(this.url, from, from + entry.compressedSize - 1, signal);
    if (entry.method === 0) return raw;
    if (entry.method !== 8) throw new Error(`${entry.name}: 不认得的压缩方式 ${entry.method}`);
    return inflateRaw(raw);
  }
}

async function contentLength(url: string, signal?: AbortSignal): Promise<number> {
  // **不用 HEAD**:R2 自定义域上 HEAD 会被 302 到别处,而且有些中间层不回 content-length。
  // 取一个字节的 Range,长度从 content-range 的分母上读,一定准。
  const res = await fetch(url, { headers: { Range: "bytes=0-0" }, signal });
  if (!res.ok) throw new Error(`取不到 ${url}(HTTP ${res.status})`);
  const cr = res.headers.get("content-range");
  await res.arrayBuffer();
  const total = cr && Number(cr.split("/")[1]);
  if (!total || !Number.isFinite(total)) throw new Error("这个地址不支持 Range 请求");
  return total;
}

async function range(
  url: string,
  from: number,
  to: number,
  signal?: AbortSignal,
): Promise<Uint8Array> {
  const res = await fetch(url, { headers: { Range: `bytes=${from}-${to}` }, signal });
  if (!res.ok) throw new Error(`取 ${from}-${to} 失败(HTTP ${res.status})`);
  return new Uint8Array(await res.arrayBuffer());
}

/** deflate(裸流,没有 zlib 头)→ 原文。浏览器自带,不必带一份 inflate 进来。 */
async function inflateRaw(data: Uint8Array): Promise<Uint8Array> {
  const stream = new Blob([data as BlobPart]).stream().pipeThrough(
    new DecompressionStream("deflate-raw"),
  );
  return new Uint8Array(await new Response(stream).arrayBuffer());
}
