//! Generic background-job handle for AI sidecars.
//!
//! A [`Job`] runs one closure on a worker thread and records progress +
//! result behind a lock. The DAW never blocks on it: poll with
//! [`Job::poll`], collect with [`Job::try_take`] / [`Job::wait`], abandon
//! with [`Job::cancel`]. Dropping a job detaches it; the worker still
//! finishes but its result is discarded.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
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

impl JobControl {
    /// Report progress in 0.0..=1.0 (clamped). Best-effort: the DAW reads
    /// it via [`Job::poll`]; transcription never depends on it.
    pub fn set_progress(&self, p: f64) {
        *self.progress.lock().expect("job progress lock") = p.clamp(0.0, 1.0);
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
            inner2.lock().expect("job lock").status = JobStatus::Running;
            let value = work(&ctl);
            let mut guard = inner2.lock().expect("job lock");
            if cancel2.load(Ordering::SeqCst) {
                guard.status = JobStatus::Cancelled;
            } else {
                guard.status = JobStatus::Done;
                guard.progress = 1.0;
                *progress2.lock().expect("job progress lock") = 1.0;
                guard.result = Some(value);
            }
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
        let guard = self.inner.lock().expect("job lock");
        let p = *self.progress.lock().expect("job progress lock");
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
        self.inner.lock().expect("job lock").result.clone()
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
                let guard = self.inner.lock().expect("job lock");
                if guard.status == JobStatus::Done
                    || guard.status == JobStatus::Cancelled
                {
                    break;
                }
            }
            if start.elapsed() >= timeout {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        self.inner.lock().expect("job lock").result.clone()
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
}
