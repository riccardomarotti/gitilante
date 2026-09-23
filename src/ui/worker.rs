//! Background execution of Git operations (SPEC section 21).
//!
//! The UI never runs Git on the main thread: tasks are queued to a dedicated
//! worker thread and their results are delivered back on the GTK main loop
//! through a channel, so completion callbacks may freely use `Rc` state.

use std::sync::mpsc::{Sender, channel};
use std::thread;

use gtk4::glib;

type Task = Box<dyn FnOnce() + Send + 'static>;

/// Runs closures on a dedicated worker thread.
pub struct Worker {
    sender: Sender<Task>,
}

impl Worker {
    /// Starts a worker thread.
    pub fn new() -> Self {
        let (sender, receiver) = channel::<Task>();
        thread::Builder::new()
            .name("gitilante-worker".to_owned())
            .spawn(move || {
                while let Ok(task) = receiver.recv() {
                    task();
                }
            })
            .expect("start git worker thread");
        Self { sender }
    }

    /// Runs `task` on the worker thread, then calls `done` on the GTK main loop
    /// with its result.
    pub fn run<T, F, D>(&self, task: F, done: D)
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
        D: FnOnce(T) + 'static,
    {
        let (result_sender, result_receiver) = async_channel::bounded::<T>(1);
        glib::spawn_future_local(async move {
            if let Ok(result) = result_receiver.recv().await {
                done(result);
            }
        });

        let task: Task = Box::new(move || {
            let result = task();
            // Fails only when the window is already gone.
            let _ = result_sender.send_blocking(result);
        });
        self.sender.send(task).expect("git worker thread alive");
    }
}

impl Default for Worker {
    fn default() -> Self {
        Self::new()
    }
}
