#[cfg(windows)]
mod win {
    #![allow(dead_code)]

    use std::ffi::c_void;

    pub type Handle = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct Coord {
        pub x: i16,
        pub y: i16,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct SmallRect {
        pub left: i16,
        pub top: i16,
        pub right: i16,
        pub bottom: i16,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct ConsoleScreenBufferInfo {
        pub size: Coord,
        pub cursor: Coord,
        pub attributes: u16,
        pub window: SmallRect,
        pub max_window: Coord,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct ConsoleFontInfoEx {
        pub cb_size: u32,
        pub index: u32,
        pub size: Coord,
        pub family: u32,
        pub weight: u32,
        pub face: [u16; 32],
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct Rect {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct MonitorInfo {
        pub cb_size: u32,
        pub monitor: Rect,
        pub work: Rect,
        pub flags: u32,
    }

    pub const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
    pub const SM_CXSCREEN: i32 = 0;
    pub const MONITOR_DEFAULTTONEAREST: u32 = 2;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetStdHandle(which: u32) -> Handle;
        pub fn GetConsoleScreenBufferInfo(
            handle: Handle,
            info: *mut ConsoleScreenBufferInfo,
        ) -> i32;
        pub fn GetCurrentConsoleFontEx(
            handle: Handle,
            maximum: i32,
            info: *mut ConsoleFontInfoEx,
        ) -> i32;
        pub fn GetConsoleWindow() -> Handle;
        pub fn IsZoomed(window: Handle) -> i32;
        pub fn GetSystemMetrics(index: i32) -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn GetForegroundWindow() -> Handle;
        pub fn GetClassNameW(window: Handle, buffer: *mut u16, capacity: i32) -> i32;
        pub fn GetWindowRect(window: Handle, rect: *mut Rect) -> i32;
        pub fn MonitorFromWindow(window: Handle, flags: u32) -> Handle;
        pub fn GetMonitorInfoW(monitor: Handle, info: *mut MonitorInfo) -> i32;
    }
}

#[cfg(windows)]
const TERMINAL_WINDOW_CLASSES: [&str; 2] = ["CASCADIA_HOSTING_WINDOW_CLASS", "ConsoleWindowClass"];

#[cfg(windows)]
fn class_name(window: win::Handle) -> Option<String> {
    use win::*;

    unsafe {
        let mut buffer = [0u16; 256];
        let len = GetClassNameW(window, buffer.as_mut_ptr(), buffer.len() as i32);
        if len <= 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..len as usize]))
    }
}

#[cfg(windows)]
fn rect_covers_area(rect: win::Rect, work: win::Rect, monitor: win::Rect) -> bool {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let work_matches = width + 8 >= work.right - work.left && height + 8 >= work.bottom - work.top;
    let monitor_matches =
        width + 8 >= monitor.right - monitor.left && height + 8 >= monitor.bottom - monitor.top;
    work_matches || monitor_matches
}

#[cfg(windows)]
fn terminal_window_state(window: win::Handle) -> Option<bool> {
    use std::mem::size_of;

    use win::*;

    let class = class_name(window)?;
    if !TERMINAL_WINDOW_CLASSES.contains(&class.as_str()) {
        return None;
    }
    unsafe {
        let mut rect = Rect::default();
        if GetWindowRect(window, &mut rect) == 0 {
            return None;
        }
        let monitor = MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST);
        if monitor.is_null() {
            return None;
        }
        let mut info = MonitorInfo {
            cb_size: size_of::<MonitorInfo>() as u32,
            ..MonitorInfo::default()
        };
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return None;
        }
        Some(rect_covers_area(rect, info.work, info.monitor))
    }
}

#[cfg(windows)]
fn coverage_estimate() -> Option<bool> {
    use std::mem::size_of;

    use win::*;

    unsafe {
        let stdout = GetStdHandle(STD_OUTPUT_HANDLE);
        if stdout.is_null() || stdout as isize == -1 {
            return None;
        }
        let mut buffer_info = ConsoleScreenBufferInfo::default();
        if GetConsoleScreenBufferInfo(stdout, &mut buffer_info) == 0 {
            return None;
        }
        let cols = i32::from(buffer_info.window.right - buffer_info.window.left + 1);
        let mut font = ConsoleFontInfoEx {
            cb_size: size_of::<ConsoleFontInfoEx>() as u32,
            ..ConsoleFontInfoEx::default()
        };
        if GetCurrentConsoleFontEx(stdout, 0, &mut font) == 0 {
            return None;
        }
        let cell_width = i32::from(font.size.x);
        let screen_width = GetSystemMetrics(SM_CXSCREEN);
        if cols <= 0 || cell_width <= 0 || screen_width <= 0 {
            return None;
        }
        Some(covers_screen(cols, cell_width, screen_width))
    }
}

#[cfg(windows)]
pub fn window_state() -> Option<bool> {
    use win::*;

    unsafe {
        let foreground = GetForegroundWindow();
        if !foreground.is_null()
            && let Some(state) = terminal_window_state(foreground)
        {
            return Some(state);
        }
        if coverage_estimate() == Some(true) {
            return Some(true);
        }
        let hwnd = GetConsoleWindow();
        if !hwnd.is_null() && class_name(hwnd).as_deref() == Some("ConsoleWindowClass") {
            return Some(IsZoomed(hwnd) != 0);
        }
    }
    None
}

#[cfg(windows)]
pub fn diagnostic() -> String {
    use std::mem::size_of;

    use win::*;

    let mut out = String::new();
    unsafe {
        let hwnd = GetConsoleWindow();
        out.push_str(&format!("hwnd_null={} ", hwnd.is_null()));
        if !hwnd.is_null() {
            out.push_str(&format!(
                "hwnd_class={:?} zoomed={} ",
                class_name(hwnd),
                IsZoomed(hwnd) != 0
            ));
        }
        let stdout = GetStdHandle(STD_OUTPUT_HANDLE);
        if stdout.is_null() || stdout as isize == -1 {
            out.push_str("stdout=invalid ");
        } else {
            let mut buffer_info = ConsoleScreenBufferInfo::default();
            if GetConsoleScreenBufferInfo(stdout, &mut buffer_info) == 0 {
                out.push_str("csbi=failed ");
            } else {
                let cols = i32::from(buffer_info.window.right - buffer_info.window.left + 1);
                out.push_str(&format!("cols={cols} "));
                let mut font = ConsoleFontInfoEx {
                    cb_size: size_of::<ConsoleFontInfoEx>() as u32,
                    ..ConsoleFontInfoEx::default()
                };
                if GetCurrentConsoleFontEx(stdout, 0, &mut font) == 0 {
                    out.push_str("font=failed ");
                } else {
                    let cell_width = i32::from(font.size.x);
                    let screen_width = GetSystemMetrics(SM_CXSCREEN);
                    let pixels = cols.max(0).saturating_mul(cell_width.max(0));
                    let ratio = if screen_width > 0 {
                        f64::from(pixels) / f64::from(screen_width)
                    } else {
                        0.0
                    };
                    out.push_str(&format!(
                        "cell={cell_width} screen={screen_width} coverage={ratio:.3} "
                    ));
                }
            }
        }
        let foreground = GetForegroundWindow();
        out.push_str(&format!("fg_null={} ", foreground.is_null()));
        if !foreground.is_null() {
            out.push_str(&format!(
                "fg_class={:?} fg_state={:?} ",
                class_name(foreground),
                terminal_window_state(foreground)
            ));
        }
        out.push_str(&format!("window_state={:?}", window_state()));
    }
    out
}

#[cfg(windows)]
fn covers_screen(cols: i32, cell_width: i32, screen_width: i32) -> bool {
    cols.saturating_mul(cell_width) * 10 >= screen_width * 9
}

#[cfg(not(windows))]
pub fn window_state() -> Option<bool> {
    None
}

#[cfg(not(windows))]
pub fn diagnostic() -> String {
    String::from("non-windows platform")
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn coverage_heuristic_separates_windowed_and_maximized() {
        assert!(covers_screen(213, 9, 1920));
        assert!(covers_screen(212, 9, 1920));
        assert!(!covers_screen(154, 9, 1920));
        assert!(covers_screen(320, 8, 2560));
        assert!(!covers_screen(200, 8, 2560));
    }

    #[test]
    fn rect_coverage_matches_work_and_monitor_areas() {
        let work = win::Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let monitor = win::Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let maximized = win::Rect {
            left: -7,
            top: -7,
            right: 1927,
            bottom: 1047,
        };
        assert!(rect_covers_area(maximized, work, monitor));
        let windowed = win::Rect {
            left: 100,
            top: 100,
            right: 1400,
            bottom: 900,
        };
        assert!(!rect_covers_area(windowed, work, monitor));
        let fullscreen = win::Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        assert!(rect_covers_area(fullscreen, work, monitor));
    }
}
