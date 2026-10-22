//! Reuse the process runtime instead of Iced creating one worker per logical CPU.
use std::{future::Future, io};

pub(super) struct SharedExecutor(tokio::runtime::Handle);

impl iced::Executor for SharedExecutor {
    /// Require the runtime supplied by the process entry point.
    fn new() -> Result<Self, io::Error> {
        tokio::runtime::Handle::try_current()
            .map(Self)
            .map_err(io::Error::other)
    }

    /// Schedule UI work on the same bounded worker pool as the service tasks.
    fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) {
        self.0.spawn(future);
    }

    /// Drive startup work from the native event-loop thread, never a runtime worker.
    fn block_on<T>(&self, future: impl Future<Output = T>) -> T {
        self.0.block_on(future)
    }

    /// Make Tokio timer and channel construction use the process runtime.
    fn enter<R>(&self, operation: impl FnOnce() -> R) -> R {
        let _context = self.0.enter();
        operation()
    }
}
