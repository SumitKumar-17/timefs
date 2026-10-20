//! Background thread that checkpoints the working tree on a fixed interval.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::shared::Shared;

pub struct Checkpointer {
    stop: Arc<(Mutex<bool>, Condvar)>,
    handle: Option<JoinHandle<()>>,
}

impl Checkpointer {
    pub fn spawn(shared: Arc<Shared>, interval: Duration) -> Self {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let signal = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("timefs-checkpoint".into())
            .spawn(move || run(shared, interval, signal))
            .expect("spawn checkpoint thread");
        Checkpointer {
            stop,
            handle: Some(handle),
        }
    }

    /// Stop the thread and wait for it to exit.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        *self.stop.0.lock().unwrap() = true;
        self.stop.1.notify_all();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Checkpointer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(shared: Arc<Shared>, interval: Duration, signal: Arc<(Mutex<bool>, Condvar)>) {
    let (lock, cv) = &*signal;
    let mut stopped = lock.lock().unwrap();
    loop {
        let (guard, timeout) = cv.wait_timeout(stopped, interval).unwrap();
        stopped = guard;
        if *stopped {
            return;
        }
        if timeout.timed_out() {
            if let Err(e) = shared.checkpoint() {
                eprintln!("timefs: periodic checkpoint failed: {e}");
            }
        }
    }
}
