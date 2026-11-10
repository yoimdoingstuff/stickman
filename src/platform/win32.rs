//! Windows backend: one small per-pixel-alpha layered window per stickman
//! that follows it around, a hidden window for the tray icon and menus,
//! plus window enumeration for z-order and surfaces.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::time::Instant;

use windows_sys::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, ClientToScreen, CreateBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject,
    EnumDisplayMonitors, GetDC, GetMonitorInfoW, GetStockObject, ReleaseDC, ScreenToClient, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC, HMONITOR,
    MONITORINFO, SRCCOPY,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Controls::Dialogs::{ChooseColorW, CC_ANYCOLOR, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW};
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForSystem, GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::Shell::{
    SHAppBarMessage, Shell_NotifyIconW, ABE_BOTTOM, ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_GETTASKBARPOS, APPBARDATA,
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW,
    DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, EnumWindows, FindWindowExW, FindWindowW,
    GetClassNameW, GetClientRect, GetCursorPos, GetForegroundWindow, GetWindowLongW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, IsZoomed,
    LoadCursorW, MessageBoxW, PeekMessageW, PostMessageW, RegisterClassW, RegisterWindowMessageW, SendMessageW,
    SetForegroundWindow, SetWindowPos, ShowWindow, SystemParametersInfoW,
    TrackPopupMenu, TranslateMessage, UpdateLayeredWindow, GWL_EXSTYLE, HICON, HMENU,
    HWND_TOPMOST, ICONINFO, IDC_ARROW, IDYES, MA_NOACTIVATE, MB_ICONQUESTION, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO,
    MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, PM_REMOVE, SPI_GETWORKAREA, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNOACTIVATE, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, ULW_ALPHA,
    WM_APP, WM_CLOSE, WM_ENDSESSION, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_NULL,
    WM_QUERYENDSESSION, WM_QUIT, WM_RBUTTONUP, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
    WS_VSCROLL,
};

use super::uia::{Element, PageBoxes, Reply, Request, Worker};
use super::{Event, Image, MenuItem, PageSnapshot, Platform, SettingItem};
use crate::math::{v2, Rect, Vec2};
use crate::world::{Edge, Taskbar, WinId, WinInfo};

thread_local! {
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    /// Which overlay window is which stickman: (window handle, slot).
    static SLOTS: RefCell<Vec<(usize, usize)>> = const { RefCell::new(Vec::new()) };
    /// Explorer restarted: the tray icon needs adding again.
    static TRAY_LOST: Cell<bool> = const { Cell::new(false) };
    /// The "TaskbarCreated" message Explorer broadcasts when it restarts.
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
}

fn push_event(e: Event) {
    EVENTS.with(|q| q.borrow_mut().push(e));
}

fn slot_of(hwnd: HWND) -> Option<usize> {
    SLOTS.with(|s| s.borrow().iter().find(|(h, _)| *h == hwnd as usize).map(|(_, slot)| *slot))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

const OVERLAY_CLASS: &str = "StickmanOverlay";
const HOST_CLASS: &str = "StickmanHost";
/// Tray icon clicks arrive as this message.
const TRAY_MSG: u32 = WM_APP + 1;

/// A stickman's overlay window.
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // Clicking the stickman must never steal focus from your apps.
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_LBUTTONDOWN => {
            if let Some(slot) = slot_of(hwnd) {
                SetCapture(hwnd);
                push_event(Event::MouseDown { slot });
            }
            0
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            if let Some(slot) = slot_of(hwnd) {
                push_event(Event::MouseUp { slot });
            }
            0
        }
        WM_RBUTTONUP => {
            if let Some(slot) = slot_of(hwnd) {
                push_event(Event::RightClick { slot });
            }
            0
        }
        // Overlays only go away when the program removes them.
        WM_CLOSE => 0,
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// The hidden window that owns the tray icon and the menus.
unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        TRAY_MSG => {
            let what = (lp as usize & 0xFFFF) as u32;
            if what == WM_LBUTTONUP || what == WM_RBUTTONUP {
                push_event(Event::Tray);
            }
            0
        }
        // Windows is shutting down / logging off: save everyone first.
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION => {
            if wp != 0 {
                push_event(Event::Quit);
            }
            0
        }
        WM_CLOSE => {
            push_event(Event::Quit);
            0
        }
        _ => {
            if msg != 0 && msg == TASKBAR_CREATED.with(|c| c.get()) {
                TRAY_LOST.with(|c| c.set(true));
                return 0;
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
    }
}

// Standard control styles/messages (plain numbers, to avoid depending on
// exactly how windows-sys types each one).
const BS_PUSHBUTTON: u32 = 0x0;
const BS_AUTOCHECKBOX: u32 = 0x3;
const CBS_DROPDOWNLIST: u32 = 0x3;
const BM_GETCHECK: u32 = 0x00F0;
const BM_SETCHECK: u32 = 0x00F1;
const CB_ADDSTRING: u32 = 0x0143;
const CB_GETCURSEL: u32 = 0x0147;
const CB_SETCURSEL: u32 = 0x014E;
const MSG_COMMAND: u32 = 0x0111;
const MSG_SETFONT: u32 = 0x0030;
const BN_CLICKED_CODE: u32 = 0;
const CBN_SELCHANGE_CODE: u32 = 1;
const DEFAULT_GUI_FONT: i32 = 17;
/// COLOR_BTNFACE + 1: the usual dialog background.
const DIALOG_BRUSH: usize = 16;

const LVM_SETITEMPOSITION: u32 = 0x100F;

unsafe extern "system" fn collect_monitor(mon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let list = &mut *(data as *mut Vec<(Rect, bool)>);
    let mut mi: MONITORINFO = zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(mon, &mut mi) != 0 {
        let r = mi.rcWork;
        list.push((Rect { l: r.left as f32, t: r.top as f32, r: r.right as f32, b: r.bottom as f32 }, mi.dwFlags & 1 != 0));
    }
    1
}

/// The settings window: turns control changes into `Event::Setting`.
unsafe extern "system" fn settings_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        MSG_COMMAND => {
            let id = (wp & 0xFFFF) as u32;
            let code = ((wp >> 16) & 0xFFFF) as u32;
            let ctrl = lp as HWND;
            if code == BN_CLICKED_CODE && id != 0 {
                let on = SendMessageW(ctrl, BM_GETCHECK, 0, 0) == 1;
                push_event(Event::Setting { id, value: on as i32 });
            } else if code == CBN_SELCHANGE_CODE {
                let sel = SendMessageW(ctrl, CB_GETCURSEL, 0, 0);
                push_event(Event::Setting { id, value: sel as i32 });
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// The window holding the desktop icons ("SysListView32" inside
/// "SHELLDLL_DefView", which sits under Progman, or under a WorkerW when a
/// wallpaper slideshow is on).
unsafe fn desktop_listview() -> HWND {
    let progman_cls = wide("Progman");
    let workerw_cls = wide("WorkerW");
    let defview_cls = wide("SHELLDLL_DefView");
    let list_cls = wide("SysListView32");
    let progman = FindWindowW(progman_cls.as_ptr(), null());
    let mut defview = if progman.is_null() {
        null_mut()
    } else {
        FindWindowExW(progman, null_mut(), defview_cls.as_ptr(), null())
    };
    if defview.is_null() {
        let mut worker: HWND = null_mut();
        loop {
            worker = FindWindowExW(null_mut(), worker, workerw_cls.as_ptr(), null());
            if worker.is_null() {
                break;
            }
            defview = FindWindowExW(worker, null_mut(), defview_cls.as_ptr(), null());
            if !defview.is_null() {
                break;
            }
        }
    }
    if defview.is_null() {
        return null_mut();
    }
    FindWindowExW(defview, null_mut(), list_cls.as_ptr(), null())
}

unsafe extern "system" fn collect_windows(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = &mut *(lparam as *mut Vec<HWND>);
    list.push(hwnd);
    1
}

unsafe fn frame_rect(h: HWND) -> Option<Rect> {
    let mut rc: RECT = zeroed();
    let hr = DwmGetWindowAttribute(
        h,
        DWMWA_EXTENDED_FRAME_BOUNDS as u32,
        &mut rc as *mut RECT as *mut c_void,
        size_of::<RECT>() as u32,
    );
    if hr != 0 && GetWindowRect(h, &mut rc) == 0 {
        return None;
    }
    Some(Rect {
        l: rc.left as f32,
        t: rc.top as f32,
        r: rc.right as f32,
        b: rc.bottom as f32,
    })
}

/// A tray icon from a square straight-alpha RGBA image.
unsafe fn make_icon(rgba: &[u8], size: u32) -> HICON {
    if rgba.len() < (size * size * 4) as usize || size == 0 {
        return null_mut();
    }
    let mut bmi: BITMAPINFO = zeroed();
    bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = size as i32;
    bmi.bmiHeader.biHeight = -(size as i32);
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;
    let screen = GetDC(null_mut());
    let mut bits: *mut c_void = null_mut();
    let color = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    ReleaseDC(null_mut(), screen);
    if color.is_null() || bits.is_null() {
        return null_mut();
    }
    let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize);
    for (d, s) in dst.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = s[3];
    }
    // The mask is ignored for icons with alpha, but must exist.
    let row = (size as usize).div_ceil(16) * 2;
    let zeros = vec![0u8; row * size as usize];
    let mask = CreateBitmap(size as i32, size as i32, 1, 1, zeros.as_ptr() as *const c_void);
    let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
    let icon = CreateIconIndirect(&info);
    DeleteObject(color);
    if !mask.is_null() {
        DeleteObject(mask);
    }
    icon
}

/// One stickman's window and its drawing buffer.
struct Overlay {
    hwnd: HWND,
    mem_dc: HDC,
    bitmap: HBITMAP,
    bits: *mut u8,
    w: u32,
    h: u32,
    shown: bool,
}

impl Overlay {
    unsafe fn resize_buffer(&mut self, w: u32, h: u32) {
        let mut bmi: BITMAPINFO = zeroed();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w as i32;
        bmi.bmiHeader.biHeight = -(h as i32); // top-down rows
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = null_mut();
        let bmp = CreateDIBSection(self.mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        SelectObject(self.mem_dc, bmp);
        if !self.bitmap.is_null() {
            DeleteObject(self.bitmap);
        }
        self.bitmap = bmp;
        self.bits = bits as *mut u8;
        self.w = w;
        self.h = h;
    }

    unsafe fn destroy(self) {
        DestroyWindow(self.hwnd);
        if !self.bitmap.is_null() {
            DeleteObject(self.bitmap);
        }
        DeleteDC(self.mem_dc);
    }
}

pub struct Win32Platform {
    /// Hidden window: owns the tray icon and the menus.
    host: HWND,
    overlays: Vec<Option<Overlay>>,
    /// The 16 "custom colours" slots in the picker, kept for the session.
    custom_colors: [u32; 16],
    /// UI Automation background thread, started the first time it's needed,
    /// and the latest answers it gave.
    uia: Option<Worker>,
    uia_failed: bool,
    icons: Vec<(String, Rect)>,
    settings_hwnd: HWND,
    settings_class: bool,
    tray_icon: HICON,
    tray_added: bool,
    /// What the tray icon shows, to put it back if Explorer restarts.
    tray_image: Option<(Vec<u8>, u32, String)>,
    buttons: Vec<(String, Rect)>,
    elements: Vec<(usize, Vec<Element>, Instant)>,
    pages: Vec<(usize, Option<PageBoxes>, Instant, Option<(u32, f32)>)>,
    /// Latest position of each followed page box: (window, tracking, top, when).
    probes: Vec<(usize, u32, f32, Instant)>,
}

impl Win32Platform {
    pub fn new() -> Result<Self, String> {
        unsafe {
            // Work in physical pixels everywhere so window rects line up.
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

            let hinst = GetModuleHandleW(null());
            type Proc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;
            let classes: [(&str, Proc); 2] = [(OVERLAY_CLASS, wndproc as Proc), (HOST_CLASS, host_proc as Proc)];
            for (name, proc_) in classes {
                let class = wide(name);
                let wc = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(proc_),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: hinst,
                    hIcon: null_mut(),
                    hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                    hbrBackground: null_mut(),
                    lpszMenuName: null(),
                    lpszClassName: class.as_ptr(),
                };
                if RegisterClassW(&wc) == 0 {
                    return Err("RegisterClassW failed".into());
                }
            }
            let class = wide(HOST_CLASS);
            let title = wide("Stickman");
            let host = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                null_mut(),
                null_mut(),
                hinst,
                null(),
            );
            if host.is_null() {
                return Err("CreateWindowExW failed".into());
            }
            let created = wide("TaskbarCreated");
            TASKBAR_CREATED.with(|c| c.set(RegisterWindowMessageW(created.as_ptr())));
            Ok(Win32Platform {
                host,
                overlays: Vec::new(),
                custom_colors: [0x00FF_FFFF; 16],
                uia: None,
                uia_failed: false,
                buttons: Vec::new(),
                elements: Vec::new(),
                pages: Vec::new(),
                probes: Vec::new(),
                icons: Vec::new(),
                settings_hwnd: null_mut(),
                settings_class: false,
                tray_icon: null_mut(),
                tray_added: false,
                tray_image: None,
            })
        }
    }

    /// Start the UI Automation thread if needed and collect its answers.
    fn uia(&mut self) -> Option<&mut Worker> {
        if self.uia.is_none() && !self.uia_failed {
            self.uia = Worker::spawn();
            self.uia_failed = self.uia.is_none();
        }
        let worker = self.uia.as_mut()?;
        let mut clicks = Vec::new();
        while let Some(reply) = worker.poll() {
            match reply {
                Reply::Taskbar(b) => self.buttons = b,
                Reply::Elements(h, e, t) => {
                    self.elements.retain(|(k, _, _)| *k != h);
                    self.elements.push((h, e, t));
                    // Only remember a few windows.
                    if self.elements.len() > 4 {
                        self.elements.remove(0);
                    }
                }
                Reply::Page(h, p, t, probe) => {
                    self.pages.retain(|(k, _, _, _)| *k != h);
                    self.pages.push((h, p, t, probe));
                    if self.pages.len() > 4 {
                        self.pages.remove(0);
                    }
                }
                Reply::Probe(h, tracking, top, at) => {
                    self.probes.retain(|(k, _, _, _)| *k != h);
                    self.probes.push((h, tracking, top, at));
                }
                Reply::ProbeLost(h) => self.probes.retain(|(k, _, _, _)| *k != h),
                Reply::InvokeFailed(h, at) => clicks.push((h, at)),
                Reply::Icons(icons) => self.icons = icons,
            }
        }
        // Couldn't press it the accessible way: click it instead.
        for (h, at) in clicks {
            self.post_click(h as WinId, at);
        }
        self.uia.as_mut()
    }

    unsafe fn describe(&self, h: HWND) -> Option<WinInfo> {
        if IsWindowVisible(h) == 0 || IsIconic(h) != 0 {
            return None;
        }
        // (The stickmen's own windows are tool windows, so they're skipped here.)
        let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOOLWINDOW != 0 {
            return None;
        }
        // Hidden UWP windows and windows on other virtual desktops are "cloaked".
        let mut cloaked: u32 = 0;
        DwmGetWindowAttribute(
            h,
            DWMWA_CLOAKED as u32,
            &mut cloaked as *mut u32 as *mut c_void,
            size_of::<u32>() as u32,
        );
        if cloaked != 0 || GetWindowTextLengthW(h) == 0 {
            return None;
        }
        let mut buf = [0u16; 64];
        let n = GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        let class = String::from_utf16_lossy(&buf[..n]);
        if matches!(
            class.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return None;
        }
        let rect = frame_rect(h)?;
        if rect.r - rect.l < 60.0 || rect.b - rect.t < 40.0 {
            return None;
        }
        // Maximized (or covering the whole work area): it's effectively the
        // background, not something to climb or stand on.
        let work = self.work_area();
        let covers = rect.l <= work.l + 8.0 && rect.t <= work.t + 8.0 && rect.r >= work.r - 8.0 && rect.b >= work.b - 8.0;
        let maximized = IsZoomed(h) != 0 || covers;
        Some(WinInfo { id: h as usize as WinId, rect, maximized })
    }
}

impl Platform for Win32Platform {
    fn poll_events(&mut self) -> Vec<Event> {
        unsafe {
            let mut msg: MSG = zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    push_event(Event::Quit);
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        // Explorer restarted: put the tray icon back.
        if TRAY_LOST.with(|c| c.replace(false)) {
            self.tray_added = false;
            if let Some((rgba, size, tip)) = self.tray_image.clone() {
                self.set_tray(&rgba, size, &tip);
            }
        }
        EVENTS.with(|q| std::mem::take(&mut *q.borrow_mut()))
    }

    fn work_area(&self) -> Rect {
        unsafe {
            let mut rc: RECT = zeroed();
            SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut rc as *mut RECT as *mut c_void, 0);
            Rect {
                l: rc.left as f32,
                t: rc.top as f32,
                r: rc.right as f32,
                b: rc.bottom as f32,
            }
        }
    }

    fn taskbar(&self) -> Option<Taskbar> {
        unsafe {
            let mut abd: APPBARDATA = zeroed();
            abd.cbSize = size_of::<APPBARDATA>() as u32;
            if SHAppBarMessage(ABM_GETTASKBARPOS, &mut abd) == 0 {
                return None;
            }
            let rc = abd.rc;
            let rect = Rect { l: rc.left as f32, t: rc.top as f32, r: rc.right as f32, b: rc.bottom as f32 };
            let edge = match abd.uEdge {
                e if e == ABE_LEFT => Edge::Left,
                e if e == ABE_TOP => Edge::Top,
                e if e == ABE_RIGHT => Edge::Right,
                e if e == ABE_BOTTOM => Edge::Bottom,
                _ => return None,
            };
            // Auto-hide: the work area then covers the taskbar's spot.
            let work = self.work_area();
            let hidden = match edge {
                Edge::Bottom => work.b > rect.t + 2.0,
                Edge::Top => work.t < rect.b - 2.0,
                Edge::Left => work.l < rect.r - 2.0,
                Edge::Right => work.r > rect.l + 2.0,
            };
            (!hidden).then_some(Taskbar { rect, edge })
        }
    }

    fn windows(&mut self) -> Vec<WinInfo> {
        let mut handles: Vec<HWND> = Vec::new();
        unsafe {
            EnumWindows(Some(collect_windows), &mut handles as *mut Vec<HWND> as LPARAM);
            handles.into_iter().filter_map(|h| self.describe(h)).collect()
        }
    }

    fn window_rect(&self, id: WinId) -> Option<Rect> {
        let h = id as usize as HWND;
        unsafe {
            if IsWindow(h) == 0 || IsWindowVisible(h) == 0 || IsIconic(h) != 0 {
                return None;
            }
            frame_rect(h)
        }
    }

    fn cursor(&self) -> Vec2 {
        unsafe {
            let mut pt: POINT = zeroed();
            GetCursorPos(&mut pt);
            v2(pt.x as f32, pt.y as f32)
        }
    }

    fn dpi_scale(&self) -> f32 {
        let dpi = unsafe { GetDpiForSystem() };
        if dpi == 0 {
            1.0
        } else {
            dpi as f32 / 96.0
        }
    }

    fn create_overlay(&mut self) -> usize {
        let slot = self.overlays.iter().position(Option::is_none).unwrap_or(self.overlays.len());
        if slot == self.overlays.len() {
            self.overlays.push(None);
        }
        unsafe {
            let hinst = GetModuleHandleW(null());
            let class = wide(OVERLAY_CLASS);
            let title = wide("Stickman");
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                null_mut(),
                null_mut(),
                hinst,
                null(),
            );
            if hwnd.is_null() {
                return slot;
            }
            let screen = GetDC(null_mut());
            let mem_dc = CreateCompatibleDC(screen);
            ReleaseDC(null_mut(), screen);
            SLOTS.with(|s| s.borrow_mut().push((hwnd as usize, slot)));
            self.overlays[slot] =
                Some(Overlay { hwnd, mem_dc, bitmap: null_mut(), bits: null_mut(), w: 0, h: 0, shown: false });
        }
        slot
    }

    fn destroy_overlay(&mut self, slot: usize) {
        if let Some(o) = self.overlays.get_mut(slot).and_then(Option::take) {
            SLOTS.with(|s| s.borrow_mut().retain(|(h, _)| *h != o.hwnd as usize));
            unsafe { o.destroy() };
        }
    }

    fn present(&mut self, slot: usize, rgba: &[u8], w: u32, h: u32, x: i32, y: i32, occluders: &[Rect]) {
        let Some(Some(o)) = self.overlays.get_mut(slot) else { return };
        unsafe {
            if w != o.w || h != o.h || o.bits.is_null() {
                o.resize_buffer(w, h);
                if o.bits.is_null() {
                    return;
                }
            }
            let dst = std::slice::from_raw_parts_mut(o.bits, (w * h * 4) as usize);
            // tiny-skia is premultiplied RGBA; GDI wants premultiplied BGRA.
            for (d, s) in dst.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
                d[0] = s[2];
                d[1] = s[1];
                d[2] = s[0];
                d[3] = s[3];
            }
            // Cut out the parts covered by windows that are "in front".
            for r in occluders {
                let x0 = (r.l.floor() as i32 - x).clamp(0, w as i32) as usize;
                let x1 = (r.r.ceil() as i32 - x).clamp(0, w as i32) as usize;
                let y0 = (r.t.floor() as i32 - y).clamp(0, h as i32) as usize;
                let y1 = (r.b.ceil() as i32 - y).clamp(0, h as i32) as usize;
                if x0 >= x1 || y0 >= y1 {
                    continue;
                }
                for row in y0..y1 {
                    let start = (row * w as usize + x0) * 4;
                    let end = (row * w as usize + x1) * 4;
                    dst[start..end].fill(0);
                }
            }

            let pt_dst = POINT { x, y };
            let size = SIZE { cx: w as i32, cy: h as i32 };
            let pt_src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let screen = GetDC(null_mut());
            UpdateLayeredWindow(o.hwnd, screen, &pt_dst, &size, o.mem_dc, &pt_src, 0, &blend, ULW_ALPHA);
            ReleaseDC(null_mut(), screen);
            if !o.shown {
                ShowWindow(o.hwnd, SW_SHOWNOACTIVATE);
                o.shown = true;
            }
        }
    }

    fn set_tray(&mut self, rgba: &[u8], size: u32, tip: &str) {
        unsafe {
            let icon = make_icon(rgba, size);
            if icon.is_null() {
                return;
            }
            let mut nid: NOTIFYICONDATAW = zeroed();
            nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = self.host;
            nid.uID = 1;
            nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
            nid.uCallbackMessage = TRAY_MSG;
            nid.hIcon = icon;
            let text: Vec<u16> = tip.encode_utf16().take(nid.szTip.len() - 1).collect();
            nid.szTip[..text.len()].copy_from_slice(&text);
            let op = if self.tray_added { NIM_MODIFY } else { NIM_ADD };
            if Shell_NotifyIconW(op, &nid) != 0 {
                self.tray_added = true;
            }
            if !self.tray_icon.is_null() {
                DestroyIcon(self.tray_icon);
            }
            self.tray_icon = icon;
        }
        self.tray_image = Some((rgba.to_vec(), size, tip.to_string()));
    }

    fn confirm(&mut self, text: &str) -> bool {
        let text = wide(text);
        let title = wide("Stickman");
        unsafe {
            MessageBoxW(self.host, text.as_ptr(), title.as_ptr(), MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND)
                == IDYES
        }
    }

    fn show_menu(&mut self, items: &[MenuItem]) -> Option<u32> {
        unsafe fn build(menu: HMENU, items: &[MenuItem]) {
            for item in items {
                let label = wide(&item.label);
                if !item.children.is_empty() {
                    let sub = CreatePopupMenu();
                    build(sub, &item.children);
                    AppendMenuW(menu, MF_POPUP, sub as usize, label.as_ptr());
                } else if item.id == 0 {
                    AppendMenuW(menu, MF_SEPARATOR, 0, null());
                } else {
                    let flags = MF_STRING | if item.checked { MF_CHECKED } else { 0 };
                    AppendMenuW(menu, flags, item.id as usize, label.as_ptr());
                }
            }
        }
        unsafe {
            let menu = CreatePopupMenu();
            build(menu, items);
            let mut pt: POINT = zeroed();
            GetCursorPos(&mut pt);
            SetForegroundWindow(self.host);
            let cmd = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                pt.x,
                pt.y,
                0,
                self.host,
                null(),
            );
            PostMessageW(self.host, WM_NULL, 0, 0);
            DestroyMenu(menu);
            if cmd > 0 {
                Some(cmd as u32)
            } else {
                None
            }
        }
    }

    fn taskbar_buttons(&mut self) -> Vec<(String, Rect)> {
        let class = wide("Shell_TrayWnd");
        let tray = unsafe { FindWindowW(class.as_ptr(), null()) } as usize;
        if let Some(w) = self.uia() {
            w.request(Request::Taskbar(tray));
        }
        self.buttons.clone()
    }

    fn ui_elements(&mut self, id: WinId) -> (Vec<(String, String, i32, Rect)>, f32) {
        let h = id as usize;
        // Returns the last answer straight away and asks for a fresh one in
        // the background (the first call for a window returns nothing).
        if let Some(w) = self.uia() {
            w.request(Request::Elements(h));
        }
        match self.elements.iter().find(|(k, _, _)| *k == h) {
            Some((_, e, t)) => (e.clone(), t.elapsed().as_secs_f32()),
            None => (Vec::new(), f32::MAX),
        }
    }

    fn press(&mut self, id: WinId, name_prefix: &str, at: Vec2) {
        match self.uia() {
            Some(w) => w.request(Request::Invoke(id as usize, name_prefix.to_string(), at)),
            None => self.post_click(id, at),
        }
    }

    fn desktop_icons(&mut self) -> Vec<(String, Rect)> {
        let list = unsafe { desktop_listview() };
        if list.is_null() {
            return Vec::new();
        }
        if let Some(w) = self.uia() {
            w.request(Request::Icons(list as usize));
        }
        self.icons.clone()
    }

    fn screens(&self) -> Vec<Rect> {
        let mut list: Vec<(Rect, bool)> = Vec::new();
        unsafe {
            EnumDisplayMonitors(null_mut(), null(), Some(collect_monitor), &mut list as *mut Vec<(Rect, bool)> as LPARAM);
        }
        // Main screen first.
        list.sort_by_key(|(_, primary)| !*primary);
        let mut out: Vec<Rect> = list.into_iter().map(|(r, _)| r).collect();
        if out.is_empty() {
            out.push(self.work_area());
        }
        out
    }

    fn move_icon(&mut self, index: usize, to: Vec2) {
        unsafe {
            let list = desktop_listview();
            if list.is_null() {
                return;
            }
            let mut pt = POINT { x: to.x.round() as i32, y: to.y.round() as i32 };
            ScreenToClient(list, &mut pt);
            let lp = (((pt.y as u32 & 0xFFFF) << 16) | (pt.x as u32 & 0xFFFF)) as LPARAM;
            PostMessageW(list, LVM_SETITEMPOSITION, index, lp);
        }
        // Update our copy right away, so it doesn't jump back until the next
        // proper read.
        if let Some((_, r)) = self.icons.get_mut(index) {
            let (w, h) = (r.r - r.l, r.b - r.t);
            *r = Rect { l: to.x, t: to.y, r: to.x + w, b: to.y + h };
        }
    }

    fn open_settings(&mut self, items: &[SettingItem]) {
        unsafe {
            if !self.settings_hwnd.is_null() && IsWindow(self.settings_hwnd) != 0 {
                DestroyWindow(self.settings_hwnd);
            }
            let hinst = GetModuleHandleW(null());
            let class = wide("StickmanSettings");
            if !self.settings_class {
                let wc = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(settings_proc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: hinst,
                    hIcon: null_mut(),
                    hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                    hbrBackground: DIALOG_BRUSH as *mut c_void,
                    lpszMenuName: null(),
                    lpszClassName: class.as_ptr(),
                };
                RegisterClassW(&wc);
                self.settings_class = true;
            }

            // Sizes in "normal DPI" pixels, scaled to this screen.
            let k = self.dpi_scale();
            let px = |v: i32| (v as f32 * k).round() as i32;
            let (width, row, pad) = (px(380), px(30), px(14));
            let client_h = pad * 2 + items.len() as i32 * row;
            let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
            let mut rc = RECT { left: 0, top: 0, right: width, bottom: client_h };
            AdjustWindowRect(&mut rc, style, 0);
            let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
            // Open next to where you clicked, kept on screen.
            let work = self.work_area();
            let c = self.cursor();
            let x = ((c.x as i32) - w / 2).clamp(work.l as i32, (work.r as i32 - w).max(work.l as i32));
            let y = ((c.y as i32) - h / 2).clamp(work.t as i32, (work.b as i32 - h).max(work.t as i32));
            let title = wide("Stickman settings");
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                style | WS_VISIBLE,
                x,
                y,
                w,
                h,
                null_mut(),
                null_mut(),
                hinst,
                null(),
            );
            if hwnd.is_null() {
                return;
            }
            let font = GetStockObject(DEFAULT_GUI_FONT);
            let child = |class: &str, text: &str, style: u32, x: i32, y: i32, w: i32, h: i32, id: u32| -> HWND {
                let cls = wide(class);
                let txt = wide(text);
                let c = CreateWindowExW(
                    0,
                    cls.as_ptr(),
                    txt.as_ptr(),
                    WS_CHILD | WS_VISIBLE | style,
                    x,
                    y,
                    w,
                    h,
                    hwnd,
                    id as usize as HMENU,
                    hinst,
                    null(),
                );
                SendMessageW(c, MSG_SETFONT, font as usize, 1);
                c
            };
            let mut y = pad;
            let inner = width - pad * 2;
            for item in items {
                match item {
                    SettingItem::Heading(text) => {
                        child("STATIC", text, 0, pad, y + px(8), inner, row - px(8), 0);
                    }
                    SettingItem::Check { id, label, on } => {
                        let c = child("BUTTON", label, BS_AUTOCHECKBOX | WS_TABSTOP, pad + px(12), y, inner - px(12), row, *id);
                        SendMessageW(c, BM_SETCHECK, *on as usize, 0);
                    }
                    SettingItem::Choice { id, label, options, selected } => {
                        child("STATIC", label, 0, pad + px(12), y + px(6), px(140), row - px(6), 0);
                        let c = child(
                            "COMBOBOX",
                            "",
                            CBS_DROPDOWNLIST | WS_TABSTOP | WS_VSCROLL,
                            pad + px(160),
                            y + px(2),
                            inner - px(160),
                            px(240),
                            *id,
                        );
                        for o in options {
                            let t = wide(o);
                            SendMessageW(c, CB_ADDSTRING, 0, t.as_ptr() as LPARAM);
                        }
                        SendMessageW(c, CB_SETCURSEL, *selected, 0);
                    }
                    SettingItem::Button { id, label } => {
                        child("BUTTON", label, BS_PUSHBUTTON | WS_TABSTOP, pad + px(12), y + px(2), px(240), row - px(4), *id);
                    }
                }
                y += row;
            }
            self.settings_hwnd = hwnd;
            SetForegroundWindow(hwnd);
        }
    }

    fn page_boxes(&mut self, id: WinId) -> Option<PageSnapshot> {
        let h = id as usize;
        if let Some(w) = self.uia() {
            w.request(Request::Page(h));
        }
        let (_, page, taken, probe) = self.pages.iter().find(|(k, _, _, _)| *k == h)?;
        let (doc, boxes) = page.as_ref()?;
        Some(PageSnapshot { doc: *doc, boxes: boxes.clone(), taken: *taken, probe: *probe })
    }

    fn track(&mut self, id: WinId, target: Option<Rect>) {
        let h = id as usize;
        if target.is_none() {
            self.probes.retain(|(k, _, _, _)| *k != h);
        }
        if let Some(w) = self.uia() {
            w.request(Request::Track(h, target));
        }
    }

    fn page_probe(&mut self, id: WinId) -> Option<(u32, f32, Instant)> {
        let h = id as usize;
        // Collect the latest replies first.
        let _ = self.uia();
        self.probes.iter().find(|(k, _, _, _)| *k == h).map(|(_, g, top, at)| (*g, *top, *at))
    }

    fn window_title(&self, id: WinId) -> String {
        let mut buf = [0u16; 256];
        let n = unsafe { GetWindowTextW(id as usize as HWND, buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    fn window_process(&self, id: WinId) -> Option<(u32, String)> {
        unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(id as usize as HWND, &mut pid);
            if pid == 0 {
                return None;
            }
            let hp = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if hp.is_null() {
                return Some((pid, String::new()));
            }
            let mut buf = [0u16; 520];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(hp, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
            CloseHandle(hp);
            if !ok {
                return Some((pid, String::new()));
            }
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            let exe = path.rsplit(|c| c == '\\' || c == '/').next().unwrap_or("").to_lowercase();
            Some((pid, exe))
        }
    }

    fn foreground(&self) -> Option<WinId> {
        let h = unsafe { GetForegroundWindow() };
        (!h.is_null()).then(|| h as usize as WinId)
    }

    fn client_rect(&self, id: WinId) -> Option<Rect> {
        let h = id as usize as HWND;
        unsafe {
            let mut rc: RECT = zeroed();
            if GetClientRect(h, &mut rc) == 0 {
                return None;
            }
            let mut tl = POINT { x: 0, y: 0 };
            let mut br = POINT { x: rc.right, y: rc.bottom };
            ClientToScreen(h, &mut tl);
            ClientToScreen(h, &mut br);
            Some(Rect { l: tl.x as f32, t: tl.y as f32, r: br.x as f32, b: br.y as f32 })
        }
    }

    fn capture(&mut self, r: Rect) -> Option<Image> {
        let (x, y) = (r.l.round() as i32, r.t.round() as i32);
        let (w, h) = ((r.r - r.l).round() as i32, (r.b - r.t).round() as i32);
        if w <= 0 || h <= 0 || w > 4096 || h > 4096 {
            return None;
        }
        unsafe {
            let screen = GetDC(null_mut());
            let dc = CreateCompatibleDC(screen);
            let mut bmi: BITMAPINFO = zeroed();
            bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h;
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB;
            let mut bits: *mut c_void = null_mut();
            let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if bmp.is_null() || bits.is_null() {
                DeleteDC(dc);
                ReleaseDC(null_mut(), screen);
                return None;
            }
            let old = SelectObject(dc, bmp);
            let ok = BitBlt(dc, 0, 0, w, h, screen, x, y, SRCCOPY) != 0;
            let mut rgba = vec![0u8; (w * h * 4) as usize];
            if ok {
                let src = std::slice::from_raw_parts(bits as *const u8, rgba.len());
                for (d, s) in rgba.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = 255;
                }
            }
            SelectObject(dc, old);
            DeleteObject(bmp);
            DeleteDC(dc);
            ReleaseDC(null_mut(), screen);
            ok.then_some(Image { w: w as u32, h: h as u32, rgba })
        }
    }

    fn post_click(&self, id: WinId, at: Vec2) {
        let h = id as usize as HWND;
        unsafe {
            let mut pt = POINT { x: at.x.round() as i32, y: at.y.round() as i32 };
            ScreenToClient(h, &mut pt);
            // Old games are usually not DPI-aware: their messages use
            // unscaled coordinates.
            let ours = GetDpiForSystem().max(1) as f32;
            let theirs = GetDpiForWindow(h).max(1) as f32;
            let k = theirs / ours;
            let cx = (pt.x as f32 * k).round() as i32;
            let cy = (pt.y as f32 * k).round() as i32;
            let lp = (((cy as u32 & 0xFFFF) << 16) | (cx as u32 & 0xFFFF)) as LPARAM;
            PostMessageW(h, WM_MOUSEMOVE, 0, lp);
            PostMessageW(h, WM_LBUTTONDOWN, 1, lp);
            PostMessageW(h, WM_LBUTTONUP, 0, lp);
        }
    }

    fn pick_color(&mut self, initial: [u8; 3]) -> Option<[u8; 3]> {
        unsafe {
            let mut cc: CHOOSECOLORW = zeroed();
            cc.lStructSize = size_of::<CHOOSECOLORW>() as u32;
            cc.hwndOwner = self.host;
            // COLORREF is 0x00BBGGRR.
            cc.rgbResult = initial[0] as u32 | (initial[1] as u32) << 8 | (initial[2] as u32) << 16;
            cc.lpCustColors = self.custom_colors.as_mut_ptr();
            cc.Flags = CC_RGBINIT | CC_FULLOPEN | CC_ANYCOLOR;
            if ChooseColorW(&mut cc) == 0 {
                return None;
            }
            let c = cc.rgbResult;
            Some([(c & 0xFF) as u8, ((c >> 8) & 0xFF) as u8, ((c >> 16) & 0xFF) as u8])
        }
    }

    fn keep_on_top(&mut self) {
        for o in self.overlays.iter().flatten() {
            unsafe {
                SetWindowPos(o.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }
}

impl Drop for Win32Platform {
    fn drop(&mut self) {
        unsafe {
            for o in self.overlays.drain(..).flatten() {
                o.destroy();
            }
            if self.tray_added {
                let mut nid: NOTIFYICONDATAW = zeroed();
                nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
                nid.hWnd = self.host;
                nid.uID = 1;
                Shell_NotifyIconW(NIM_DELETE, &nid);
            }
            if !self.tray_icon.is_null() {
                DestroyIcon(self.tray_icon);
            }
            DestroyWindow(self.host);
        }
    }
}
