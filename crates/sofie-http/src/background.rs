/// Tasks a server starts for itself, stopped when `serve` returns, however it returns: each
/// holds the database, and one forgotten would outlive the server for the life of the process.
#[derive(Default)]
pub(crate) struct Background(Vec<tokio::task::AbortHandle>);

impl Background {
    pub(crate) fn spawn(&mut self, task: impl Future<Output = ()> + Send + 'static) {
        self.0.push(tokio::spawn(task).abort_handle());
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}
