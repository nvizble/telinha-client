//! The running application's icon reflects actual screen/webcam publication.
#[derive(Default)]
pub struct State {
    live: Option<bool>,
}

impl State {
    pub fn update(&mut self, live: bool, window: &gpui::Window) {
        if self.live == Some(live) {
            return;
        }
        self.live = Some(live);
        if let Err(error) = set(live, window) {
            tracing::warn!(event = "app_icon_failed", %error, "Could not update application icon");
        }
    }
}

#[cfg(target_os = "macos")]
fn set(live: bool, _: &gpui::Window) -> anyhow::Result<()> {
    unsafe extern "C" {
        fn telinha_set_application_icon(png: *const u8, length: usize) -> bool;
    }
    let image: &[u8] = if live {
        include_bytes!("../assets/icon-live.png")
    } else {
        include_bytes!("../assets/icon.png")
    };
    // The Objective-C bridge copies the bytes and runs on the UI/main thread.
    anyhow::ensure!(
        unsafe { telinha_set_application_icon(image.as_ptr(), image.len()) },
        "AppKit rejected the icon"
    );
    Ok(())
}

#[cfg(target_os = "windows")]
fn set(live: bool, window: &gpui::Window) -> anyhow::Result<()> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::{
        Win32::{
            Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
            System::LibraryLoader::GetModuleHandleW,
            UI::WindowsAndMessaging::{
                IMAGE_ICON, LR_SHARED, LoadImageW, SendMessageW, WM_SETICON,
            },
        },
        core::PCWSTR,
    };
    let native = HasWindowHandle::window_handle(window)
        .map_err(|error| anyhow::anyhow!("Could not access native window: {error}"))?;
    let RawWindowHandle::Win32(handle) = native.as_raw() else {
        anyhow::bail!("No Win32 window handle");
    };
    let hwnd = HWND(handle.hwnd.get() as *mut _);
    let resource = PCWSTR(if live { 2usize } else { 1usize } as *const u16);
    unsafe {
        let module = HINSTANCE(GetModuleHandleW(None)?.0);
        for (kind, size) in [(0usize, 16), (1usize, 32)] {
            // LR_SHARED returns OS-owned cached resources: never DestroyIcon them.
            let image = LoadImageW(Some(module), resource, IMAGE_ICON, size, size, LR_SHARED)?;
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(kind)),
                Some(LPARAM(image.0 as isize)),
            );
        }
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn set(_: bool, _: &gpui::Window) -> anyhow::Result<()> {
    Ok(())
}
