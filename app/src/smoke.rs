//! SFU integration test with synthetic video/audio. --remote opts into the VPS.
use anyhow::{Context, ensure};
use futures_util::StreamExt;
use livekit::{
    options::{TrackPublishOptions, VideoCodec},
    prelude::*,
    webrtc::{
        audio_source::native::NativeAudioSource, audio_stream::native::NativeAudioStream,
        prelude::*, video_frame::native::VideoFrameBufferExt,
        video_source::native::NativeVideoSource, video_stream::native::NativeVideoStream,
    },
};
use std::{borrow::Cow, time::Duration};
use telinha_shared::RoomCode;
mod encryption;
mod multi_video;

pub fn run() -> anyhow::Result<()> {
    crate::camera::probe()?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(120), check())
            .await
            .context("O teste de mídia excedeu 120 segundos.")?
    })
}

/// Local browser interoperability fixture: generated video, never screen/camera.
pub fn viewer() -> anyhow::Result<()> {
    tokio::runtime::Runtime::new()?.block_on(async {
        let server = std::env::var("TELINHA_SERVER_URL").unwrap_or_else(|_| "http://localhost:3010".into());
        let url = reqwest::Url::parse(&server)?;
        ensure!(matches!(url.host_str(), Some("localhost" | "127.0.0.1")), "The browser fixture only uses a local test server");
        let code = RoomCode::generate();
        let (credentials, mut encryption) = crate::session::resolve(&server, &code, "Teste E2EE desktop").await?;
        let media_url = reqwest::Url::parse(&credentials.url)?;
        ensure!(matches!(media_url.host_str(), Some("localhost" | "127.0.0.1")), "The browser fixture requires a local SFU");
        let (room, _) = Room::connect(&credentials.url, &credentials.token, encryption.options()).await?;
        let source = NativeVideoSource::new(VideoResolution { width: 320, height: 180 }, true);
        room.local_participant().publish_track(LocalTrack::Video(LocalVideoTrack::create_video_track("Imagem vermelha gerada", RtcVideoSource::Native(source.clone()))), TrackPublishOptions { source: TrackSource::Screenshare, simulcast: false, video_codec: VideoCodec::VP8, ..Default::default() }).await?;
        println!("Sala local para verificar o viewer: {} (imagem vermelha sintética, sem câmera ou captura de tela).", code.as_str());
        let mut tick = tokio::time::interval(Duration::from_millis(33));
        let feed = async {
            loop {
                tick.tick().await;
                let mut buffer = I420Buffer::new(320,180);
                let (y,u,v) = buffer.data_mut(); y.fill(82);u.fill(90);v.fill(240);
                source.capture_frame(&VideoFrame::new(VideoRotation::VideoRotation0,buffer));
            }
        };
        tokio::select! {
            result = encryption.run() => result?,
            _ = feed => {},
            _ = tokio::signal::ctrl_c() => {},
            _ = tokio::time::sleep(Duration::from_secs(600)) => {},
        }
        encryption.leave().await;
        room.close().await?;
        Ok(())
    })
}

async fn check() -> anyhow::Result<()> {
    let server =
        std::env::var("TELINHA_SERVER_URL").unwrap_or_else(|_| "http://localhost:3010".into());
    let remote = std::env::args().any(|arg| arg == "--remote");
    let relay = std::env::args().any(|arg| arg == "--relay");
    let url = reqwest::Url::parse(&server)?;
    ensure!(
        remote || matches!(url.host_str(), Some("localhost" | "127.0.0.1")),
        "Para testar a VPS, informe --remote explicitamente."
    );
    let code = RoomCode::generate();
    let (a, mut a_crypto) = crate::session::resolve(&server, &code, "Teste de mídia A").await?;
    let (b, mut b_crypto) = crate::session::resolve(&server, &code, "Teste de mídia B").await?;
    ensure!(
        a.identity != b.identity,
        "As identidades devem ser diferentes."
    );
    for credentials in [&a, &b] {
        let media_url = reqwest::Url::parse(&credentials.url)?;
        ensure!(
            remote || matches!(media_url.host_str(), Some("localhost" | "127.0.0.1")),
            "A API local retornou um SFU remoto. Use --remote para publicar nele."
        );
        ensure!(
            media_url.scheme() == "wss" || (!remote && media_url.scheme() == "ws"),
            "A mídia remota requer WSS."
        );
    }
    let mut options = RoomOptions::default();
    if relay {
        options.rtc_config.ice_transport_type =
            livekit::webrtc::peer_connection_factory::IceTransportsType::Relay;
    }
    println!(
        "Testando mídia sintética em {server} ({}).",
        if relay {
            "somente TURN"
        } else {
            "ICE automático"
        }
    );
    let mut a_options = a_crypto.options();
    a_options.rtc_config = options.rtc_config.clone();
    let mut b_options = b_crypto.options();
    b_options.rtc_config = options.rtc_config.clone();
    let (publisher, _) = Room::connect(&a.url, &a.token, a_options).await?;
    let (subscriber, mut events) = Room::connect(&b.url, &b.token, b_options).await?;
    let mut key_workers = tokio::task::JoinSet::new();
    key_workers.spawn(async move { a_crypto.run().await });
    key_workers.spawn(async move { b_crypto.run().await });
    let video = NativeVideoSource::new(
        livekit::webrtc::video_source::VideoResolution {
            width: 320,
            height: 180,
        },
        true,
    );
    let audio = NativeAudioSource::new(
        AudioSourceOptions {
            echo_cancellation: false,
            noise_suppression: false,
            auto_gain_control: false,
        },
        48000,
        2,
        100,
    );
    publisher
        .local_participant()
        .publish_track(
            LocalTrack::Video(LocalVideoTrack::create_video_track(
                "Teste de vídeo",
                RtcVideoSource::Native(video.clone()),
            )),
            TrackPublishOptions {
                source: TrackSource::Screenshare,
                simulcast: false,
                video_codec: VideoCodec::VP8,
                ..Default::default()
            },
        )
        .await?;
    publisher
        .local_participant()
        .publish_track(
            LocalTrack::Audio(LocalAudioTrack::create_audio_track(
                "Teste de áudio",
                RtcAudioSource::Native(audio.clone()),
            )),
            TrackPublishOptions {
                source: TrackSource::ScreenshareAudio,
                ..Default::default()
            },
        )
        .await?;
    let feed = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        let mut n = 0;
        loop {
            tick.tick().await;
            if n % 3 == 0 {
                let mut buffer = I420Buffer::new(320, 180);
                let (y, u, v) = buffer.data_mut();
                y.fill(82);
                u.fill(90);
                v.fill(240);
                video.capture_frame(&VideoFrame::new(VideoRotation::VideoRotation0, buffer));
            }
            let data = (0..480)
                .flat_map(|i| {
                    let x = (((n * 480 + i) as f32 * 440.0 * std::f32::consts::TAU / 48000.0).sin()
                        * 8000.) as i16;
                    [x, x]
                })
                .collect::<Vec<_>>();
            let _ = audio
                .capture_frame(&AudioFrame {
                    data: Cow::Owned(data),
                    sample_rate: 48000,
                    num_channels: 2,
                    samples_per_channel: 480,
                })
                .await;
            n += 1;
        }
    });
    let result=tokio::time::timeout(Duration::from_secs(25),async {
            let mut checks=tokio::task::JoinSet::new();let mut video_ok=false;let mut audio_ok=false;
            loop {
                tokio::select! {
                    event=events.recv()=>if let Some(RoomEvent::TrackSubscribed{track,publication,..})=event {
                        ensure!(publication.encryption_type() == livekit::e2ee::EncryptionType::Gcm, "Mídia sintética chegou sem E2EE");
                        match track {
                            RemoteTrack::Video(track)=>{checks.spawn(async move {
                                let mut stream=NativeVideoStream::new(track.rtc_track());
                                while let Some(frame)=stream.next().await {
                                    let buffer=frame.buffer.to_i420();
                                    let (w,h)=(frame.buffer.width(),frame.buffer.height());
                                    let mut bgra=vec![0;w as usize*h as usize*4];buffer.to_argb(VideoFormatType::ARGB,&mut bgra,w*4,w as i32,h as i32);
                                    if bgra[2]>180 && bgra[0]<80 && bgra[1]<80 {return "video";}
                                } "ended"
                            });}
                            RemoteTrack::Audio(track)=>{checks.spawn(async move {
                                let mut stream=NativeAudioStream::new(track.rtc_track(),48000,2);
                                while let Some(frame)=stream.next().await {if frame.data.iter().any(|x|x.unsigned_abs()>500){return "audio";}}
                                "ended"
                            });}
                        }
                    },
                    Some(result)=checks.join_next(),if !checks.is_empty()=>match result? {"video"=>video_ok=true,"audio"=>audio_ok=true,_=>{}},
                }
                if video_ok && audio_ok {return Ok::<_,anyhow::Error>(());}
            }
        }).await;
    feed.abort();
    let _ = publisher.close().await;
    let _ = subscriber.close().await;
    key_workers.abort_all();
    result.context("Vídeo/áudio não chegaram pelo SFU em 25 segundos.")??;
    println!(
        "PASS: dois clientes com E2EE, troca automática de chaves, vídeo decodificado com cor correta e áudio PCM recebido pelo LiveKit."
    );
    multi_video::check(&server, options).await?;
    encryption::check(&server).await?;
    let backend = crate::session::Backend::start(server);
    backend.commands.send(crate::session::Command::Connect {
        generation: 42,
        code: RoomCode::generate(),
        guest_name: Some("Teste de conexão".into()),
    })?;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let message = backend.events.recv().await?;
            match message.event {
                crate::session::Event::Connected(_) => break,
                crate::session::Event::Error(e) => anyhow::bail!(e),
                _ => {}
            }
        }
        backend.commands.send(crate::session::Command::Leave)?;
        loop {
            let message = backend.events.recv().await?;
            if matches!(message.event, crate::session::Event::Disconnected) {
                break;
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    println!("PASS: backend do app conecta e encerra a sessão sem bloquear a UI.");
    Ok(())
}
