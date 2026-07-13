//! Process-wide phase timing counters, used to break a full-snix eval+realise into
//! eval / substitute / build time. Callers wrap the interesting awaits with [Phase::record]
//! (cumulative time, summed across concurrent tasks) or [WallPhase::enter] (wall-clock time
//! during which the phase is active on at least one task — the right measure under concurrency).
//! snix-eval prints [report_json] at exit. Note substitute happens inside pathinfo_get.
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Monotonic base so [WallPhase] can store timestamps as plain u64 nanos in atomics.
fn base() -> Instant {
    static BASE: OnceLock<Instant> = OnceLock::new();
    *BASE.get_or_init(Instant::now)
}

pub struct Phase {
    name: &'static str,
    nanos: AtomicU64,
    count: AtomicU64,
}

impl Phase {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            nanos: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    pub fn record(&self, started: Instant) {
        self.nanos
            .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    fn secs(&self) -> f64 {
        self.nanos.load(Ordering::Relaxed) as f64 / 1e9
    }
}

/// Wall-clock tracker: measures the union of intervals during which the phase is active on at
/// least one task. Unlike [Phase] (which sums per-task durations and so inflates ~Nx under
/// N-way concurrency), this yields time that is directly comparable to the process wall.
/// Lock-free: only the 0->1 enter sets the start and only the 1->0 exit accumulates, so `start`
/// is stable in between (no task can drive active to 0 while the first is still inside).
pub struct WallPhase {
    name: &'static str,
    active: AtomicU64,
    start_nanos: AtomicU64,
    wall_nanos: AtomicU64,
}

impl WallPhase {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            active: AtomicU64::new(0),
            start_nanos: AtomicU64::new(0),
            wall_nanos: AtomicU64::new(0),
        }
    }

    pub fn enter(&'static self) -> WallGuard {
        if self.active.fetch_add(1, Ordering::SeqCst) == 0 {
            self.start_nanos
                .store(base().elapsed().as_nanos() as u64, Ordering::SeqCst);
        }
        WallGuard(self)
    }

    fn secs(&self) -> f64 {
        self.wall_nanos.load(Ordering::Relaxed) as f64 / 1e9
    }
}

pub struct WallGuard(&'static WallPhase);

impl Drop for WallGuard {
    fn drop(&mut self) {
        if self.0.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            let end = base().elapsed().as_nanos() as u64;
            let start = self.0.start_nanos.load(Ordering::SeqCst);
            self.0
                .wall_nanos
                .fetch_add(end.saturating_sub(start), Ordering::Relaxed);
        }
    }
}

pub static PATHINFO_GET: Phase = Phase::new("pathinfo_get");
pub static SUBSTITUTE: Phase = Phase::new("substitute");
pub static FETCH: Phase = Phase::new("fetch");
pub static BUILD: Phase = Phase::new("build");
pub static NAR_CALC: Phase = Phase::new("nar_calc");
pub static DESCEND: Phase = Phase::new("descend");
pub static BLOB_READ: Phase = Phase::new("blob_read");
pub static DIR_GET: Phase = Phase::new("dir_get");
/// Cumulative time spent finalizing (close()/PUT) substituted blobs — the small-file ingest write.
pub static WRITE: Phase = Phase::new("write");

const ALL: [&Phase; 9] = [
    &PATHINFO_GET,
    &SUBSTITUTE,
    &FETCH,
    &BUILD,
    &NAR_CALC,
    &DESCEND,
    &BLOB_READ,
    &DIR_GET,
    &WRITE,
];

/// Union wall-clock during which any store I/O is in flight; wall minus this is eval/compute time.
pub static IO_WALL: WallPhase = WallPhase::new("io_wall");
/// Union wall-clock during which a blob finalize (ingest write) is in flight.
pub static WRITE_WALL: WallPhase = WallPhase::new("write_wall");

const ALL_WALL: [&WallPhase; 2] = [&IO_WALL, &WRITE_WALL];

/// One-line JSON: {"phase":{"secs":1.2,"count":34},...}. WallPhase entries carry count 0.
pub fn report_json() -> String {
    let mut out = String::from("{");
    for p in ALL.iter() {
        out.push_str(&format!(
            "\"{}\":{{\"secs\":{:.3},\"count\":{}}},",
            p.name,
            p.secs(),
            p.count.load(Ordering::Relaxed)
        ));
    }
    for (i, p) in ALL_WALL.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "\"{}\":{{\"secs\":{:.3},\"count\":0}}",
            p.name,
            p.secs()
        ));
    }
    out.push('}');
    out
}
