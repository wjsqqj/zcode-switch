//! macOS 原生玻璃：WKWebView 把整窗画进一层，原生材质只能放在它**下方**。
//!
//! 层次（自下而上）：底色层（CAShapeLayer，按 desktop 面板镂空）→ 玻璃容器 → WKWebView。
//! 前端测量带 `data-glass` 的元素后调用 `glass_sync` 提交整帧：按 id 复用面板、移除未出现的面板。
//! macOS 26+ 用 NSGlassEffectView（Liquid Glass），旧系统降级为 NSVisualEffectView；其它平台为空操作。

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
// 非 macOS 目标只反序列化不读取字段（glass_sync 直接跳过），避免 dead_code 告警
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct GlassPane {
    pub id: String,
    /// WebView 内 CSS 像素（左上原点，等于 AppKit points）
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default)]
    pub radius: f64,
    /// "desktop"：底色层在此镂空，玻璃取样桌面与后方窗口；其它：取样窗口底色
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, Deserialize)]
// 非 macOS 目标只反序列化不收字段（glass_sync 直接跳过），避免 dead_code 告警
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct GlassFrame {
    pub panes: Vec<GlassPane>,
    /// 底色 `#rrggbb`，与前端 `--glass-base` 一致
    pub base: String,
    pub dark: bool,
}

/// 首帧前的底色，与 mac.css 的 `--glass-base` 保持一致
#[cfg(target_os = "macos")]
const BASE_DARK: &str = "#1c1c1f";
#[cfg(target_os = "macos")]
const BASE_LIGHT: &str = "#efeff2";

#[tauri::command]
pub fn glass_sync(window: tauri::WebviewWindow, frame: GlassFrame) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let label = window.label().to_string();
        window
            .with_webview(move |wv| mac::commit(&label, wv.inner(), &frame))
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (window, frame);
    Ok(())
}

/// 窗口创建后立刻铺底色：透明窗口在前端首帧之前不露出桌面
pub fn install(window: &tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        let dark = !matches!(window.theme(), Ok(tauri::Theme::Light));
        let frame = GlassFrame {
            panes: Vec::new(),
            base: (if dark { BASE_DARK } else { BASE_LIGHT }).to_string(),
            dark,
        };
        let label = window.label().to_string();
        let _ = window.with_webview(move |wv| mac::commit(&label, wv.inner(), &frame));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// 窗口销毁后释放注册项（原生视图随窗口一起释放）
pub fn forget(app: &tauri::AppHandle, label: &str) {
    #[cfg(target_os = "macos")]
    {
        let label = label.to_string();
        let _ = app.run_on_main_thread(move || mac::forget(&label));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, label);
}

#[cfg(target_os = "macos")]
mod mac {
    use super::GlassFrame;
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};
    use std::ffi::c_void;
    use std::ptr;

    use objc2::rc::Retained;
    use objc2::runtime::AnyClass;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSAutoresizingMaskOptions, NSGlassEffectContainerView, NSGlassEffectView,
        NSGlassEffectViewStyle, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
        NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
    };
    use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{CGColor, CGMutablePath};
    use objc2_quartz_core::{kCAFillRuleEvenOdd, CAShapeLayer, CATransaction};

    /// 面板离窗口边缘多近算“贴边”，决定 live resize 期间的 autoresizing
    const EDGE: f64 = 24.0;

    /// 按创建时的后端保存：旧系统上不能再对 NSGlassEffectView 做类查找
    enum Pane {
        Liquid(Retained<NSGlassEffectView>),
        Vibrancy(Retained<NSVisualEffectView>),
    }

    impl Pane {
        fn new(liquid: bool, mtm: MainThreadMarker) -> Self {
            let zero = CGRect::new(CGPoint::new(0., 0.), CGSize::new(0., 0.));
            if liquid {
                let glass = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), zero);
                glass.setStyle(NSGlassEffectViewStyle::Regular);
                Pane::Liquid(glass)
            } else {
                let effect =
                    NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), zero);
                effect.setState(NSVisualEffectState::FollowsWindowActiveState);
                effect.setWantsLayer(true);
                Pane::Vibrancy(effect)
            }
        }

        fn view(&self) -> &NSView {
            match self {
                Pane::Liquid(v) => v,
                Pane::Vibrancy(v) => v,
            }
        }

        fn set_radius(&self, radius: CGFloat) {
            match self {
                Pane::Liquid(v) => v.setCornerRadius(radius),
                Pane::Vibrancy(v) => {
                    if let Some(layer) = v.layer() {
                        layer.setCornerRadius(radius);
                        layer.setMasksToBounds(true);
                    }
                }
            }
        }

        /// 只有传统材质区分取样来源；Liquid Glass 取样视图层次中它后方的内容
        fn set_source(&self, desktop: bool) {
            if let Pane::Vibrancy(v) = self {
                let (material, blending) = if desktop {
                    (NSVisualEffectMaterial::Sidebar, NSVisualEffectBlendingMode::BehindWindow)
                } else {
                    (NSVisualEffectMaterial::HeaderView, NSVisualEffectBlendingMode::WithinWindow)
                };
                v.setMaterial(material);
                v.setBlendingMode(blending);
            }
        }
    }

    struct WinGlass {
        webview: Retained<NSView>,
        base_view: Retained<NSView>,
        base_layer: Retained<CAShapeLayer>,
        /// 外观设在这里（Liquid 时是容器），面板继承
        outer: Retained<NSView>,
        /// 面板父视图：Liquid 时是容器的 contentView
        root: Retained<NSView>,
        liquid: bool,
        panes: HashMap<String, (Pane, bool)>,
        base: String,
        dark: Option<bool>,
    }

    thread_local! {
        static WINDOWS: RefCell<HashMap<String, WinGlass>> = RefCell::new(HashMap::new());
    }

    pub fn commit(label: &str, webview: *mut c_void, frame: &GlassFrame) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        // SAFETY: wry 在 macOS 上 PlatformWebview::inner() 返回窗口持有的 WKWebView（NSView 子类）；
        // with_webview 的回调在主线程执行，期间该视图存活；retain 后由本模块按窗口生命周期持有。
        let Some(webview) = (unsafe { Retained::retain(webview.cast::<NSView>()) }) else { return };
        WINDOWS.with(|cell| {
            let mut map = cell.borrow_mut();
            if map.get(label).is_some_and(|g| !ptr::eq(&*g.webview, &*webview)) {
                if let Some(old) = map.remove(label) {
                    old.detach();
                }
            }
            if !map.contains_key(label) {
                let Some(g) = WinGlass::attach(webview, mtm) else { return };
                map.insert(label.to_string(), g);
            }
            if let Some(g) = map.get_mut(label) {
                g.apply(frame, mtm);
            }
        });
    }

    pub fn forget(label: &str) {
        WINDOWS.with(|cell| {
            if let Some(g) = cell.borrow_mut().remove(label) {
                g.detach();
            }
        });
    }

    fn fill_mask() -> NSAutoresizingMaskOptions {
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable
    }

    fn parse_hex(s: &str) -> Option<(f64, f64, f64)> {
        let h = s.trim().trim_start_matches('#');
        if h.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        let c = |shift: u32| ((v >> shift) & 0xff) as f64 / 255.0;
        Some((c(16), c(8), c(0)))
    }

    impl WinGlass {
        fn attach(webview: Retained<NSView>, mtm: MainThreadMarker) -> Option<Self> {
            // SAFETY: 主线程读取视图层次；wry 把 WKWebView 放在窗口 contentView（WryWebViewParent）里
            let host = unsafe { webview.superview() }?;
            let bounds = host.bounds();

            // 底色层：layer-hosting 视图，先 setLayer 再 setWantsLayer，由本模块全权管理图层内容
            let base_layer = CAShapeLayer::new();
            // SAFETY: kCAFillRuleEvenOdd 是 QuartzCore 导出的常量字符串
            base_layer.setFillRule(unsafe { kCAFillRuleEvenOdd });
            let base_view = NSView::initWithFrame(NSView::alloc(mtm), bounds);
            base_view.setLayer(Some(&base_layer));
            base_view.setWantsLayer(true);
            base_view.setAutoresizingMask(fill_mask());
            host.addSubview_positioned_relativeTo(
                &base_view,
                NSWindowOrderingMode::Below,
                Some(&webview),
            );

            let liquid = AnyClass::get(c"NSGlassEffectView").is_some()
                && AnyClass::get(c"NSGlassEffectContainerView").is_some();
            let (outer, root) = if liquid {
                let container = NSGlassEffectContainerView::initWithFrame(
                    NSGlassEffectContainerView::alloc(mtm),
                    bounds,
                );
                container.setAutoresizingMask(fill_mask());
                let content = NSView::initWithFrame(NSView::alloc(mtm), bounds);
                content.setAutoresizingMask(fill_mask());
                container.setContentView(Some(&content));
                let outer: Retained<NSView> = Retained::into_super(container);
                (outer, content)
            } else {
                let root = NSView::initWithFrame(NSView::alloc(mtm), bounds);
                root.setAutoresizingMask(fill_mask());
                (root.clone(), root)
            };
            // 紧贴在 WKWebView 下方，因而位于底色层之上
            host.addSubview_positioned_relativeTo(&outer, NSWindowOrderingMode::Below, Some(&webview));

            Some(Self {
                webview,
                base_view,
                base_layer,
                outer,
                root,
                liquid,
                panes: HashMap::new(),
                base: String::new(),
                dark: None,
            })
        }

        fn detach(self) {
            self.outer.removeFromSuperview();
            self.base_view.removeFromSuperview();
        }

        fn apply(&mut self, f: &GlassFrame, mtm: MainThreadMarker) {
            CATransaction::begin();
            CATransaction::setDisableActions(true);

            if self.dark != Some(f.dark) {
                // SAFETY: AppKit 导出的外观名常量
                let name = unsafe { if f.dark { NSAppearanceNameDarkAqua } else { NSAppearanceNameAqua } };
                let appearance = NSAppearance::appearanceNamed(name);
                self.outer.setAppearance(appearance.as_deref());
                self.dark = Some(f.dark);
            }
            if self.base != f.base {
                if let Some((r, g, b)) = parse_hex(&f.base) {
                    let color = CGColor::new_srgb(r, g, b, 1.0);
                    self.base_layer.setFillColor(Some(&color));
                }
                self.base = f.base.clone();
            }

            let wv_bounds = self.webview.bounds();
            let (vw, vh) = (wv_bounds.size.width, wv_bounds.size.height);
            let flipped = self.webview.isFlipped();

            // 外框取得足够大：live resize 期间、前端重新提交之前，新露出的区域也有底色
            let path = CGMutablePath::new();
            let outer_rect = CGRect::new(CGPoint::new(-8000., -8000.), CGSize::new(24000., 24000.));
            // SAFETY: transform 传空指针表示不变换
            unsafe { CGMutablePath::add_rect(Some(&path), ptr::null(), outer_rect) };

            let mut seen = HashSet::new();
            for p in &f.panes {
                if p.w <= 0. || p.h <= 0. {
                    continue;
                }
                seen.insert(p.id.clone());
                let y = if flipped { p.y } else { vh - p.y - p.h };
                let in_webview = CGRect::new(CGPoint::new(p.x, y), CGSize::new(p.w, p.h));
                let radius = p.radius.max(0.).min(p.w / 2.).min(p.h / 2.);
                let desktop = p.source == "desktop";

                let liquid = self.liquid;
                let root = &self.root;
                let entry = self.panes.entry(p.id.clone()).or_insert_with(|| {
                    let pane = Pane::new(liquid, mtm);
                    pane.set_source(desktop);
                    root.addSubview(pane.view());
                    (pane, desktop)
                });
                if entry.1 != desktop {
                    entry.0.set_source(desktop);
                    entry.1 = desktop;
                }
                let view = entry.0.view();
                view.setFrame(self.webview.convertRect_toView(in_webview, Some(&self.root)));
                entry.0.set_radius(radius);
                view.setAutoresizingMask(edge_mask(p.x, p.y, p.w, p.h, vw, vh));

                if desktop {
                    let hole = self.webview.convertRect_toView(in_webview, Some(&self.base_view));
                    // SAFETY: 同上，transform 为空
                    unsafe {
                        CGMutablePath::add_rounded_rect(Some(&path), ptr::null(), hole, radius, radius)
                    };
                }
            }
            self.panes.retain(|id, (pane, _)| {
                let keep = seen.contains(id);
                if !keep {
                    pane.view().removeFromSuperview();
                }
                keep
            });

            self.base_layer.setPath(Some(&path));
            CATransaction::commit();
        }
    }

    /// 按面板贴近的边设置 autoresizing（父视图非 flipped），live resize 时在下一次提交前先贴住边缘
    fn edge_mask(x: f64, y: f64, w: f64, h: f64, vw: f64, vh: f64) -> NSAutoresizingMaskOptions {
        let (left, right) = (x <= EDGE, vw - (x + w) <= EDGE);
        let (top, bottom) = (y <= EDGE, vh - (y + h) <= EDGE);
        let horizontal = match (left, right) {
            (true, true) => NSAutoresizingMaskOptions::ViewWidthSizable,
            (false, true) => NSAutoresizingMaskOptions::ViewMinXMargin,
            _ => NSAutoresizingMaskOptions::ViewMaxXMargin,
        };
        // 网页布局默认顶部锚定；只贴底边的面板才保持底部距离
        let vertical = match (top, bottom) {
            (true, true) => NSAutoresizingMaskOptions::ViewHeightSizable,
            (false, true) => NSAutoresizingMaskOptions::ViewMaxYMargin,
            _ => NSAutoresizingMaskOptions::ViewMinYMargin,
        };
        horizontal | vertical
    }
}
