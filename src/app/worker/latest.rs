//! Single-slot mailbox: pending work is replaced by the latest request.
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
struct Slot<T> {
    job: Mutex<Option<T>>,
    wake: Condvar,
    stopped: AtomicBool,
}
pub struct Latest<T> {
    slot: Arc<Slot<T>>,
}
impl<T: Send + 'static> Latest<T> {
    pub fn new(mut run: impl FnMut(T) + Send + 'static) -> Self {
        let slot = Arc::new(Slot {
            job: Mutex::new(None),
            wake: Condvar::new(),
            stopped: AtomicBool::new(false),
        });
        let thread = slot.clone();
        std::thread::spawn(move || {
            loop {
                let mut lock = thread.job.lock().unwrap();
                while lock.is_none() && !thread.stopped.load(Ordering::Relaxed) {
                    lock = thread.wake.wait(lock).unwrap();
                }
                if thread.stopped.load(Ordering::Relaxed) {
                    break;
                }
                let job = lock.take().unwrap();
                drop(lock);
                // A panic must not end the thread, or later jobs would wait forever.
                // Workers with state catch their own panics to report and rebuild it.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(job)));
            }
        });
        Self { slot }
    }
    pub fn submit(&self, job: T) {
        *self.slot.job.lock().unwrap() = Some(job);
        self.slot.wake.notify_one();
    }
}
/// The message a caught panic carries.
pub(crate) fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}
impl<T> Drop for Latest<T> {
    fn drop(&mut self) {
        let _guard = self.slot.job.lock().unwrap();
        self.slot.stopped.store(true, Ordering::Relaxed);
        self.slot.wake.notify_one();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_jobs_are_coalesced() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = Latest::new(move |i: u32| {
            started_tx.send(i).unwrap();
            if i == 1 {
                release_rx.recv().unwrap();
            }
        });
        worker.submit(1);
        assert_eq!(started_rx.recv().unwrap(), 1);
        worker.submit(2);
        worker.submit(3);
        release_tx.send(()).unwrap();
        assert_eq!(
            started_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            3
        );
    }
    #[test]
    fn a_panicking_job_does_not_stop_the_worker() {
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = Latest::new(move |i: u32| {
            tx.send(i).unwrap();
            assert_ne!(i, 1, "job 1 panics");
        });
        let next = || rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        worker.submit(1);
        assert_eq!(next(), 1);
        worker.submit(2);
        assert_eq!(next(), 2);
    }
}
