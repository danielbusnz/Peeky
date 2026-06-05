//! macOS menu bar icon. Shows the Aegis icon in the top-right menu bar
//! with a "Quit Aegis" option. Native NSStatusBar integration via tray-icon.

#[cfg(target_os = "macos")]
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};

#[cfg(target_os = "macos")]
static QUIT_ID: std::sync::OnceLock<tray_icon::menu::MenuId> = std::sync::OnceLock::new();

#[cfg(target_os = "macos")]
const ICON_PNG: &[u8] = include_bytes!("../assets/icon-menubar.png");

/// Initialize the menu bar icon. Call once from main thread before event loop.
#[cfg(target_os = "macos")]
pub fn init() -> TrayIcon {
    let quit_item = MenuItem::new("Quit Aegis", true, None);
    let _ = QUIT_ID.set(quit_item.id().clone());

    let menu = Menu::new();
    menu.append(&quit_item).expect("menu append");

    // Load the icon from embedded PNG
    let icon = load_icon();

    TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .with_tooltip("Aegis")
        .build()
        .expect("tray icon build")
}

#[cfg(target_os = "macos")]
fn load_icon() -> Icon {
    let img = image::load_from_memory(ICON_PNG).expect("load icon png");
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), w, h).expect("icon from rgba")
}

/// Poll for menu events. Call from event loop. Returns true if quit was clicked.
#[cfg(target_os = "macos")]
pub fn poll() -> bool {
    if let Ok(event) = MenuEvent::receiver().try_recv() {
        if let Some(quit_id) = QUIT_ID.get() {
            if &event.id == quit_id {
                return true;
            }
        }
    }
    false
}

// No-op stubs for non-macOS platforms
#[cfg(not(target_os = "macos"))]
pub struct TrayStub;

#[cfg(not(target_os = "macos"))]
pub fn init() -> TrayStub {
    TrayStub
}

#[cfg(not(target_os = "macos"))]
pub fn poll() -> bool {
    false
}
