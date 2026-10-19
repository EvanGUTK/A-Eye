//! One owner serializes native side effects; async IPC replies cannot reorder Connect and Stop.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::host::{Action, Host, Outcome};

const MAX_PENDING_ACTIONS: usize = 8;
type Reply = Box<dyn FnOnce(Outcome) + Send>;

enum Job {
    Action(Action, Reply),
    Shutdown,
}

pub struct Worker {
    jobs: mpsc::SyncSender<Job>,
    stopping: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Worker {
    pub fn start(mut host: Host) -> Self {
        let (jobs, receiver) = mpsc::sync_channel(MAX_PENDING_ACTIONS);
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = stopping.clone();
        let thread = thread::spawn(move || {
            while let Ok(Job::Action(action, reply)) = receiver.recv() {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                reply(host.perform(action));
            }
            host.shutdown();
        });
        Self {
            jobs,
            stopping,
            thread: Mutex::new(Some(thread)),
        }
    }

    pub fn submit(
        &self,
        action: Action,
        reply: impl FnOnce(Outcome) + Send + 'static,
    ) -> Result<(), String> {
        if self.stopping.load(Ordering::Acquire) {
            return Err("Desktop is closing".into());
        }
        self.jobs
            .try_send(Job::Action(action, Box::new(reply)))
            .map_err(|_| "Desktop command queue is full or unavailable; wait and reconnect".into())
    }

    pub fn shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        // A full queue already wakes the worker; the stop flag prevents queued work after close.
        let _ = self.jobs.try_send(Job::Shutdown);
        let mut thread = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(thread) = thread.take() {
            if thread.join().is_err() {
                eprintln!("Desktop worker failed during shutdown");
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn native_action_errors_preserve_fifo_and_shutdown_rejects_work() {
        let worker = Worker::start(Host::new(PathBuf::from("missing-mcp-sidecar")));
        let (sender, receiver) = mpsc::channel();
        for (index, action) in [Action::Connect {}, Action::Disconnect {}, Action::Status {}]
            .into_iter()
            .enumerate()
        {
            let sender = sender.clone();
            worker
                .submit(action, move |outcome| {
                    sender
                        .send((index, serde_json::to_value(outcome).unwrap()))
                        .unwrap()
                })
                .unwrap();
        }
        for index in 0..3 {
            let (received, snapshot) = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(received, index);
            assert_eq!(snapshot["status"]["connected"], false);
            assert_eq!(snapshot["status"]["error"].is_string(), index == 0);
        }
        worker.shutdown();
        assert!(worker.submit(Action::Connect {}, |_| {}).is_err());
    }
}
