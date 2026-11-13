//! Real frame-cryptor checks using generated pixels only.
use anyhow::{Context, Result, ensure};
use futures_util::StreamExt;
use livekit::{
    options::TrackPublishOptions,
    prelude::*,
    webrtc::{
        prelude::*, video_source::native::NativeVideoSource,
        video_stream::native::NativeVideoStream,
    },
};
use std::time::Duration;
use telinha_shared::RoomCode;

async fn video(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
) -> Result<NativeVideoStream> {
    tokio::time::timeout(Duration::from_secs(12), async {
        while let Some(event) = events.recv().await {
            if let RoomEvent::TrackSubscribed {
                track: RemoteTrack::Video(track),
                publication,
                ..
            } = event
            {
                ensure!(
                    publication.encryption_type() == livekit::e2ee::EncryptionType::Gcm,
                    "Plaintext track in encrypted room"
                );
                return Ok(NativeVideoStream::new(track.rtc_track()));
            }
        }
        anyhow::bail!("Room ended before video subscription")
    })
    .await
    .context("No encrypted video subscription")?
}

pub(super) async fn check(server: &str) -> Result<()> {
    let code = RoomCode::generate();
    let (a, mut alice) = crate::session::resolve(server, &code, "E2EE publisher").await?;
    let (b, mut bob) = crate::session::resolve(server, &code, "E2EE receiver").await?;
    let (c, mut carol) = crate::session::resolve(server, &code, "E2EE late receiver").await?;
    let (publisher, _) = Room::connect(&a.url, &a.token, alice.options()).await?;
    let (receiver, mut b_events) = Room::connect(&b.url, &b.token, bob.options()).await?;
    let (late, mut c_events) = Room::connect(&c.url, &c.token, carol.options()).await?;
    let mut workers = tokio::task::JoinSet::new();
    workers.spawn(async move { alice.run().await });
    workers.spawn(async move { bob.run().await });
    let source = NativeVideoSource::new(
        VideoResolution {
            width: 160,
            height: 90,
        },
        true,
    );
    publisher
        .local_participant()
        .publish_track(
            LocalTrack::Video(LocalVideoTrack::create_video_track(
                "E2EE synthetic",
                RtcVideoSource::Native(source.clone()),
            )),
            TrackPublishOptions {
                source: TrackSource::Screenshare,
                simulcast: false,
                ..Default::default()
            },
        )
        .await?;
    let feed = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(33));
        loop {
            tick.tick().await;
            let mut frame = I420Buffer::new(160, 90);
            let (y, u, v) = frame.data_mut();
            y.fill(82);
            u.fill(90);
            v.fill(240);
            source.capture_frame(&VideoFrame::new(VideoRotation::VideoRotation0, frame));
        }
    });
    struct Stop(tokio::task::JoinHandle<()>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _feed = Stop(feed);
    let mut b_video = video(&mut b_events).await?;
    let mut c_video = video(&mut c_events).await?;
    ensure!(
        tokio::time::timeout(Duration::from_secs(12), b_video.next())
            .await?
            .is_some(),
        "Receiver with key decoded no frame"
    );
    ensure!(
        tokio::time::timeout(Duration::from_secs(2), c_video.next())
            .await
            .is_err(),
        "Receiver without sender key decoded video"
    );
    let (revoke, revoked) = tokio::sync::oneshot::channel::<()>();
    workers.spawn(async move {
        tokio::select! { result = carol.run() => return result, _ = revoked => {} }
        carol.leave().await;
        Ok(())
    });
    ensure!(
        tokio::time::timeout(Duration::from_secs(12), c_video.next())
            .await?
            .is_some(),
        "Late receiver did not decode after key exchange"
    );
    revoke.send(()).ok();
    tokio::time::sleep(Duration::from_secs(3)).await;
    // Drain frames already decoded before revocation, with a hard deadline.
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match tokio::time::timeout(Duration::from_millis(100), c_video.next()).await {
                Ok(Some(_)) => {}
                Ok(None) => anyhow::bail!("Revocation closed the synthetic stream unexpectedly"),
                Err(_) => return Ok::<_, anyhow::Error>(()),
            }
        }
    })
    .await
    .context("Revoked receiver continued decoding after key rotation")??;
    ensure!(
        tokio::time::timeout(Duration::from_secs(2), c_video.next())
            .await
            .is_err(),
        "Old media key still decrypts after rotation"
    );
    ensure!(
        tokio::time::timeout(Duration::from_secs(5), b_video.next())
            .await?
            .is_some(),
        "Remaining receiver lost video after rotation"
    );
    publisher.close().await?;
    receiver.close().await?;
    late.close().await?;
    workers.abort_all();
    println!(
        "PASS: no video without sender key; automatic late join; revoked keys stop decoding while the room continues."
    );
    Ok(())
}
