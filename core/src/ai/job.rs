//! Generic background-job handle for AI sidecars.
//!
//! A [`Job`] runs one closure on a worker thread and records progress +
//! result behind a lock. The DAW never blocks on it: poll with
//! [`Job::poll`], collect with [`Job::try_take`] / [`Job::wait`], abandon
//! with [`Job::cancel`]. Dropping a job detaches it; the worker still
//! finishes but its result is discarded.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Lifecycle of one background job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Running,
    Done,
    Cancelled,
}

/// Cooperative control handed to the worker closure.
#[derive(Debug, Clone)]
pub struct JobControl {
    progress: Arc<Mutex<f64>>,
    cancel: Arc<AtomicBool>,
}

fn lock_recover<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            m.clear_poison();
            poisoned.into_inner()
        }
    }
}

impl JobControl {
    /// Report progress in 0.0..=1.0 (clamped). Best-effort: the DAW reads
    /// it via [`Job::poll`]; transcription never depends on it.
    pub fn set_progress(&self, p: f64) {
        *lock_recover(&self.progress) = p.clamp(0.0, 1.0);
    }

    /// Workers check this between chunks and return early when set, so
    /// cancel stays prompt even on long inputs.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

struct JobInner<T> {
    status: JobStatus,
    progress: f64,
    result: Option<T>,
}

/// One background job. `T` is the finished value (e.g. a `MidiClip`).
pub struct Job<T> {
    inner: Arc<Mutex<JobInner<T>>>,
    progress: Arc<Mutex<f64>>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

struct PanicCancelGuard<T> {
    inner: Arc<Mutex<JobInner<T>>>,
    finished: bool,
}

impl<T> Drop for PanicCancelGuard<T> {
    fn drop(&mut self) {
        if !self.finished {
            lock_recover(&self.inner).status = JobStatus::Cancelled;
        }
    }
}

impl<T: Send + 'static> Job<T> {
    /// Spawn `work` on a new thread. The closure receives a [`JobControl`]
    /// and returns the finished value.
    pub fn submit(work: impl FnOnce(&JobControl) -> T + Send + 'static) -> Self {
        let inner = Arc::new(Mutex::new(JobInner {
            status: JobStatus::Pending,
            progress: 0.0,
            result: None,
        }));
        let progress = Arc::new(Mutex::new(0.0f64));
        let cancel = Arc::new(AtomicBool::new(false));
        let ctl = JobControl {
            progress: Arc::clone(&progress),
            cancel: Arc::clone(&cancel),
        };
        let inner2 = Arc::clone(&inner);
        let progress2 = Arc::clone(&progress);
        let cancel2 = Arc::clone(&cancel);
        let handle = std::thread::spawn(move || {
            let mut panic_guard = PanicCancelGuard {
                inner: Arc::clone(&inner2),
                finished: false,
            };
            lock_recover(&inner2).status = JobStatus::Running;
            let value = work(&ctl);
            let mut guard = lock_recover(&inner2);
            if cancel2.load(Ordering::SeqCst) {
                guard.status = JobStatus::Cancelled;
            } else {
                guard.status = JobStatus::Done;
                guard.progress = 1.0;
                *lock_recover(&progress2) = 1.0;
                guard.result = Some(value);
            }
            panic_guard.finished = true;
        });
        Self {
            inner,
            progress,
            cancel,
            handle: Some(handle),
        }
    }

    /// Non-blocking snapshot: current status plus last reported progress.
    pub fn poll(&self) -> (JobStatus, f64) {
        let guard = lock_recover(&self.inner);
        let p = *lock_recover(&self.progress);
        (guard.status, p)
    }

    /// Ask the worker to stop early. The result (if any) is discarded.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Take the finished value when ready; `None` while pending/running
    /// (or after cancel / a previous take).
    pub fn try_take(&self) -> Option<T>
    where
        T: Clone,
    {
        lock_recover(&self.inner).result.clone()
    }

    /// Block until the worker finishes (at most `timeout`), then take the
    /// value. Returns `None` on timeout, cancel, or a previous take.
    pub fn wait(&mut self, timeout: Duration) -> Option<T>
    where
        T: Clone,
    {
        let start = Instant::now();
        loop {
            {
                let guard = lock_recover(&self.inner);
                if guard.status == JobStatus::Done || guard.status == JobStatus::Cancelled {
                    break;
                }
            }
            if start.elapsed() >= timeout {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        lock_recover(&self.inner).result.clone()
    }
}

impl<T> Job<T> {
    /// Join the worker thread. Called automatically on drop.
    pub fn join(&mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        self.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_runs_to_done_with_progress() {
        let mut job = Job::submit(|ctl| {
            ctl.set_progress(0.5);
            41u32 + 1
        });
        let v = job.wait(Duration::from_secs(5)).expect("job value");
        assert_eq!(v, 42);
        let (status, p) = job.poll();
        assert_eq!(status, JobStatus::Done);
        assert_eq!(p, 1.0);
    }

    #[test]
    fn cancelled_job_discards_result() {
        let mut job = Job::submit(|ctl| {
            ctl.set_progress(0.1);
            while !ctl.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            7u32
        });
        // Let the worker start, then cancel and join.
        std::thread::sleep(Duration::from_millis(20));
        job.cancel();
        job.join();
        assert_eq!(job.poll().0, JobStatus::Cancelled);
    }

    #[test]
    fn panicking_worker_marks_cancelled_and_recovers_locks() {
        let mut job: Job<u32> = Job::submit(|ctl| {
            ctl.set_progress(0.25);
            panic!("simulated worker failure");
        });
        assert_eq!(job.wait(Duration::from_millis(500)), None);
        let (status, _) = job.poll();
        assert_eq!(status, JobStatus::Cancelled);
        assert_eq!(job.try_take(), None);
    }
}
