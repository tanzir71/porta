use std::{
    collections::HashSet,
    net::IpAddr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareStats {
    pub visitors: u64,
    pub requests: u64,
    pub bytes_served: u64,
}

type StatsSink = dyn Fn(ShareStats) + Send + Sync + 'static;
type FirstVisitorSink = dyn Fn() + Send + Sync + 'static;

/// Session-scoped counters with a coalesced, at-most-once-per-second sink.
pub struct StatsReporter {
    visitors: Mutex<HashSet<IpAddr>>,
    requests: AtomicU64,
    bytes_served: AtomicU64,
    active: AtomicBool,
    dirty: AtomicBool,
    tick_scheduled: AtomicBool,
    first_visitor_seen: AtomicBool,
    sink: Box<StatsSink>,
    first_visitor_sink: Box<FirstVisitorSink>,
}

impl StatsReporter {
    pub fn new(sink: impl Fn(ShareStats) + Send + Sync + 'static) -> Arc<Self> {
        Self::with_first_visitor(sink, || {})
    }

    pub fn with_first_visitor(
        sink: impl Fn(ShareStats) + Send + Sync + 'static,
        first_visitor_sink: impl Fn() + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            visitors: Mutex::new(HashSet::new()),
            requests: AtomicU64::new(0),
            bytes_served: AtomicU64::new(0),
            active: AtomicBool::new(true),
            dirty: AtomicBool::new(false),
            tick_scheduled: AtomicBool::new(false),
            first_visitor_seen: AtomicBool::new(false),
            sink: Box::new(sink),
            first_visitor_sink: Box::new(first_visitor_sink),
        })
    }

    pub fn record_request(self: &Arc<Self>, visitor: Option<&str>) {
        if !self.active.load(Ordering::Acquire) {
            return;
        }
        saturating_add(&self.requests, 1);
        let is_new_visitor = visitor
            .and_then(|value| value.parse::<IpAddr>().ok())
            .is_some_and(|visitor| {
                self.visitors
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(visitor)
            });
        if is_new_visitor
            && self
                .first_visitor_seen
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            (self.first_visitor_sink)();
        }
        self.mark_dirty();
    }

    pub fn record_bytes(self: &Arc<Self>, bytes: usize) {
        if bytes == 0 || !self.active.load(Ordering::Acquire) {
            return;
        }
        saturating_add(&self.bytes_served, bytes as u64);
        self.mark_dirty();
    }

    pub fn snapshot(&self) -> ShareStats {
        ShareStats {
            visitors: self
                .visitors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len() as u64,
            requests: self.requests.load(Ordering::Relaxed),
            bytes_served: self.bytes_served.load(Ordering::Relaxed),
        }
    }

    pub fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
        self.dirty.store(false, Ordering::Release);
    }

    fn mark_dirty(self: &Arc<Self>) {
        self.dirty.store(true, Ordering::Release);
        if self
            .tick_scheduled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let reporter = Arc::clone(self);
            tauri::async_runtime::spawn(async move {
                reporter.run_ticks().await;
            });
        }
    }

    async fn run_ticks(self: Arc<Self>) {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if self.active.load(Ordering::Acquire) && self.dirty.swap(false, Ordering::AcqRel) {
                (self.sink)(self.snapshot());
            }

            self.tick_scheduled.store(false, Ordering::Release);
            if !self.active.load(Ordering::Acquire) || !self.dirty.load(Ordering::Acquire) {
                return;
            }
            if self
                .tick_scheduled
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }
        }
    }
}

fn saturating_add(counter: &AtomicU64, amount: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_add(amount))
    });
}
