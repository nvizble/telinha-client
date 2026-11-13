use crate::{
    capture::Source,
    icons::icon,
    session::{Backend, Command, Event, RemoteStream, VideoKind},
    sounds::{self, Cue},
};
use gpui::{div, img, prelude::*, px, rgb, svg, *};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
use telinha_shared::{CODE_ALPHABET, CallStatus, CallUnavailable, DiscordCall, Quality, RoomCode};

mod audio_selection;
mod cameras;
mod feedback;
mod guest;
mod home;
mod people;
mod picker;
mod session_view;

const BG: u32 = 0x2A2C30;
const PANEL: u32 = 0x34373C;
const SUNK: u32 = 0x1D1F22;
const LINE: u32 = 0x4A4D54;
const FG: u32 = 0xF2F2F0;
const MUTED: u32 = 0xA9ACB1;
const WARN: u32 = 0xE8B339;
const LIVE: u32 = 0xF0443A;
const DISCORD: u32 = 0x5865F2;

fn audio_application_key(
    application: &crate::audio::Application,
) -> audio_selection::ApplicationKey {
    audio_selection::ApplicationKey {
        window_id: application.window_id,
        process_id: application.process_id,
        created_at: application.created_at,
    }
}

#[derive(PartialEq)]
enum Connection {
    Home,
    Connecting,
    Connected,
}

pub struct Telinha {
    backend: Backend,
    update_state: crate::updates::State,
    update_installing: bool,
    generation: u64,
    connection: Connection,
    focus: FocusHandle,
    input: String,
    cursor: usize,
    select_all: bool,
    guest_name: Option<String>,
    guest_prompt: Option<guest::GuestPrompt>,
    feedback: crate::feedback::State,
    feedback_dialog: Option<Entity<feedback::FeedbackDialog>>,
    feedback_subscription: Option<Subscription>,
    feedback_open: bool,
    room: Option<RoomCode>,
    error: Option<String>,
    notice: Option<(String, Instant)>,
    reconnecting: bool,
    participants: Vec<crate::participants::Person>,
    participants_open: bool,
    streams: Vec<RemoteStream>,
    cameras: Vec<RemoteStream>,
    camera_images: HashMap<String, Arc<RenderImage>>,
    camera_devices: Vec<crate::camera::Device>,
    camera_picker: bool,
    camera_loading: bool,
    camera_starting: bool,
    camera_request: u64,
    camera_error: Option<String>,
    camera_selected: Option<usize>,
    camera_publishing: Option<(String, String)>,
    image: Option<Arc<RenderImage>>,
    picker: bool,
    sources: Vec<Source>,
    selected_source: Option<usize>,
    loading_sources: bool,
    refreshing_sources: bool,
    source_request: u64,
    source_previews: HashMap<usize, Arc<RenderImage>>,
    source_preview_done: HashSet<usize>,
    quality: Quality,
    audio_sources: Vec<crate::audio::Application>,
    audio_selection: audio_selection::Selection,
    sharing_audio: audio_selection::Selection,
    audio_apps_open: bool,
    audio_selection_warning: Option<&'static str>,
    publishing: Option<String>,
    starting: bool,
    fps: u64,
    volume: u8,
    previous_volume: u8,
    volume_bounds: Rc<Cell<Bounds<Pixels>>>,
    dragging_volume: bool,
    fullscreen: bool,
    controls_visible: bool,
    mouse_at: Instant,
    link_input: async_channel::Receiver<String>,
    account: Option<telinha_shared::UserProfile>,
    account_menu_open: bool,
    auth_generation: u64,
    auth_busy: bool,
    auth_error: Option<String>,
    pending_link: Option<String>,
    avatars: HashMap<String, Arc<RenderImage>>,
    avatar_requests: HashSet<String>,
    call_status: Option<CallStatus>,
    call_error: Option<String>,
    call_inflight: bool,
    next_call_check: Instant,
    auto_join: crate::discovery::AutoJoin,
}

impl Telinha {
    pub fn new(
        backend: Backend,
        link_input: async_channel::Receiver<String>,
        initial: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut app = Self {
            backend,
            update_state: crate::updates::State::Idle,
            update_installing: false,
            generation: 0,
            connection: Connection::Home,
            focus: cx.focus_handle(),
            input: String::new(),
            cursor: 0,
            select_all: false,
            guest_name: crate::guest::load_name(),
            guest_prompt: None,
            feedback: crate::feedback::State::default(),
            feedback_dialog: None,
            feedback_subscription: None,
            feedback_open: false,
            room: None,
            error: std::env::var("TELINHA_UPDATE_ERROR").ok(),
            notice: None,
            reconnecting: false,
            participants: vec![],
            participants_open: false,
            streams: vec![],
            cameras: vec![],
            camera_images: HashMap::new(),
            camera_devices: vec![],
            camera_picker: false,
            camera_loading: false,
            camera_starting: false,
            camera_request: 0,
            camera_error: None,
            camera_selected: None,
            camera_publishing: None,
            image: None,
            picker: false,
            sources: vec![],
            selected_source: None,
            loading_sources: false,
            refreshing_sources: false,
            source_request: 0,
            source_previews: HashMap::new(),
            source_preview_done: HashSet::new(),
            quality: Quality::FullHd30,
            audio_sources: vec![],
            sharing_audio: audio_selection::Selection::Off,
            audio_selection: if cfg!(target_os = "windows") {
                audio_selection::Selection::FollowWindow
            } else {
                audio_selection::Selection::Off
            },
            audio_apps_open: false,
            audio_selection_warning: None,
            publishing: None,
            starting: false,
            fps: 0,
            volume: 75,
            previous_volume: 75,
            volume_bounds: Rc::new(Cell::new(Bounds::default())),
            dragging_volume: false,
            fullscreen: false,
            controls_visible: true,
            mouse_at: Instant::now(),
            link_input,
            account: None,
            account_menu_open: false,
            auth_generation: 1,
            auth_busy: true,
            auth_error: None,
            pending_link: initial,
            avatars: HashMap::new(),
            avatar_requests: HashSet::new(),
            call_status: None,
            call_error: None,
            call_inflight: false,
            next_call_check: Instant::now(),
            auto_join: crate::discovery::AutoJoin::default(),
        };
        app.restore_account(cx);
        // A single latest-frame slot gives bounded memory. No frames queue up
        // behind GPUI; the rendering cadence does not block Tokio or capture.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        app
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        while let Ok(reply) = self.backend.feedback.events.try_recv() {
            if let Some(dialog) = &self.feedback_dialog
                && dialog.read(cx).id() == reply.id
            {
                dialog.update(cx, |dialog, cx| dialog.completed(reply.result, cx));
            }
        }
        let status = self.backend.updates.lock().clone();
        if self.update_state != status {
            self.update_state = status;
            changed = true;
        }
        while let Ok(envelope) = self.backend.account.events.try_recv() {
            if envelope
                .generation
                .is_some_and(|generation| generation != self.auth_generation)
            {
                continue;
            }
            changed = true;
            match envelope.event {
                crate::account::Event::Browser(url) => cx.open_url(&url),
                crate::account::Event::Ready { user, save_token } => {
                    self.call_inflight = false;
                    self.next_call_check = Instant::now();
                    self.auth_error = None;
                    self.request_avatar(&user.avatar_url);
                    self.account = Some(user.clone());
                    if let Some(token) = save_token {
                        let save = cx.write_credentials(
                            &self.backend.account.credential_key,
                            &user.id,
                            token.as_bytes(),
                        );
                        let generation = self.auth_generation;
                        cx.spawn(async move |this, cx| {
                            let result = save.await;
                            let _ = this.update(cx, |this, cx| {
                                if this.auth_generation != generation { return; }
                                this.auth_busy = false;
                                if result.is_err() { this.auth_error = Some("Você entrou, mas não foi possível salvar a conta neste computador.".into()); }
                                cx.notify();
                            });
                        }).detach();
                    } else {
                        self.auth_busy = false;
                    }
                }
                crate::account::Event::SignedOut { clear_saved } => {
                    self.call_status = None;
                    self.call_error = None;
                    self.call_inflight = false;
                    self.auto_join = crate::discovery::AutoJoin::default();
                    self.account = None;
                    self.account_menu_open = false;
                    self.auth_busy = false;
                    if clear_saved {
                        self.auth_busy = true;
                        let deletion = cx.delete_credentials(&self.backend.account.credential_key);
                        let generation = self.auth_generation;
                        cx.spawn(async move |this, cx| {
                            let result = deletion.await;
                            let _ = this.update(cx, |this, cx| {
                                if this.auth_generation != generation { return; }
                                this.auth_busy = false;
                                if result.as_ref().is_err_and(|error| !crate::account::credentials_missing(error)) { this.auth_error = Some("A sessão foi encerrada, mas o sistema não conseguiu apagar a credencial salva.".into()); }
                                cx.notify();
                            });
                        }).detach();
                    }
                }
                crate::account::Event::Error(message) => {
                    self.auth_busy = false;
                    self.auth_error = Some(message);
                }
                crate::account::Event::Call(status) => {
                    let calls = match &status {
                        CallStatus::InCall { call } => std::slice::from_ref(call),
                        CallStatus::Multiple { calls } => calls.as_slice(),
                        _ => &[],
                    };
                    for call in calls {
                        for peer in &call.participants {
                            self.request_avatar(&peer.avatar_url);
                        }
                    }
                    self.call_status = Some(status);
                    self.call_error = None;
                    self.call_inflight = false;
                    self.next_call_check = Instant::now() + Duration::from_secs(3);
                }
                crate::account::Event::CallError { message, retry_in } => {
                    self.call_status = None;
                    self.call_error = Some(message);
                    self.call_inflight = false;
                    self.next_call_check = Instant::now() + Duration::from_secs(retry_in);
                }
                crate::account::Event::Avatar {
                    url,
                    width,
                    height,
                    bgra,
                } => {
                    if let Some(pixels) = image::RgbaImage::from_raw(width, height, bgra) {
                        self.avatars
                            .insert(url, Arc::new(RenderImage::new([image::Frame::new(pixels)])));
                    }
                }
            }
        }
        if !self.auth_busy
            && self.account.is_some()
            && let Some(prompt) = self.guest_prompt.take()
        {
            self.start_room(prompt.code, None, cx);
            changed = true;
        }
        if !self.auth_busy
            && let Some(link) = self.pending_link.take()
        {
            self.open_link(&link, cx);
            changed = true;
        }
        while let Ok(link) = self.link_input.try_recv() {
            self.open_link(&link, cx);
            changed = true;
        }
        while let Ok(envelope) = self.backend.events.try_recv() {
            if envelope.generation != self.generation {
                if let Event::Connected(receipt) = envelope.event {
                    self.feedback.late_connected(envelope.generation, receipt);
                    changed = true;
                }
                continue;
            }
            changed = true;
            match envelope.event {
                Event::Connected(receipt) => {
                    let label = self
                        .auto_join
                        .active
                        .as_ref()
                        .filter(|call| {
                            self.room
                                .as_ref()
                                .is_some_and(|room| room.as_str() == call.room)
                        })
                        .map(|call| format!("#{} · {}", call.channel_name, call.guild_name))
                        .unwrap_or_else(|| "Sessão por link".into());
                    self.feedback.connected(self.generation, receipt, label);
                    self.connection = Connection::Connected;
                    self.error = None;
                }
                Event::Disconnected => {
                    self.feedback.end(
                        self.generation,
                        crate::feedback::Reason::Disconnected,
                        Instant::now(),
                    );
                    self.participants.clear();
                    self.participants_open = false;
                    self.connection = Connection::Home;
                    self.set_publishing(None);
                    self.starting = false;
                    self.close_source_picker(cx);
                    self.reconnecting = false;
                    self.streams.clear();
                    self.reset_cameras(cx);
                    self.select_stream(None, cx);
                }
                Event::Error(message) => {
                    self.error = Some(message);
                    self.starting = false;
                    self.loading_sources = false;
                }
                Event::Notice(message) => self.toast(message),
                Event::Reconnecting(value) => self.reconnecting = value,
                Event::Participants(people) => {
                    for person in &people {
                        if let Some(profile) = &person.profile {
                            self.request_avatar(&profile.avatar_url);
                        }
                    }
                    self.participants = people;
                }
                Event::Sources {
                    request,
                    result,
                    audio_sources,
                } => {
                    if request != self.source_request || !self.picker {
                        continue;
                    }
                    self.loading_sources = false;
                    match result {
                        Ok(sources) => {
                            let previous = self
                                .selected_source
                                .and_then(|index| self.sources.get(index))
                                .map(|source| (source.id, source.kind));
                            self.sources = sources;
                            self.selected_source = previous
                                .and_then(|(id, kind)| {
                                    self.sources
                                        .iter()
                                        .position(|source| source.id == id && source.kind == kind)
                                })
                                .or_else(|| {
                                    (!self.refreshing_sources && !self.sources.is_empty())
                                        .then_some(0)
                                });
                            self.audio_sources = audio_sources;
                            self.update_audio_for_source();
                        }
                        Err(message) => {
                            self.error = Some(message);
                            self.sources.clear();
                            self.selected_source = None;
                            self.audio_sources.clear();
                        }
                    }
                    if self
                        .audio_selection
                        .refresh(self.audio_sources.iter().map(audio_application_key))
                    {
                        self.audio_selection_warning = Some(
                            "O aplicativo de áudio não está mais disponível. Escolha outro ou selecione Sem áudio.",
                        );
                        self.audio_apps_open = true;
                    }
                }
                Event::SourcePreview {
                    request,
                    index,
                    image,
                } => {
                    if request != self.source_request
                        || !self.picker
                        || self.starting
                        || index >= self.sources.len()
                    {
                        continue;
                    }
                    self.source_preview_done.insert(index);
                    if let Some(image) = image {
                        let new = Arc::new(RenderImage::new([image::Frame::new(image)]));
                        if let Some(old) = self.source_previews.insert(index, new) {
                            cx.drop_image(old, None);
                        }
                    }
                }
                Event::StreamAdded(stream) => {
                    if let Some(url) = &stream.avatar_url {
                        self.request_avatar(url);
                    }
                    let id = stream.id.clone();
                    if stream.kind == VideoKind::Camera {
                        self.cameras.retain(|s| s.id != id);
                        self.cameras.push(stream);
                        continue;
                    }
                    self.streams.retain(|s| s.id != id);
                    self.streams.push(stream);
                    if self.backend.media.selected_track.lock().is_none() {
                        self.select_stream(Some(id), cx);
                    }
                }
                Event::StreamRemoved(id) => {
                    self.cameras.retain(|s| s.id != id);
                    self.drop_camera_image(&id, cx);
                    let selected = self.backend.media.selected_track.lock().as_deref() == Some(&id);
                    if let Some(removed) = self.streams.iter().find(|s| s.id == id) {
                        self.toast(format!("{} parou de transmitir.", removed.title));
                    }
                    self.streams.retain(|s| s.id != id);
                    if selected {
                        self.select_stream(self.streams.first().map(|s| s.id.clone()), cx);
                    }
                }
                Event::StreamSound(cue) => {
                    if !self.reconnecting {
                        sounds::play(cue, self.backend.volume.clone());
                    }
                }
                Event::Publishing(name) => {
                    if name.is_some() {
                        self.sharing_audio = self.audio_selection;
                    }
                    if name.is_some() {
                        self.close_source_picker(cx);
                        self.starting = false;
                    }
                    self.set_publishing(name);
                }
                Event::CaptureFps(fps) => self.fps = fps,
                Event::Cameras { request, result } => {
                    if request != self.camera_request || !self.camera_picker {
                        continue;
                    }
                    self.camera_loading = false;
                    match result {
                        Ok(devices) => {
                            self.camera_selected = (!devices.is_empty()).then_some(0);
                            self.camera_devices = devices;
                        }
                        Err(message) => self.camera_error = Some(message),
                    }
                }
                Event::CameraPublishing { request, stream } => {
                    if request != self.camera_request {
                        continue;
                    }
                    self.clear_own_camera(cx);
                    self.camera_starting = false;
                    if stream.is_some() {
                        self.camera_picker = false;
                    }
                    self.camera_publishing = stream;
                }
                Event::CameraError { request, message } => {
                    if request != self.camera_request {
                        continue;
                    }
                    self.clear_own_camera(cx);
                    self.camera_starting = false;
                    self.camera_error = Some(message.clone());
                    if !self.camera_picker {
                        self.toast(message);
                    }
                }
            }
        }
        if self.account.is_some() && !self.auth_busy {
            if !self.call_inflight && Instant::now() >= self.next_call_check {
                self.call_inflight = self
                    .backend
                    .account
                    .commands
                    .send(crate::account::Command::Discover(self.auth_generation))
                    .is_ok();
                self.next_call_check = Instant::now() + Duration::from_secs(3);
            }
            if let Some(status) = &self.call_status
                && let Some(call) = self.auto_join.next(
                    status,
                    self.connection == Connection::Home,
                    self.connection == Connection::Connecting
                        || self.publishing.is_some()
                        || self.starting
                        || self.picker
                        || self.camera_publishing.is_some()
                        || self.camera_starting
                        || self.camera_picker,
                )
            {
                self.connect_call(call, cx);
                changed = true;
            }
        }
        let latest = self.backend.media.latest.lock().take();
        if let Some(frame) = latest
            && self.backend.media.selected_track.lock().as_deref() == Some(&frame.track)
            && let Some(buffer) = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)
        {
            let new = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
            if let Some(previous) = self.image.replace(new) {
                cx.drop_image(previous, None);
            }
            changed = true;
        }
        let camera_frames = std::mem::take(&mut *self.backend.media.cameras.lock());
        for (id, frame) in camera_frames {
            let known = self.cameras.iter().any(|camera| camera.id == id)
                || self
                    .camera_publishing
                    .as_ref()
                    .is_some_and(|(own, _)| own == &id);
            if known
                && let Some(buffer) =
                    image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)
            {
                let new = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
                if let Some(previous) = self.camera_images.insert(id, new) {
                    cx.drop_image(previous, None);
                }
                changed = true;
            }
        }
        if self
            .notice
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(4))
        {
            self.notice = None;
            changed = true;
        }
        if self.fullscreen
            && self.controls_visible
            && !self.dragging_volume
            && self.mouse_at.elapsed() >= Duration::from_secs(2)
        {
            self.controls_visible = false;
            changed = true;
        }
        self.present_feedback(cx);
        if changed {
            cx.notify();
        }
    }

    fn toast(&mut self, message: String) {
        self.notice = Some((message, Instant::now()));
    }
    fn open_link(&mut self, input: &str, cx: &mut Context<Self>) {
        if self.auth_busy {
            self.pending_link = Some(input.to_owned());
            return;
        }
        match RoomCode::from_input(input) {
            Ok(code) => self.connect(code, cx),
            Err(e) => {
                self.error = Some(e.to_string());
                cx.notify();
            }
        }
    }
    fn connect(&mut self, code: RoomCode, cx: &mut Context<Self>) {
        if self.auth_busy || self.connection == Connection::Connecting {
            return;
        }
        self.auto_join.manual();
        self.connect_room(code, cx);
    }

    fn connect_call(&mut self, call: DiscordCall, cx: &mut Context<Self>) {
        if self.auth_busy || self.connection == Connection::Connecting {
            return;
        }
        match call.room.parse::<RoomCode>() {
            Ok(code) => {
                self.auto_join.enter(call);
                self.connect_room(code, cx);
            }
            Err(_) => {
                self.call_error = Some("A sessão da call não é válida. Tente novamente.".into());
                cx.notify();
            }
        }
    }

    fn connect_room(&mut self, code: RoomCode, cx: &mut Context<Self>) {
        if self.auth_busy || self.connection == Connection::Connecting {
            return;
        }
        if self.account.is_none() {
            self.ask_guest_name(code, cx);
            return;
        }
        self.start_room(code, None, cx);
    }

    fn start_room(&mut self, code: RoomCode, guest_name: Option<String>, cx: &mut Context<Self>) {
        self.feedback.end(
            self.generation,
            crate::feedback::Reason::Switched,
            Instant::now(),
        );
        self.participants.clear();
        self.participants_open = false;
        self.generation += 1;
        self.guest_prompt = None;
        self.account_menu_open = false;
        self.streams.clear();
        self.reset_cameras(cx);
        self.select_stream(None, cx);
        self.input = code.as_str().to_owned();
        self.room = Some(code.clone());
        let label = self
            .auto_join
            .active
            .as_ref()
            .filter(|call| call.room == code.as_str())
            .map(|call| format!("#{} · {}", call.channel_name, call.guild_name))
            .unwrap_or_else(|| "Sessão por link".into());
        self.feedback.starting(self.generation, label);
        self.error = None;
        self.notice = None;
        self.connection = Connection::Connecting;
        self.close_source_picker(cx);
        self.set_publishing(None);
        self.starting = false;
        self.reconnecting = false;
        let _ = self.backend.commands.send(Command::Connect {
            generation: self.generation,
            code,
            guest_name,
        });
        cx.notify();
    }
    fn join(&mut self, cx: &mut Context<Self>) {
        let input = self.input.clone();
        self.open_link(&input, cx);
    }
    fn leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.feedback.end(
            self.generation,
            crate::feedback::Reason::Left,
            Instant::now(),
        );
        self.guest_prompt = None;
        self.participants.clear();
        self.participants_open = false;
        self.auto_join.leave(self.call_status.as_ref());
        self.generation += 1;
        let _ = self.backend.commands.send(Command::Leave);
        self.connection = Connection::Home;
        self.close_source_picker(cx);
        self.set_publishing(None);
        self.starting = false;
        self.error = None;
        self.streams.clear();
        self.reset_cameras(cx);
        self.reconnecting = false;
        self.select_stream(None, cx);
        if self.fullscreen {
            window.toggle_fullscreen();
            self.fullscreen = false;
        }
        cx.notify();
    }
    fn set_publishing(&mut self, name: Option<String>) {
        if self.publishing.is_some() != name.is_some() {
            let cue = if name.is_some() {
                Cue::Started
            } else {
                Cue::Stopped
            };
            sounds::play(cue, self.backend.volume.clone());
        }
        self.publishing = name;
    }

    fn select_stream(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        let participant = id.as_ref().and_then(|id| {
            self.streams
                .iter()
                .find(|s| &s.id == id)
                .map(|s| s.participant.clone())
        });
        *self.backend.media.selected_track.lock() = id;
        *self.backend.media.selected_participant.lock() = participant;
        self.backend.media.latest.lock().take();
        if let Some(image) = self.image.take() {
            cx.drop_image(image, None);
        }
        cx.notify();
    }
    fn clear_source_previews(&mut self, cx: &mut Context<Self>) {
        self.source_request += 1;
        self.source_preview_done.clear();
        for (_, image) in self.source_previews.drain() {
            cx.drop_image(image, None);
        }
        let _ = self.backend.commands.send(Command::StopSourcePreviews);
    }
    fn close_source_picker(&mut self, cx: &mut Context<Self>) {
        self.clear_source_previews(cx);
        self.picker = false;
        self.loading_sources = false;
        self.refreshing_sources = false;
        self.sources.clear();
        self.selected_source = None;
        self.audio_sources.clear();
        self.audio_selection = if cfg!(target_os = "windows") {
            audio_selection::Selection::FollowWindow
        } else {
            audio_selection::Selection::Off
        };
        self.audio_apps_open = false;
        self.audio_selection_warning = None;
    }
    fn choose_source(&mut self, cx: &mut Context<Self>) {
        if self.starting {
            return;
        }
        if self.picker {
            self.clear_source_previews(cx);
            self.refreshing_sources = true;
        } else {
            self.close_source_picker(cx);
        }
        if self.publishing.is_some() {
            self.audio_selection = self.sharing_audio;
        }
        self.camera_picker = false;
        self.picker = true;
        self.loading_sources = true;
        self.error = None;
        let _ = self.backend.commands.send(Command::Sources {
            generation: self.generation,
            request: self.source_request,
        });
        cx.notify();
    }
    fn selected_audio_application(&self) -> Option<&crate::audio::Application> {
        if !cfg!(target_os = "windows") || self.loading_sources {
            return None;
        }
        let shared_window = self
            .selected_source
            .and_then(|index| self.sources.get(index))
            .filter(|source| source.supports_audio())
            .and_then(|source| source.id);
        let key = self.audio_selection.resolve(
            shared_window,
            self.audio_sources.iter().map(audio_application_key),
        )?;
        self.audio_sources
            .iter()
            .find(|application| audio_application_key(application) == key)
    }
    fn audio_selection_ready(&self) -> bool {
        !cfg!(target_os = "windows")
            || self.audio_selection == audio_selection::Selection::Off
            || self.selected_audio_application().is_some()
    }
    fn update_audio_for_source(&mut self) {
        if cfg!(target_os = "windows")
            && self
                .selected_source
                .and_then(|i| self.sources.get(i))
                .is_some_and(|s| !s.supports_audio())
            && self.audio_selection.select_display()
        {
            self.audio_apps_open = true;
            self.audio_selection_warning = Some(
                "Ao compartilhar o monitor, escolha qual aplicativo seus amigos vão ouvir — ou selecione Sem áudio.",
            );
        }
    }
    fn share(&mut self, cx: &mut Context<Self>) {
        if self.starting || self.loading_sources || !self.audio_selection_ready() {
            return;
        }
        if let Some(source) = self
            .selected_source
            .and_then(|index| self.sources.get(index))
            .cloned()
        {
            self.starting = true;
            self.error = None;
            let _ = self.backend.commands.send(Command::StopSourcePreviews);
            let _ = self.backend.commands.send(Command::Share {
                audio: self.selected_audio_application().cloned(),
                source,
                quality: self.quality,
            });
        }
        cx.notify();
    }
    fn sign_in(&mut self, cx: &mut Context<Self>) {
        if self.auth_busy {
            return;
        }
        self.auth_generation += 1;
        self.auth_busy = true;
        self.auth_error = None;
        let _ = self
            .backend
            .account
            .commands
            .send(crate::account::Command::Begin(self.auth_generation));
        cx.notify();
    }
    fn set_volume(&mut self, value: u8, cx: &mut Context<Self>) {
        self.volume = value.min(100);
        if self.volume > 0 {
            self.previous_volume = self.volume;
        }
        self.backend
            .volume
            .store((self.volume as f32 / 100.).to_bits(), Ordering::Relaxed);
        cx.notify();
    }
    fn volume_at(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let bounds = self.volume_bounds.get();
        if bounds.size.width > px(0.) {
            let position = f32::from(x - bounds.origin.x) / f32::from(bounds.size.width);
            self.set_volume((position.clamp(0., 1.) * 100.).round() as u8, cx);
        }
    }
    fn copy(&mut self, link: bool, cx: &mut Context<Self>) {
        if let Some(code) = &self.room {
            cx.write_to_clipboard(ClipboardItem::new_string(if link {
                code.invite_url()
            } else {
                code.grouped()
            }));
            self.toast(
                if link {
                    "Link copiado. Mande pros seus amigos."
                } else {
                    "Código copiado."
                }
                .into(),
            );
            cx.notify();
        }
    }
    fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            match RoomCode::from_input(&text) {
                Ok(code) => {
                    self.input = code.as_str().to_owned();
                    self.cursor = 11;
                    self.error = None;
                }
                Err(error) => {
                    self.error = Some(error.to_string());
                }
            }
            self.select_all = false;
            cx.notify();
        }
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.guest_prompt.is_some() {
            self.guest_key(event, window, cx);
            return;
        }
        if self.feedback_open && self.feedback_dialog.is_some() {
            return;
        }
        let key = &event.keystroke;
        if key.key == "escape" {
            if self.participants_open {
                self.participants_open = false;
            } else if self.account_menu_open {
                self.account_menu_open = false;
            } else if self.fullscreen {
                window.toggle_fullscreen();
                self.fullscreen = false;
            } else if self.camera_picker {
                self.stop_camera(cx);
            } else if self.picker && !self.starting {
                self.close_source_picker(cx);
            } else if self.connection == Connection::Connecting {
                self.leave(window, cx);
            }
            cx.notify();
            return;
        }
        if self.connection != Connection::Home {
            return;
        }
        if key.modifiers.control || key.modifiers.platform {
            match key.key.as_str() {
                "v" => self.paste(cx),
                "a" => {
                    self.select_all = true;
                    cx.notify();
                }
                "n" => self.connect(RoomCode::generate(), cx),
                _ => {}
            }
            return;
        }
        match key.key.as_str() {
            "enter" => self.join(cx),
            "left" => {
                self.cursor = self.cursor.saturating_sub(1);
                self.select_all = false;
            }
            "right" => {
                self.cursor = (self.cursor + 1).min(self.input.len()).min(11);
                self.select_all = false;
            }
            "home" => self.cursor = 0,
            "end" => self.cursor = self.input.len().min(11),
            "backspace" | "delete" => {
                if self.select_all {
                    self.input.clear();
                    self.cursor = 0;
                    self.select_all = false;
                } else if key.key == "backspace" && self.cursor > 0 {
                    self.cursor -= 1;
                    if self.cursor < self.input.len() {
                        self.input.remove(self.cursor);
                    }
                } else if key.key == "delete" && self.cursor < self.input.len() {
                    self.input.remove(self.cursor);
                }
                self.error = None;
            }
            _ => {
                if let Some(character) = key
                    .key_char
                    .as_deref()
                    .filter(|s| s.len() == 1 && s.as_bytes()[0].is_ascii_alphanumeric())
                {
                    if self.select_all {
                        self.input.clear();
                        self.cursor = 0;
                        self.select_all = false;
                    }
                    let character = character.to_ascii_lowercase();
                    if self.cursor < self.input.len() {
                        self.input
                            .replace_range(self.cursor..self.cursor + 1, &character);
                    } else if self.input.len() < 12 {
                        self.input.push_str(&character);
                    }
                    self.cursor = (self.cursor + 1).min(12);
                    self.error = None;
                }
            }
        }
        cx.notify();
    }

    fn restore_account(&mut self, cx: &mut Context<Self>) {
        let read = cx.read_credentials(&self.backend.account.credential_key);
        let generation = self.auth_generation;
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |this, cx| {
                if this.auth_generation != generation {
                    return;
                }
                match result {
                    Ok(Some((_, token))) => {
                        if let Ok(token) = String::from_utf8(token) {
                            let _ = this
                                .backend
                                .account
                                .commands
                                .send(crate::account::Command::Restore(generation, token));
                        } else {
                            this.auth_busy = false;
                        }
                    }
                    Ok(None) => this.auth_busy = false,
                    Err(error) if crate::account::credentials_missing(&error) => {
                        this.auth_busy = false;
                    }
                    Err(_) => {
                        this.auth_busy = false;
                        this.auth_error = Some(
                            "Não foi possível ler a conta salva. Você pode entrar novamente."
                                .into(),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn request_avatar(&mut self, url: &str) {
        if self.avatar_requests.len() < 128 && self.avatar_requests.insert(url.to_owned()) {
            let _ = self
                .backend
                .account
                .commands
                .send(crate::account::Command::Avatar(url.to_owned()));
        }
    }

    fn avatar(&self, url: Option<&str>, name: &str, size: f32) -> AnyElement {
        let frame = div()
            .size(px(size))
            .flex_shrink_0()
            .rounded_full()
            .overflow_hidden()
            .bg(rgb(PANEL))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(size * 0.44));
        if let Some(image) = url.and_then(|url| self.avatars.get(url)) {
            frame
                .child(
                    img(image.clone())
                        .size_full()
                        .rounded_full()
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element()
        } else {
            frame
                .child(
                    name.chars()
                        .next()
                        .unwrap_or('t')
                        .to_uppercase()
                        .to_string(),
                )
                .into_any_element()
        }
    }

    fn account_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div().relative().flex().items_center().gap_3();
        if let Some(user) = &self.account {
            row = row.child(
                div()
                    .id("account-menu-trigger")
                    .h(px(36.))
                    .pl(px(4.))
                    .pr_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_full()
                    .cursor_pointer()
                    .when(self.account_menu_open, |trigger| trigger.bg(rgb(PANEL)))
                    .hover(|style| style.bg(rgb(PANEL)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.participants_open = false;
                        this.account_menu_open = !this.account_menu_open;
                        cx.notify();
                    }))
                    .child(self.avatar(Some(&user.avatar_url), &user.display_name, 28.))
                    .child(
                        div()
                            .max_w(px(136.))
                            .truncate()
                            .text_sm()
                            .child(user.display_name.clone()),
                    )
                    .child(icon("icons/chevron.svg", 12., MUTED)),
            );
            if self.account_menu_open {
                let can_logout = self.connection == Connection::Home && !self.auth_busy;
                let menu = div()
                    .id("account-menu")
                    .absolute()
                    .top(px(42.))
                    .right_0()
                    .w(px(224.))
                    .p_1()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgb(LINE))
                    .bg(rgb(SUNK))
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .mb_1()
                            .border_b_1()
                            .border_color(rgb(LINE))
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .truncate()
                            .child(format!("@{}", user.username)),
                    )
                    .child(
                        div()
                            .id("discord-logout")
                            .w_full()
                            .px_3()
                            .py_2()
                            .rounded(px(6.))
                            .text_sm()
                            .child(if self.auth_busy {
                                "Conectando sua conta…"
                            } else {
                                "Sair da conta"
                            })
                            .when(can_logout, |item| {
                                item.cursor_pointer()
                                    .hover(|style| style.bg(rgb(PANEL)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.account_menu_open = false;
                                        this.auth_generation += 1;
                                        this.auth_busy = true;
                                        this.auth_error = None;
                                        let _ = this.backend.account.commands.send(
                                            crate::account::Command::Logout(this.auth_generation),
                                        );
                                        cx.notify();
                                    }))
                            })
                            .when(!can_logout, |item| item.text_color(rgb(MUTED))),
                    )
                    .when(self.connection != Connection::Home, |menu| {
                        menu.child(
                            div()
                                .px_3()
                                .pb_2()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child("Saia da sessão para encerrar sua conta."),
                        )
                    });
                row = row.child(deferred(menu).with_priority(1));
            }
        } else if self.auth_busy {
            row = row
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("Conectando sua conta…"),
                )
                .child(
                    button("discord-cancel", "Cancelar", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.auth_generation += 1;
                            // Wait for cancellation before connecting as a guest: an
                            // authorization response may already be queued by Tokio.
                            this.auth_busy = true;
                            this.auth_error = None;
                            let _ = this
                                .backend
                                .account
                                .commands
                                .send(crate::account::Command::Cancel(this.auth_generation));
                            cx.notify();
                        },
                    )),
                );
        }
        row.into_any_element()
    }

    fn call_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let active = self.auto_join.active.as_ref()?;
        let mut destination = None;
        let message =
            match &self.call_status {
                Some(CallStatus::InCall { call }) if call.room == active.room => return None,
                Some(CallStatus::InCall { call }) => {
                    destination = Some(call.clone());
                    format!(
                        "Você mudou para # {}. Sua sessão em # {} continua aberta.",
                        call.channel_name, active.channel_name
                    )
                }
                Some(CallStatus::Waiting) => {
                    "Você saiu da call do Discord. Sua sessão continua aberta.".into()
                }
                Some(CallStatus::Multiple { .. }) => {
                    "Há mais de uma call. Sua sessão atual continua aberta.".into()
                }
                _ => {
                    return Some(banner(
                    "Não foi possível conferir a call do Discord. Sua sessão continua aberta.",
                    false,
                ).into_any_element());
                }
            };
        let mut row = div()
            .px_4()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .bg(rgb(PANEL))
            .child(div().flex_1().text_sm().child(message))
            .child(
                button("keep-discord-session", "Continuar aqui", false).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.auto_join.manual();
                        cx.notify();
                    },
                )),
            );
        if let Some(call) = destination {
            row = row.child(
                button(
                    "switch-discord-call",
                    if self.publishing.is_some() {
                        "Parar e ir para a nova call"
                    } else {
                        "Ir para a nova call"
                    },
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.connect_call(call.clone(), cx))),
            );
        } else {
            row = row.child(
                button("leave-discord-session", "Sair da sessão", false)
                    .on_click(cx.listener(|this, _, window, cx| this.leave(window, cx))),
            );
        }
        Some(row.into_any_element())
    }

    fn update_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let text = match &self.update_state {
            crate::updates::State::Idle => return None,
            crate::updates::State::Downloading { version, percent } => {
                format!("Baixando atualização {version} · {percent}%")
            }
            crate::updates::State::Ready(prepared) => {
                if self.connection == Connection::Home {
                    format!("Atualização {} pronta.", prepared.version)
                } else {
                    format!(
                        "Atualização {} pronta. Saia da sessão para instalar.",
                        prepared.version
                    )
                }
            }
        };
        let mut row = div()
            .flex_shrink_0()
            .px_6()
            .py_2()
            .bg(rgb(PANEL))
            .flex()
            .items_center()
            .gap_4()
            .child(div().flex_1().min_w_0().text_sm().child(text));
        if let crate::updates::State::Ready(_) = &self.update_state {
            let enabled = self.connection == Connection::Home && !self.update_installing;
            row = row.child(
                button_with_icon(
                    "apply-update",
                    "Reiniciar e atualizar",
                    "icons/refresh.svg",
                    true,
                )
                .when(!enabled, |button| button.opacity(0.45).cursor_default())
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.connection != Connection::Home || this.update_installing {
                        return;
                    }
                    if let crate::updates::State::Ready(prepared) = this.update_state.clone() {
                        this.update_installing = true;
                        match crate::updates::launch(&prepared) {
                            Ok(()) => cx.quit(),
                            Err(error) => {
                                this.update_installing = false;
                                this.error = Some(error.to_string());
                                cx.notify();
                            }
                        }
                    }
                })),
            );
        }
        Some(row.into_any_element())
    }

    fn code_boxes(&self, editable: bool, cx: &mut Context<Self>) -> AnyElement {
        let input = if editable {
            self.input.as_str()
        } else {
            self.room.as_ref().map_or("", RoomCode::as_str)
        };
        let mut row = div().flex().gap(px(12.));
        for group in 0..3 {
            let mut cells = div().flex().gap(px(4.));
            for n in group * 4..group * 4 + 4 {
                let value = input.as_bytes().get(n).copied();
                let invalid = value.is_some_and(|c| !CODE_ALPHABET.contains(&c));
                let active = editable && (self.cursor.min(11) == n || self.select_all);
                let cell = div()
                    .id(("code", n))
                    .w(px(if editable { 28. } else { 26. }))
                    .h(px(if editable { 40. } else { 36. }))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgb(SUNK))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(if invalid {
                        WARN
                    } else if active {
                        FG
                    } else {
                        LINE
                    }))
                    .text_size(px(if editable { 18. } else { 17. }))
                    .font_family(if cfg!(target_os = "windows") {
                        "Consolas"
                    } else {
                        "Menlo"
                    })
                    .child(value.map(|c| (c as char).to_string()).unwrap_or_default());
                cells = cells.child(if editable {
                    cell.cursor_text()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.cursor = n.min(this.input.len());
                            this.select_all = false;
                            this.focus.focus(window);
                            cx.notify();
                        }))
                } else {
                    cell
                });
            }
            row = row.child(cells);
        }
        row.into_any_element()
    }
}

impl Focusable for Telinha {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for Telinha {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_window_title(
            if self.publishing.is_some() || self.camera_publishing.is_some() {
                "telinha · no ar"
            } else {
                "telinha"
            },
        );
        let mut root = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(FG))
            .text_size(px(14.))
            .font_family(if cfg!(target_os = "windows") {
                "Segoe UI"
            } else {
                "Helvetica Neue"
            })
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.account_menu_open || this.participants_open {
                        this.account_menu_open = false;
                        this.participants_open = false;
                        cx.notify();
                    }
                }),
            )
            .capture_any_mouse_up(cx.listener(|this, _, _, _| this.dragging_volume = false))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                this.mouse_at = Instant::now();
                if this.dragging_volume {
                    if event.pressed_button == Some(MouseButton::Left) {
                        this.volume_at(event.position.x, cx);
                    } else {
                        this.dragging_volume = false;
                    }
                }
                if this.fullscreen && !this.controls_visible {
                    this.controls_visible = true;
                    cx.notify();
                }
            }));
        if !self.fullscreen {
            if self.connection == Connection::Connected {
                root = root.child(self.top(window, cx));
            } else {
                root = root.child(
                    div()
                        .h(px(60.))
                        .px_6()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            svg()
                                .path("logo.svg")
                                .w(px(82.))
                                .h(px(25.))
                                .text_color(rgb(FG)),
                        )
                        .child(div().flex_1())
                        .child(self.account_bar(cx)),
                );
            }
        }
        if self.reconnecting {
            root = root.child(banner("A conexão caiu. Reconectando…", true));
        }
        if !self.fullscreen
            && let Some(update) = self.update_banner(cx)
        {
            root = root.child(update);
        }
        if self.feedback_dialog.is_some() && !self.feedback_open && self.guest_prompt.is_none() {
            root = root.child(self.feedback_banner(cx));
        }
        if let Some(error) = &self.error {
            root = root.child(banner(error, true));
        }
        if let Some(error) = &self.auth_error {
            root = root.child(banner(error, true));
        }
        if self.guest_prompt.is_some() {
            if self.feedback_dialog.is_some() {
                window.focus(&self.focus);
            }
            root = root.child(self.guest_form(cx));
        } else {
            match self.connection {
                Connection::Home => root = root.child(self.home(window, cx)),
                Connection::Connecting => {
                    root = root.child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap_5()
                            .child(
                                div()
                                    .size(px(64.))
                                    .rounded(px(16.))
                                    .bg(rgb(SUNK))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(icon("icons/link.svg", 28., MUTED)),
                            )
                            .child(
                                div()
                                    .text_size(px(24.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Entrando na sessão…"),
                            )
                            .child(
                                div()
                                    .text_color(rgb(MUTED))
                                    .child("Preparando a conexão com seus amigos."),
                            )
                            .child(quiet_button("cancel-connect", "Cancelar").on_click(
                                cx.listener(|this, _, window, cx| this.leave(window, cx)),
                            )),
                    );
                }
                Connection::Connected => {
                    if !self.fullscreen
                        && let Some(notice) = self.call_notice(cx)
                    {
                        root = root.child(notice);
                    }
                    if self.camera_picker {
                        root = root.child(self.camera_picker(cx));
                    } else if self.picker {
                        root = root.child(self.picker(window, cx));
                    } else {
                        root = root.child(self.stage(window, cx));
                        if !self.fullscreen {
                            root = root.child(self.controls(window, cx));
                        }
                    }
                }
            }
        }
        if self.connection == Connection::Connected
            && self.publishing.is_some()
            && !self.fullscreen
            && !self.picker
            && !self.camera_picker
            && !self.reconnecting
            && self.guest_prompt.is_none()
            && self.feedback_dialog.is_none()
            && self.notice.is_none()
            && self.feedback.can_prompt_sharing(self.generation)
        {
            root = root.child(deferred(self.sharing_feedback_card(cx)).with_priority(1));
        }
        if let Some((notice, _)) = &self.notice {
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(if self.connection == Connection::Home {
                            128.
                        } else {
                            88.
                        }))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .px_4()
                                .py_3()
                                .rounded(px(10.))
                                .border_1()
                                .border_color(rgb(LINE))
                                .bg(rgb(SUNK))
                                .shadow_lg()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_sm()
                                .child(icon("icons/check.svg", 16., FG))
                                .child(notice.clone()),
                        ),
                )
                .with_priority(2),
            );
        }
        if self.feedback_open
            && self.guest_prompt.is_none()
            && let Some(dialog) = &self.feedback_dialog
        {
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .bg(rgba(0x101114CC))
                        .p_6()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(dialog.clone()),
                )
                .with_priority(5),
            );
        }
        root
    }
}

fn button_base(id: impl Into<ElementId>, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(40.))
        .flex_shrink_0()
        .px_4()
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .rounded(px(8.))
        .border_1()
        .border_color(rgb(if primary { FG } else { LINE }))
        .bg(rgb(if primary { FG } else { BG }))
        .text_color(rgb(if primary { 0x1B1C1F } else { FG }))
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
}
fn standard_button(id: impl Into<ElementId>, primary: bool) -> Stateful<Div> {
    button_base(id, primary)
        .hover(move |style| style.bg(rgb(if primary { 0xDCDDDA } else { PANEL })))
        .active(move |style| style.bg(rgb(if primary { 0xC9CBC8 } else { SUNK })))
}
fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
) -> Stateful<Div> {
    standard_button(id, primary).child(label.into())
}
fn button_with_icon(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    path: &'static str,
    primary: bool,
) -> Stateful<Div> {
    button_with_colored_icon(
        id,
        label,
        path,
        primary,
        if primary { 0x1B1C1F } else { FG },
    )
}
fn button_with_colored_icon(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    path: &'static str,
    primary: bool,
    color: u32,
) -> Stateful<Div> {
    standard_button(id, primary)
        .text_color(rgb(color))
        .child(icon(path, 16., color))
        .child(label.into())
}
fn quiet_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    button_base(id, false)
        .border_color(gpui::rgba(0))
        .bg(gpui::rgba(0))
        .text_color(rgb(MUTED))
        .hover(|style| style.bg(rgb(PANEL)).text_color(rgb(FG)))
        .active(|style| style.bg(rgb(SUNK)))
        .child(label.into())
}
fn discord_button(label: &'static str) -> Stateful<Div> {
    button_base("home-discord-login", true)
        .h(px(48.))
        .px_6()
        .text_size(px(15.))
        .bg(rgb(DISCORD))
        .border_color(rgb(DISCORD))
        .text_color(rgb(0xFFFFFF))
        .hover(|style| style.bg(rgb(0x6571F5)))
        .active(|style| style.bg(rgb(0x4752C4)))
        .child(icon("icons/arrow.svg", 16., 0xFFFFFF))
        .child(label)
}
fn heading(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(32.))
        .line_height(px(39.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}
fn mono() -> &'static str {
    if cfg!(target_os = "windows") {
        "Consolas"
    } else {
        "Menlo"
    }
}
fn live_badge() -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .h(px(30.))
        .rounded(px(6.))
        .bg(gpui::rgba(0xF0443A24))
        .text_color(rgb(LIVE))
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .child(div().size(px(6.)).rounded_full().bg(rgb(LIVE)))
        .child("NO AR")
}
fn banner(message: &str, warning: bool) -> Div {
    div()
        .mx_4()
        .mb_2()
        .px_4()
        .py_3()
        .rounded(px(6.))
        .bg(rgb(SUNK))
        .text_color(rgb(if warning { WARN } else { FG }))
        .text_sm()
        .child(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{button, button_with_icon, discord_button, quiet_button};
    use core::prelude::v1::test;

    #[test]
    fn button_variants_register_hover_and_active_styles_once() {
        // GPUI panics during construction if a composed button sets hover twice.
        // This regression must be caught before the native application is opened.
        let _ = button("primary", "Entrar", true);
        let _ = button("secondary", "Cancelar", false);
        let _ = button_with_icon("icon", "Copiar", "icons/copy.svg", false);
        let _ = quiet_button("quiet", "Sair");
        let _ = discord_button("Entrar com o Discord");
    }
}
