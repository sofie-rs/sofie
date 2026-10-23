use super::{JobContext, Worker};
use crate::{Call, Trigger, erase};

impl Worker {
    #[must_use]
    pub fn job<Args: 'static>(self, name: &'static str, job: impl Call<JobContext, Args>) -> Self {
        self.register(name, Trigger::Queued, erase(job))
    }
}
