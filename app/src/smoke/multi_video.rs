//! Exercise the real receiving backend and source-specific teardown without cameras.
use crate::session::{Backend, Command, Event, VideoKind, unpublish_kind};
use anyhow::{Context, ensure};
use livekit::{
    options::{TrackPublishOptions, VideoCodec, VideoEncoding},
    prelude::*,
    webrtc::{prelude::*, video_source::native::NativeVideoSource},
};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use telinha_shared::RoomCode;

async fn publish(
    room: &Room,
    source: TrackSource,
) -> anyhow::Result<(String, NativeVideoSource, LocalVideoTrack)> {
    let video = NativeVideoSource::new(
        VideoResolution {
            width: if source == TrackSource::Camera {
                1280
            } else {
                320
            },
            height: if source == TrackSource::Camera {
                720
            } else {
                180
            },
        },
        source != TrackSource::Camera,
    );
    let track = LocalVideoTrack::create_video_track(
        &format!("Mídia sintética {source:?}"),
        RtcVideoSource::Native(video.clone()),
    );
    let publication = room
        .local_participant()
        .publish_track(
            LocalTrack::Video(track.clone()),
            TrackPublishOptions {
                source,
                simulcast: false,
                video_codec: if cfg!(target_os = "macos") {
                    VideoCodec::H264
                } else {
                    VideoCodec::VP8
                },
                // Use the desktop's actual camera publication budget. Advertising
                // 180p and then feeding 720p tests encoder adaptation instead of
                // the viewer's spotlight/thumbnail transition.
                video_encoding: (source == TrackSource::Camera).then_some(VideoEncoding {
                    max_bitrate: 1_200_000,
                    max_framerate: 30.,
                }),
                ..Default::default()
            },
        )
        .await
        .with_context(|| format!("Publicação sintética {source:?}"))?;
    Ok((publication.sid().to_string(), video, track))
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
                backend.media.cameras.lock().get(*id).is_some_and(|frame| {
                    frame.bgra[1] > 120 && frame.width <= 640 && frame.height <= 480
                })
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
    let (mut screen_a, video_a, track_a) = publish(&a, TrackSource::Screenshare).await?;
    let (mut camera_a, cam_a, _) = publish(&a, TrackSource::Camera).await?;
    let (screen_b, video_b, _) = publish(&b, TrackSource::Screenshare).await?;
    let (camera_b, cam_b, _) = publish(&b, TrackSource::Camera).await?;
    let control = crate::adaptive::Control::new(telinha_shared::Quality::FullHd30);
    let _adaptive = crate::adaptive::Worker::start(track_a, control.clone());
    let screen_dimensions = Arc::new(AtomicU64::new((320_u64 << 32) | 180));
    let feed_dimensions = screen_dimensions.clone();
    let camera_dimensions = Arc::new(AtomicU64::new((1280_u64 << 32) | 720));
    let feed_camera_dimensions = camera_dimensions.clone();
    let mut feed = tokio::task::JoinSet::new();
    feed.spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(33));
        loop {
            tick.tick().await;
            let dimensions = feed_dimensions.load(Ordering::Relaxed);
            let camera_size = feed_camera_dimensions.load(Ordering::Relaxed);
            for (source, camera, width, height, blue) in [
                (
                    &video_a,
                    false,
                    (dimensions >> 32) as u32,
                    dimensions as u32,
                    false,
                ),
                (&cam_a, true, 320, 180, false),
                (&video_b, false, 320, 180, true),
                (
                    &cam_b,
                    true,
                    (camera_size >> 32) as u32,
                    camera_size as u32,
                    false,
                ),
            ] {
                let mut buffer = I420Buffer::new(width, height);
                let (y, u, v) = buffer.data_mut();
                y.fill(if camera {
                    145
                } else if blue {
                    105
                } else {
                    82
                });
                u.fill(if camera {
                    54
                } else if blue {
                    212
                } else {
                    90
                });
                v.fill(if camera {
                    34
                } else if blue {
                    234
                } else {
                    240
                });
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
                        streams.insert(stream.id.clone(), stream);
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
            streams[&screen_a].kind == VideoKind::Screen
                && streams[&screen_b].kind == VideoKind::Screen,
            "Telas classificadas incorretamente."
        );
        ensure!(
            streams[&camera_a].kind == VideoKind::Camera
                && streams[&camera_b].kind == VideoKind::Camera,
            "Câmeras classificadas incorretamente."
        );
        frames(&backend, &screen_a, &[&camera_a, &camera_b]).await?;
        frames(&backend, &screen_b, &[&camera_a, &camera_b]).await?;

        // Spotlight changes only camera display quality. The existing screen and
        // its audio owner remain selected while the camera occupies the stage.
        let audio_owner = streams[&screen_b].participant.clone();
        *backend.media.selected_participant.lock() = Some(audio_owner.clone());
        camera_dimensions.store((1280_u64 << 32) | 720, Ordering::Relaxed);
        for (focused, dimensions) in [(true, (1280, 720)), (false, (640, 360))] {
            *backend.media.focused_camera.lock() = focused.then(|| camera_b.clone());
            backend.media.cameras.lock().remove(&camera_b);
            tokio::time::timeout(Duration::from_secs(12), async {
                loop {
                    if backend.media.cameras.lock().get(&camera_b).is_some_and(|frame| {
                        (frame.width, frame.height) == dimensions && frame.bgra[1] > 120
                    }) { break; }
                    tokio::time::sleep(Duration::from_millis(30)).await;
                }
            }).await.with_context(|| {
                let received = backend.media.cameras.lock().get(&camera_b)
                    .map(|frame| (frame.width, frame.height, frame.bgra[1]));
                format!("A webcam não alternou entre destaque e miniatura: foco={focused}, esperado={dimensions:?}, recebido={received:?}.")
            })?;
            ensure!(backend.media.selected_track.lock().as_deref() == Some(&screen_b)
                && backend.media.selected_participant.lock().as_deref() == Some(&audio_owner),
                "O destaque da webcam alterou a transmissão ou o áudio escolhido.");
        }
        println!("PASS: webcam cifrada em destaque usa 720p; miniaturas continuam leves e o áudio selecionado é preservado.");

        for id in [&screen_a, &screen_b, &screen_a, &screen_b, &screen_a, &screen_b] {
            *backend.media.selected_participant.lock() = Some(streams[id].participant.clone());
            frames(&backend, id, &[&camera_a, &camera_b]).await?;
            let frame = backend.media.latest.lock();
            let frame = frame.as_ref().context("A tela escolhida desapareceu.")?;
            ensure!((frame.bgra[0] > 180) == (id == &screen_b),
                "A troca exibiu pixels da outra transmissão.");
        }
        println!("PASS: trocas repetidas entre duas telas da mesma sala exibem os pixels do participante escolhido.");

        // Both screens must reach distinct bounded slots in gallery mode, while
        // cameras keep updating and audio remains on the chosen publisher.
        screen_dimensions.store((1920_u64 << 32) | 800, Ordering::Relaxed);
        backend.media.gallery.store(true, Ordering::Relaxed);
        backend.media.screens.lock().clear();
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                let ready = {
                    let screens = backend.media.screens.lock();
                    screens.get(&screen_a).is_some_and(|p| p.width == 640 && p.height == 266 && p.bgra[0] < 80)
                        && screens.get(&screen_b).is_some_and(|p| p.width == 320 && p.bgra[0] > 180)
                };
                if ready { break; }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }).await.context("A galeria não recebeu as duas telas cifradas separadamente.")?;
        ensure!(backend.media.screens.lock().len() == 2, "A galeria acumulou frames ou misturou câmeras com telas.");
        ensure!(backend.media.selected_participant.lock().as_deref() == Some(&audio_owner),
            "A galeria mudou o participante ouvido.");
        backend.media.gallery.store(false, Ordering::Relaxed);
        backend.media.screens.lock().clear();
        frames(&backend, &screen_a, &[&camera_a, &camera_b]).await?;
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                if backend.media.latest.lock().as_ref().is_some_and(|p| p.width == 1920 && p.height == 800) { break; }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }).await.context("A saída da galeria não restaurou a resolução do destaque.")?;
        println!("PASS: galeria recebe duas telas e duas webcams; miniaturas preservam ultrawide, áudio escolhido e retorno ao destaque em resolução integral.");

        // Exercise resolution changes through the real encrypted H.264/VP8
        // sender and decoder while two cameras and another screen stay live.
        for (width, height) in [(1280_u32, 532_u32), (640, 268), (1920, 800), (320, 180)] {
            screen_dimensions.store(
                (u64::from(width) << 32) | u64::from(height),
                Ordering::Relaxed,
            );
            frames(&backend, &screen_a, &[&camera_a, &camera_b]).await?;
            tokio::time::timeout(Duration::from_secs(12), async {
                loop {
                    if backend.media.latest.lock().as_ref().is_some_and(|frame| {
                        frame.track == screen_a
                            && frame.width == width
                            && frame.height == height
                            && frame.bgra[2] > 180
                    }) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(30)).await;
                }
            })
            .await
            .context("A faixa cifrada não recebeu a nova resolução da tela.")?;
            ensure!(
                a.local_participant().track_publications().len() == 2
                    && a.local_participant()
                        .track_publications()
                        .values()
                        .any(|p| p.sid().to_string() == screen_a),
                "Adaptar a resolução republicou a faixa ou interrompeu a webcam."
            );
        }
        println!(
            "PASS: resolução ultrawide sobe e desce na mesma faixa cifrada com webcams simultâneas."
        );

        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                let status = control.status();
                if status.sent_fps.is_some_and(|fps| fps > 0.)
                    && status.bitrate.is_some_and(|bps| bps > 0)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .context("O controlador não recebeu estatísticas válidas do sender real.")?;
        println!("PASS: controlador obtém FPS e bitrate das estatísticas reais da faixa cifrada.");

        // Use the same selection state as the UI while a real publisher replaces
        // its connection and both track SIDs. Bob must never become a fallback.
        let mut selected = crate::stream_selection::Selection::default();
        selected.choose(Some(&streams[&screen_a]));
        let owner = streams[&screen_a].participant.clone();
        *backend.media.selected_track.lock() = Some(screen_a.clone());
        a.simulate_scenario(livekit::rtc_engine::SimulateScenario::FullReconnect)
            .await?;
        let (new_screen, new_camera) = tokio::time::timeout(Duration::from_secs(25), async {
            let mut new_screen = None;
            let mut new_camera = None;
            loop {
                match backend.events.recv().await?.event {
                    Event::StreamAdded(stream) => {
                        selected.added(&stream);
                        if stream.participant == owner {
                            if stream.kind == VideoKind::Screen && stream.id != screen_a {
                                new_screen = Some(stream.id.clone());
                            } else if stream.kind == VideoKind::Camera && stream.id != camera_a {
                                new_camera = Some(stream.id.clone());
                            }
                        }
                        streams.insert(stream.id.clone(), stream);
                    }
                    Event::StreamRemoved(id) => {
                        streams.remove(&id);
                        if selected.removed(&id) { selected.recover(streams.values(), &id); }
                    }
                    Event::StreamPaused(id) => {
                        if selected.removed(&id) { selected.recover(streams.values(), &id); }
                    }
                    Event::Error(message) => anyhow::bail!(message),
                    _ => {}
                }
                ensure!(
                    selected.participant() == Some(owner.as_str()),
                    "A reconexão mudou a pessoa escolhida."
                );
                ensure!(
                    selected.track() != Some(screen_b.as_str()),
                    "A outra transmissão assumiu a seleção."
                );
                *backend.media.selected_track.lock() = selected.track().map(str::to_owned);
                if let (Some(screen), Some(camera)) = (&new_screen, &new_camera) {
                    if selected.track() == Some(screen.as_str()) {
                        break Ok::<_, anyhow::Error>((screen.clone(), camera.clone()));
                    }
                }
            }
        })
        .await
        .context("A publicação não voltou após a reconexão completa.")??;
        screen_a = new_screen;
        camera_a = new_camera;
        frames(&backend, &screen_a, &[&camera_a, &camera_b]).await?;
        println!("PASS: reconexão real republica tela e webcam sem trocar para a outra pessoa.");

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
        *backend.media.focused_camera.lock() = Some(camera_a.clone());
        backend.commands.send(Command::Leave)?;
        tokio::time::timeout(Duration::from_secs(8), async {
            while let Ok(envelope) = backend.events.recv().await {
                if matches!(envelope.event, Event::Disconnected) { return; }
            }
        }).await.context("Sair não encerrou a conexão de mídia.")?;
        ensure!(backend.media.focused_camera.lock().is_none(), "O foco vazou para fora da sessão.");
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
