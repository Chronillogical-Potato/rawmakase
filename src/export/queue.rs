//! The app's exports, one batch at a time: an Export chosen while another runs
//! waits its turn rather than running beside it, so however many are queued, one
//! photo's images are in memory. A waiting batch holds edits, not images, and can
//! be removed before it starts.
use super::batch::{self, Batch, Outcome, Progress};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

/// The queue of export batches and the thread that runs them.
pub struct Queue {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    waiting: VecDeque<(u64, Batch)>,
    running: Option<Running>,
    next: u64,
    closed: bool,
}

struct Running {
    ticket: u64,
    cancel: Arc<AtomicBool>,
    progress: Progress,
}

/// What the queue is doing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// The batch running, and how far it is.
    pub running: Option<(u64, Progress)>,
    /// The batches waiting, by ticket, with how many photos each has.
    pub waiting: Vec<(u64, usize)>,
}

impl Queue {
    /// Starts the queue's thread; `finished` gets each batch's outcomes, in its
    /// photos' order, and `changed` is called whenever its status changes.
    pub fn new(
        finished: impl Fn(u64, Vec<Outcome>) + Send + 'static,
        changed: impl Fn() + Send + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let worker = shared.clone();
        std::thread::spawn(move || {
            while let Some((ticket, batch, cancel)) = worker.next() {
                changed();
                let outcomes = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    batch::run(&batch, &cancel, |progress| {
                        if let Ok(mut state) = worker.state.lock()
                            && let Some(running) = &mut state.running
                        {
                            running.progress = progress;
                        }
                        changed();
                    })
                }))
                // A panic still finishes the batch, with what was not exported said.
                .unwrap_or_else(|_| {
                    vec![
                        Outcome::Failed("the export failed unexpectedly".into());
                        batch.photos.len()
                    ]
                });
                if let Ok(mut state) = worker.state.lock() {
                    state.running = None;
                }
                finished(ticket, outcomes);
                changed();
            }
        });
        Self { shared }
    }

    /// Queues `batch`, returning its ticket.
    pub fn submit(&self, batch: Batch) -> u64 {
        let mut state = self.shared.state.lock().expect("export queue");
        state.next += 1;
        let ticket = state.next;
        state.waiting.push_back((ticket, batch));
        self.shared.wake.notify_one();
        ticket
    }

    /// Removes a waiting batch; false when it has already started or finished.
    pub fn remove(&self, ticket: u64) -> bool {
        let mut state = self.shared.state.lock().expect("export queue");
        let before = state.waiting.len();
        state.waiting.retain(|(t, _)| *t != ticket);
        state.waiting.len() != before
    }

    /// Cancels the running batch if it is `ticket`; false otherwise.
    pub fn cancel(&self, ticket: u64) -> bool {
        let state = self.shared.state.lock().expect("export queue");
        match &state.running {
            Some(running) if running.ticket == ticket => {
                running.cancel.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    pub fn status(&self) -> Status {
        let state = self.shared.state.lock().expect("export queue");
        Status {
            running: state.running.as_ref().map(|r| (r.ticket, r.progress)),
            waiting: state
                .waiting
                .iter()
                .map(|(ticket, batch)| (*ticket, batch.photos.len()))
                .collect(),
        }
    }

    /// Whether a batch is running or waiting.
    pub fn busy(&self) -> bool {
        let state = self.shared.state.lock().expect("export queue");
        state.running.is_some() || !state.waiting.is_empty()
    }
}

impl Shared {
    /// The next batch to run, once there is one; `None` once the queue is dropped.
    fn next(&self) -> Option<(u64, Batch, Arc<AtomicBool>)> {
        let mut state = self.state.lock().ok()?;
        loop {
            if state.closed {
                return None;
            }
            if let Some((ticket, batch)) = state.waiting.pop_front() {
                let cancel = Arc::new(AtomicBool::new(false));
                state.running = Some(Running {
                    ticket,
                    cancel: cancel.clone(),
                    progress: Progress {
                        done: 0,
                        total: batch.photos.len(),
                        fraction: 0.,
                    },
                });
                return Some((ticket, batch, cancel));
            }
            state = self.wake.wait(state).ok()?;
        }
    }
}

impl Drop for Queue {
    /// Stops the thread once the running batch is done; waiting ones never start.
    fn drop(&mut self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.closed = true;
            state.waiting.clear();
        }
        self.shared.wake.notify_all();
    }
}
