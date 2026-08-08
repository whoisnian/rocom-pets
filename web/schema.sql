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

-- 默认排序是「下载次数倒序」,这条索引让首屏那一次全表扫免了排序开销。
CREATE INDEX IF NOT EXISTS idx_stats_downloads ON asset_stats (downloads DESC);

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
