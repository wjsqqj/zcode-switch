/// 活动时间的唯一语义实现：格式化 / 文案解析 / 取值优先级 / 区间 kind 判定。
///
/// 契约（见 .trellis/spec/backend/claim-plans.md §10）：
/// - `starts_at` 只是**活动开始**，永远不能当「生效时间」；
/// - 真实生效时刻是 `entitlements[].effective_at`（0 = 立即生效）；
/// - 字段缺失时才允许从活动正文兜底解析，且调用方必须标注来源；
/// - 都没有 → 返回 null，由调用方显示「领取后确认」，禁止猜测。

const pad2 = (n) => String(n).padStart(2, "0");

/// 统一时间格式：`YYYY-MM-DD HH:mm`（带年份，跨年不歧义）。
/// 手工补零而不是 toLocaleString，避免 hour12 / 分隔符 / 千分位的本地化差异。
export function fmtDateTime(ms) {
  if (ms == null || ms === "") return "";
  const n = Number(ms);
  if (!isFinite(n)) return "";
  const d = new Date(n);
  if (isNaN(d.getTime())) return "";
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

const numOrNull = (v) => {
  if (v == null || v === "") return null;
  const n = Number(v);
  return isFinite(n) ? n : null;
};

// 一个组合正则：完整日期优先，其次是「M月D日HH:mm」「M/D HH:mm」。
// 组合而不是逐条跑，是为了避免同一段文本被多次匹配出重复候选。
const RE_DT = new RegExp(
  [
    "(\\d{4})\\s*[年\\-/.·]\\s*(\\d{1,2})\\s*[月\\-/.·]\\s*(\\d{1,2})\\s*日?\\s*[T ]?\\s*(\\d{1,2})\\s*[:：]\\s*(\\d{2})",
    "(\\d{1,2})\\s*月\\s*(\\d{1,2})\\s*日\\s*(\\d{1,2})\\s*[:：]\\s*(\\d{2})",
    "(\\d{1,2})/(\\d{1,2})\\s+(\\d{1,2})\\s*[:：]\\s*(\\d{2})",
  ].join("|"),
  "g",
);

const buildMs = (y, mo, d, h, mi) => {
  if (!(mo >= 1 && mo <= 12) || !(d >= 1 && d <= 31) || !(h >= 0 && h <= 23) || !(mi >= 0 && mi <= 59)) {
    return null;
  }
  const dt = new Date(y, mo - 1, d, h, mi, 0, 0);
  // 回读各字段：挡掉 2月30日 / 25:00 这类被 Date 静默进位的脏数据
  if (
    dt.getFullYear() !== y ||
    dt.getMonth() !== mo - 1 ||
    dt.getDate() !== d ||
    dt.getHours() !== h ||
    dt.getMinutes() !== mi
  ) {
    return null;
  }
  return dt.getTime();
};

const inferYear = (mo, d, h, mi, now) => {
  const y = new Date(now).getFullYear();
  const ms = buildMs(y, mo, d, h, mi);
  if (ms == null) return null;
  // 文案通常描述当期活动：算出来比现在早 30 天以上，视为「下一年」的同月日
  return ms < now - 30 * 86400000 ? buildMs(y + 1, mo, d, h, mi) : ms;
};

/// 从一段文案里抽出全部时间候选（保持出现顺序）。
function candidatesIn(seg, now) {
  const out = [];
  RE_DT.lastIndex = 0;
  let m;
  while ((m = RE_DT.exec(seg)) !== null) {
    let ms = null;
    if (m[1] != null) {
      ms = buildMs(+m[1], +m[2], +m[3], +m[4], +m[5]);
    } else if (m[6] != null) {
      ms = inferYear(+m[6], +m[7], +m[8], +m[9], now);
    } else if (m[10] != null) {
      ms = inferYear(+m[10], +m[11], +m[12], +m[13], now);
    }
    if (ms != null) out.push({ at: ms, start: m.index, end: m.index + m[0].length });
    RE_DT.lastIndex = m.index + Math.max(1, m[0].length);
  }
  return out;
}

/// 从活动正文解析「将于 YYYY年M月D日HH:mm 生效」里的生效时间。
///
/// 只在**含「生效」的句段**里找时间，并取「生效」之前最近的那个时间，
/// 避免把同句的活动开始/结束时间误当生效时间。解析不出来返回 null。
export function parseEffectiveFromCopy(text, now = Date.now()) {
  const raw = String(text == null ? "" : text).trim();
  if (!raw || !raw.includes("生效")) return null;
  for (const seg of raw.split(/[。；;\n\r]+/)) {
    const pos = seg.indexOf("生效");
    if (pos < 0) continue;
    const cands = candidatesIn(seg, now);
    if (!cands.length) continue;
    const before = cands.filter((c) => c.start < pos);
    return (before.length ? before[before.length - 1] : cands[0]).at;
  }
  return null;
}

/// 生效时间取值优先级：字段 → 文案兜底 → 无。
/// 返回 `{ at, source }`，source ∈ {"field","copy"}；无则 `{ at: null, source: null }`。
export function resolveEffective(plan, campaign) {
  const field = numOrNull(plan?.effective_at ?? plan?.effectiveAt);
  if (field != null) return { at: field, source: "field" };
  const copy = parseEffectiveFromCopy(
    campaign?.body || campaign?.description || plan?.description || "",
  );
  if (copy != null) return { at: copy, source: "copy" };
  return { at: null, source: null };
}

/// 行内/汇总条要展示的区间形态。
///
/// kind: `effective`（已生效/立即生效）| `pending`（字段给的待生效）|
///       `estimated`（文案推测的待生效）| `activity`（只有活动时间窗）| `none`
/// 调用方按 kind 选 i18n key，禁止再用 starts_at 顶替生效时间。
export function rangeOf(row, now = Date.now()) {
  const startsAt = numOrNull(row?.startsAt);
  const endsAt = numOrNull(row?.endsAt);
  const effectiveAt = numOrNull(row?.effectiveAt);
  const source = row?.effectiveSource || (effectiveAt != null ? "field" : null);
  if (effectiveAt != null) {
    if (effectiveAt > now) {
      return { kind: source === "copy" ? "estimated" : "pending", startMs: effectiveAt, endMs: endsAt };
    }
    return { kind: "effective", startMs: effectiveAt, endMs: endsAt };
  }
  if (startsAt != null) return { kind: "activity", startMs: startsAt, endMs: endsAt };
  return { kind: "none", startMs: null, endMs: endsAt };
}

/// 详情「生效时间」字段的形态：`immediate` | `field` | `copy` | `pending`。
export function effectiveDetail(row) {
  const effectiveAt = numOrNull(row?.effectiveAt);
  const source = row?.effectiveSource || (effectiveAt != null ? "field" : null);
  if (effectiveAt === 0) return { kind: "immediate", at: 0 };
  if (effectiveAt != null) return { kind: source === "copy" ? "copy" : "field", at: effectiveAt };
  return { kind: "pending", at: null };
}
