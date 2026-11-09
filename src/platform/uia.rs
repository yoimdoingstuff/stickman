//! UI Automation: finds the app buttons on the taskbar (works for the stock
//! Windows 11 taskbar and ExplorerPatcher's Windows 10-style one) and the
//! tiles inside game windows.
//!
//! Every UI Automation call is a slow round trip into another program, and
//! some windows have thousands of elements, so all of it runs on a
//! background thread: the stickman never waits for it.

use std::ffi::c_void;
use std::sync::mpsc::{channel, Receiver, Sender};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTreeWalker, UIA_ButtonControlTypeId,
};

use crate::math::Rect;

pub type Element = (String, String, i32, Rect);

pub enum Request {
    /// Taskbar window handle.
    Taskbar(usize),
    /// Window handle to list the elements of.
    Elements(usize),
}

pub enum Reply {
    Taskbar(Vec<(String, Rect)>),
    Elements(usize, Vec<Element>),
}

/// The background thread that does the UI Automation work.
pub struct Worker {
    tx: Sender<Request>,
    rx: Receiver<Reply>,
    busy: bool,
}

impl Worker {
    pub fn spawn() -> Option<Self> {
        let (tx, requests) = channel::<Request>();
        let (replies, rx) = channel::<Reply>();
        std::thread::Builder::new()
            .name("ui-automation".into())
            .spawn(move || {
                let Some(uia) = Uia::new() else { return };
                while let Ok(req) = requests.recv() {
                    let reply = match req {
                        Request::Taskbar(h) => Reply::Taskbar(uia.taskbar_buttons(h as *mut c_void)),
                        Request::Elements(h) => Reply::Elements(h, uia.elements(h as *mut c_void)),
                    };
                    if replies.send(reply).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Worker { tx, rx, busy: false })
    }

    /// Ask for something, unless it's still working on the last request.
    pub fn request(&mut self, r: Request) {
        if !self.busy {
            self.busy = self.tx.send(r).is_ok();
        }
    }

    /// A finished answer, if there is one.
    pub fn poll(&mut self) -> Option<Reply> {
        let r = self.rx.try_recv().ok();
        if r.is_some() {
            self.busy = false;
        }
        r
    }
}

/// Windows 11: each app button has this class.
const WIN11_BUTTON: &str = "Taskbar.TaskListButtonAutomationPeer";
/// Windows 10 / ExplorerPatcher: app buttons live inside this list.
const WIN10_LIST: &str = "MSTaskListWClass";
/// Don't wander forever if the tree is huge.
const MAX_NODES: u32 = 800;
const MAX_DEPTH: u32 = 10;

pub struct Uia {
    auto: IUIAutomation,
    walker: IUIAutomationTreeWalker,
}

impl Uia {
    pub fn new() -> Option<Self> {
        unsafe {
            // Fine if COM was already initialised on this thread.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let auto: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
            let walker = auto.ControlViewWalker().ok()?;
            Some(Uia { auto, walker })
        }
    }

    /// (button name, screen rectangle) of every app button on the taskbar.
    pub fn taskbar_buttons(&self, tray: *mut c_void) -> Vec<(String, Rect)> {
        let mut out = Vec::new();
        if tray.is_null() {
            return out;
        }
        unsafe {
            let Ok(root) = self.auto.ElementFromHandle(HWND(tray)) else { return out };
            let mut budget = 0;
            self.walk(&root, "", 0, &mut out, &mut budget);
        }
        out
    }

    /// Every element inside a window: (name, class name, control type id,
    /// screen rect). Used to find game tiles and for diagnostics.
    pub fn elements(&self, hwnd: *mut c_void) -> Vec<Element> {
        let mut out = Vec::new();
        if hwnd.is_null() {
            return out;
        }
        unsafe {
            let Ok(root) = self.auto.ElementFromHandle(HWND(hwnd)) else { return out };
            let mut stack = vec![(root, 0u32)];
            let walker = match self.auto.RawViewWalker() {
                Ok(w) => w,
                Err(_) => return out,
            };
            while let Some((el, depth)) = stack.pop() {
                if out.len() > 2000 || depth > 12 {
                    continue;
                }
                let name = el.CurrentName().map(|b| b.to_string()).unwrap_or_default();
                let class = el.CurrentClassName().map(|b| b.to_string()).unwrap_or_default();
                let kind = el.CurrentControlType().map(|t| t.0).unwrap_or(0);
                if let Ok(r) = el.CurrentBoundingRectangle() {
                    out.push((name, class, kind, Rect { l: r.left as f32, t: r.top as f32, r: r.right as f32, b: r.bottom as f32 }));
                }
                let mut child = walker.GetFirstChildElement(&el).ok();
                while let Some(c) = child {
                    child = walker.GetNextSiblingElement(&c).ok();
                    stack.push((c, depth + 1));
                }
            }
        }
        out
    }

    unsafe fn walk(
        &self,
        el: &IUIAutomationElement,
        parent_class: &str,
        depth: u32,
        out: &mut Vec<(String, Rect)>,
        budget: &mut u32,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        let mut child = self.walker.GetFirstChildElement(el).ok();
        while let Some(c) = child {
            *budget += 1;
            if *budget > MAX_NODES {
                return;
            }
            let class = c.CurrentClassName().map(|b| b.to_string()).unwrap_or_default();
            let is_button = c.CurrentControlType().map(|t| t == UIA_ButtonControlTypeId).unwrap_or(false);
            if is_button && (class == WIN11_BUTTON || parent_class == WIN10_LIST) {
                if let (Ok(name), Ok(r)) = (c.CurrentName(), c.CurrentBoundingRectangle()) {
                    let rect = Rect { l: r.left as f32, t: r.top as f32, r: r.right as f32, b: r.bottom as f32 };
                    if rect.r > rect.l && rect.b > rect.t {
                        out.push((name.to_string(), rect));
                    }
                }
            } else {
                self.walk(&c, &class, depth + 1, out, budget);
            }
            child = self.walker.GetNextSiblingElement(&c).ok();
        }
    }
}
