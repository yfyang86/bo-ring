//! Menu-bar icon for the daemon: quick ring switcher (follow app / default /
//! pin a profile), Open Ring, Settings…, Show Log.
//!
//! Runs the NSApplication event loop on the main thread (accessory policy, so
//! no Dock icon); the daemon's own event loop runs on a worker thread.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{declare_class, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu,
    NSMenuDelegate, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSString};

use crate::config::ring_mode::{self, RingMode};
use crate::config::Config;

thread_local! {
    /// Mode for each switcher item, indexed by the item's tag.
    static MODES: RefCell<Vec<RingMode>> = const { RefCell::new(Vec::new()) };
    static STATUS_ITEM: RefCell<Option<Retained<NSStatusItem>>> = const { RefCell::new(None) };
    static TARGET: RefCell<Option<Retained<MenuTarget>>> = const { RefCell::new(None) };
    /// Last mode shown, so the 1 s poll only touches AppKit on a change.
    static SHOWN: RefCell<Option<RingMode>> = const { RefCell::new(None) };
}

declare_class!(
    struct MenuTarget;

    unsafe impl ClassType for MenuTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "BoRingMenuTarget";
    }

    impl DeclaredClass for MenuTarget {}

    unsafe impl MenuTarget {
        #[method(selectMode:)]
        fn select_mode(&self, sender: &NSMenuItem) {
            let tag = unsafe { sender.tag() } as usize;
            if let Some(mode) = MODES.with(|m| m.borrow().get(tag).cloned()) {
                match ring_mode::save(&mode) {
                    Ok(()) => println!("🔁 Ring mode → {}", mode.as_config_str()),
                    Err(e) => eprintln!("⚠️ Could not save ring mode: {e}"),
                }
                refresh_icon(&mode);
            }
        }

        /// Timer tick: pick up changes made with `bo-ring --ring-mode`.
        #[method(refreshIcon:)]
        fn refresh_icon_tick(&self, _sender: Option<&AnyObject>) {
            let mode = ring_mode::load();
            if SHOWN.with(|s| s.borrow().as_ref() != Some(&mode)) {
                refresh_icon(&mode);
            }
        }

        #[method(openRing:)]
        fn open_ring(&self, _sender: Option<&AnyObject>) {
            crate::toggle_ring_menu();
        }

        #[method(openSettings:)]
        fn open_settings(&self, _sender: Option<&AnyObject>) {
            let exe = std::env::current_exe().unwrap_or_else(|_| "bo-ring".into());
            let _ = std::process::Command::new(exe).spawn();
        }

        #[method(showLog:)]
        fn show_log(&self, _sender: Option<&AnyObject>) {
            let log = crate::platform::autostart::log_dir().join("daemon.log");
            let _ = std::process::Command::new("open").arg("-a").arg("Console").arg(log).spawn();
        }
    }

    unsafe impl NSObjectProtocol for MenuTarget {}

    unsafe impl NSMenuDelegate for MenuTarget {
        #[method(menuNeedsUpdate:)]
        fn menu_needs_update(&self, menu: &NSMenu) {
            rebuild(menu, self);
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(());
        unsafe { msg_send_id![super(this), init] }
    }
}

fn item(mtm: MainThreadMarker, title: &str, action: Option<objc2::runtime::Sel>, target: &MenuTarget) -> Retained<NSMenuItem> {
    let it = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &NSString::from_str(title), action, &NSString::from_str(""))
    };
    if action.is_some() {
        unsafe { it.setTarget(Some(target)) };
    }
    it
}

fn header(mtm: MainThreadMarker, title: &str, target: &MenuTarget) -> Retained<NSMenuItem> {
    let it = item(mtm, title, None, target);
    unsafe { it.setEnabled(false) };
    it
}

/// Rebuild the menu each time it opens, so it reflects the current mode file
/// and the ring profiles on disk.
fn rebuild(menu: &NSMenu, target: &MenuTarget) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let config = Config::load_or_default();
    let lang = crate::utils::i18n::resolve_language_code(&config.general.language);
    let current = ring_mode::load();
    refresh_icon(&current);

    unsafe { menu.removeAllItems() };
    menu.addItem(&header(mtm, "Bo-Ring", target));
    menu.addItem(&item(mtm, "Open Ring", Some(sel!(openRing:)), target));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&header(mtm, "Ring to show", target));

    let mut modes = vec![RingMode::Auto, RingMode::Default];
    let mut titles = vec!["Follow Frontmost App".to_string(), "Always Default Ring".to_string()];
    for p in &config.app_profiles {
        modes.push(RingMode::Profile(p.app_id.clone()));
        titles.push(format!("Always: {}", p.label_lang(&lang)));
    }
    for (tag, (mode, title)) in modes.iter().zip(&titles).enumerate() {
        let it = item(mtm, title, Some(sel!(selectMode:)), target);
        unsafe {
            it.setTag(tag as isize);
            it.setState(if *mode == current { NSControlStateValueOn } else { NSControlStateValueOff });
            it.setIndentationLevel(1);
        }
        menu.addItem(&it);
    }
    MODES.with(|m| *m.borrow_mut() = modes);

    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&item(mtm, "Settings…", Some(sel!(openSettings:)), target));
    menu.addItem(&item(mtm, "Show Log", Some(sel!(showLog:)), target));
}

/// Hollow ring when following the app, filled when a ring is pinned.
fn refresh_icon(mode: &RingMode) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let symbol = match mode {
        RingMode::Auto => "circle.circle",
        _ => "circle.circle.fill",
    };
    SHOWN.with(|s| *s.borrow_mut() = Some(mode.clone()));
    STATUS_ITEM.with(|s| {
        if let Some(status) = s.borrow().as_ref() {
            if let Some(button) = unsafe { status.button(mtm) } {
                let img = unsafe {
                    NSImage::imageWithSystemSymbolName_accessibilityDescription(
                        &NSString::from_str(symbol),
                        Some(&NSString::from_str("Bo-Ring")),
                    )
                };
                if let Some(img) = img {
                    unsafe { img.setTemplate(true) };
                    unsafe { button.setImage(Some(&img)) };
                }
                let profiles = Config::load_or_default().app_profiles;
                let tip = format!("Bo-Ring — {}", ring_mode::describe(mode, &profiles));
                unsafe { button.setToolTip(Some(&NSString::from_str(&tip))) };
                // Short label when a ring is pinned, so the state is visible at a glance.
                let label = match mode {
                    RingMode::Auto => String::new(),
                    RingMode::Default => " Default".to_string(),
                    RingMode::Profile(id) => {
                        let l = profiles.iter().find(|p| &p.app_id == id).map(|p| p.label.clone()).unwrap_or_default();
                        let short: String = l.split(" (").next().unwrap_or(&l).chars().take(18).collect();
                        format!(" {short}")
                    }
                };
                unsafe { button.setTitle(&NSString::from_str(&label)) };
            }
        }
    });
}

/// Install the status item and run the AppKit event loop. Never returns.
/// Must be called on the main thread.
pub fn run() -> ! {
    let mtm = MainThreadMarker::new().expect("menu bar must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let target = MenuTarget::new(mtm);
    let status = unsafe { NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength) };
    let menu = NSMenu::new(mtm);
    unsafe { menu.setDelegate(Some(ProtocolObject::from_ref(&*target))) };
    unsafe { status.setMenu(Some(&menu)) };
    STATUS_ITEM.with(|s| *s.borrow_mut() = Some(status));
    TARGET.with(|t| *t.borrow_mut() = Some(target));
    refresh_icon(&ring_mode::load());
    TARGET.with(|t| {
        if let Some(target) = t.borrow().as_ref() {
            let _: Retained<AnyObject> = unsafe {
                msg_send_id![
                    objc2::class!(NSTimer),
                    scheduledTimerWithTimeInterval: 1.0f64,
                    target: &**target,
                    selector: sel!(refreshIcon:),
                    userInfo: None::<&AnyObject>,
                    repeats: true
                ]
            };
        }
    });
    println!("📍 Menu-bar icon installed (ring switcher)");

    unsafe { app.run() };
    std::process::exit(0)
}
