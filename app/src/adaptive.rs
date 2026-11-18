//! Adapt source preparation, not only the encoder's output, using public WebRTC stats.
//! WebRTC still controls bitrate beneath the publication's maximum. One encrypted
//! publication is retained through every resolution/FPS change.
use livekit::{
    track::LocalVideoTrack,
    webrtc::stats::{OutboundRtpStats, QualityLimitationReason, RtcStats},
};
use parking_lot::Mutex;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use telinha_shared::Quality;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

const PROFILES: [Profile; 5] = [
    Profile {
        width: 1920,
        height: 1080,
        fps: 60,
    },
    Profile {
        width: 1920,
        height: 1080,
        fps: 30,
    },
    Profile {
        width: 1280,
        height: 720,
        fps: 30,
    },
    Profile {
        width: 854,
        height: 480,
        fps: 30,
    },
    Profile {
        width: 640,
        height: 360,
        fps: 15,
    },
];

fn profiles(ceiling: Quality) -> Vec<Profile> {
    match ceiling {
        Quality::FullHd60 => PROFILES.to_vec(),
        Quality::FullHd30 => PROFILES[1..].to_vec(),
        Quality::Hd60 => [
            Profile {
                width: 1280,
                height: 720,
                fps: 60,
            },
            PROFILES[2],
            PROFILES[3],
            PROFILES[4],
        ]
        .to_vec(),
    }
}

#[derive(Default)]
pub struct Cadence {
    next: Option<Instant>,
    fps: u32,
}
impl Cadence {
    pub fn accepts(&mut self, now: Instant, fps: u32) -> bool {
        let period = Duration::from_secs_f64(1. / fps.max(1) as f64);
        if self.fps != fps {
            self.next = None;
            self.fps = fps;
        }
        if let Some(next) = self.next {
            if now + Duration::from_millis(1) < next {
                return false;
            }
            self.next = Some(if now >= next + period {
                now + period
            } else {
                next + period
            });
        } else {
            self.next = Some(now + period);
        }
        true
    }
}

#[derive(Clone, Debug)]
pub struct Status {
    pub width: u32,
    pub height: u32,
    pub sent_fps: Option<f64>,
    pub bitrate: Option<u64>,
}

struct State {
    ceiling: Quality,
    profile: Profile,
    status: Status,
    prepared: u64,
    prepare_ms: f64,
}

#[derive(Clone)]
pub struct Control(Arc<Mutex<State>>);

impl Control {
    pub fn new(quality: Quality) -> Self {
        let profile = profiles(quality)[0];
        Self(Arc::new(Mutex::new(State {
            ceiling: quality,
            profile,
            status: Status {
                width: 0,
                height: 0,
                sent_fps: None,
                bitrate: None,
            },
            prepared: 0,
            prepare_ms: 0.,
        })))
    }

    pub fn profile(&self) -> Profile {
        self.0.lock().profile
    }
    pub fn status(&self) -> Status {
        self.0.lock().status.clone()
    }

    pub fn prepared(&self, width: u32, height: u32, elapsed: Duration) {
        let mut state = self.0.lock();
        let ms = elapsed.as_secs_f64() * 1000.;
        state.prepare_ms = if state.prepared == 0 {
            ms
        } else {
            state.prepare_ms * 0.8 + ms * 0.2
        };
        state.prepared += 1;
        state.status.width = width;
        state.status.height = height;
    }

    pub fn preserve_profile(&self, previous: &Self) {
        let profile = previous.profile();
        let mut state = self.0.lock();
        if profiles(state.ceiling).contains(&profile) {
            state.profile = profile;
        }
    }

    fn observe(&self, sample: Option<&Sample>, changed: Option<(Profile, &'static str)>) {
        let mut state = self.0.lock();
        state.status.sent_fps = sample.map(|s| s.sent_fps);
        state.status.bitrate = sample.map(|s| s.bitrate);
        if let Some((profile, reason)) = changed {
            state.profile = profile;
            tracing::info!(
                event = "quality_adapted",
                width = profile.width,
                height = profile.height,
                fps = profile.fps,
                reason,
                "Qualidade da transmissão ajustada"
            );
        }
    }
}

#[derive(Clone, Debug)]
struct Sample {
    network_observed: bool,
    sent_fps: f64,
    encoded_frames: u32,
    bitrate: u64,
    encode_ms: f64,
    loss: f64,
    rtt: f64,
    limitation: QualityLimitationReason,
}

#[derive(Default)]
struct Sampler {
    previous: Option<(Instant, OutboundRtpStats)>,
    remote_report: Option<(String, u64, Instant)>,
}

impl Sampler {
    fn sample(&mut self, now: Instant, stats: &[RtcStats]) -> Option<Sample> {
        let current = stats.iter().find_map(|stat| match stat {
            RtcStats::OutboundRtp(out)
                if out.stream.kind == "video" && out.outbound.frame_width > 0 =>
            {
                Some(out)
            }
            _ => None,
        });
        let Some(current) = current else {
            self.previous = None;
            self.remote_report = None;
            return None;
        };
        let previous = self.previous.replace((now, current.clone()))?;
        let seconds = now.checked_duration_since(previous.0)?.as_secs_f64();
        if current.rtc.id != previous.1.rtc.id
            || !(0.25..=6.).contains(&seconds)
            || current.rtc.timestamp <= previous.1.rtc.timestamp
        {
            self.remote_report = None;
            return None;
        }
        let encoded_frames = current
            .outbound
            .frames_encoded
            .checked_sub(previous.1.outbound.frames_encoded)?;
        let sent_frames = current
            .outbound
            .frames_sent
            .checked_sub(previous.1.outbound.frames_sent)?;
        let bytes = current
            .sent
            .bytes_sent
            .checked_sub(previous.1.sent.bytes_sent)?;
        let encode = current.outbound.total_encode_time - previous.1.outbound.total_encode_time;
        if !encode.is_finite() || encode < 0. {
            return None;
        }
        let remote = stats.iter().find_map(|stat| match stat {
            RtcStats::RemoteInboundRtp(remote)
                if remote.remote_inbound.local_id == current.rtc.id =>
            {
                Some(remote)
            }
            _ => None,
        });
        let (loss, rtt) = remote
            .map(|r| {
                (
                    r.remote_inbound.fraction_lost,
                    r.remote_inbound.round_trip_time,
                )
            })
            .unwrap_or((0., 0.));
        if let Some(remote) = remote {
            let count = remote.remote_inbound.round_trip_time_measurements;
            if count > 0
                && self
                    .remote_report
                    .as_ref()
                    .is_none_or(|(id, previous, _)| id != &remote.rtc.id || count != *previous)
            {
                self.remote_report = Some((remote.rtc.id.clone(), count, now));
            }
        }
        let network_observed = remote.is_some()
            && self.remote_report.as_ref().is_some_and(|(_, _, measured)| {
                now.saturating_duration_since(*measured) <= Duration::from_secs(6)
            });
        if !loss.is_finite() || !rtt.is_finite() || !(0. ..=1.).contains(&loss) || rtt < 0. {
            return None;
        }
        Some(Sample {
            network_observed,
            sent_fps: sent_frames as f64 / seconds,
            encoded_frames,
            bitrate: (bytes as f64 * 8. / seconds) as u64,
            encode_ms: if encoded_frames == 0 {
                0.
            } else {
                encode * 1000. / encoded_frames as f64
            },
            loss,
            rtt,
            limitation: current.outbound.quality_limitation_reason,
        })
    }
}

struct Policy {
    profiles: Vec<Profile>,
    level: usize,
    bad_samples: u8,
    healthy_since: Option<u64>,
    changed_at: u64,
}

impl Policy {
    fn new(ceiling: Quality, profile: Profile) -> Self {
        let profiles = profiles(ceiling);
        Self {
            level: profiles.iter().position(|p| *p == profile).unwrap_or(0),
            profiles,
            bad_samples: 0,
            healthy_since: None,
            changed_at: 0,
        }
    }

    fn next(
        &mut self,
        now: u64,
        sample: Option<&Sample>,
        prepare_ms: f64,
    ) -> Option<(Profile, &'static str)> {
        let Some(sample) = sample else {
            self.bad_samples = 0;
            self.healthy_since = None;
            self.changed_at = now; // Warm up again after missing/reset/reconnect stats.
            return None;
        };
        let active = sample.encoded_frames >= 5;
        let budget = 1000. / self.profiles[self.level].fps as f64;
        let cpu = prepare_ms > budget * 0.65
            || (active
                && (sample.limitation == QualityLimitationReason::Cpu
                    || sample.encode_ms > budget * 0.8));
        let network = sample.encoded_frames > 0
            && ((sample.network_observed && (sample.loss >= 0.05 || sample.rtt >= 0.4))
                || sample.limitation == QualityLimitationReason::Bandwidth);
        if cpu || network {
            self.healthy_since = None;
            self.bad_samples = self.bad_samples.saturating_add(1);
            if self.bad_samples >= 2
                && now.saturating_sub(self.changed_at) >= 8
                && self.level + 1 < self.profiles.len()
            {
                self.level += 1;
                self.changed_at = now;
                self.bad_samples = 0;
                return Some((
                    self.profiles[self.level],
                    if cpu {
                        "Computador sobrecarregado"
                    } else {
                        "Conexão limitada"
                    },
                ));
            }
        } else {
            self.bad_samples = 0;
            // A static screen or absent stats cannot prove spare encoder/network capacity.
            let healthy = active
                && sample.network_observed
                && sample.loss < 0.01
                && sample.rtt < 0.2
                && sample.limitation == QualityLimitationReason::None
                && prepare_ms < budget * 0.35
                && sample.encode_ms < budget * 0.45;
            if healthy {
                let since = self.healthy_since.get_or_insert(now);
                if self.level > 0
                    && now.saturating_sub(*since) >= 30
                    && now.saturating_sub(self.changed_at) >= 30
                {
                    self.level -= 1;
                    self.changed_at = now;
                    self.healthy_since = None;
                    return Some((self.profiles[self.level], "Qualidade recuperada"));
                }
            } else {
                self.healthy_since = None;
            }
        }
        None
    }
}

pub struct Worker(tokio::task::JoinHandle<()>);
impl Drop for Worker {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Worker {
    pub fn start(track: LocalVideoTrack, control: Control) -> Self {
        Self(tokio::spawn(async move {
            let start = Instant::now();
            let mut policy = {
                let state = control.0.lock();
                Policy::new(state.ceiling, state.profile)
            };
            let mut sampler = Sampler::default();
            let mut prepared = 0;
            let mut timer = tokio::time::interval(Duration::from_secs(2));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                timer.tick().await;
                let sample = match tokio::time::timeout(
                    Duration::from_millis(1500),
                    track.get_stats(),
                )
                .await
                {
                    Ok(Ok(stats)) => sampler.sample(Instant::now(), &stats),
                    _ => {
                        sampler.previous = None;
                        sampler.remote_report = None;
                        None
                    }
                };
                let prepare_ms = {
                    let state = control.0.lock();
                    let cost = if state.prepared > prepared {
                        state.prepare_ms
                    } else {
                        0.
                    };
                    prepared = state.prepared;
                    cost
                };
                let changed = policy.next(start.elapsed().as_secs(), sample.as_ref(), prepare_ms);
                control.observe(sample.as_ref(), changed);
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn healthy() -> Sample {
        Sample {
            network_observed: true,
            sent_fps: 30.,
            encoded_frames: 60,
            bitrate: 2_000_000,
            encode_ms: 2.,
            loss: 0.,
            rtt: 0.03,
            limitation: QualityLimitationReason::None,
        }
    }

    #[test]
    fn cadence_skips_work_before_conversion_and_can_change_without_drift() {
        let start = Instant::now();
        let mut cadence = Cadence::default();
        let accepted = (0..60)
            .filter(|i| cadence.accepts(start + Duration::from_micros(i * 16_667), 30))
            .count();
        assert_eq!(accepted, 30);
        assert!(cadence.accepts(start + Duration::from_secs(1), 15));
        assert!(!cadence.accepts(start + Duration::from_millis(1030), 15));
        assert!(cadence.accepts(start + Duration::from_millis(1067), 15));
        assert!(cadence.accepts(start + Duration::from_secs(20), 15));
        assert!(!cadence.accepts(start + Duration::from_millis(20010), 15));
    }

    #[test]
    fn sustained_network_pressure_lowers_quality_without_changing_publication() {
        let mut policy = Policy::new(Quality::FullHd60, PROFILES[0]);
        let bad = Sample {
            loss: 0.08,
            ..healthy()
        };
        assert!(policy.next(6, Some(&bad), 2.).is_none());
        assert_eq!(
            policy.next(8, Some(&bad), 2.),
            Some((PROFILES[1], "Conexão limitada"))
        );
        assert!(policy.next(10, Some(&bad), 2.).is_none());
        assert!(policy.next(12, Some(&bad), 2.).is_none());
        assert_eq!(policy.next(16, Some(&bad), 2.).unwrap().0, PROFILES[2]);
    }

    #[test]
    fn cpu_pressure_reduces_preparation_and_respects_a_floor() {
        let mut policy = Policy::new(Quality::FullHd60, PROFILES[0]);
        for now in (2..100).step_by(2) {
            policy.next(now, Some(&healthy()), 80.);
        }
        assert_eq!(policy.level, PROFILES.len() - 1);
        assert_eq!(PROFILES[policy.level].fps, 15);
        let fit = telinha_shared::fit_video(
            3440,
            1440,
            PROFILES[policy.level].width,
            PROFILES[policy.level].height,
        );
        assert!(fit.0 <= 640 && fit.1 <= 360);
        assert!((fit.0 as f64 / fit.1 as f64 - 3440. / 1440.).abs() < 0.03);
    }

    #[test]
    fn recovery_is_gradual_and_a_spike_resets_the_healthy_window() {
        let mut policy = Policy::new(Quality::FullHd60, PROFILES[3]);
        for now in (2..30).step_by(2) {
            assert!(policy.next(now, Some(&healthy()), 2.).is_none());
        }
        let spike = Sample {
            loss: 0.03,
            ..healthy()
        };
        assert!(policy.next(30, Some(&spike), 2.).is_none());
        for now in (32..62).step_by(2) {
            assert!(policy.next(now, Some(&healthy()), 2.).is_none());
        }
        assert_eq!(
            policy.next(62, Some(&healthy()), 2.).unwrap().0,
            PROFILES[2]
        );
        assert!(policy.next(64, Some(&healthy()), 2.).is_none());
    }

    #[test]
    fn static_content_and_missing_stats_never_trigger_recovery_or_network_downgrades() {
        let mut policy = Policy::new(Quality::FullHd60, PROFILES[2]);
        let still = Sample {
            sent_fps: 0.,
            encoded_frames: 0,
            bitrate: 0,
            limitation: QualityLimitationReason::Bandwidth,
            ..healthy()
        };
        for now in (2..100).step_by(2) {
            assert!(policy.next(now, Some(&still), 2.).is_none());
        }
        assert!(policy.next(102, None, 2.).is_none());
        assert_eq!(policy.level, 2);
    }

    #[test]
    fn low_output_under_congestion_still_adapts_but_missing_network_evidence_never_recovers() {
        let mut policy = Policy::new(Quality::FullHd60, PROFILES[0]);
        let congested = Sample {
            encoded_frames: 1,
            sent_fps: 0.5,
            loss: 0.2,
            ..healthy()
        };
        policy.next(6, Some(&congested), 2.);
        assert_eq!(policy.next(8, Some(&congested), 2.).unwrap().0, PROFILES[1]);
        let missing = Sample {
            network_observed: false,
            ..healthy()
        };
        for now in (10..120).step_by(2) {
            assert!(policy.next(now, Some(&missing), 2.).is_none());
        }
        assert_eq!(policy.level, 1);
    }

    #[test]
    fn every_selected_quality_adapts_and_recovers_only_up_to_its_ceiling() {
        for quality in [Quality::FullHd30, Quality::Hd60, Quality::FullHd60] {
            let control = Control::new(quality);
            assert_eq!(
                (control.profile().width, control.profile().height),
                quality.dimensions()
            );
            assert_eq!(control.profile().fps, quality.fps());
            let mut policy = Policy::new(quality, control.profile());
            for now in (2..100).step_by(2) {
                policy.next(now, Some(&healthy()), 80.);
            }
            assert_eq!(policy.level, policy.profiles.len() - 1);
            for now in (100..400).step_by(2) {
                policy.next(now, Some(&healthy()), 1.);
            }
            assert_eq!(policy.level, 0);
            assert_eq!(policy.profiles[0], control.profile());
        }
        let original = Control::new(Quality::FullHd60);
        original.observe(None, Some((PROFILES[3], "Conexão limitada")));
        let next = Control::new(Quality::FullHd60);
        next.preserve_profile(&original);
        assert_eq!(next.profile(), PROFILES[3]);
    }

    #[test]
    fn remote_reports_expire_and_only_new_measurements_restore_network_evidence() {
        use livekit::webrtc::stats::RemoteInboundRtpStats;
        let mut out = OutboundRtpStats::default();
        out.rtc.id = "sender".into();
        out.stream.kind = "video".into();
        out.outbound.frame_width = 1280;
        let mut remote = RemoteInboundRtpStats::default();
        remote.rtc.id = "remote".into();
        remote.remote_inbound.local_id = "sender".into();
        remote.remote_inbound.round_trip_time_measurements = 1;
        remote.remote_inbound.round_trip_time = 0.03;
        let mut sampler = Sampler::default();
        let start = Instant::now();
        for step in 0..=6 {
            out.rtc.timestamp = step + 1;
            out.outbound.frames_encoded += 60;
            if step == 6 {
                remote.remote_inbound.round_trip_time_measurements += 1;
            }
            let sample = sampler.sample(
                start + Duration::from_secs(step as u64 * 2),
                &[
                    RtcStats::OutboundRtp(out.clone()),
                    RtcStats::RemoteInboundRtp(remote.clone()),
                ],
            );
            if step > 0 {
                assert_eq!(sample.unwrap().network_observed, step != 5);
            }
        }
    }

    #[test]
    fn stats_deltas_use_sent_frames_and_reset_when_sender_changes_or_counters_decrease() {
        let mut sampler = Sampler::default();
        let now = Instant::now();
        let mut out = OutboundRtpStats::default();
        out.rtc.id = "synthetic".into();
        out.rtc.timestamp = 1;
        out.stream.kind = "video".into();
        out.outbound.frame_width = 1280;
        out.outbound.frames_encoded = 100;
        out.outbound.frames_sent = 90;
        out.sent.bytes_sent = 1000;
        out.outbound.total_encode_time = 1.;
        assert!(
            sampler
                .sample(now, &[RtcStats::OutboundRtp(out.clone())])
                .is_none()
        );
        out.rtc.timestamp = 2;
        out.outbound.frames_encoded += 60;
        out.outbound.frames_sent += 40;
        out.sent.bytes_sent += 500_000;
        out.outbound.total_encode_time += 0.12;
        let sample = sampler
            .sample(
                now + Duration::from_secs(2),
                &[RtcStats::OutboundRtp(out.clone())],
            )
            .unwrap();
        assert_eq!(sample.sent_fps, 20.);
        assert_eq!(sample.bitrate, 2_000_000);
        assert!((sample.encode_ms - 2.).abs() < 0.001);
        out.rtc.id = "reconnected".into();
        out.rtc.timestamp = 3;
        assert!(
            sampler
                .sample(
                    now + Duration::from_secs(4),
                    &[RtcStats::OutboundRtp(out.clone())]
                )
                .is_none()
        );
        out.rtc.timestamp = 4;
        out.outbound.frames_encoded = 1;
        assert!(
            sampler
                .sample(now + Duration::from_secs(6), &[RtcStats::OutboundRtp(out)])
                .is_none()
        );
    }
}
