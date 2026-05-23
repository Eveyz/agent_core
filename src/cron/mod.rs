use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub description: String,
    pub schedule: CronSchedule,
    pub task_prompt: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_run: Option<DateTime<Utc>>,
    pub run_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronSchedule {
    pub interval_secs: u64,
}

impl CronSchedule {
    pub fn once_after(secs: u64) -> Self {
        Self {
            interval_secs: secs,
        }
    }

    pub fn every_minutes(mins: u64) -> Self {
        Self {
            interval_secs: mins * 60,
        }
    }

    pub fn every_hours(hours: u64) -> Self {
        Self {
            interval_secs: hours * 3600,
        }
    }
}

pub struct CronScheduler {
    jobs: HashMap<String, CronJob>,
}

impl Default for CronScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl CronScheduler {
    pub fn new() -> Self {
        Self {
            jobs: HashMap::new(),
        }
    }

    pub fn add(&mut self, job: CronJob) {
        self.jobs.insert(job.id.clone(), job);
    }

    pub fn remove(&mut self, id: &str) -> Option<CronJob> {
        self.jobs.remove(id)
    }

    pub fn enable(&mut self, id: &str) -> Result<(), String> {
        let job = self
            .jobs
            .get_mut(id)
            .ok_or_else(|| format!("cron job '{}' not found", id))?;
        job.enabled = true;
        Ok(())
    }

    pub fn disable(&mut self, id: &str) -> Result<(), String> {
        let job = self
            .jobs
            .get_mut(id)
            .ok_or_else(|| format!("cron job '{}' not found", id))?;
        job.enabled = false;
        Ok(())
    }

    pub fn due_jobs(&mut self) -> Vec<&mut CronJob> {
        let now = Utc::now();
        self.jobs
            .values_mut()
            .filter(|job| {
                if !job.enabled {
                    return false;
                }
                match job.last_run {
                    None => true,
                    Some(last) => {
                        let elapsed = (now - last).num_seconds() as u64;
                        elapsed >= job.schedule.interval_secs
                    }
                }
            })
            .collect()
    }

    pub fn mark_run(&mut self, id: &str) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.last_run = Some(Utc::now());
            job.run_count += 1;
        }
    }

    pub fn list(&self) -> Vec<&CronJob> {
        self.jobs.values().collect()
    }

    pub fn get(&self, id: &str) -> Option<&CronJob> {
        self.jobs.get(id)
    }

    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }
}
