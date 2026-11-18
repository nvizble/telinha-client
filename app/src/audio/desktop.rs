//! Windows desktop audio with native Discord/Telinha excluded. WASAPI's process
//! loopback can include/exclude only one tree, so capture disjoint allowed trees
//! and mix them. Never fall back to unfiltered endpoint/system loopback.
use super::{mix, policy, target};
use ::windows::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    },
};
use anyhow::{Context, ensure};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};
use wasapi::{
    AudioCaptureClient, AudioClient, DeviceEnumerator, Direction, Handle, SampleType, SessionState,
    StreamMode, WaveFormat,
};

struct Snapshot(HANDLE);
impl Drop for Snapshot {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn processes() -> anyhow::Result<BTreeMap<u32, mix::Process>> {
    let snapshot = Snapshot(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }?);
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    unsafe { Process32FirstW(snapshot.0, &mut entry) }?;
    let mut result = BTreeMap::new();
    loop {
        let name = String::from_utf16_lossy(
            &entry.szExeFile[..entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len())],
        );
        let process = target::Process::open(entry.th32ProcessID).ok();
        // Inaccessible processes cannot become capture roots. Creation times also
        // keep a recycled PID from becoming a parent of an unrelated application.
        result.insert(
            entry.th32ProcessID,
            mix::Process {
                pid: entry.th32ProcessID,
                parent: entry.th32ParentProcessID,
                created_at: process.as_ref().map_or(0, |p| p.created_at),
                allowed: process
                    .as_ref()
                    .is_some_and(|p| p.check_audio_allowed().is_ok()),
                echo: entry.th32ProcessID == std::process::id() || policy::echo_executable(&name),
            },
        );
        if unsafe { Process32NextW(snapshot.0, &mut entry) }.is_err() {
            break;
        }
    }
    // Windows' idle process has PID/parent 0. Do not treat it as a cycle in every
    // root process's ancestry; it cannot own a usable application audio stream.
    result.remove(&0);
    Ok(result)
}

fn sessions() -> anyhow::Result<BTreeSet<u32>> {
    let devices = DeviceEnumerator::new()?.get_device_collection(Direction::Render)?;
    let mut pids = BTreeSet::new();
    for index in 0..devices.get_nbr_devices()? {
        let Ok(device) = devices.get_device_at_index(index) else {
            continue;
        };
        let Ok(manager) = device.get_iaudiosessionmanager() else {
            continue;
        };
        let Ok(sessions) = manager.get_audiosessionenumerator() else {
            continue;
        };
        for index in 0..sessions.get_count()? {
            let Ok(session) = sessions.get_session(index) else {
                continue;
            };
            // Inactive sessions stay prepared so resuming a game does not require
            // another click. System sounds (PID 0) have no capturable app tree.
            if matches!(
                session.get_state(),
                Ok(SessionState::Active | SessionState::Inactive)
            ) && let Ok(pid) = session.get_process_id()
                && pid != 0
            {
                pids.insert(pid);
            }
        }
    }
    Ok(pids)
}

struct Stream {
    client: AudioClient,
    capture: AudioCaptureClient,
    _event: Handle,
    process: target::Process,
    raw: VecDeque<u8>,
    buffer: mix::Buffer,
}
impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.client.stop_stream();
    }
}
impl Stream {
    fn open(pid: u32, created_at: u64, stop: &AtomicBool) -> anyhow::Result<Self> {
        let process = target::Process::open(pid)?;
        ensure!(
            process.created_at == created_at,
            "Um aplicativo de áudio mudou durante a seleção."
        );
        process.check_audio_allowed()?;
        let mut client = AudioClient::new_application_loopback_client(pid, true)?;
        client.initialize_client(
            &WaveFormat::new(32, 32, SampleType::Float, 48000, 2, None),
            Direction::Capture,
            StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: 0,
            },
            None,
        )?;
        let event = client.set_get_eventhandle()?;
        let capture = client.get_audiocaptureclient()?;
        ensure!(!stop.load(Ordering::Relaxed), "Captura cancelada.");
        process.check_alive()?;
        client.start_stream()?;
        Ok(Self {
            client,
            capture,
            _event: event,
            process,
            raw: VecDeque::new(),
            buffer: mix::Buffer::default(),
        })
    }
    fn mix_into(&mut self, output: &mut [f32; mix::SAMPLES]) -> anyhow::Result<()> {
        self.process.check_alive()?;
        // Bound work even if an audio driver produces packets faster than we read.
        for _ in 0..20 {
            if self.capture.get_next_packet_size()?.unwrap_or(0) == 0 {
                break;
            }
            self.capture.read_from_device_to_deque(&mut self.raw)?;
            self.buffer.push(self.raw.make_contiguous());
            self.raw.clear();
        }
        self.process.check_alive()?;
        self.buffer.add_to(output);
        Ok(())
    }
}

pub(super) fn run(
    stop: &AtomicBool,
    pcm: &mpsc::Sender<Vec<i16>>,
    ready: &mut Option<oneshot::Sender<Result<(), String>>>,
) -> anyhow::Result<()> {
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }
    // Validate process-loopback support even on a silent desktop. This probe is
    // never started and never captures audio from the client itself.
    drop(AudioClient::new_application_loopback_client(
        std::process::id(),
        true,
    )?);
    let mut streams = BTreeMap::<(u32, u64), Stream>::new();
    let mut refresh = Instant::now();
    let mut next = Instant::now();
    while !stop.load(Ordering::Relaxed) && !pcm.is_closed() {
        let now = Instant::now();
        if now >= refresh {
            let roots = mix::roots(&sessions()?, &processes()?);
            ensure!(
                roots.len() <= 32,
                "Há aplicativos demais para misturar o áudio. Feche alguns ou escolha Outro app."
            );
            // Remove old/overlapping/now-blocked roots before adding replacements.
            streams
                .retain(|key, stream| roots.contains(key) && stream.process.check_alive().is_ok());
            for (pid, created) in roots {
                if stop.load(Ordering::Relaxed) {
                    return Ok(());
                }
                if let std::collections::btree_map::Entry::Vacant(slot) =
                    streams.entry((pid, created))
                {
                    match Stream::open(pid, created, stop) {
                        Ok(stream) => {
                            slot.insert(stream);
                        }
                        Err(error) => {
                            // Processes routinely exit between enumeration and activation.
                            // Other activation failures are reported, never replaced by global capture.
                            if target::Process::open(pid)
                                .is_ok_and(|p| p.created_at == created && p.check_alive().is_ok())
                            {
                                return Err(error)
                                    .context("Não deu pra iniciar o áudio do desktop.");
                            }
                        }
                    }
                }
            }
            if let Some(ready) = ready.take() {
                let _ = ready.send(Ok(()));
            }
            refresh = Instant::now() + Duration::from_secs(1);
        }
        let mut output = [0.; mix::SAMPLES];
        streams.retain(|_, stream| stream.process.check_alive().is_ok());
        for stream in streams.values_mut() {
            if let Err(error) = stream.mix_into(&mut output) {
                if stream.process.check_alive().is_ok() {
                    return Err(error);
                }
            }
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let _ = pcm.try_send(mix::pcm(&output));
        next += Duration::from_millis(10);
        let now = Instant::now();
        if next <= now {
            next = now + Duration::from_millis(10);
        }
        std::thread::sleep(next.saturating_duration_since(now));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_process_snapshot_excludes_the_current_client_without_opening_audio_devices() {
        let processes = processes().unwrap();
        let own = processes.get(&std::process::id()).unwrap();
        assert!(own.echo);
        assert!(!own.allowed);
        assert!(mix::roots(&[own.pid].into(), &processes).is_empty());
    }
}
