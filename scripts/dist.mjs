import { execSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const fail = (msg) => {
  console.error(`[dist] ✗ ${msg}`);
  process.exit(1);
};
const mb = (p) => (statSync(p).size / 1024 / 1024).toFixed(2);

const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
const conf = JSON.parse(readFileSync(join(root, "src-tauri/tauri.conf.json"), "utf8"));
const cargoToml = readFileSync(join(root, "src-tauri/Cargo.toml"), "utf8");
const cargoVer = cargoToml.slice(0, cargoToml.indexOf("[lib]")).match(/^\s*version\s*=\s*"([^"]+)"/m)?.[1];
const versions = { "package.json": pkg.version, "tauri.conf.json": conf.version, "Cargo.toml": cargoVer };
const uniq = [...new Set(Object.values(versions))];
if (uniq.length !== 1) {
  fail(`版本号不一致：${Object.entries(versions).map(([k, v]) => `${k}=${v}`).join("  ")}`);
}
const ver = uniq[0];
console.log(`[dist] ✓ 版本一致：v${ver}`);

// ── 版本快照：安装包内置「官方最新版本」，换机 / 缓存被清 / 首次离线时兜底 ──
// 优先用本机 changelog 缓存（无网络也能发包），否则请求官方 changelog；
// 两者都失败时保留已有快照，绝不写空。
const snapshotPath = join(root, "src-tauri/resources/known-latest.json");
const readCachedLatest = () => {
  const home = process.env.USERPROFILE || process.env.HOME || "";
  try {
    const v = JSON.parse(readFileSync(join(home, ".zcode-switch", "changelog-cache.json"), "utf8")).version;
    return typeof v === "string" && v.trim() ? v.trim() : null;
  } catch {
    return null;
  }
};
const fetchChangelogLatest = async () => {
  const res = await fetch("https://zcode.z.ai/cn/changelog", { signal: AbortSignal.timeout(10000) });
  const html = await res.text();
  const hits = [...html.matchAll(/Release\s+v(\d+\.\d+\.\d+)/gi)].map((m) => m[1]);
  if (!hits.length) return null;
  const cmp = (a, b) => {
    const pa = a.split(".").map(Number);
    const pb = b.split(".").map(Number);
    for (let i = 0; i < 3; i += 1) if (pa[i] !== pb[i]) return pa[i] - pb[i];
    return 0;
  };
  return hits.sort(cmp).pop();
};
let snapshotVersion = readCachedLatest();
if (!snapshotVersion) {
  try {
    snapshotVersion = await fetchChangelogLatest();
  } catch {
    snapshotVersion = null;
  }
}
if (snapshotVersion) {
  mkdirSync(dirname(snapshotPath), { recursive: true });
  writeFileSync(
    snapshotPath,
    `${JSON.stringify({ version: snapshotVersion, generated_at: Math.floor(Date.now() / 1000) }, null, 2)}\n`,
  );
  console.log(`[dist] ✓ 版本快照 ${snapshotVersion}`);
} else if (existsSync(snapshotPath)) {
  console.warn("[dist] ! 未能获取官方最新版本，沿用已有快照");
} else {
  fail("缺少 resources/known-latest.json 且无法获取官方版本，构建会失败");
}

console.log("[dist] npm run tauri build …");
try {
  execSync("npm run tauri build", { cwd: root, stdio: "inherit" });
} catch {
  fail("构建失败（若是 os error 32：旧实例还在托盘驻留锁住了 exe，退出后重试）");
}

const bundleDir = join(root, "src-tauri/target/release/bundle/nsis");
if (!existsSync(bundleDir)) fail(`找不到打包目录：${bundleDir}`);
const setups = readdirSync(bundleDir)
  .filter((f) => f.endsWith("-setup.exe"))
  .map((f) => ({ f, t: statSync(join(bundleDir, f)).mtimeMs }))
  .sort((a, b) => b.t - a.t);
if (!setups.length) fail("bundle/nsis 下没有 *-setup.exe");
const setup = setups[0].f;
if (!setup.includes(`_${ver}_`)) {
  fail(`最新安装包 ${setup} 不含当前版本 ${ver} —— 本次构建可能未产出 NSIS 包，请检查上方构建日志`);
}

const rawExe = join(root, "src-tauri/target/release/zcode-switch.exe");
if (!existsSync(rawExe)) fail(`找不到裸 exe：${rawExe}`);

const outDir = join(root, "release");
mkdirSync(outDir, { recursive: true });
const setupDst = join(outDir, setup);
const portableDst = join(outDir, `zcode-switch_${ver}_portable.exe`);
copyFileSync(join(bundleDir, setup), setupDst);
copyFileSync(rawExe, portableDst);
// 便携版与 exe 同目录：运行时优先读 exe 旁边的快照
if (existsSync(snapshotPath)) copyFileSync(snapshotPath, join(outDir, "known-latest.json"));

console.log(`[dist] ✓ 安装包  ${setup}  (${mb(setupDst)} MB)`);
console.log(`[dist] ✓ 便携版  ${basename(portableDst)}  (${mb(portableDst)} MB)`);
console.log(`[dist] 完成 → ${outDir}`);
