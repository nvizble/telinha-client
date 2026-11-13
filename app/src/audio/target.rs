//! Bind an audio choice to a process, without opening an audio device.
use super::Application;
use crate::capture::Source;
use ::windows::Win32::{
    Foundation::{CloseHandle, FILETIME, HANDLE, HWND, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, WaitForSingleObject,
    },
    UI::WindowsAndMessaging::GetWindowThreadProcessId,
};
use anyhow::{Context, bail, ensure};
use livekit::webrtc::desktop_capturer::DesktopCaptureSourceType;

const STALE: &str = "O aplicativo escolhido para o áudio foi fechado ou mudou. Atualize a lista e escolha o aplicativo novamente.";
const ENDED: &str = "O aplicativo escolhido para o áudio foi encerrado. Escolha outro aplicativo para continuar com som.";
const BLOCKED: &str = "O áudio do Discord e do Telinha não pode ser compartilhado, para evitar eco. Escolha o jogo ou outro aplicativo.";

fn owner(window_id: u64) -> Option<u32> {
    let mut process_id = 0;
    let thread_id = unsafe {
        GetWindowThreadProcessId(HWND(window_id as usize as *mut _), Some(&mut process_id))
    };
    (thread_id != 0 && process_id != 0).then_some(process_id)
}

/// Kept on the capture worker until WASAPI stops. A PID alone cannot detect exit
/// or prove that a later process is the one the person selected.
struct Process {
    handle: HANDLE,
    created_at: u64,
}

impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

impl Process {
    fn open(process_id: u32) -> anyhow::Result<Self> {
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                process_id,
            )
        }
        .context(STALE)?;
        let mut process = Self {
            handle,
            created_at: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) }
            .context(STALE)?;
        process.created_at =
            (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        ensure!(process.created_at != 0, STALE);
        process.check_alive()?;
        Ok(process)
    }

    fn check_alive(&self) -> anyhow::Result<()> {
        match unsafe { WaitForSingleObject(self.handle, 0) } {
            WAIT_TIMEOUT => Ok(()),
            WAIT_OBJECT_0 => bail!(ENDED),
            _ => bail!(
                "Não deu pra verificar o aplicativo escolhido para o áudio. Escolha o aplicativo novamente."
            ),
        }
    }

    fn check_audio_allowed(&self) -> anyhow::Result<()> {
        let mut path = vec![0u16; 32768];
        let mut length = path.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                self.handle,
                PROCESS_NAME_WIN32,
                ::windows::core::PWSTR(path.as_mut_ptr()),
                &mut length,
            )
        }
        .context("Não deu pra identificar o aplicativo de áudio. Atualize a lista.")?;
        let path = String::from_utf16(&path[..length as usize])?;
        let own = std::env::current_exe()?;
        ensure!(
            !super::policy::blocked_executable(&path)
                && !path.eq_ignore_ascii_case(&own.to_string_lossy()),
            BLOCKED
        );
        Ok(())
    }
}

pub(super) struct Target {
    process: Process,
    process_id: u32,
}

impl Target {
    pub(super) fn open(application: &Application) -> anyhow::Result<Self> {
        let target = Self::bind(application)?;
        target.process.check_audio_allowed()?;
        Ok(target)
    }

    fn bind(application: &Application) -> anyhow::Result<Self> {
        ensure!(
            owner(application.window_id) == Some(application.process_id),
            STALE
        );
        let process = Process::open(application.process_id)?;
        ensure!(process.created_at == application.created_at, STALE);
        // Close the enumeration/open race before WASAPI receives the PID.
        ensure!(
            owner(application.window_id) == Some(application.process_id),
            STALE
        );
        process.check_alive()?;
        Ok(Self {
            process,
            process_id: application.process_id,
        })
    }

    pub(super) fn process_id(&self) -> u32 {
        self.process_id
    }

    pub(super) fn check_alive(&self) -> anyhow::Result<()> {
        // Closing one window is not process termination: the application may keep
        // playing audio through another window. Visibility and silence do not matter.
        self.process.check_alive()
    }
}

pub(super) fn applications(sources: &[Source]) -> Vec<Application> {
    sources
        .iter()
        .filter(|source| source.kind == DesktopCaptureSourceType::Window)
        .filter_map(|source| {
            let window_id = source.id?;
            let process_id = owner(window_id)?;
            let process = Process::open(process_id).ok()?;
            process.check_audio_allowed().ok()?;
            if owner(window_id) != Some(process_id) || process.check_alive().is_err() {
                return None;
            }
            Some(Application {
                window_id,
                process_id,
                created_at: process.created_at,
                name: source.name.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::windows::{
        Win32::{
            System::Threading::CREATE_NO_WINDOW,
            UI::WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, SW_MINIMIZE, ShowWindow, WINDOW_EX_STYLE,
                WS_OVERLAPPEDWINDOW,
            },
        },
        core::w,
    };
    use std::{
        io::Read,
        os::windows::process::CommandExt,
        process::{Child, Command, Stdio},
    };

    struct Window(Option<HWND>);
    impl Window {
        fn new() -> Self {
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("Telinha synthetic audio target"),
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
            Self(Some(hwnd))
        }

        fn source(&self) -> Source {
            Source {
                id: Some(self.0.unwrap().0 as usize as u64),
                name: "Aplicativo de teste".into(),
                kind: DesktopCaptureSourceType::Window,
            }
        }

        fn close(&mut self) {
            if let Some(hwnd) = self.0.take() {
                unsafe { DestroyWindow(hwnd).unwrap() };
            }
        }
    }
    impl Drop for Window {
        fn drop(&mut self) {
            self.close();
        }
    }

    #[test]
    fn enumeration_binds_windows_to_their_original_process() {
        let mut window = Window::new();
        let source = window.source();
        let display = Source {
            kind: DesktopCaptureSourceType::Screen,
            ..source.clone()
        };
        let missing = Source {
            id: None,
            ..source.clone()
        };
        let invalid = Source {
            id: Some(0),
            ..source.clone()
        };
        let choices = applications(&[display, missing, invalid, source.clone()]);
        // The test window belongs to this client, so it must never appear as an
        // audio choice. Validate the binding independently of that policy.
        assert!(choices.is_empty());
        let process = Process::open(std::process::id()).unwrap();
        let application = &Application {
            window_id: source.id.unwrap(),
            process_id: std::process::id(),
            created_at: process.created_at,
            name: source.name.clone(),
        };
        assert_eq!(application.window_id, source.id.unwrap());
        assert_eq!(application.process_id, std::process::id());
        assert_ne!(application.created_at, 0);
        assert_eq!(application.name, source.name);
        assert!(
            Target::open(application)
                .err()
                .unwrap()
                .to_string()
                .contains("evitar eco")
        );
        let target = Target::bind(application).unwrap();
        assert_eq!(target.process_id(), std::process::id());

        let wrong_process = Application {
            process_id: 0,
            ..application.clone()
        };
        assert!(Target::open(&wrong_process).is_err());
        let reused_process = Application {
            created_at: application.created_at.wrapping_add(1),
            ..application.clone()
        };
        assert!(Target::open(&reused_process).is_err());
        let wrong_window = Application {
            window_id: 0,
            ..application.clone()
        };
        assert!(Target::open(&wrong_window).is_err());

        unsafe {
            let _ = ShowWindow(window.0.unwrap(), SW_MINIMIZE);
        }
        assert!(target.check_alive().is_ok());
        assert!(Target::bind(application).is_ok());
        window.close();
        assert!(Target::open(application).is_err());
        assert!(applications(&[source]).is_empty());
        // Existing capture remains tied to the original, still-live process.
        assert!(target.check_alive().is_ok());
    }

    struct ChildProcess(Child);
    impl Drop for ChildProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn retained_handle_detects_process_exit_without_an_audio_device() {
        let mut child = ChildProcess(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "audio::target::tests::synthetic_child_process",
                    "--nocapture",
                ])
                .env("TELINHA_AUDIO_TARGET_TEST_CHILD", "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW.0)
                .spawn()
                .unwrap(),
        );
        let original = Process::open(child.0.id()).unwrap();
        assert!(original.check_alive().is_ok());
        let window = Window::new();
        let another_process = Application {
            window_id: window.source().id.unwrap(),
            process_id: child.0.id(),
            created_at: original.created_at,
            name: "Processo diferente da janela".into(),
        };
        assert!(Target::open(&another_process).is_err());
        let second_handle = Process::open(child.0.id()).unwrap();
        assert_eq!(original.created_at, second_handle.created_at);
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        assert_eq!(original.check_alive().unwrap_err().to_string(), ENDED);
        assert_eq!(second_handle.check_alive().unwrap_err().to_string(), ENDED);
    }

    #[test]
    fn failed_lifetime_check_never_means_a_live_process() {
        // Deliberately invalid, so no real process handle should be closed on drop.
        let invalid = std::mem::ManuallyDrop::new(Process {
            handle: HANDLE::default(),
            created_at: 1,
        });
        assert!(invalid.check_alive().is_err());
    }

    #[test]
    fn synthetic_child_process() {
        if std::env::var_os("TELINHA_AUDIO_TARGET_TEST_CHILD").is_some() {
            // The parent owns the pipe and kills only this synthetic test process.
            let _ = std::io::stdin().read_exact(&mut [0]);
        }
    }
}
