// 活动时间语义单测：node scripts/test-claim-time.mjs
//
// 锁定的契约：
// 1. 时间格式统一 `YYYY-MM-DD HH:mm`；
// 2. `starts_at` 永不参与「生效时间」（rangeOf 只能给 activity）；
// 3. 文案解析只认「生效」句段里、位于「生效」之前最近的时间，失败返回 null；
// 4. 字段优先于文案。
import assert from "node:assert/strict";
import {
  fmtDateTime,
  parseEffectiveFromCopy,
  resolveEffective,
  rangeOf,
  effectiveDetail,
} from "../src/claim-time.js";

const at = (y, mo, d, h, mi) => new Date(y, mo - 1, d, h, mi, 0, 0).getTime();
const NOW = at(2026, 11, 3, 12, 0);
let n = 0;
const test = (name, fn) => {
  fn();
  n++;
  console.log(`ok ${n} - ${name}`);
};

test("fmtDateTime 带年份且补零", () => {
  assert.equal(fmtDateTime(at(2026, 11, 3, 23, 0)), "2026-11-03 23:00");
  assert.equal(fmtDateTime(at(2026, 1, 5, 9, 7)), "2026-01-05 09:07");
  assert.equal(fmtDateTime(null), "");
  assert.equal(fmtDateTime(""), "");
  assert.equal(fmtDateTime("abc"), "");
});

test("解析完整日期文案（真实活动原文）", () => {
  const body = "GLM-5.3-Flash将于2026年11月3日23:00生效";
  assert.equal(parseEffectiveFromCopy(body, NOW), at(2026, 11, 3, 23, 0));
});

test("同句有开始时间时取「生效」前最近的那个", () => {
  const body = "活动 2026-11-03 19:00 开始，2026-11-03 23:00 生效";
  assert.equal(parseEffectiveFromCopy(body, NOW), at(2026, 11, 3, 23, 0));
});

test("没有「生效」字样不解析（避免把活动时间当生效）", () => {
  assert.equal(parseEffectiveFromCopy("活动 2026-11-03 19:00 至 11-04 09:00", NOW), null);
  assert.equal(parseEffectiveFromCopy("", NOW), null);
});

test("无年份的「M月D日HH:mm」按当年推断", () => {
  assert.equal(parseEffectiveFromCopy("将于11月3日23:00生效", NOW), at(2026, 11, 3, 23, 0));
});

test("无年份且已过去 30 天以上 → 顺延到次年", () => {
  assert.equal(parseEffectiveFromCopy("将于1月5日09:00生效", NOW), at(2027, 1, 5, 9, 0));
});

test("非法日期解析失败返回 null，不猜测", () => {
  assert.equal(parseEffectiveFromCopy("将于2026年2月30日23:00生效", NOW), null);
});

test("字段优先于文案", () => {
  const field = resolveEffective({ effective_at: 0 }, { body: "将于2026年11月3日23:00生效" });
  assert.deepEqual(field, { at: 0, source: "field" });
});

test("字段缺失时用文案并标记来源", () => {
  const copy = resolveEffective({}, { body: "将于2026年11月3日23:00生效" });
  assert.deepEqual(copy, { at: at(2026, 11, 3, 23, 0), source: "copy" });
  assert.deepEqual(resolveEffective({}, {}), { at: null, source: null });
});

test("rangeOf：starts_at 只能给 activity，不给 effective/pending", () => {
  const startsAt = at(2026, 11, 3, 19, 0);
  const endsAt = at(2026, 11, 4, 9, 0);
  assert.equal(rangeOf({ startsAt, endsAt }, NOW).kind, "activity");
  assert.equal(rangeOf({ endsAt }, NOW).kind, "none");
});

test("rangeOf：字段待生效 → pending，文案推测 → estimated", () => {
  const eff = at(2026, 11, 3, 23, 0);
  assert.equal(rangeOf({ effectiveAt: eff, effectiveSource: "field" }, NOW).kind, "pending");
  assert.equal(rangeOf({ effectiveAt: eff, effectiveSource: "copy" }, NOW).kind, "estimated");
  assert.equal(rangeOf({ effectiveAt: at(2026, 11, 3, 10, 0) }, NOW).kind, "effective");
  assert.equal(rangeOf({ effectiveAt: 0 }, NOW).kind, "effective");
});

test("effectiveDetail：0=立即生效，缺失=领取后确认，文案=copy", () => {
  assert.equal(effectiveDetail({ effectiveAt: 0 }).kind, "immediate");
  assert.equal(effectiveDetail({}).kind, "pending");
  assert.equal(effectiveDetail({ effectiveAt: at(2026, 11, 3, 23, 0), effectiveSource: "copy" }).kind, "copy");
  assert.equal(effectiveDetail({ effectiveAt: at(2026, 11, 3, 23, 0) }).kind, "field");
});

console.log(`\n${n} passed`);
