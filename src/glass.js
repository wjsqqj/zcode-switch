// macOS 原生玻璃同步：测量带 data-glass 的元素，整帧提交给 Rust（src-tauri/src/glass.rs）。
// 玻璃在 WKWebView 下方，因此这些元素自身及其上方各层都不能绘制底色；底色由原生底色层提供（--glass-base）。
// data-glass-source="desktop"：底色层在此镂空、取样桌面；缺省取样窗口底色。其它平台为空操作。
import { invoke } from "@tauri-apps/api/core";

export const IS_MAC = document.documentElement.classList.contains("mac");

let queued = false;
let last = "";
let ro = null;

function collect() {
  const panes = [];
  for (const el of document.querySelectorAll("[data-glass]")) {
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) continue;
    const cs = getComputedStyle(el);
    if (cs.display === "none" || cs.visibility === "hidden") continue;
    panes.push({
      id: el.dataset.glass,
      x: r.left,
      y: r.top,
      w: r.width,
      h: r.height,
      radius: parseFloat(cs.borderTopLeftRadius) || 0,
      source: el.dataset.glassSource || "base",
    });
  }
  const root = getComputedStyle(document.documentElement);
  return {
    panes,
    base: root.getPropertyValue("--glass-base").trim() || "#1c1c1f",
    dark: matchMedia("(prefers-color-scheme: dark)").matches,
  };
}

function flush() {
  queued = false;
  const frame = collect();
  const key = JSON.stringify(frame);
  if (key === last) return;
  last = key;
  invoke("glass_sync", { frame }).catch(() => { last = ""; });
}

// 不用 requestAnimationFrame：窗口被其它窗口完全遮挡时 WebKit 会暂停 rAF，玻璃几何就停在旧帧；
// getBoundingClientRect 本身会同步完成布局，用一个宏任务合并同一轮里的多次触发即可
export function syncGlass() {
  if (!IS_MAC || queued) return;
  queued = true;
  setTimeout(flush, 0);
}

export function initGlass() {
  if (!IS_MAC) return;
  ro = new ResizeObserver(syncGlass);
  // render() 整体替换 innerHTML：每次换新节点后重新观察，旧节点随 disconnect 释放
  const observe = () => {
    ro.disconnect();
    for (const el of document.querySelectorAll("[data-glass]")) ro.observe(el);
    syncGlass();
  };
  new MutationObserver(observe).observe(document.body, { childList: true, subtree: true });
  addEventListener("resize", syncGlass);
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
    last = "";
    syncGlass();
  });
  observe();
}
