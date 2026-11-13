use crate::{
    audio, camera,
    capture::{Capture, Source},
};
use anyhow::{Context, anyhow};
use futures_util::StreamExt;
use livekit::webrtc::video_frame::native::VideoFrameBufferExt;
use livekit::{
    options::{TrackPublishOptions, VideoCodec, VideoEncoding},
    prelude::*,
    webrtc::{
        prelude::VideoFormatType, video_source::RtcVideoSource,
        video_stream::native::NativeVideoStream,
    },
};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};
use telinha_shared::{Quality, RoomCode, SessionCredentials};
use tokio::sync::{mpsc, watch};

pub struct Pixels {
    pub track: String,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}
#[derive(Clone)]
pub struct RemoteStream {
    pub id: String,
    pub title: String,
    pub participant: String,
    pub avatar_url: Option<String>,
    pub kind: VideoKind,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VideoKind {
    Screen,
    Camera,
}
pub enum Event {
    Connected(Option<crate::feedback::Receipt>),
    Disconnected,
    Error(String),
    Notice(String),
    Reconnecting(bool),
    Participants(Vec<crate::participants::Person>),
    Sources {
        request: u64,
        result: Result<Vec<Source>, String>,
        audio_sources: Vec<audio::Application>,
    },
    SourcePreview {
        request: u64,
        index: usize,
        image: Option<image::RgbaImage>,
    },
    StreamAdded(RemoteStream),
    StreamRemoved(String),
    StreamSound(crate::sounds::Cue),
    Publishing(Option<String>),
    CaptureFps(u64),
    Cameras {
        request: u64,
        result: Result<Vec<camera::Device>, String>,
    },
    CameraPublishing {
        request: u64,
        stream: Option<(String, String)>,
    },
    CameraError {
        request: u64,
        message: String,
    },
}
pub struct Envelope {
    pub generation: u64,
    pub event: Event,
}
#[derive(Clone)]
struct Events {
    generation: u64,
    sender: async_channel::Sender<Envelope>,
}
impl Events {
    fn send(&self, event: Event) {
        let _ = self.sender.try_send(Envelope {
            generation: self.generation,
            event,
        });
    }
}

pub enum Command {
    Connect {
        generation: u64,
        code: RoomCode,
        guest_name: Option<String>,
    },
    Leave,
    Sources {
        generation: u64,
        request: u64,
    },
    StopSourcePreviews,
    Share {
        source: Source,
        quality: Quality,
        audio: Option<audio::Application>,
    },
    StopSharing,
    CancelShare,
    Cameras {
        generation: u64,
        request: u64,
    },
    StartCamera {
        request: u64,
        device: camera::Device,
    },
    StopCamera {
        request: u64,
    },
}
enum SessionCommand {
    Share {
        source: Source,
        quality: Quality,
        audio: Option<audio::Application>,
    },
    Stop,
    CancelShare,
    StartCamera {
        request: u64,
        device: camera::Device,
    },
    StopCamera {
        request: u64,
    },
}

#[derive(Clone, Default)]
pub struct MediaState {
    pub latest: Arc<Mutex<Option<Pixels>>>,
    pub selected_track: Arc<Mutex<Option<String>>>,
    pub selected_participant: Arc<Mutex<Option<String>>>,
    pub cameras: Arc<Mutex<HashMap<String, Pixels>>>,
}
pub struct Backend {
    pub updates: crate::updates::Status,
    pub commands: mpsc::UnboundedSender<Command>,
    pub events: async_channel::Receiver<Envelope>,
    pub media: MediaState,
    pub volume: Arc<AtomicU32>,
    pub account: crate::account::Account,
    pub feedback: crate::feedback::Client,
}

impl Backend {
    pub fn start(server_url: String) -> Self {
        let updates = crate::updates::Status::default();
        let update_status = updates.clone();
        let (account, account_worker) = crate::account::channel(&server_url);
        let account_token = account.token.clone();
        let (feedback, feedback_worker) = crate::feedback::channel(&server_url);
        let (commands, mut receiver) = mpsc::unbounded_channel();
        let (sender, events) = async_channel::unbounded();
        let media = MediaState::default();
        let worker_media = media.clone();
        let volume = Arc::new(AtomicU32::new(0.75f32.to_bits()));
        let worker_volume = volume.clone();
        std::thread::Builder::new()
            .name("telinha-network".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("runtime Tokio");
                runtime.block_on(async move {
                    let update_task = tokio::spawn(crate::updates::run(update_status));
                    let account_task = tokio::spawn(account_worker.run());
                    let feedback_task = tokio::spawn(feedback_worker.run());
                    let mut current: Option<(
                        watch::Sender<bool>,
                        mpsc::UnboundedSender<SessionCommand>,
                    )> = None;
                    let mut tasks = tokio::task::JoinSet::new();
                    let mut source_task: Option<tokio::task::AbortHandle> = None;
                    while let Some(command) = receiver.recv().await {
                        // Reap completed tasks so repeated sessions do not retain resources.
                        while tasks.try_join_next().is_some() {}
                        match command {
                            Command::Connect {
                                generation,
                                code,
                                guest_name,
                            } => {
                                if let Some(task) = source_task.take() {
                                    task.abort();
                                }
                                if let Some((stop, _)) = current.take() {
                                    let _ = stop.send(true);
                                }
                                let (stop, shutdown) = watch::channel(false);
                                let (tx, rx) = mpsc::unbounded_channel();
                                current = Some((stop, tx));
                                tasks.spawn(session(
                                    SessionRequest {
                                        server: server_url.clone(),
                                        code,
                                        account_token: account_token.lock().clone(),
                                        guest_name,
                                    },
                                    shutdown,
                                    rx,
                                    Events {
                                        generation,
                                        sender: sender.clone(),
                                    },
                                    worker_media.clone(),
                                    worker_volume.clone(),
                                ));
                            }
                            Command::Leave => {
                                if let Some(task) = source_task.take() {
                                    task.abort();
                                }
                                if let Some((stop, _)) = current.take() {
                                    let _ = stop.send(true);
                                }
                            }
                            Command::Sources {
                                generation,
                                request,
                            } => {
                                if let Some(task) = source_task.take() {
                                    task.abort();
                                }
                                let events = Events {
                                    generation,
                                    sender: sender.clone(),
                                };
                                source_task = Some(tasks.spawn(async move {
                                    let result = tokio::task::spawn_blocking(|| {
                                        let sources = crate::capture::sources()?;
                                        let audio_sources = audio::applications(&sources);
                                        Ok::<_, anyhow::Error>((sources, audio_sources))
                                    })
                                    .await
                                    .unwrap_or_else(|_| {
                                        Err(anyhow!(
                                            "Não deu pra listar as janelas. Tente de novo."
                                        ))
                                    })
                                    .map_err(|e| e.to_string());
                                    let (result, audio_sources) = match result {
                                        Ok((sources, audio_sources)) => {
                                            (Ok(sources), audio_sources)
                                        }
                                        Err(error) => (Err(error), Vec::new()),
                                    };
                                    let sources = result.as_ref().cloned().unwrap_or_default();
                                    events.send(Event::Sources {
                                        request,
                                        result,
                                        audio_sources,
                                    });
                                    let mut previews =
                                        futures_util::stream::iter(sources.into_iter().enumerate())
                                            .map(|(index, source)| async move {
                                                let image = if index
                                                    < crate::source_preview::MAX_PREVIEWS
                                                    && source.id.is_some()
                                                {
                                                    crate::source_preview::snapshot(source)
                                                        .await
                                                        .ok()
                                                } else {
                                                    None
                                                };
                                                (index, image)
                                            })
                                            .buffer_unordered(2);
                                    while let Some((index, image)) = previews.next().await {
                                        events.send(Event::SourcePreview {
                                            request,
                                            index,
                                            image,
                                        });
                                    }
                                }));
                            }
                            Command::StopSourcePreviews => {
                                if let Some(task) = source_task.take() {
                                    task.abort();
                                }
                            }
                            Command::Share {
                                source,
                                quality,
                                audio,
                            } => {
                                if let Some((_, tx)) = &current {
                                    let _ = tx.send(SessionCommand::Share {
                                        source,
                                        quality,
                                        audio,
                                    });
                                }
                            }
                            Command::StopSharing => {
                                if let Some((_, tx)) = &current {
                                    let _ = tx.send(SessionCommand::Stop);
                                }
                            }
                            Command::CancelShare => {
                                if let Some((_, tx)) = &current {
                                    let _ = tx.send(SessionCommand::CancelShare);
                                }
                            }
                            Command::Cameras {
                                generation,
                                request,
                            } => {
                                let events = Events {
                                    generation,
                                    sender: sender.clone(),
                                };
                                tasks.spawn(async move {
                                    let result = camera::devices().await.map_err(|e| e.to_string());
                                    events.send(Event::Cameras { request, result });
                                });
                            }
                            Command::StartCamera { request, device } => {
                                if let Some((_, tx)) = &current {
                                    let _ =
                                        tx.send(SessionCommand::StartCamera { request, device });
                                }
                            }
                            Command::StopCamera { request } => {
                                if let Some((_, tx)) = &current {
                                    let _ = tx.send(SessionCommand::StopCamera { request });
                                }
                            }
                        }
                    }
                    if let Some((stop, _)) = current {
                        let _ = stop.send(true);
                    }
                    if let Some(task) = source_task {
                        task.abort();
                    }
                    let _ = tokio::time::timeout(Duration::from_secs(3), async {
                        while tasks.join_next().await.is_some() {}
                    })
                    .await;
                    feedback_task.abort();
                    account_task.abort();
                    update_task.abort();
                });
            })
            .expect("thread de rede");
        Self {
            updates,
            commands,
            events,
            media,
            volume,
            account,
            feedback,
        }
    }
}

/// The only resolver: Discord can later provide the same room credentials.
pub async fn resolve(
    server_url: &str,
    code: &RoomCode,
    guest_name: &str,
) -> anyhow::Result<(SessionCredentials, crate::encryption::Session)> {
    let identity = telinha_shared::e2ee::Identity::new();
    let mut credentials = resolve_with_account(
        server_url,
        code,
        None,
        Some(guest_name),
        &identity.public_key(),
    )
    .await?;
    let encryption = crate::encryption::Session::new(server_url, &mut credentials, identity)?;
    Ok((credentials, encryption))
}

async fn resolve_with_account(
    server_url: &str,
    code: &RoomCode,
    account_token: Option<&str>,
    guest_name: Option<&str>,
    public_key: &str,
) -> anyhow::Result<SessionCredentials> {
    let url = reqwest::Url::parse(server_url).context("Endereço do servidor inválido.")?;
    anyhow::ensure!(
        url.scheme() == "https"
            || (url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))),
        "Use HTTPS no servidor, ou HTTP apenas em localhost."
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()?;
    let mut request = client
        .get(format!("{}/token", server_url.trim_end_matches('/')))
        .query(&[
            ("room", code.as_str()),
            ("e2ee_version", "1"),
            ("e2ee_key", public_key),
        ]);
    if let Some(token) = account_token {
        request = request.bearer_auth(token);
    } else {
        let name = telinha_shared::normalize_guest_name(guest_name.unwrap_or_default())
            .map_err(|message| anyhow!(message))?;
        request = request.query(&[("guest_name", name)]);
    }
    let response = request.send().await.map_err(|_| {
        anyhow!("Não deu pra conectar ao servidor. Confira sua internet e tente de novo.")
    })?;
    if !response.status().is_success() {
        #[derive(serde::Deserialize)]
        struct ErrorResponse {
            message: String,
        }
        let message = response
            .json::<ErrorResponse>()
            .await
            .map(|e| e.message)
            .unwrap_or_else(|_| "O servidor não conseguiu abrir a sessão. Tente de novo.".into());
        return Err(anyhow!(message));
    }
    let credentials = response
        .json::<SessionCredentials>()
        .await
        .context("O servidor enviou uma resposta inválida.")?;
    anyhow::ensure!(
        credentials.room == code.as_str(),
        "O servidor respondeu com outra sessão."
    );
    Ok(credentials)
}

struct SessionRequest {
    server: String,
    code: RoomCode,
    account_token: Option<String>,
    guest_name: Option<String>,
}

fn people_in_room(room: &Room) -> Vec<crate::participants::Person> {
    crate::participants::roster(
        std::iter::once(Participant::Local(room.local_participant()))
            .chain(
                room.remote_participants()
                    .into_values()
                    .map(Participant::Remote),
            )
            .map(|peer| {
                let tracks = peer.track_publications();
                crate::participants::Peer {
                    identity: peer.identity().to_string(),
                    name: peer.name(),
                    metadata: peer.metadata(),
                    local: matches!(peer, Participant::Local(_)),
                    screen: tracks.values().any(|track| {
                        track.source() == TrackSource::Screenshare && !track.is_muted()
                    }),
                    camera: tracks
                        .values()
                        .any(|track| track.source() == TrackSource::Camera && !track.is_muted()),
                }
            }),
    )
}

async fn session(
    request: SessionRequest,
    mut shutdown: watch::Receiver<bool>,
    commands: mpsc::UnboundedReceiver<SessionCommand>,
    events: Events,
    media: MediaState,
    volume: Arc<AtomicU32>,
) {
    let connect = async {
        let identity = telinha_shared::e2ee::Identity::new();
        let mut credentials = resolve_with_account(
            &request.server,
            &request.code,
            request.account_token.as_deref(),
            request.guest_name.as_deref(),
            &identity.public_key(),
        )
        .await?;
        let encryption =
            crate::encryption::Session::new(&request.server, &mut credentials, identity)?;
        let connection = tokio::time::timeout(
            Duration::from_secs(20),
            Room::connect(&credentials.url, &credentials.token, encryption.options()),
        )
        .await
        .context("A conexão demorou demais. Tente de novo.")?
        .map_err(|_| {
            anyhow!("Não deu pra entrar na sessão. Confira a conexão com o servidor de vídeo.")
        })?;
        let receipt = credentials
            .feedback_token
            .map(|token| crate::feedback::Receipt {
                token,
                connected_at: Instant::now(),
            });
        Ok::<_, anyhow::Error>((connection, receipt, encryption))
    };
    let result = tokio::select! { _ = shutdown.changed() => return, result = connect => result };
    let ((room, room_events), receipt, mut encryption) = match result {
        Ok(connection) => connection,
        Err(error) => {
            events.send(Event::Error(error.to_string()));
            events.send(Event::Disconnected);
            return;
        }
    };
    events.send(Event::Connected(receipt));
    events.send(Event::Participants(people_in_room(&room)));
    tokio::select! {
        _ = shutdown.changed() => {},
        result = encryption.run() => {
            if result.is_err() {
                events.send(Event::Error("A troca segura de chaves foi interrompida. Entre na sessão novamente.".into()));
            }
        },
        _ = connected(&room, commands, room_events, events.clone(), media, volume) => {},
    }
    encryption.leave().await;
    let _ = tokio::time::timeout(Duration::from_secs(3), room.close()).await;
    events.send(Event::Disconnected);
}

struct RemoteTasks(HashMap<String, tokio::task::JoinHandle<()>>);
impl Drop for RemoteTasks {
    fn drop(&mut self) {
        for task in self.0.values() {
            task.abort();
        }
    }
}
struct Sharing {
    capture: Capture,
    #[cfg(target_os = "windows")]
    _audio: Option<audio::windows::ProcessAudio>,
    #[cfg(target_os = "windows")]
    audio_application: Option<audio::Application>,
}

async fn unpublish(room: &Room, sharing: &mut Option<Sharing>, events: &Events) {
    sharing.take(); // Stop capture immediately, even if signalling is unavailable.
    unpublish_kind(room, VideoKind::Screen).await;
    events.send(Event::Publishing(None));
}

pub(crate) async fn unpublish_kind(room: &Room, kind: VideoKind) {
    for (_, publication) in room.local_participant().track_publications() {
        let owned = match kind {
            VideoKind::Screen => matches!(
                publication.source(),
                TrackSource::Screenshare | TrackSource::ScreenshareAudio
            ),
            VideoKind::Camera => publication.source() == TrackSource::Camera,
        };
        if !owned {
            continue;
        }
        let _ = tokio::time::timeout(
            Duration::from_secs(4),
            room.local_participant().unpublish_track(&publication.sid()),
        )
        .await;
    }
}

struct Webcam {
    id: String,
    capture: camera::Capture,
}

async fn connected(
    room: &Room,
    mut commands: mpsc::UnboundedReceiver<SessionCommand>,
    mut room_events: mpsc::UnboundedReceiver<RoomEvent>,
    events: Events,
    media: MediaState,
    volume: Arc<AtomicU32>,
) {
    let mut tasks = RemoteTasks(HashMap::new());
    let mut sharing: Option<Sharing> = None;
    let mut share_request = 0u64;
    let mut preparing_screen = tokio::task::JoinSet::new();
    let (_, mut capture_faults) = mpsc::unbounded_channel();
    let (_, mut audio_errors) = mpsc::unbounded_channel::<String>();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut previous_frames = 0;
    let mut webcam: Option<Webcam> = None;
    let mut camera_request = 0;
    let mut preparing_camera = tokio::task::JoinSet::new();
    let (camera_faults, mut camera_errors) = mpsc::unbounded_channel();
    let mut camera_tick = tokio::time::interval(Duration::from_millis(67));
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(SessionCommand::Stop) => {
                    share_request = share_request.wrapping_add(1);
                    preparing_screen.abort_all();
                    unpublish(room, &mut sharing, &events).await;
                },
                Some(SessionCommand::CancelShare) => {
                    share_request = share_request.wrapping_add(1);
                    preparing_screen.abort_all();
                },
                Some(SessionCommand::Share { source, quality, audio }) => {
                    share_request = share_request.wrapping_add(1);
                    let request = share_request;
                    preparing_screen.abort_all();
                    let (faults, receiver) = mpsc::unbounded_channel();
                    preparing_screen.spawn(async move {
                        let capture = Capture::start(source.clone(), quality, faults).await;
                        (request, source, quality, audio, capture, receiver)
                    });
                }
                Some(SessionCommand::StartCamera { request, device }) => {
                    camera_request = request;
                    preparing_camera.abort_all();
                    webcam.take();
                    unpublish_kind(room, VideoKind::Camera).await;
                    let faults = camera_faults.clone();
                    preparing_camera.spawn(async move {
                        let result = camera::Capture::start(device.clone(), request, faults).await;
                        (request, device, result)
                    });
                }
                Some(SessionCommand::StopCamera { request }) => {
                    camera_request = request;
                    preparing_camera.abort_all();
                    webcam.take();
                    unpublish_kind(room, VideoKind::Camera).await;
                    events.send(Event::CameraPublishing { request, stream: None });
                }
                None => break,
            },
            Some(result) = preparing_screen.join_next(), if !preparing_screen.is_empty() => {
                if let Ok((request, source, quality, audio, capture, receiver)) = result && request == share_request {
                    match capture {
                        Ok(mut capture) => {
                            if let Some(current) = &mut sharing {
                                capture.replace(&current.capture);
                                current.capture = capture;
                                capture_faults = receiver;
                                previous_frames = 0;
                                events.send(Event::Publishing(Some(source.name)));
                                #[cfg(target_os = "windows")]
                                if current.audio_application != audio {
                                    current._audio.take();
                                    unpublish_audio(room).await;
                                    let (faults, receiver) = mpsc::unbounded_channel();
                                    audio_errors = receiver;
                                    current._audio = publish_audio(room, audio.clone(), faults, &events).await;
                                    current.audio_application = audio;
                                }
                            } else {
                                let (audio_faults, audio_receiver) = mpsc::unbounded_channel();
                                match publish(room, capture, source.clone(), quality, audio, audio_faults, &events).await {
                                    Ok(value) => {
                                        sharing = Some(value);
                                        capture_faults = receiver;
                                        audio_errors = audio_receiver;
                                        previous_frames = 0;
                                        events.send(Event::Publishing(Some(source.name)));
                                    }
                                    Err(error) => {
                                        unpublish(room, &mut sharing, &events).await;
                                        events.send(Event::Error(error.to_string()));
                                    }
                                }
                            }
                        }
                        Err(error) => events.send(Event::Error(error.to_string())),
                    }
                }
            },
            Some(result) = preparing_camera.join_next(), if !preparing_camera.is_empty() => {
                if let Ok((request, device, result)) = result && request == camera_request {
                    match result {
                        Ok(capture) => {
                            match publish_camera(room, &capture).await {
                                Ok(()) => {
                                    let id = format!("local-camera-{}-{request}", events.generation);
                                    events.send(Event::CameraPublishing { request, stream: Some((id.clone(), device.name)) });
                                    webcam = Some(Webcam { id, capture });
                                }
                                Err(error) => {
                                    drop(capture);
                                    unpublish_kind(room, VideoKind::Camera).await;
                                    events.send(Event::CameraError { request, message: error.to_string() });
                                }
                            }
                        }
                        Err(error) => events.send(Event::CameraError { request, message: error.to_string() }),
                    }
                }
            },
            Some((request, message)) = camera_errors.recv() => if request == camera_request {
                preparing_camera.abort_all();
                webcam.take();
                unpublish_kind(room, VideoKind::Camera).await;
                events.send(Event::CameraError { request, message });
            },
            _ = camera_tick.tick(), if webcam.is_some() => {
                if let Some(webcam) = &webcam && let Some(frame) = webcam.capture.preview.lock().take() {
                    media.cameras.lock().insert(webcam.id.clone(), Pixels { track: webcam.id.clone(), width: frame.width, height: frame.height, bgra: frame.bgra });
                }
            },
            fault = capture_faults.recv(), if sharing.is_some() => {
                unpublish(room, &mut sharing, &events).await;
                events.send(Event::Error(fault.unwrap_or_else(|| "A captura terminou. Escolha a janela de novo para continuar.".into())));
            },
            Some(fault) = audio_errors.recv(), if sharing.is_some() => {
                #[cfg(target_os = "windows")]
                if let Some(sharing) = &mut sharing { sharing._audio.take(); }
                for (_, publication) in room.local_participant().track_publications() {
                    if publication.source() == TrackSource::ScreenshareAudio {
                        let _ = tokio::time::timeout(Duration::from_secs(4), room.local_participant().unpublish_track(&publication.sid())).await;
                    }
                }
                events.send(Event::Notice(format!("O vídeo continua sem som. {fault}")));
            },
            _ = tick.tick() => if let Some(sharing) = &sharing {
                let frames = sharing.capture.frames.load(Ordering::Relaxed);
                events.send(Event::CaptureFps(frames.saturating_sub(previous_frames))); previous_frames = frames;
            },
            event = room_events.recv() => {
                // Publication events describe actual starts/stops. Subscription
                // changes also happen on reconnect and must not repeat the cue.
                let sound = match &event {
                    Some(RoomEvent::TrackPublished { publication, .. })
                        if publication.source() == TrackSource::Screenshare => Some(crate::sounds::Cue::Started),
                    Some(RoomEvent::TrackUnpublished { publication, .. })
                        if publication.source() == TrackSource::Screenshare => Some(crate::sounds::Cue::Stopped),
                    _ => None,
                };
                if let Some(sound) = sound { events.send(Event::StreamSound(sound)); }
                match event {
                    Some(RoomEvent::TrackSubscribed { track, publication, participant }) => {
                        if publication.encryption_type() != livekit::e2ee::EncryptionType::Gcm {
                            events.send(Event::Error("Uma transmissão sem criptografia foi bloqueada. Todos precisam atualizar o Telinha.".into()));
                            break;
                        }
                        let sid = publication.sid().to_string();
                        if let Some(previous) = tasks.0.remove(&sid) { previous.abort(); }
                        match track {
                            RemoteTrack::Video(track) => {
                                let kind = if publication.source() == TrackSource::Camera { VideoKind::Camera } else { VideoKind::Screen };
                                let title = if participant.name().is_empty() { publication.name() } else if kind == VideoKind::Camera { participant.name() } else { format!("{} · {}", participant.name(), publication.name()) };
                                let avatar_url = serde_json::from_str::<telinha_shared::UserProfile>(&participant.metadata()).ok().map(|user| user.avatar_url).filter(|url| crate::account::avatar_url_is_valid(url));
                                events.send(Event::StreamAdded(RemoteStream { id: sid.clone(), title, participant: participant.identity().to_string(), avatar_url, kind }));
                                let media = media.clone(); let stream_id = sid.clone();
                                tasks.0.insert(sid, tokio::spawn(async move {
                                    let mut stream = NativeVideoStream::new(track.rtc_track());
                                    let mut last_camera = Instant::now() - Duration::from_secs(1);
                                    while let Some(frame) = stream.next().await {
                                        if kind == VideoKind::Screen && media.selected_track.lock().as_deref() != Some(&stream_id) { continue; }
                                        if kind == VideoKind::Camera && last_camera.elapsed() < Duration::from_millis(66) { continue; }
                                        let mut buffer = frame.buffer.to_i420();
                                        let (width,height) = if kind == VideoKind::Camera { camera::fit(frame.buffer.width(),frame.buffer.height(),640,480) } else { Quality::FullHd30.fit(frame.buffer.width(),frame.buffer.height()) };
                                        if (width,height) != (frame.buffer.width(), frame.buffer.height()) { buffer = buffer.scale(width as i32,height as i32); }
                                        let mut bgra = vec![0; width as usize * height as usize * 4];
                                        buffer.to_argb(VideoFormatType::ARGB, &mut bgra, width * 4, width as i32, height as i32);
                                        let pixels = Pixels { track: stream_id.clone(), width, height, bgra };
                                        if kind == VideoKind::Camera {
                                            last_camera = Instant::now();
                                            media.cameras.lock().insert(stream_id.clone(), pixels);
                                        } else { *media.latest.lock() = Some(pixels); }
                                    }
                                }));
                            }
                            RemoteTrack::Audio(track) => {
                                let events = events.clone(); let volume = volume.clone();
                                let selected = media.selected_participant.clone(); let participant = participant.identity().to_string();
                                tasks.0.insert(sid, tokio::spawn(async move {
                                    if let Err(error) = audio::play(track, volume, selected, participant).await { events.send(Event::Notice(error.to_string())); }
                                }));
                            }
                        }
                    }
                    Some(RoomEvent::TrackUnsubscribed { publication, .. }) | Some(RoomEvent::TrackUnpublished { publication, .. }) => {
                        let sid = publication.sid().to_string();
                        if let Some(task) = tasks.0.remove(&sid) { task.abort(); }
                        media.cameras.lock().remove(&sid);
                        let mut latest = media.latest.lock();
                        if latest.as_ref().is_some_and(|frame| frame.track == sid) { latest.take(); }
                        drop(latest);
                        events.send(Event::StreamRemoved(sid));
                        events.send(Event::Participants(people_in_room(room)));
                    }
                    Some(RoomEvent::ParticipantConnected(_)) | Some(RoomEvent::ParticipantDisconnected(_))
                    | Some(RoomEvent::ParticipantNameChanged { .. }) | Some(RoomEvent::ParticipantMetadataChanged { .. })
                    | Some(RoomEvent::TrackPublished { .. }) | Some(RoomEvent::LocalTrackPublished { .. })
                    | Some(RoomEvent::LocalTrackUnpublished { .. }) | Some(RoomEvent::LocalTrackRepublished { .. })
                    | Some(RoomEvent::TrackMuted { .. }) | Some(RoomEvent::TrackUnmuted { .. }) => events.send(Event::Participants(people_in_room(room))),
                    Some(RoomEvent::Reconnecting) => events.send(Event::Reconnecting(true)),
                    Some(RoomEvent::Reconnected) => {
                        events.send(Event::Participants(people_in_room(room)));
                        events.send(Event::Reconnecting(false));
                    },
                    Some(RoomEvent::TrackSubscriptionFailed { .. }) => events.send(Event::Notice("Não deu pra receber uma transmissão. Tente sair e entrar de novo.".into())),
                    Some(RoomEvent::Disconnected { .. }) | None => { events.send(Event::Error("A sessão foi desconectada. Tente entrar de novo.".into())); break; },
                    _ => {},
                }
            }
        }
    }
}

async fn publish_camera(room: &Room, capture: &camera::Capture) -> anyhow::Result<()> {
    let track = LocalVideoTrack::create_video_track(
        "Webcam",
        RtcVideoSource::Native(capture.source.clone()),
    );
    tokio::time::timeout(
        Duration::from_secs(12),
        room.local_participant().publish_track(
            LocalTrack::Video(track),
            TrackPublishOptions {
                source: TrackSource::Camera,
                simulcast: false,
                video_codec: if cfg!(target_os = "macos") {
                    VideoCodec::H264
                } else {
                    VideoCodec::VP8
                },
                video_encoding: Some(VideoEncoding {
                    max_bitrate: 1_200_000,
                    max_framerate: 30.,
                }),
                ..Default::default()
            },
        ),
    )
    .await
    .context("A conexão demorou para publicar a webcam. Tente novamente.")?
    .context("Não deu pra publicar a webcam. Confira a conexão e tente novamente.")?;
    Ok(())
}

async fn publish(
    room: &Room,
    capture: Capture,
    source: Source,
    quality: Quality,
    audio_application: Option<audio::Application>,
    audio_faults: mpsc::UnboundedSender<String>,
    events: &Events,
) -> anyhow::Result<Sharing> {
    let track = LocalVideoTrack::create_video_track(
        &source.name,
        RtcVideoSource::Native(capture.source.clone()),
    );
    room.local_participant()
        .publish_track(
            LocalTrack::Video(track),
            TrackPublishOptions {
                source: TrackSource::Screenshare,
                simulcast: false,
                // H.264 on macOS uses VideoToolbox. VP8 is the conservative CPU baseline
                // on Windows; the spike can override this without changing capture.
                video_codec: match std::env::var("TELINHA_CODEC").as_deref() {
                    Ok("h264") => VideoCodec::H264,
                    Ok("vp9") => VideoCodec::VP9,
                    Ok("av1") => VideoCodec::AV1,
                    _ if cfg!(target_os = "macos") => VideoCodec::H264,
                    _ => VideoCodec::VP8,
                },
                video_encoding: Some(VideoEncoding {
                    max_bitrate: quality.bitrate(),
                    max_framerate: quality.fps() as f64,
                }),
                ..Default::default()
            },
        )
        .await
        .context("Não deu pra publicar a tela. Tente de novo.")?;
    #[cfg(target_os = "windows")]
    let audio = publish_audio(room, audio_application.clone(), audio_faults, events).await;
    #[cfg(not(target_os = "windows"))]
    let _ = (audio_application, audio_faults, events);
    Ok(Sharing {
        capture,
        #[cfg(target_os = "windows")]
        _audio: audio,
        #[cfg(target_os = "windows")]
        audio_application,
    })
}

#[cfg(target_os = "windows")]
async fn unpublish_audio(room: &Room) {
    for (_, publication) in room.local_participant().track_publications() {
        if publication.source() == TrackSource::ScreenshareAudio {
            let _ = tokio::time::timeout(
                Duration::from_secs(4),
                room.local_participant().unpublish_track(&publication.sid()),
            )
            .await;
        }
    }
}

#[cfg(target_os = "windows")]
async fn publish_audio(
    room: &Room,
    audio_application: Option<audio::Application>,
    audio_faults: mpsc::UnboundedSender<String>,
    events: &Events,
) -> Option<audio::windows::ProcessAudio> {
    if let Some(application) = audio_application {
        let result = async {
            let capture = audio::windows::ProcessAudio::start(application, audio_faults).await?;
            let track = livekit::track::LocalAudioTrack::create_audio_track(
                "Áudio do aplicativo",
                livekit::webrtc::audio_source::RtcAudioSource::Native(capture.source.clone()),
            );
            tokio::time::timeout(
                Duration::from_secs(12),
                room.local_participant().publish_track(
                    LocalTrack::Audio(track),
                    TrackPublishOptions {
                        source: TrackSource::ScreenshareAudio,
                        ..Default::default()
                    },
                ),
            )
            .await
            .context("A conexão demorou para publicar o som do programa.")?
            .context("Não deu pra publicar o som do programa.")?;
            Ok::<_, anyhow::Error>(capture)
        }
        .await;
        match result {
            Ok(capture) => Some(capture),
            Err(error) => {
                events.send(Event::Notice(format!("O vídeo continua sem som. {error} O áudio por programa requer Windows build 20348 ou posterior.")));
                None
            }
        }
    } else {
        None
    }
}
