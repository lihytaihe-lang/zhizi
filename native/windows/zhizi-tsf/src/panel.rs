use super::*;
use crate::service::{execute, State, WM_RANK};
use zhizi_host::Key;
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};
use windows::Win32::{
    Graphics::Gdi::*,
    System::Registry::*,
    UI::{HiDpi::*, WindowsAndMessaging::*},
};
const CLASS: PCWSTR = w!("ZhiziCandidatePanelV1");
struct DpiContext(DPI_AWARENESS_CONTEXT);
impl DpiContext {
    unsafe fn enter() -> Self {
        Self(SetThreadDpiAwarenessContext(
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        ))
    }
}
impl Drop for DpiContext {
    fn drop(&mut self) {
        unsafe {
            SetThreadDpiAwarenessContext(self.0);
        }
    }
}
fn rgb(r: u32, g: u32, b: u32) -> COLORREF {
    COLORREF(r | (g << 8) | (b << 16))
}
fn dark() -> bool {
    unsafe {
        let mut value = 1u32;
        let mut size = 4;
        let _ = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut _ as _),
            Some(&mut size),
        );
        value == 0
    }
}
pub fn create(state: &Rc<RefCell<State>>) -> Result<()> {
    unsafe {
        let _dpi = DpiContext::enter();
        let instance = HINSTANCE(MODULE.load(Ordering::Relaxed) as _);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(procedure),
            hInstance: instance,
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let weak = Box::into_raw(Box::new(Rc::downgrade(state)));
        let result = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED,
            CLASS,
            w!("知字候选"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance),
            Some(weak as _),
        );
        match result {
            Ok(hwnd) => {
                state.borrow_mut().hwnd = hwnd;
                SetLayeredWindowAttributes(hwnd, rgb(255, 0, 255), 255, LWA_COLORKEY)?;
                SetTimer(Some(hwnd), 1, 100, None);
                Ok(())
            }
            Err(e) => {
                drop(Box::from_raw(weak));
                Err(e)
            }
        }
    }
}
pub fn hide(state: &Rc<RefCell<State>>) {
    unsafe {
        let _ = ShowWindow(state.borrow().hwnd, SW_HIDE);
    }
}
fn scale(hwnd: HWND) -> f32 {
    unsafe { GetDpiForWindow(hwnd).max(96) as f32 / 96.0 }
}
pub fn show(state: &Rc<RefCell<State>>) {
    unsafe {
        let Ok(s) = state.try_borrow() else { return };
        let badge = badge_text(&s);
        if !s.active || (badge.is_none() && (s.view.preedit.is_empty() || s.view.candidates.is_empty())) {
            let _ = ShowWindow(s.hwnd, SW_HIDE);
            return;
        }
        let _dpi = DpiContext::enter();
        let monitor = MonitorFromRect(&s.caret, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(monitor, &mut info);
        let mut dx = 96;
        let mut dy = 96;
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let dpi = dx as f32 / 96.0;
        let rows = s.grid.as_ref().map(|g| g.rows.len()).unwrap_or(1);
        let (width, height) = if badge.is_some() {
            // the small 中 / 英 badge after Shift switched the mode
            ((46.0 * dpi) as i32, (40.0 * dpi) as i32)
        } else {
            let (_, fitted) = layout(dpi, &s);
            (
                fitted.min((1000.0 * dpi) as i32).min(info.rcWork.right - info.rcWork.left - 16),
                ((34.0 + 54.0 * rows as f32) * dpi) as i32,
            )
        };
        let x = s.caret.left.clamp(
            info.rcWork.left,
            (info.rcWork.right - width).max(info.rcWork.left),
        );
        let below = s.caret.bottom + 4;
        let y = if below + height <= info.rcWork.bottom {
            below
        } else {
            (s.caret.top - height - 4).max(info.rcWork.top)
        };
        let mut previous = RECT::default();
        let _ = GetWindowRect(s.hwnd, &mut previous);
        if previous.left != x
            || previous.top != y
            || previous.right - previous.left != width
            || previous.bottom - previous.top != height
            || !IsWindowVisible(s.hwnd).as_bool()
        {
            let _ = SetWindowPos(
                s.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
        let _ = InvalidateRect(Some(s.hwnd), None, false);
    }
}
/// The mode badge to show instead of the candidates: 英 or 中 for a moment after Shift, while nothing is being composed.
fn badge_text(state: &State) -> Option<&'static str> {
    match state.toast {
        Some((english, at)) if state.view.preedit.is_empty() && at.elapsed() < service::TOAST_FOR => Some(if english { "英" } else { "中" }),
        _ => None,
    }
}
unsafe fn font(size: i32) -> HFONT {
    CreateFontW(
        -size,
        0,
        0,
        0,
        400,
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_DEFAULT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        CLEARTYPE_QUALITY,
        DEFAULT_PITCH.0 as u32,
        w!("Microsoft YaHei UI"),
    )
}
unsafe fn text_width(dc: HDC, font: HFONT, text: &str) -> i32 {
    if text.is_empty() {
        return 0;
    }
    let old = SelectObject(dc, HGDIOBJ(font.0));
    let wide = text.encode_utf16().collect::<Vec<_>>();
    let mut size = SIZE::default();
    let _ = GetTextExtentPoint32W(dc, &wide, &mut size);
    SelectObject(dc, old);
    size.cx
}
/// Widths of the (up to five) columns, fitted to the candidates they hold, and the whole panel width.
/// Every row shares the column widths so the number keys line up; the old panel was a fixed 1000 dp wide
/// whatever was inside (a friend on Windows: "这个宽度太大了").
unsafe fn layout(dpi: f32, state: &State) -> (Vec<i32>, i32) {
    let dc = GetDC(None);
    let main = font((18.0 * dpi) as i32);
    let small = font((11.0 * dpi) as i32);
    let fallback = vec![state.view.candidates.clone()];
    let rows = state.grid.as_ref().map(|g| &g.rows).unwrap_or(&fallback);
    let pad = (10.0 * dpi) as i32;
    let columns = rows.iter().map(|r| r.len().min(5)).max().unwrap_or(1).max(1);
    let mut widths = vec![(84.0 * dpi) as i32; columns];
    for candidates in rows {
        for (col, candidate) in candidates.iter().take(5).enumerate() {
            let need = (26.0 * dpi) as i32
                + text_width(dc, main, &candidate.text).max(text_width(dc, small, &candidate.comment))
                + (16.0 * dpi) as i32;
            widths[col] = widths[col].max(need);
        }
    }
    let heading = text_width(dc, small, &format!("{}    知字拼音  ·  顺手·知字", state.view.preedit)) + (32.0 * dpi) as i32;
    let total = (pad * 2 + widths.iter().sum::<i32>()).max(heading);
    DeleteObject(HGDIOBJ(main.0));
    DeleteObject(HGDIOBJ(small.0));
    ReleaseDC(None, dc);
    (widths, total)
}
/// The cell under a click: (row, column), found with the same geometry `paint` uses.
unsafe fn cell_at(hwnd: HWND, state: &State, x: i32, y: i32) -> Option<(usize, usize)> {
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);
    let dpi = scale(hwnd);
    let pad = (10.0 * dpi) as i32;
    let (mut widths, _) = layout(dpi, state);
    let room = client.right - pad * 2;
    let wanted = widths.iter().sum::<i32>();
    if wanted > room && wanted > 0 {
        for w in widths.iter_mut() {
            *w = *w * room / wanted;
        }
    }
    let fallback = vec![state.view.candidates.clone()];
    let rows = state.grid.as_ref().map(|g| &g.rows).unwrap_or(&fallback);
    let row = ((y as f32 / dpi - 29.0) / 54.0).floor();
    if row < 0.0 || ((y as f32 / dpi - 29.0) % 54.0) > 50.0 {
        return None;
    }
    let row = row as usize;
    let mut left = pad;
    for (col, width) in widths.iter().enumerate() {
        if x >= left && x < left + width - 4 {
            return (col < rows.get(row)?.len().min(5)).then_some((row, col));
        }
        left += width;
    }
    None
}
/// A click on a candidate picks it, as the number key would (the panel never takes the focus).
unsafe fn click(hwnd: HWND, state: &Rc<RefCell<State>>, l: LPARAM) {
    let (x, y) = ((l.0 & 0xFFFF) as i16 as i32, ((l.0 >> 16) & 0xFFFF) as i16 as i32);
    let (cell, active, context) = match state.try_borrow() {
        Ok(s) => (cell_at(hwnd, &s, x, y), s.grid.as_ref().map(|g| g.row).unwrap_or(0), s.context.clone()),
        Err(_) => return,
    };
    let (Some((row, col)), Some(context)) = (cell, context) else { return };
    // The numbers belong to the highlighted row: walk to the clicked row first.
    for _ in 0..row.saturating_sub(active) {
        let _ = execute(state, Some(&context), Key::Down, 0);
    }
    for _ in 0..active.saturating_sub(row) {
        let _ = execute(state, Some(&context), Key::Up, 0);
    }
    let _ = execute(state, Some(&context), Key::Text((col + 1).to_string()), 0);
}
unsafe fn label(dc: HDC, font: HFONT, text: &str, rect: RECT, color: COLORREF) {
    // DrawTextW may inspect lpchText even when cchText is zero. An empty Rust
    // slice has a dangling sentinel, not a readable UTF-16 buffer.
    if text.is_empty() {
        return;
    }
    let old = SelectObject(dc, HGDIOBJ(font.0));
    SetTextColor(dc, color);
    SetBkMode(dc, TRANSPARENT);
    let mut text = text.encode_utf16().collect::<Vec<_>>();
    let mut rect = rect;
    DrawTextW(
        dc,
        &mut text,
        &mut rect,
        DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
    SelectObject(dc, old);
}
unsafe fn paint(hwnd: HWND, state: &Rc<RefCell<State>>) {
    let mut ps = PAINTSTRUCT::default();
    let dc = BeginPaint(hwnd, &mut ps);
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);
    let mem = CreateCompatibleDC(Some(dc));
    let bitmap = CreateCompatibleBitmap(dc, client.right.max(1), client.bottom.max(1));
    let oldbitmap = SelectObject(mem, HGDIOBJ(bitmap.0));
    let transparent = CreateSolidBrush(rgb(255, 0, 255));
    FillRect(mem, &client, transparent);
    DeleteObject(HGDIOBJ(transparent.0));
    let isdark = dark();
    let back = if isdark {
        rgb(43, 43, 46)
    } else {
        rgb(248, 248, 250)
    };
    let ink = if isdark {
        rgb(240, 240, 245)
    } else {
        rgb(36, 36, 40)
    };
    let muted = if isdark {
        rgb(171, 171, 181)
    } else {
        rgb(110, 110, 120)
    };
    let brush = CreateSolidBrush(back);
    let oldbrush = SelectObject(mem, HGDIOBJ(brush.0));
    let oldpen = SelectObject(mem, GetStockObject(NULL_PEN));
    let dpi = scale(hwnd);
    let corner = (14.0 * dpi) as i32;
    RoundRect(mem, 0, 0, client.right, client.bottom, corner, corner);
    SelectObject(mem, oldbrush);
    SelectObject(mem, oldpen);
    DeleteObject(HGDIOBJ(brush.0));
    if let Ok(s) = state.try_borrow() {
        let main = font((18.0 * dpi) as i32);
        let small = font((11.0 * dpi) as i32);
        if let Some(text) = badge_text(&s) {
            let big = font((20.0 * dpi) as i32);
            let mut rect = client;
            let mut wide = text.encode_utf16().collect::<Vec<_>>();
            let old = SelectObject(mem, HGDIOBJ(big.0));
            SetTextColor(mem, ink);
            SetBkMode(mem, TRANSPARENT);
            DrawTextW(mem, &mut wide, &mut rect, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);
            SelectObject(mem, old);
            DeleteObject(HGDIOBJ(big.0));
            DeleteObject(HGDIOBJ(main.0));
            DeleteObject(HGDIOBJ(small.0));
        } else {
        label(
            mem,
            small,
            &format!("{}    知字拼音  ·  顺手·知字", s.view.preedit),
            RECT {
                left: (16.0 * dpi) as i32,
                top: 0,
                right: client.right - 10,
                bottom: (29.0 * dpi) as i32,
            },
            muted,
        );
        let fallback = vec![s.view.candidates.clone()];
        let (rows, active) = if let Some(g) = &s.grid {
            (&g.rows, g.row)
        } else {
            (&fallback, 0)
        };
        let pad = (10.0 * dpi) as i32;
        let (mut widths, _) = layout(dpi, &s);
        let room = client.right - pad * 2;
        let wanted = widths.iter().sum::<i32>();
        if wanted > room && wanted > 0 {
            // the monitor is narrower than the candidates want: shrink every column in proportion
            for w in widths.iter_mut() {
                *w = *w * room / wanted;
            }
        }
        for (r, candidates) in rows.iter().enumerate() {
            for (col, candidate) in candidates.iter().take(5).enumerate() {
                let cell = widths[col];
                let left = pad + widths[..col].iter().sum::<i32>();
                let top = ((29.0 + r as f32 * 54.0) * dpi) as i32;
                let highlight = r == active && col == s.view.highlighted;
                if highlight {
                    let brush = CreateSolidBrush(if isdark {
                        rgb(68, 90, 127)
                    } else {
                        rgb(221, 233, 252)
                    });
                    let old = SelectObject(mem, HGDIOBJ(brush.0));
                    let pen = SelectObject(mem, GetStockObject(NULL_PEN));
                    RoundRect(
                        mem,
                        left,
                        top,
                        left + cell - 4,
                        top + (50.0 * dpi) as i32,
                        corner,
                        corner,
                    );
                    SelectObject(mem, old);
                    SelectObject(mem, pen);
                    DeleteObject(HGDIOBJ(brush.0));
                }
                let number = if r == active {
                    format!("{}", col + 1)
                } else {
                    String::new()
                };
                label(
                    mem,
                    small,
                    &number,
                    RECT {
                        left: left + 6,
                        top,
                        right: left + (22.0 * dpi) as i32,
                        bottom: top + (30.0 * dpi) as i32,
                    },
                    muted,
                );
                label(
                    mem,
                    main,
                    &candidate.text,
                    RECT {
                        left: left + (26.0 * dpi) as i32,
                        top,
                        right: left + cell - 10,
                        bottom: top + (30.0 * dpi) as i32,
                    },
                    ink,
                );
                label(
                    mem,
                    small,
                    &candidate.comment,
                    RECT {
                        left: left + (26.0 * dpi) as i32,
                        top: top + (29.0 * dpi) as i32,
                        right: left + cell - 10,
                        bottom: top + (47.0 * dpi) as i32,
                    },
                    muted,
                );
            }
        }
        DeleteObject(HGDIOBJ(main.0));
        DeleteObject(HGDIOBJ(small.0));
        }
    }
    let _ = BitBlt(
        dc,
        0,
        0,
        client.right,
        client.bottom,
        Some(mem),
        0,
        0,
        SRCCOPY,
    );
    SelectObject(mem, oldbitmap);
    DeleteObject(HGDIOBJ(bitmap.0));
    DeleteDC(mem);
    EndPaint(hwnd, &ps);
}
unsafe extern "system" fn procedure(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(l.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Weak<RefCell<State>>;
    if msg == WM_NCDESTROY {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        if !ptr.is_null() {
            drop(Box::from_raw(ptr));
        }
        return DefWindowProcW(hwnd, msg, w, l);
    }
    if !ptr.is_null() {
        if let Some(state) = (*ptr).upgrade() {
            match msg {
                WM_PAINT => {
                    paint(hwnd, &state);
                    return LRESULT(0);
                }
                WM_RANK | WM_TIMER => {
                    service::poll(&state);
                    return LRESULT(0);
                }
                WM_SETTINGCHANGE | WM_DPICHANGED => {
                    show(&state);
                    return LRESULT(0);
                }
                WM_LBUTTONDOWN => {
                    click(hwnd, &state, l);
                    return LRESULT(0);
                }
                WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
                WM_ERASEBKGND => return LRESULT(1),
                _ => {}
            }
        }
    }
    DefWindowProcW(hwnd, msg, w, l)
}
