//! Windows backend: a small per-pixel-alpha layered window that follows the
//! stickman around, plus window enumeration for z-order and surfaces.

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
    DIB_RGB_COLORS, HBITMAP, HDC,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::Dialogs::{
    ChooseColorW, CC_ANYCOLOR, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW,
};
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::Shell::{
    SHAppBarMessage, ABE_BOTTOM, ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_GETTASKBARPOS, APPBARDATA,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    EnumWindows, GetClassNameW, GetCursorPos, GetWindowLongW, GetWindowRect,
    GetWindowTextLengthW, IsIconic, IsWindow, IsWindowVisible, LoadCursorW, PeekMessageW,
    PostMessageW, PostQuitMessage, RegisterClassW, SetForegroundWindow, SetWindowPos, ShowWindow,
    SystemParametersInfoW, TrackPopupMenu, TranslateMessage, UpdateLayeredWindow, GWL_EXSTYLE,
    HMENU, HWND_TOPMOST, IDC_ARROW, MA_NOACTIVATE, MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG,
    PM_REMOVE,
    SPI_GETWORKAREA, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNOACTIVATE, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, ULW_ALPHA, WM_CLOSE, WM_DESTROY, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEACTIVATE, WM_NULL, WM_QUIT, WM_RBUTTONUP, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::{Event, MenuItem, Platform};
use crate::math::{v2, Rect, Vec2};
use crate::world::{Edge, Taskbar, WinId, WinInfo};

thread_local! {
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
}

fn push_event(e: Event) {
    EVENTS.with(|q| q.borrow_mut().push(e));
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // Clicking the stickman must never steal focus from your apps.
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_LBUTTONDOWN => {
            SetCapture(hwnd);
            push_event(Event::MouseDown);
            0
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            push_event(Event::MouseUp);
            0
        }
        WM_RBUTTONUP => {
            push_event(Event::RightClick);
            0
        }
        WM_CLOSE => {
            push_event(Event::Quit);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
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

pub struct Win32Platform {
    hwnd: HWND,
    mem_dc: HDC,
    bitmap: HBITMAP,
    bits: *mut u8,
    w: u32,
    h: u32,
    shown: bool,
    /// The 16 "custom colours" slots in the picker, kept for the session.
    custom_colors: [u32; 16],
}

impl Win32Platform {
    pub fn new() -> Result<Self, String> {
        unsafe {
            // Work in physical pixels everywhere so window rects line up.
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

            let hinst = GetModuleHandleW(null());
            let class = wide("StickmanOverlay");
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(wndproc),
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
                return Err("CreateWindowExW failed".into());
            }
            let screen = GetDC(null_mut());
            let mem_dc = CreateCompatibleDC(screen);
            ReleaseDC(null_mut(), screen);
            Ok(Win32Platform {
                hwnd,
                mem_dc,
                bitmap: null_mut(),
                bits: null_mut(),
                w: 0,
                h: 0,
                shown: false,
                custom_colors: [0x00FF_FFFF; 16],
            })
        }
    }

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

    unsafe fn describe(&self, h: HWND) -> Option<WinInfo> {
        if h == self.hwnd || IsWindowVisible(h) == 0 || IsIconic(h) != 0 {
            return None;
        }
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
        Some(WinInfo { id: h as usize as WinId, rect })
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
        let dpi = unsafe { GetDpiForWindow(self.hwnd) };
        if dpi == 0 {
            1.0
        } else {
            dpi as f32 / 96.0
        }
    }

    fn present(&mut self, rgba: &[u8], w: u32, h: u32, x: i32, y: i32, occluders: &[Rect]) {
        unsafe {
            if w != self.w || h != self.h || self.bits.is_null() {
                self.resize_buffer(w, h);
                if self.bits.is_null() {
                    return;
                }
            }
            let dst = std::slice::from_raw_parts_mut(self.bits, (w * h * 4) as usize);
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
            UpdateLayeredWindow(
                self.hwnd, screen, &pt_dst, &size, self.mem_dc, &pt_src, 0, &blend, ULW_ALPHA,
            );
            ReleaseDC(null_mut(), screen);
            if !self.shown {
                ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                self.shown = true;
            }
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
            SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                pt.x,
                pt.y,
                0,
                self.hwnd,
                null(),
            );
            PostMessageW(self.hwnd, WM_NULL, 0, 0);
            DestroyMenu(menu);
            if cmd > 0 {
                Some(cmd as u32)
            } else {
                None
            }
        }
    }

    fn pick_color(&mut self, initial: [u8; 3]) -> Option<[u8; 3]> {
        unsafe {
            let mut cc: CHOOSECOLORW = zeroed();
            cc.lStructSize = size_of::<CHOOSECOLORW>() as u32;
            cc.hwndOwner = self.hwnd;
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
        unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

impl Drop for Win32Platform {
    fn drop(&mut self) {
        unsafe {
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
            DeleteDC(self.mem_dc);
        }
    }
}
