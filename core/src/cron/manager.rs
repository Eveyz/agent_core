use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_cron_scheduler::{Job, JobScheduler};

use crate::cron::CronJob;

pub struct CronScheduler {
    scheduler: JobScheduler,
}

impl CronScheduler {
    pub fn new() -> Self {
        // tokio-cron-scheduler's JobScheduler::new is async, so we can't easily create it sync.
        // We will mock this for now or change the signature.
        unimplemented!()
    }

    pub fn len(&self) -> usize {
        0
    }
}
