//! Process-wide phase timing counters, used to break a full-snix eval+realise into
//! eval / substitute / build time. Callers wrap the interesting awaits with [Phase::record];
//! snix-eval prints [report_json] at exit. Note substitute happens inside pathinfo_get.
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

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
}

pub static PATHINFO_GET: Phase = Phase::new("pathinfo_get");
pub static SUBSTITUTE: Phase = Phase::new("substitute");
pub static FETCH: Phase = Phase::new("fetch");
pub static BUILD: Phase = Phase::new("build");
pub static NAR_CALC: Phase = Phase::new("nar_calc");
pub static DESCEND: Phase = Phase::new("descend");
pub static BLOB_READ: Phase = Phase::new("blob_read");
pub static DIR_GET: Phase = Phase::new("dir_get");

const ALL: [&Phase; 8] = [
    &PATHINFO_GET,
    &SUBSTITUTE,
    &FETCH,
    &BUILD,
    &NAR_CALC,
    &DESCEND,
    &BLOB_READ,
    &DIR_GET,
];

/// One-line JSON: {"phase":{"secs":1.2,"count":34},...}
pub fn report_json() -> String {
    let mut out = String::from("{");
    for (i, p) in ALL.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "\"{}\":{{\"secs\":{:.3},\"count\":{}}}",
            p.name,
            p.nanos.load(Ordering::Relaxed) as f64 / 1e9,
            p.count.load(Ordering::Relaxed)
        ));
    }
    out.push('}');
    out
}
