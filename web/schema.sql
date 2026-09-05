-- rocom-pets 下载站的 D1 结构
--   wrangler d1 create rocom-pets
--   wrangler d1 execute rocom-pets --remote --file schema.sql

-- 计数表。id 同时容纳宠物包(「002-喵喵」)与应用本体(「app-windows-x64」),
-- 两者共用一套下载/异常标记的统计口径。
-- 自增走 UPSERT 的 excluded 语法,单条语句原子完成 —— 不要读出来加一再写回。
CREATE TABLE IF NOT EXISTS asset_stats (
  id         TEXT PRIMARY KEY,
  kind       TEXT NOT NULL CHECK (kind IN ('pack', 'app')),
  downloads  INTEGER NOT NULL DEFAULT 0,
  reports    INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL DEFAULT 0
);

-- 曾经这里有条 idx_stats_downloads (downloads DESC),理由是「默认排序按下载数」。
-- 那个理由不成立:/api/stats 的查询没有 ORDER BY,排序是前端做的(src/lib/search.ts
-- 的 sortHits)。于是这条索引一次也没被读到过,却让每次下载计数**多写一行** ——
-- D1 的「写入行数」是把索引行算进去的,而写入的免费额度(10 万行/天)比读取
-- (500 万行/天)紧得多。删掉,下载计数的写入成本直接减半。
DROP INDEX IF EXISTS idx_stats_downloads;

-- 统计摘要:asset_stats 的**派生物**,整张表压成一行 JSON,形状就是 shared/types.ts 的
-- `StatsResponse`(`{ id: { downloads, reports } }`)。
--
-- 为什么要这一行:D1 按「读了多少行」计费,而 /api/stats 是全站唯一一个全表扫 ——
-- 200 多个包,每回源一次就是 200 多行。压成一行之后那次查询**只读 1 行**,而且从此
-- 与包的数量脱钩(以后出到 500 个包也还是 1 行)。
--
-- 代价是每次计数多写一行(明细一行 + 摘要一行)。这笔钱由上面删掉的那条索引出:
-- 索引行本来就要写一行,一换一,写入成本和以前持平。
--
-- 摘要**不是**真相,asset_stats 才是。摘要坏了/漂了就从明细重算 —— 下面那条
-- INSERT … ON CONFLICT DO UPDATE 干的就是这件事,所以 `npm run db:init` 既是初始化
-- 也是修复工具,而且对**已经有数据的老库**会把现有计数原样搬进摘要,不会清零。
CREATE TABLE IF NOT EXISTS stats_summary (
  -- 只有一行。CHECK 把它钉死,免得哪天多插一行出来两份摘要谁也不知道该信哪个
  id         INTEGER PRIMARY KEY CHECK (id = 1),
  payload    TEXT    NOT NULL DEFAULT '{}',
  updated_at INTEGER NOT NULL DEFAULT 0
);

INSERT INTO stats_summary (id, payload, updated_at)
VALUES (
  1,
  (SELECT COALESCE(
     json_group_object(id, json_object('downloads', downloads, 'reports', reports)),
     '{}'
   ) FROM asset_stats),
  0
)
ON CONFLICT(id) DO UPDATE SET payload = excluded.payload, updated_at = excluded.updated_at;

-- 异常标记的明细。只记数字的话维护者看到「017-火花 被标了 9 次」也无从下手,
-- 所以把原因和可选备注一起留下;IP 不入库,只留当天的去重哈希前 12 位便于甄别刷量。
CREATE TABLE IF NOT EXISTS report_log (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  asset_id   TEXT NOT NULL,
  reason     TEXT NOT NULL,
  note       TEXT,
  ip_tag     TEXT,
  created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_report_asset ON report_log (asset_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_report_time  ON report_log (created_at DESC);
