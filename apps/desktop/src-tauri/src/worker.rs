//! One owner serializes native side effects; async IPC replies cannot reorder Connect and Stop.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::host::{Action, Description, Host, Outcome};

const MAX_PENDING_ACTIONS: usize = 8;
type Reply = Box<dyn FnOnce(Outcome) + Send>;
type DescribeReply = Box<dyn FnOnce(Description) + Send>;

enum Job {
    Action(Action, Reply),
    Describe(u64, DescribeReply),
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
            while let Ok(job) = receiver.recv() {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                match job {
                    Job::Action(action, reply) => reply(host.perform(action)),
                    Job::Describe(capture_id, reply) => {
                        start_description(&mut host, capture_id, reply)
                    }
                    Job::Shutdown => break,
                }
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

    /// Queued in order with actions, so a describe sees the frame its preceding capture stored.
    pub fn submit_describe(
        &self,
        capture_id: u64,
        reply: impl FnOnce(Description) + Send + 'static,
    ) -> Result<(), String> {
        if self.stopping.load(Ordering::Acquire) {
            return Err("Desktop is closing".into());
        }
        self.jobs
            .try_send(Job::Describe(capture_id, Box::new(reply)))
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

/// The model call can take minutes, so it runs on its own thread: the host thread must keep
/// serving status polls, overlay expiry and Disconnect meanwhile. The thread is detached; its
/// request is bounded by the client timeout and a closed window discards the reply.
fn start_description(host: &mut Host, capture_id: u64, reply: DescribeReply) {
    let task = match host.begin_describe(capture_id) {
        Ok(task) => task,
        Err(error) => return reply(Description::failed(capture_id, error)),
    };
    // Shared so the failure paths below can still answer: the UI would otherwise wait for its deadline.
    let reply = Arc::new(Mutex::new(Some(reply)));
    let thread_reply = reply.clone();
    let spawned = thread::Builder::new()
        .name("aeye-describe".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task.run()));
            let description = outcome.unwrap_or_else(|_| {
                eprintln!("The description thread panicked");
                Description::failed(
                    capture_id,
                    "The local vision step failed unexpectedly".into(),
                )
            });
            answer_shared(&thread_reply, description);
        });
    if let Err(error) = spawned {
        eprintln!("Could not start the description thread: {error}");
        answer_shared(
            &reply,
            Description::failed(capture_id, "Could not start the description".into()),
        );
    }
}

fn answer_shared(reply: &Mutex<Option<DescribeReply>>, description: Description) {
    let taken = reply
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(send) = taken {
        send(description);
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
    use crate::vision::Describer;
    use std::path::PathBuf;
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(5);

    /// Blocks inside `describe` until the test releases it, like a cold model load.
    struct Gated(Mutex<mpsc::Receiver<()>>);

    impl Describer for Gated {
        fn model(&self) -> &str {
            "gated:1"
        }

        fn describe(
            &self,
            _png_base64: &str,
            _stopped: &dyn Fn() -> bool,
        ) -> Result<String, String> {
            self.0
                .lock()
                .unwrap()
                .recv_timeout(WAIT)
                .map_err(|_| "never released".to_string())?;
            Ok("a window".into())
        }
    }

    struct Panics;

    impl Describer for Panics {
        fn model(&self) -> &str {
            "panics:1"
        }

        fn describe(&self, _: &str, _: &dyn Fn() -> bool) -> Result<String, String> {
            panic!("model client bug");
        }
    }

    #[test]
    fn a_panicking_description_still_answers_and_frees_the_slot() {
        let mut host = Host::with_describer(PathBuf::from("missing"), Arc::new(Panics));
        host.set_test_frame(5, "display", "UE5H");
        let worker = Worker::start(host);
        for _ in 0..2 {
            let (tx, rx) = mpsc::channel();
            worker
                .submit_describe(5, move |d| {
                    tx.send(serde_json::to_value(d).unwrap()).unwrap()
                })
                .unwrap();
            let reply = rx.recv_timeout(WAIT).expect("no reply after a panic");
            assert_eq!(reply["error"], "The local vision step failed unexpectedly");
            assert!(reply["text"].is_null());
        }
        worker.shutdown();
    }

    #[test]
    fn a_slow_description_never_blocks_the_host_thread() {
        let (release, gate) = mpsc::channel();
        let mut host = Host::with_describer(
            PathBuf::from("missing-mcp-sidecar"),
            Arc::new(Gated(Mutex::new(gate))),
        );
        host.set_test_frame(3, "display", "UE5H");
        let worker = Worker::start(host);
        let (first_tx, first) = mpsc::channel();
        worker
            .submit_describe(3, move |d| {
                first_tx.send(serde_json::to_value(d).unwrap()).unwrap()
            })
            .unwrap();

        // The model call is stuck, yet status polls and disconnect-class actions still answer.
        let (status_tx, status) = mpsc::channel();
        worker
            .submit(Action::Status {}, move |o| status_tx.send(o).unwrap())
            .unwrap();
        status
            .recv_timeout(WAIT)
            .expect("host thread was blocked by the description");
        assert!(
            first.try_recv().is_err(),
            "description finished before release"
        );

        // A second request while one runs is refused immediately, not queued behind the model.
        let (busy_tx, busy) = mpsc::channel();
        worker
            .submit_describe(3, move |d| {
                busy_tx.send(serde_json::to_value(d).unwrap()).unwrap()
            })
            .unwrap();
        let busy = busy.recv_timeout(WAIT).unwrap();
        assert!(busy["error"].as_str().unwrap().contains("still running"));

        release.send(()).unwrap();
        let done = first.recv_timeout(WAIT).unwrap();
        assert_eq!(
            (done["text"].as_str(), done["capture_id"].as_u64()),
            (Some("a window"), Some(3))
        );
        assert_eq!(done["model"], "gated:1");

        // A stale id is rejected without touching the model.
        let (stale_tx, stale) = mpsc::channel();
        worker
            .submit_describe(99, move |d| {
                stale_tx.send(serde_json::to_value(d).unwrap()).unwrap()
            })
            .unwrap();
        let stale = stale.recv_timeout(WAIT).unwrap();
        assert!(stale["error"]
            .as_str()
            .unwrap()
            .contains("no longer current"));
        worker.shutdown();
        assert!(worker.submit_describe(3, |_| {}).is_err());
    }

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
