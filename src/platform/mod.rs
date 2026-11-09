//! OS integration. Everything platform-specific lives behind `Platform`,
//! so macOS / Linux backends can be added later without touching the rest.

use crate::math::{Rect, Vec2};
use crate::world::{Taskbar, WinId, WinInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    MouseDown,
    MouseUp,
    RightClick,
    Quit,
}

pub struct MenuItem {
    /// 0 with no children means separator.
    pub id: u32,
    pub label: String,
    pub checked: bool,
    /// Non-empty makes this a submenu.
    pub children: Vec<MenuItem>,
}

impl MenuItem {
    pub fn new(id: u32, label: &str, checked: bool) -> Self {
        MenuItem { id, label: label.to_string(), checked, children: Vec::new() }
    }
    pub fn separator() -> Self {
        MenuItem { id: 0, label: String::new(), checked: false, children: Vec::new() }
    }
    pub fn submenu(label: &str, children: Vec<MenuItem>) -> Self {
        MenuItem { id: 0, label: label.to_string(), checked: false, children }
    }
}

pub trait Platform {
    fn poll_events(&mut self) -> Vec<Event>;
    /// Screen area not covered by the taskbar, in physical pixels.
    fn work_area(&self) -> Rect;
    /// The taskbar, if it's showing (not auto-hidden). Works with the stock
    /// Windows 11 taskbar and ExplorerPatcher's, on any screen edge.
    fn taskbar(&self) -> Option<Taskbar>;
    /// Visible top-level windows, top-most first.
    fn windows(&mut self) -> Vec<WinInfo>;
    /// Fresh rectangle of one window, or None if it is gone/minimized.
    fn window_rect(&self, id: WinId) -> Option<Rect>;
    fn cursor(&self) -> Vec2;
    fn dpi_scale(&self) -> f32;
    /// Show a premultiplied RGBA frame at screen position (x, y), with the
    /// parts under `occluders` cut out.
    fn present(&mut self, rgba: &[u8], width: u32, height: u32, x: i32, y: i32, occluders: &[Rect]);
    fn show_menu(&mut self, items: &[MenuItem]) -> Option<u32>;
    /// Open a colour picker. Returns the chosen RGB, or None if cancelled.
    fn pick_color(&mut self, initial: [u8; 3]) -> Option<[u8; 3]>;
    fn keep_on_top(&mut self);

    /// (name, screen rect) of each app button on the taskbar.
    fn taskbar_buttons(&mut self) -> Vec<(String, Rect)>;
    fn window_title(&self, id: WinId) -> String;
    /// (process id, lowercase exe file name) of the process owning a window.
    fn window_process(&self, id: WinId) -> Option<(u32, String)>;
    fn foreground(&self) -> Option<WinId>;
    /// A window's client area (no frame/title bar) in screen pixels.
    fn client_rect(&self, id: WinId) -> Option<Rect>;
    /// Grab what's on screen in `r` (the stickman itself is left out).
    fn capture(&mut self, r: Rect) -> Option<Image>;
    /// Left-click a window at a screen position, without moving your mouse.
    fn post_click(&self, id: WinId, at: Vec2);
    /// Everything UI Automation can see inside a window:
    /// (name, class, control type id, screen rect).
    fn ui_elements(&mut self, id: WinId) -> Vec<(String, String, i32, Rect)>;
}

/// Plain RGBA image (not premultiplied; screen captures are opaque).
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn px(&self, x: u32, y: u32) -> [u8; 3] {
        let i = ((y.min(self.h - 1) * self.w + x.min(self.w - 1)) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2]]
    }
}

#[cfg(windows)]
mod uia;
#[cfg(windows)]
mod win32;

#[cfg(windows)]
pub fn create() -> Result<Box<dyn Platform>, String> {
    win32::Win32Platform::new().map(|p| Box::new(p) as Box<dyn Platform>)
}

#[cfg(not(windows))]
pub fn create() -> Result<Box<dyn Platform>, String> {
    Err("only Windows is supported so far".into())
}
