//! UI Automation: the app buttons on the taskbar (stock Windows 11 taskbar
//! and ExplorerPatcher's Windows 10-style one), the tiles inside game
//! windows, and the boxes on web pages.
//!
//! Every UI Automation call is a slow round trip into another program, and
//! some windows have thousands of elements, so all of it runs on a
//! background thread: the stickman never waits for it.

use std::ffi::c_void;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Instant;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern, IUIAutomationTreeWalker,
    TreeScope_Descendants, UIA_BoundingRectanglePropertyId, UIA_ButtonControlTypeId, UIA_ControlTypePropertyId,
    UIA_DocumentControlTypeId, UIA_InvokePatternId,
};

use crate::math::{Rect, Vec2};

/// (name, class name, control type id, screen rect)
pub type Element = (String, String, i32, Rect);

/// A web page: the document's rect, and every box in it with its control type.
pub type PageBoxes = (Rect, Vec<(Rect, i32)>);

pub enum Request {
    /// Taskbar window handle.
    Taskbar(usize),
    /// Window handle to list the elements of.
    Elements(usize),
    /// Browser window handle: list the boxes on the page.
    Page(usize),
    /// Press the element whose name starts with this, in this window.
    /// If that can't be done, the platform falls back to a click at `at`.
    Invoke(usize, String, Vec2),
    /// The desktop's icon list window: list the icons.
    Icons(usize),
}

pub enum Reply {
    Taskbar(Vec<(String, Rect)>),
    Elements(usize, Vec<Element>, Instant),
    Page(usize, Option<PageBoxes>, Instant),
    InvokeFailed(usize, Vec2),
    Icons(Vec<(String, Rect)>),
}

const KINDS: usize = 4;

fn kind_of(r: &Request) -> Option<usize> {
    match r {
        Request::Taskbar(_) => Some(0),
        Request::Elements(_) => Some(1),
        Request::Page(_) => Some(2),
        Request::Icons(_) => Some(3),
        Request::Invoke(..) => None,
    }
}

/// UI Automation's "list item" control type (each desktop icon is one).
const LIST_ITEM: i32 = 50007;

/// The background thread that does the UI Automation work.
pub struct Worker {
    tx: Sender<Request>,
    rx: Receiver<Reply>,
    /// One refresh of each kind in flight at a time.
    pending: [bool; KINDS],
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
                        Request::Elements(h) => Reply::Elements(h, uia.elements(h as *mut c_void), Instant::now()),
                        Request::Page(h) => Reply::Page(h, uia.page(h as *mut c_void), Instant::now()),
                        Request::Invoke(h, prefix, at) => {
                            if uia.invoke(h as *mut c_void, &prefix) {
                                continue;
                            }
                            Reply::InvokeFailed(h, at)
                        }
                        Request::Icons(h) => Reply::Icons(
                            uia.elements(h as *mut c_void)
                                .into_iter()
                                .filter(|e| e.2 == LIST_ITEM && e.3.r > e.3.l && e.3.b > e.3.t)
                                .map(|e| (e.0, e.3))
                                .collect(),
                        ),
                    };
                    if replies.send(reply).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Worker { tx, rx, pending: [false; KINDS] })
    }

    /// Ask for something. Refreshes are skipped while the same kind is still
    /// being worked on; presses always go through.
    pub fn request(&mut self, r: Request) {
        match kind_of(&r) {
            Some(k) if self.pending[k] => {}
            Some(k) => self.pending[k] = self.tx.send(r).is_ok(),
            None => {
                let _ = self.tx.send(r);
            }
        }
    }

    /// A finished answer, if there is one.
    pub fn poll(&mut self) -> Option<Reply> {
        let r = self.rx.try_recv().ok()?;
        let k = match &r {
            Reply::Taskbar(_) => Some(0),
            Reply::Elements(..) => Some(1),
            Reply::Page(..) => Some(2),
            Reply::Icons(_) => Some(3),
            Reply::InvokeFailed(..) => None,
        };
        if let Some(k) = k {
            self.pending[k] = false;
        }
        Some(r)
    }
}

/// Windows 11: each app button has this class.
const WIN11_BUTTON: &str = "Taskbar.TaskListButtonAutomationPeer";
/// Windows 10 / ExplorerPatcher: app buttons live inside this list.
const WIN10_LIST: &str = "MSTaskListWClass";
/// Don't wander forever if the tree is huge.
const MAX_NODES: u32 = 800;
const MAX_DEPTH: u32 = 10;

fn rect(r: windows::Win32::Foundation::RECT) -> Rect {
    Rect { l: r.left as f32, t: r.top as f32, r: r.right as f32, b: r.bottom as f32 }
}

pub struct Uia {
    auto: IUIAutomation,
    walker: IUIAutomationTreeWalker,
    /// The page element last found in each browser window (searching for it
    /// is the slow part), and how many times it's been reused.
    docs: std::cell::RefCell<Vec<(usize, IUIAutomationElement, u32)>>,
}

impl Uia {
    pub fn new() -> Option<Self> {
        unsafe {
            // Fine if COM was already initialised on this thread.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let auto: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
            let walker = auto.ControlViewWalker().ok()?;
            Some(Uia { auto, walker, docs: std::cell::RefCell::new(Vec::new()) })
        }
    }

    /// The page element of a browser window: reused for a while, re-found
    /// now and then (switching tabs swaps it).
    unsafe fn document(&self, hwnd: *mut c_void) -> Option<(IUIAutomationElement, Rect)> {
        let key = hwnd as usize;
        {
            let mut docs = self.docs.borrow_mut();
            if let Some(entry) = docs.iter_mut().find(|(k, _, _)| *k == key) {
                entry.2 += 1;
                if entry.2 < 8 {
                    if let Ok(r) = entry.1.CurrentBoundingRectangle() {
                        let r = rect(r);
                        if r.r > r.l && r.b > r.t {
                            return Some((entry.1.clone(), r));
                        }
                    }
                }
            }
            docs.retain(|(k, _, _)| *k != key);
        }
        let mut doc: Option<(IUIAutomationElement, Rect)> = None;
        self.each(hwnd, 1500, |el, depth| {
            if depth > 12 {
                return true;
            }
            let is_doc = el.CurrentControlType().map(|t| t == UIA_DocumentControlTypeId).unwrap_or(false);
            if is_doc {
                if let Ok(r) = el.CurrentBoundingRectangle() {
                    let r = rect(r);
                    let area = (r.r - r.l) * (r.b - r.t);
                    if area > 0.0 && doc.as_ref().map_or(true, |(_, d)| area > (d.r - d.l) * (d.b - d.t)) {
                        doc = Some((el.clone(), r));
                    }
                }
            }
            true
        });
        if let Some((el, _)) = &doc {
            let mut docs = self.docs.borrow_mut();
            docs.push((key, el.clone(), 0));
            if docs.len() > 4 {
                docs.remove(0);
            }
        }
        doc
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

    /// Visit elements inside a window (depth-first), until `f` says stop.
    unsafe fn each(&self, hwnd: *mut c_void, max: usize, mut f: impl FnMut(&IUIAutomationElement, u32) -> bool) {
        if hwnd.is_null() {
            return;
        }
        let Ok(root) = self.auto.ElementFromHandle(HWND(hwnd)) else { return };
        let Ok(walker) = self.auto.RawViewWalker() else { return };
        let mut stack = vec![(root, 0u32)];
        let mut seen = 0;
        while let Some((el, depth)) = stack.pop() {
            seen += 1;
            if seen > max || depth > 14 {
                continue;
            }
            if !f(&el, depth) {
                return;
            }
            let mut child = walker.GetFirstChildElement(&el).ok();
            while let Some(c) = child {
                child = walker.GetNextSiblingElement(&c).ok();
                stack.push((c, depth + 1));
            }
        }
    }

    /// Every element inside a window: (name, class name, control type id,
    /// screen rect). Used to read game tiles and for diagnostics.
    pub fn elements(&self, hwnd: *mut c_void) -> Vec<Element> {
        let mut out = Vec::new();
        unsafe {
            self.each(hwnd, 2000, |el, _| {
                let name = el.CurrentName().map(|b| b.to_string()).unwrap_or_default();
                let class = el.CurrentClassName().map(|b| b.to_string()).unwrap_or_default();
                let kind = el.CurrentControlType().map(|t| t.0).unwrap_or(0);
                if let Ok(r) = el.CurrentBoundingRectangle() {
                    out.push((name, class, kind, rect(r)));
                }
                true
            });
        }
        out
    }

    /// Press (UI Automation "Invoke") the first element whose name starts
    /// with `prefix`. Same as clicking it, without touching your mouse.
    pub fn invoke(&self, hwnd: *mut c_void, prefix: &str) -> bool {
        let mut done = false;
        unsafe {
            self.each(hwnd, 2000, |el, _| {
                let name = el.CurrentName().map(|b| b.to_string()).unwrap_or_default();
                if !name.starts_with(prefix) {
                    return true;
                }
                if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) {
                    done = p.Invoke().is_ok();
                }
                false
            });
        }
        done
    }

    /// The boxes on the web page shown in a browser window, fetched in one
    /// go (much faster than asking element by element).
    pub fn page(&self, hwnd: *mut c_void) -> Option<PageBoxes> {
        unsafe {
            // The page itself (the biggest "document" in the window).
            let (doc, doc_rect) = self.document(hwnd)?;
            let cache = self.auto.CreateCacheRequest().ok()?;
            cache.AddProperty(UIA_BoundingRectanglePropertyId).ok()?;
            cache.AddProperty(UIA_ControlTypePropertyId).ok()?;
            let all = self.auto.CreateTrueCondition().ok()?;
            let found = doc.FindAllBuildCache(TreeScope_Descendants, &all, &cache).ok()?;
            let n = found.Length().ok()?.min(6000);
            let mut boxes = Vec::with_capacity(n as usize);
            for i in 0..n {
                let Ok(el) = found.GetElement(i) else { continue };
                let (Ok(r), Ok(t)) = (el.CachedBoundingRectangle(), el.CachedControlType()) else { continue };
                boxes.push((rect(r), t.0));
            }
            Some((doc_rect, boxes))
        }
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
                    let rect = rect(r);
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
