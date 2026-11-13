//! Check the chosen window's lifetime without capturing anything else.
use super::{DesktopCaptureSourceType, Source};
use anyhow::ensure;
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow},
};

const CLOSED: &str = "A janela compartilhada foi fechada. A transmissão de tela foi encerrada. Escolha outra janela para continuar.";

pub(super) struct Target {
    id: u64,
    owner: (u32, u32),
}

fn owner(id: u64) -> Option<(u32, u32)> {
    let window = HWND(id as usize as *mut _);
    // Only queries the handle. Visibility and pixel changes are deliberately
    // ignored: minimizing or showing a static image must not end a share.
    unsafe {
        if !IsWindow(Some(window)).as_bool() {
            return None;
        }
        let mut process = 0;
        let thread = GetWindowThreadProcessId(window, Some(&mut process));
        (thread != 0 && process != 0).then_some((process, thread))
    }
}

impl Target {
    pub(super) fn for_source(source: &Source) -> anyhow::Result<Option<Self>> {
        if source.kind != DesktopCaptureSourceType::Window {
            return Ok(None);
        }
        let id = source.id.ok_or_else(|| anyhow::anyhow!(CLOSED))?;
        let owner = owner(id).ok_or_else(|| anyhow::anyhow!(CLOSED))?;
        Ok(Some(Self { id, owner }))
    }

    pub(super) fn check(&self) -> anyhow::Result<()> {
        ensure!(owner(self.id) == Some(self.owner), CLOSED);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::{
        Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, SW_MINIMIZE, ShowWindow, WINDOW_EX_STYLE,
            WS_OVERLAPPEDWINDOW,
        },
        core::w,
    };

    #[test]
    fn hidden_or_minimized_window_survives_but_destroyed_window_ends_capture() {
        // A synthetic window, with no screen access or user application involved.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("Telinha lifetime test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                16,
                16,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        };
        let source = Source {
            id: Some(hwnd.0 as usize as u64),
            name: "Synthetic".into(),
            kind: DesktopCaptureSourceType::Window,
        };
        let target = Target::for_source(&source).unwrap().unwrap();
        assert!(target.check().is_ok());
        unsafe {
            let _ = ShowWindow(hwnd, SW_MINIMIZE);
        }
        assert!(target.check().is_ok());
        let different_owner = Target {
            id: target.id,
            owner: (0, 0),
        };
        assert!(different_owner.check().is_err());
        unsafe {
            DestroyWindow(hwnd).unwrap();
        }
        assert!(
            target
                .check()
                .unwrap_err()
                .to_string()
                .contains("foi fechada")
        );
        assert!(Target::for_source(&source).is_err());
        let display = Source {
            kind: DesktopCaptureSourceType::Screen,
            ..source
        };
        assert!(Target::for_source(&display).unwrap().is_none());
    }
}
