//! UI Automation: the app buttons on the taskbar (stock Windows 11 taskbar
//! and ExplorerPatcher's Windows 10-style one), the tiles inside game
//! windows, and the boxes on web pages.
//!
//! Every UI Automation call is a slow round trip into another program, and
//! some windows have thousands of elements, so all of it runs on a
//! background thread: the stickman never waits for it.

use std::ffi::c_void;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationElementArray, IUIAutomationInvokePattern,
    IUIAutomationTreeWalker, TreeScope_Descendants, UIA_BoundingRectanglePropertyId, UIA_ButtonControlTypeId,
    UIA_ControlTypePropertyId, UIA_DocumentControlTypeId, UIA_InvokePatternId,
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
    /// Follow the box at this rect (as of the last page read) in this
    /// window closely; None stops.
    Track(usize, Option<Rect>),
}

pub enum Reply {
    Taskbar(Vec<(String, Rect)>),
    Elements(usize, Vec<Element>, Instant),
    /// The page, when it was read, and where the followed box was then.
    Page(usize, Option<PageBoxes>, Instant, Option<(u32, f32)>),
    InvokeFailed(usize, Vec2),
    Icons(Vec<(String, Rect)>),
    /// The followed box moved: (window, which tracking, its top, when).
    Probe(usize, u32, f32, Instant),
    /// The followed box is gone.
    ProbeLost(usize),
}

/// A box being followed closely (polled every few milliseconds).
struct Probe {
    hwnd: usize,
    tracking: u32,
    el: IUIAutomationElement,
    top: f32,
}

/// The last page read in a window: the elements, and each box's index
/// into them.
struct LastPage {
    hwnd: usize,
    found: IUIAutomationElementArray,
    boxes: Vec<(Rect, i32)>,
}

/// How often followed boxes are checked.
const PROBE_EVERY: Duration = Duration::from_millis(15);

const KINDS: usize = 4;

fn kind_of(r: &Request) -> Option<usize> {
    match r {
        Request::Taskbar(_) => Some(0),
        Request::Elements(_) => Some(1),
        Request::Page(_) => Some(2),
        Request::Icons(_) => Some(3),
        Request::Invoke(..) | Request::Track(..) => None,
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
                let mut probes: Vec<Probe> = Vec::new();
                let mut last_pages: Vec<LastPage> = Vec::new();
                let mut tracking = 0u32;
                loop {
                    // Wait for work; while following boxes, only briefly.
                    let req = if probes.is_empty() {
                        match requests.recv() {
                            Ok(r) => Some(r),
                            Err(_) => break,
                        }
                    } else {
                        match requests.recv_timeout(PROBE_EVERY) {
                            Ok(r) => Some(r),
                            Err(RecvTimeoutError::Timeout) => None,
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    };
                    let reply = match req {
                        None => None,
                        Some(Request::Taskbar(h)) => Some(Reply::Taskbar(uia.taskbar_buttons(h as *mut c_void))),
                        Some(Request::Elements(h)) => {
                            Some(Reply::Elements(h, uia.elements(h as *mut c_void), Instant::now()))
                        }
                        Some(Request::Page(h)) => {
                            let page = uia.page(h as *mut c_void);
                            // Where the followed box is right now, to line
                            // this read up with the live tracking.
                            let probe = probes
                                .iter()
                                .find(|p| p.hwnd == h)
                                .and_then(|p| unsafe { p.el.CurrentBoundingRectangle() }.ok().map(|r| (p.tracking, rect(r).t)));
                            let taken = Instant::now();
                            let boxes = page.as_ref().map(|(doc, boxes, _)| (*doc, boxes.clone()));
                            last_pages.retain(|l| l.hwnd != h);
                            if let Some((_, boxes, found)) = page {
                                last_pages.push(LastPage { hwnd: h, found, boxes });
                                if last_pages.len() > 4 {
                                    last_pages.remove(0);
                                }
                            }
                            Some(Reply::Page(h, boxes, taken, probe))
                        }
                        Some(Request::Invoke(h, prefix, at)) => {
                            if uia.invoke(h as *mut c_void, &prefix) {
                                None
                            } else {
                                Some(Reply::InvokeFailed(h, at))
                            }
                        }
                        Some(Request::Icons(h)) => Some(Reply::Icons(uia.list_items(h as *mut c_void))),
                        Some(Request::Track(h, target)) => {
                            probes.retain(|p| p.hwnd != h);
                            let found = target.and_then(|r| {
                                let last = last_pages.iter().find(|l| l.hwnd == h)?;
                                let el = closest_box(&last.boxes, r).and_then(|i| unsafe { last.found.GetElement(i as i32) }.ok())?;
                                let top = rect(unsafe { el.CurrentBoundingRectangle() }.ok()?).t;
                                Some((el, top))
                            });
                            match found {
                                Some((el, top)) => {
                                    tracking = tracking.wrapping_add(1);
                                    probes.push(Probe { hwnd: h, tracking, el, top });
                                    Some(Reply::Probe(h, tracking, top, Instant::now()))
                                }
                                None => Some(Reply::ProbeLost(h)),
                            }
                        }
                    };
                    if let Some(reply) = reply {
                        if replies.send(reply).is_err() {
                            break;
                        }
                    }
                    // Check the followed boxes: one cheap call each.
                    let mut lost = Vec::new();
                    for p in probes.iter_mut() {
                        match unsafe { p.el.CurrentBoundingRectangle() } {
                            Ok(r) => {
                                let top = rect(r).t;
                                if (top - p.top).abs() >= 0.5 {
                                    p.top = top;
                                    let _ = replies.send(Reply::Probe(p.hwnd, p.tracking, top, Instant::now()));
                                }
                            }
                            Err(_) => lost.push(p.hwnd),
                        }
                    }
                    for h in lost {
                        probes.retain(|p| p.hwnd != h);
                        let _ = replies.send(Reply::ProbeLost(h));
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
            Reply::InvokeFailed(..) | Reply::Probe(..) | Reply::ProbeLost(_) => None,
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

/// The box (index) that best matches a ledge: same top, most overlap.
fn closest_box(boxes: &[(Rect, i32)], target: Rect) -> Option<usize> {
    boxes
        .iter()
        .enumerate()
        .filter(|(_, (b, _))| (b.t - target.t).abs() < 6.0 && b.r > target.l && b.l < target.r)
        .min_by(|(_, (a, _)), (_, (b, _))| {
            let score = |r: &Rect| (r.t - target.t).abs() * 4.0 + (r.l - target.l).abs() + (r.r - target.r).abs();
            score(a).partial_cmp(&score(b)).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
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

    /// The items of a list (the desktop's icons), in their real order, so
    /// item N here is item N to the list itself (needed to move icons).
    pub fn list_items(&self, hwnd: *mut c_void) -> Vec<(String, Rect)> {
        let mut out = Vec::new();
        if hwnd.is_null() {
            return out;
        }
        unsafe {
            let Ok(root) = self.auto.ElementFromHandle(HWND(hwnd)) else { return out };
            let Ok(walker) = self.auto.RawViewWalker() else { return out };
            let mut child = walker.GetFirstChildElement(&root).ok();
            while let Some(c) = child {
                if c.CurrentControlType().map(|t| t.0 == LIST_ITEM).unwrap_or(false) {
                    let name = c.CurrentName().map(|b| b.to_string()).unwrap_or_default();
                    let r = c.CurrentBoundingRectangle().map(rect).unwrap_or(Rect { l: 0.0, t: 0.0, r: 0.0, b: 0.0 });
                    out.push((name, r));
                }
                if out.len() > 1000 {
                    break;
                }
                child = walker.GetNextSiblingElement(&c).ok();
            }
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
    /// go (much faster than asking element by element). Also returns the
    /// elements themselves, so one can be followed closely later.
    pub fn page(&self, hwnd: *mut c_void) -> Option<(Rect, Vec<(Rect, i32)>, IUIAutomationElementArray)> {
        unsafe {
            // The page itself (the biggest "document" in the window). Apps
            // without one (media players, games...) use the whole window.
            let (doc, doc_rect) = match self.document(hwnd) {
                Some(d) => d,
                None => {
                    let root = self.auto.ElementFromHandle(HWND(hwnd)).ok()?;
                    let r = rect(root.CurrentBoundingRectangle().ok()?);
                    (root, r)
                }
            };
            let cache = self.auto.CreateCacheRequest().ok()?;
            cache.AddProperty(UIA_BoundingRectanglePropertyId).ok()?;
            cache.AddProperty(UIA_ControlTypePropertyId).ok()?;
            let all = self.auto.CreateTrueCondition().ok()?;
            let found = doc.FindAllBuildCache(TreeScope_Descendants, &all, &cache).ok()?;
            let n = found.Length().ok()?.min(6000);
            // Index i here is element i in `found` (unreadable ones get an
            // empty box, so the numbering stays right).
            let mut boxes = Vec::with_capacity(n as usize);
            for i in 0..n {
                let b = found
                    .GetElement(i)
                    .ok()
                    .and_then(|el| Some((rect(el.CachedBoundingRectangle().ok()?), el.CachedControlType().ok()?.0)));
                boxes.push(b.unwrap_or((Rect { l: 0.0, t: 0.0, r: 0.0, b: 0.0 }, 0)));
            }
            Some((doc_rect, boxes, found))
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
