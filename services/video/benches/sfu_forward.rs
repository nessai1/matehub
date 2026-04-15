use criterion::{Criterion, criterion_group, criterion_main};
use std::collections::HashMap;

use str0m::media::{MediaKind, Mid};
use uuid::Uuid;

use matehub_video::sfu::{SfuParticipant, SfuSession, TrackIn, TrackOut, TrackOutState};

fn mid(s: &str) -> Mid {
    Mid::from(s)
}

/// Build a session with 1 publisher + N subscribers, each with Open video TrackOut.
fn build_session(n_subscribers: usize) -> (Uuid, SfuSession, Uuid) {
    let session_id = Uuid::new_v4();
    let mut session = SfuSession::new(session_id);

    let publisher_id = Uuid::new_v4();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

    let mut publisher = SfuParticipant {
        id: publisher_id,
        user_id: "publisher".into(),
        rtc: str0m::Rtc::new(),
        ws_tx: tx.clone(),
        tracks_in: vec![
            TrackIn {
                mid: mid("audio0"),
                kind: MediaKind::Audio,
            },
            TrackIn {
                mid: mid("video0"),
                kind: MediaKind::Video,
            },
        ],
        tracks_out: vec![],
        pending_offer: None,
    };

    let addr: std::net::SocketAddr = "127.0.0.1:10000".parse().unwrap();
    publisher
        .rtc
        .add_local_candidate(str0m::Candidate::host(addr, "udp").unwrap());

    session.participants.insert(publisher_id, publisher);

    for i in 0..n_subscribers {
        let sub_id = Uuid::new_v4();
        let (sub_tx, _) = tokio::sync::mpsc::unbounded_channel();

        let subscriber = SfuParticipant {
            id: sub_id,
            user_id: format!("sub_{i}"),
            rtc: str0m::Rtc::new(),
            ws_tx: sub_tx,
            tracks_in: vec![],
            tracks_out: vec![
                TrackOut {
                    origin: publisher_id,
                    origin_mid: mid("audio0"),
                    kind: MediaKind::Audio,
                    state: TrackOutState::Open(mid(&format!("a_out_{i}"))),
                },
                TrackOut {
                    origin: publisher_id,
                    origin_mid: mid("video0"),
                    kind: MediaKind::Video,
                    state: TrackOutState::Open(mid(&format!("v_out_{i}"))),
                },
            ],
            pending_offer: None,
        };

        session.participants.insert(sub_id, subscriber);
    }

    (session_id, session, publisher_id)
}

/// Benchmark: collect forwarding targets (the lookup in forward_media_now).
fn bench_target_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("sfu_target_lookup");

    for n in [1, 5, 10, 20, 50] {
        group.bench_function(format!("{n}_subscribers"), |b| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let (_, session, publisher_id) = rt.block_on(async { build_session(n) });
            let video_mid = mid("video0");

            b.iter(|| {
                let targets: Vec<(Uuid, Mid)> = session
                    .participants
                    .iter()
                    .filter(|(pid, _)| **pid != publisher_id)
                    .filter_map(|(pid, p)| {
                        p.tracks_out
                            .iter()
                            .find(|t| t.origin == publisher_id && t.origin_mid == video_mid)
                            .and_then(|t| t.open_mid())
                            .map(|m| (*pid, m))
                    })
                    .collect();
                criterion::black_box(targets);
            });
        });
    }

    group.finish();
}

/// Benchmark: TrackOut.open_mid() scan with many tracks (large room).
fn bench_track_out_scan(c: &mut Criterion) {
    let publisher_id = Uuid::new_v4();
    let target_mid = mid("video0");

    let tracks: Vec<TrackOut> = (0..20)
        .map(|i| TrackOut {
            origin: if i == 5 { publisher_id } else { Uuid::new_v4() },
            origin_mid: target_mid,
            kind: MediaKind::Video,
            state: TrackOutState::Open(mid(&format!("out_{i}"))),
        })
        .collect();

    c.bench_function("track_out_find_in_20", |b| {
        b.iter(|| {
            let result = tracks
                .iter()
                .find(|t| t.origin == publisher_id && t.origin_mid == target_mid)
                .and_then(|t| t.open_mid());
            criterion::black_box(result);
        });
    });
}

/// Benchmark: HashMap<Uuid, _> get (simulates session.participants.get).
fn bench_participant_hashmap(c: &mut Criterion) {
    let mut group = c.benchmark_group("participant_hashmap_get");

    for n in [5, 20, 100] {
        group.bench_function(format!("{n}_entries"), |b| {
            let mut map: HashMap<Uuid, String> = HashMap::new();
            let target = Uuid::new_v4();
            for _ in 0..n - 1 {
                map.insert(Uuid::new_v4(), "other".into());
            }
            map.insert(target, "target".into());

            b.iter(|| {
                criterion::black_box(map.get(&target));
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_target_lookup,
    bench_track_out_scan,
    bench_participant_hashmap
);
criterion_main!(benches);
