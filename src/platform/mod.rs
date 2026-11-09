//! OS integration. Everything platform-specific lives behind `Platform`,
//! so macOS / Linux backends can be added later without touching the rest.

use crate::math::{Rect, Vec2};
use crate::world::{WinId, WinInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    MouseDown,
    MouseUp,
    RightClick,
    Quit,
}

pub struct MenuItem {
    /// 0 means separator.
    pub id: u32,
    pub label: String,
    pub checked: bool,
}

impl MenuItem {
    pub fn new(id: u32, label: &str, checked: bool) -> Self {
        MenuItem { id, label: label.to_string(), checked }
    }
    pub fn separator() -> Self {
        MenuItem { id: 0, label: String::new(), checked: false }
    }
}

pub trait Platform {
    fn poll_events(&mut self) -> Vec<Event>;
    /// Screen area not covered by the taskbar, in physical pixels.
    fn work_area(&self) -> Rect;
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
}

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
