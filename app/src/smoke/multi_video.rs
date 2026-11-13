//! Exercise the real receiving backend and source-specific teardown without cameras.
use crate::session::{Backend, Command, Event, VideoKind, unpublish_kind};
use anyhow::{Context, ensure};
use livekit::{
    options::{TrackPublishOptions, VideoCodec},
    prelude::*,
    webrtc::{prelude::*, video_source::native::NativeVideoSource},
};
use std::{collections::HashMap, time::Duration};
use telinha_shared::RoomCode;

async fn publish(room: &Room, source: TrackSource) -> anyhow::Result<(String, NativeVideoSource)> {
    let video = NativeVideoSource::new(
        VideoResolution {
            width: 320,
            height: 180,
        },
        source != TrackSource::Camera,
    );
    let publication = room
        .local_participant()
        .publish_track(
            LocalTrack::Video(LocalVideoTrack::create_video_track(
                &format!("Mídia sintética {source:?}"),
                RtcVideoSource::Native(video.clone()),
            )),
            TrackPublishOptions {
                source,
                simulcast: false,
                video_codec: if cfg!(target_os = "macos") {
                    VideoCodec::H264
                } else {
                    VideoCodec::VP8
                },
                ..Default::default()
            },
        )
        .await
        .with_context(|| format!("Publicação sintética {source:?}"))?;
    Ok((publication.sid().to_string(), video))
}

async fn frames(backend: &Backend, screen: &str, cameras: &[&str]) -> anyhow::Result<()> {
    *backend.media.selected_track.lock() = Some(screen.to_owned());
    backend.media.latest.lock().take();
    backend.media.cameras.lock().clear();
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let screen_ok = backend
                .media
                .latest
                .lock()
                .as_ref()
                .is_some_and(|frame| frame.track == screen && frame.bgra[2] > 180);
            let cameras_ok = cameras.iter().all(|id| {
                backend
                    .media
                    .cameras
                    .lock()
                    .get(*id)
                    .is_some_and(|frame| frame.bgra[1] > 120 && frame.width == 320)
            });
            if screen_ok && cameras_ok {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .context("Tela e webcams não chegaram juntas ao backend.")?;
    Ok(())
}

pub(super) async fn check(server: &str, options: RoomOptions) -> anyhow::Result<()> {
    let code = RoomCode::generate();
    let (a, mut a_crypto) = crate::session::resolve(server, &code, "Teste de telas A").await?;
    let (b, mut b_crypto) = crate::session::resolve(server, &code, "Teste de telas B").await?;
    let mut a_options = a_crypto.options();
    a_options.rtc_config = options.rtc_config.clone();
    let mut b_options = b_crypto.options();
    b_options.rtc_config = options.rtc_config.clone();
    let (a, _) = Room::connect(&a.url, &a.token, a_options).await?;
    let (b, _) = Room::connect(&b.url, &b.token, b_options).await?;
    let mut key_workers = tokio::task::JoinSet::new();
    key_workers.spawn(async move { a_crypto.run().await });
    key_workers.spawn(async move { b_crypto.run().await });
    let (screen_a, video_a) = publish(&a, TrackSource::Screenshare).await?;
    let (camera_a, cam_a) = publish(&a, TrackSource::Camera).await?;
    let (screen_b, video_b) = publish(&b, TrackSource::Screenshare).await?;
    let (camera_b, cam_b) = publish(&b, TrackSource::Camera).await?;
    let mut feed = tokio::task::JoinSet::new();
    feed.spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(33));
        loop {
            tick.tick().await;
            for (source, camera) in [
                (&video_a, false),
                (&cam_a, true),
                (&video_b, false),
                (&cam_b, true),
            ] {
                let mut buffer = I420Buffer::new(320, 180);
                let (y, u, v) = buffer.data_mut();
                y.fill(if camera { 145 } else { 82 });
                u.fill(if camera { 54 } else { 90 });
                v.fill(if camera { 34 } else { 240 });
                source.capture_frame(&VideoFrame::new(VideoRotation::VideoRotation0, buffer));
            }
        }
    });
    let backend = Backend::start(server.to_owned());
    backend.commands.send(Command::Connect {
        generation: 73,
        code,
        guest_name: Some("Teste de telas C".into()),
    })?;
    let result = async {
        let mut streams = HashMap::new();
        let mut roster_ready = false;
        tokio::time::timeout(Duration::from_secs(15), async {
            while streams.len() < 4 || !roster_ready {
                let envelope = backend.events.recv().await?;
                ensure!(envelope.generation == 73, "Evento de outra sessão.");
                match envelope.event {
                    Event::StreamAdded(stream) => {
                        ensure!(
                            stream.title.starts_with("Teste de telas "),
                            "A transmissão perdeu o nome do convidado."
                        );
                        streams.insert(stream.id, stream.kind);
                    }
                    Event::Participants(people) => {
                        roster_ready = people.len() == 3
                            && people.iter().filter(|p| p.local).count() == 1
                            && people.iter().filter(|p| p.screen && p.camera).count() == 2
                            && people.iter().all(|p| p.profile.is_none())
                            && ["Teste de telas A", "Teste de telas B", "Teste de telas C"]
                                .into_iter()
                                .all(|name| people.iter().any(|p| p.name == name));
                    }
                    Event::Error(message) => anyhow::bail!(message),
                    _ => {}
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("As quatro faixas não foram recebidas.")??;
        ensure!(
            streams[&screen_a] == VideoKind::Screen && streams[&screen_b] == VideoKind::Screen,
            "Telas classificadas incorretamente."
        );
        ensure!(
            streams[&camera_a] == VideoKind::Camera && streams[&camera_b] == VideoKind::Camera,
            "Câmeras classificadas incorretamente."
        );
        frames(&backend, &screen_a, &[&camera_a, &camera_b]).await?;
        frames(&backend, &screen_b, &[&camera_a, &camera_b]).await?;

        unpublish_kind(&a, VideoKind::Screen).await;
        ensure!(
            a.local_participant()
                .track_publications()
                .values()
                .all(|p| p.source() == TrackSource::Camera),
            "Parar tela removeu a câmera."
        );
        ensure!(
            a.local_participant().track_publications().len() == 1,
            "Webcam não foi preservada."
        );
        frames(&backend, &screen_b, &[&camera_a, &camera_b]).await?;

        unpublish_kind(&b, VideoKind::Camera).await;
        ensure!(
            b.local_participant()
                .track_publications()
                .values()
                .all(|p| p.source() == TrackSource::Screenshare),
            "Desligar webcam removeu a tela."
        );
        ensure!(
            b.local_participant().track_publications().len() == 1,
            "Tela não foi preservada."
        );
        frames(&backend, &screen_b, &[&camera_a]).await?;
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut removed = std::collections::HashSet::new();
            let mut roster_stopped = false;
            while removed.len() < 2 || !roster_stopped {
                match backend.events.recv().await?.event {
                    Event::StreamRemoved(id) if id == screen_a || id == camera_b => {
                        removed.insert(id);
                    }
                    Event::Participants(people) => {
                        roster_stopped = people.len() == 3
                            && people.iter().filter(|p| p.screen).count() == 1
                            && people.iter().filter(|p| p.camera).count() == 1;
                    }
                    Event::Error(message) => anyhow::bail!(message),
                    _ => {}
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let _ = backend.commands.send(Command::Leave);
    feed.abort_all();
    let _ = a.close().await;
    let _ = b.close().await;
    result?;
    println!(
        "PASS: três clientes, duas telas + duas webcams simultâneas, seleção de tela sem perder câmeras, contador de pessoas e encerramento independente de cada fonte."
    );
    Ok(())
}
