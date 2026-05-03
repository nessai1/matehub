//! SFU hot-path microbenchmarks.
//!
//! These deliberately do NOT spin up `Rtc`/`SfuParticipant` — full media
//! forwarding needs live ICE/DTLS/SRTP and is covered by integration tests
//! against real browsers. Here we isolate the pure data-structure operations
//! that run per packet (`forward_media_now` lookup) and per leave
//! (`drop_from_forwarding`), plus the UDP-demux cache that sits in front of
//! every incoming datagram.

use std::collections::HashMap;
use std::hint::black_box;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use str0m::media::Mid;
use uuid::Uuid;

use matehub_video::sfu::SfuSession;

type Pid = Uuid;

// ─── helpers ──────────────────────────────────────────────────────────────

/// Build a session where one publisher fans out to N subscribers, one
/// (publisher, mid) → N (subscriber, mid) entry in the forwarding map.
fn session_with_subscribers(n: usize) -> (SfuSession, Pid, Mid) {
    let mut s = SfuSession::new(Uuid::new_v4());
    let publisher = Uuid::new_v4();
    let pub_mid = Mid::new();

    let targets: Vec<(Pid, Mid)> = (0..n).map(|_| (Uuid::new_v4(), Mid::new())).collect();
    s.forwarding_map.insert((publisher, pub_mid), targets);
    (s, publisher, pub_mid)
}

/// Populate a forwarding map shaped like a real N-way room: every
/// participant publishes both audio and video to every other participant.
/// Returns the session plus the pid we'll drop in the bench (last one).
fn room_shaped_map(n_participants: usize) -> (SfuSession, Pid) {
    let mut s = SfuSession::new(Uuid::new_v4());
    let pids: Vec<Pid> = (0..n_participants).map(|_| Uuid::new_v4()).collect();

    for &publisher in &pids {
        for kind_mid in [Mid::new(), Mid::new()] {
            let subscribers: Vec<(Pid, Mid)> = pids
                .iter()
                .copied()
                .filter(|&p| p != publisher)
                .map(|p| (p, Mid::new()))
                .collect();
            s.forwarding_map.insert((publisher, kind_mid), subscribers);
        }
    }

    (s, *pids.last().unwrap())
}

// ─── benchmarks ───────────────────────────────────────────────────────────

/// Hot path: how expensive is the fan-out target lookup per incoming RTP
/// packet? Compares the new O(1) `forwarding_map` against the old O(N)
/// scan (iterate all participants' tracks_out looking for origin match).
fn bench_forwarding_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("forwarding_lookup");

    for n in [1, 5, 20, 50, 200] {
        let (session, publisher, pub_mid) = session_with_subscribers(n);

        // New: HashMap lookup.
        group.bench_with_input(BenchmarkId::new("map_get", n), &n, |b, _| {
            b.iter(|| {
                let entry = session.forwarding_map.get(&(publisher, pub_mid));
                black_box(entry.map(|v| v.len()));
            });
        });

        // Baseline: simulate the pre-refactor scan. Not over real tracks_out
        // (we don't have SfuParticipants here) but over an equivalent flat
        // vec of (subscriber, origin, origin_mid, target_mid) — same big-O.
        let scan: Vec<(Pid, Pid, Mid, Mid)> = session
            .forwarding_map
            .iter()
            .flat_map(|(&(pub_p, pub_m), subs)| {
                subs.iter()
                    .map(move |&(sub_p, sub_m)| (sub_p, pub_p, pub_m, sub_m))
            })
            .collect();

        group.bench_with_input(BenchmarkId::new("linear_scan", n), &n, |b, _| {
            b.iter(|| {
                let hits: Vec<(Pid, Mid)> = scan
                    .iter()
                    .filter(|&&(_, p, m, _)| p == publisher && m == pub_mid)
                    .map(|&(s, _, _, sm)| (s, sm))
                    .collect();
                black_box(hits.len());
            });
        });
    }

    group.finish();
}

/// Leave path: how expensive is purging a participant from the forwarding
/// map? Parametrised by room size (N participants = 2N entries, one per
/// kind, each with (N-1) subscribers).
fn bench_drop_from_forwarding(c: &mut Criterion) {
    let mut group = c.benchmark_group("drop_from_forwarding");

    for n in [5, 20, 100] {
        group.bench_with_input(BenchmarkId::new("room_size", n), &n, |b, &n| {
            b.iter_batched(
                || room_shaped_map(n),
                |(mut s, gone)| {
                    s.drop_from_forwarding(black_box(gone));
                    black_box(s);
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

/// UDP demux cache: every incoming datagram hits this lookup BEFORE ICE
/// classification. Compares O(1) map hit against the old linear scan
/// (rtc.accepts() called on every participant in the worst case).
fn bench_addr_demux(c: &mut Criterion) {
    let mut group = c.benchmark_group("addr_demux");

    for n in [5, 20, 100, 500] {
        let sid = Uuid::new_v4();
        let mut map: HashMap<SocketAddr, (Uuid, Pid)> = HashMap::new();
        let mut addrs: Vec<SocketAddr> = Vec::with_capacity(n);
        for i in 0..n {
            let port: u16 = 10_000 + (i as u16);
            let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), port);
            let pid = Uuid::new_v4();
            map.insert(addr, (sid, pid));
            addrs.push(addr);
        }
        // Target: the last one (worst case for linear scan).
        let target = *addrs.last().unwrap();

        group.bench_with_input(BenchmarkId::new("hashmap_get", n), &n, |b, _| {
            b.iter(|| {
                black_box(map.get(&target).copied());
            });
        });

        // Simulate the slow path: compare addr to each entry's key. This
        // mirrors the shape of rtc.accepts() — one `== addr` check per
        // participant — which is what we used to do O(N) times per packet.
        let entries: Vec<(SocketAddr, (Uuid, Pid))> = map.iter().map(|(k, v)| (*k, *v)).collect();
        group.bench_with_input(BenchmarkId::new("linear_scan", n), &n, |b, _| {
            b.iter(|| {
                let hit = entries.iter().find(|(a, _)| *a == target).map(|(_, v)| *v);
                black_box(hit);
            });
        });
    }

    group.finish();
}

/// Baseline: `stream_id_for`-equivalent format cost. The function itself is
/// private; inline an equivalent `format!` so we can tell whether a
/// regression is in the format call or elsewhere.
fn bench_stream_id_format(c: &mut Criterion) {
    let pid = Uuid::new_v4();
    c.bench_function("stream_id_format", |b| {
        b.iter(|| {
            let s: String = format!("{}-{}", black_box(pid), black_box("audio"));
            black_box(s);
        });
    });
}

criterion_group!(
    benches,
    bench_forwarding_lookup,
    bench_drop_from_forwarding,
    bench_addr_demux,
    bench_stream_id_format,
);
criterion_main!(benches);
