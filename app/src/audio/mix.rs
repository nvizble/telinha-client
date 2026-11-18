//! Bounded, stereo desktop mixer and process-tree selection, independent of WASAPI.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(super) const SAMPLES: usize = 480 * 2; // 10 ms at 48 kHz, stereo.

#[derive(Default)]
pub(super) struct Buffer(VecDeque<f32>);

impl Buffer {
    pub fn push(&mut self, bytes: &[u8]) {
        // WASAPI packets contain complete stereo frames. Discard malformed tails.
        for frame in bytes.chunks_exact(8) {
            for value in frame.chunks_exact(4) {
                let sample = f32::from_le_bytes(value.try_into().unwrap());
                self.0
                    .push_back(if sample.is_finite() { sample } else { 0. });
            }
        }
        let overflow = self.0.len().saturating_sub(SAMPLES * 3);
        self.0.drain(..overflow); // Keep at most 30 ms; never replay a backlog.
    }

    pub fn add_to(&mut self, output: &mut [f32; SAMPLES]) {
        for sample in output {
            *sample += self.0.pop_front().unwrap_or(0.);
        }
    }
}

pub(super) fn pcm(samples: &[f32; SAMPLES]) -> Vec<i16> {
    samples
        .iter()
        .map(|sample| (sample.clamp(-1., 1.) * i16::MAX as f32) as i16)
        .collect()
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Process {
    pub pid: u32,
    pub parent: u32,
    pub created_at: u64,
    pub allowed: bool,
    pub echo: bool,
}

fn lineage(pid: u32, processes: &BTreeMap<u32, Process>) -> Option<Vec<u32>> {
    let mut path = Vec::new();
    let mut current = processes.get(&pid)?;
    loop {
        if path.contains(&current.pid) || path.len() >= 64 {
            return None;
        }
        path.push(current.pid);
        let Some(parent) = processes.get(&current.parent) else {
            break;
        };
        // A recycled parent PID no longer describes this process's ancestry.
        if parent.created_at > current.created_at && current.created_at != 0 {
            break;
        }
        current = parent;
    }
    Some(path)
}

/// Process loopback includes descendants. Do not capture overlapping trees, any
/// call-client tree, or a broader tree that contains a call client.
pub(super) fn roots(
    sessions: &BTreeSet<u32>,
    processes: &BTreeMap<u32, Process>,
) -> BTreeSet<(u32, u64)> {
    let paths: BTreeMap<_, _> = processes
        .keys()
        .filter_map(|pid| lineage(*pid, processes).map(|path| (*pid, path)))
        .collect();
    let candidates: BTreeSet<_> = sessions
        .iter()
        .copied()
        .filter(|pid| {
            let Some(process) = processes.get(pid) else {
                return false;
            };
            let Some(path) = paths.get(pid) else {
                return false;
            };
            process.allowed
                && process.created_at != 0
                && !path.iter().any(|p| processes[p].echo)
                && !processes
                    .values()
                    .any(|p| p.echo && paths.get(&p.pid).is_some_and(|path| path.contains(pid)))
        })
        .collect();
    candidates
        .iter()
        .filter(|pid| {
            !paths[pid]
                .iter()
                .skip(1)
                .any(|parent| candidates.contains(parent))
        })
        .map(|pid| (*pid, processes[pid].created_at))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bytes(left: f32, right: f32, frames: usize) -> Vec<u8> {
        [left.to_le_bytes(), right.to_le_bytes()]
            .concat()
            .into_iter()
            .cycle()
            .take(frames * 8)
            .collect()
    }
    #[test]
    fn mixes_without_attenuating_a_single_app_and_preserves_stereo() {
        let mut a = Buffer::default();
        let mut b = Buffer::default();
        a.push(&bytes(0.25, -0.5, 480));
        b.push(&bytes(0.5, -0.75, 480));
        let mut output = [0.; SAMPLES];
        a.add_to(&mut output);
        assert_eq!(output[0..2], [0.25, -0.5]);
        b.add_to(&mut output);
        let pcm = pcm(&output);
        assert_eq!(pcm[0], (0.75 * i16::MAX as f32) as i16);
        assert_eq!(pcm[1], -i16::MAX);
        let mut next = [0.; SAMPLES];
        a.add_to(&mut next);
        assert_eq!(next, [0.; SAMPLES]);
    }
    #[test]
    fn queue_discards_old_audio_and_invalid_samples_stay_silent() {
        let mut buffer = Buffer::default();
        buffer.push(&bytes(0.5, -0.5, 480 * 10));
        buffer.push(&bytes(f32::NAN, f32::INFINITY, 480 * 3));
        assert_eq!(buffer.0.len(), SAMPLES * 3);
        let mut output = [0.; SAMPLES];
        buffer.add_to(&mut output);
        assert_eq!(output, [0.; SAMPLES]);
    }
    fn process(pid: u32, parent: u32, echo: bool, allowed: bool) -> Process {
        Process {
            pid,
            parent,
            created_at: pid as u64,
            echo,
            allowed,
        }
    }
    #[test]
    fn excludes_call_clients_and_their_helpers_without_blocking_every_explorer_child() {
        let processes = [
            process(1, 0, false, false),
            process(2, 1, true, false),
            process(3, 2, false, true),
            process(4, 1, false, true),
            process(5, 4, false, true),
            process(6, 1, true, false),
            process(7, 6, false, true),
        ]
        .into_iter()
        .map(|p| (p.pid, p))
        .collect();
        assert_eq!(
            roots(&[1, 2, 3, 4, 5, 6, 7].into(), &processes),
            [(4, 4)].into()
        );
    }
    #[test]
    fn removes_broad_roots_when_a_call_client_appears_under_them() {
        let mut processes: BTreeMap<_, _> =
            [process(1, 0, false, true), process(2, 1, false, true)]
                .into_iter()
                .map(|p| (p.pid, p))
                .collect();
        assert_eq!(roots(&[1, 2].into(), &processes), [(1, 1)].into());
        processes.insert(3, process(3, 1, true, false));
        assert_eq!(roots(&[1, 2, 3].into(), &processes), [(2, 2)].into());
    }
    #[test]
    fn recycled_parent_and_cycles_do_not_capture_an_unrelated_tree() {
        let mut processes: BTreeMap<_, _> =
            [process(10, 20, false, true), process(20, 0, true, false)]
                .into_iter()
                .map(|p| (p.pid, p))
                .collect();
        assert_eq!(roots(&[10, 20].into(), &processes), [(10, 10)].into());
        processes.get_mut(&10).unwrap().created_at = 20;
        processes.get_mut(&20).unwrap().parent = 10;
        assert!(roots(&[10, 20].into(), &processes).is_empty());
    }
}
