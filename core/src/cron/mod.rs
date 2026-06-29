pub mod manager;
pub use manager::CronScheduler;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, Result as SqlResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub cadence_type: String, // "Hourly", "Daily", "Weekly", "Custom"
    pub cadence_value: String,
    pub prompt: String,
    pub project: Option<String>,
    pub skills: Vec<String>,
    pub permission_level: String,
    pub max_concurrency: Option<u32>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobRun {
    pub id: String,
    pub cronjob_id: String,
    pub session_id: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: String,
}

pub struct CronjobStore;

impl CronjobStore {
    pub fn insert(db: &Connection, job: &CronJob) -> SqlResult<()> {
        let skills_json = serde_json::to_string(&job.skills).unwrap_or_else(|_| "[]".to_string());
        db.execute(
            "INSERT INTO cronjobs (id, name, cadence_type, cadence_value, prompt, project, skills, permission_level, max_concurrency, enabled, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                job.id,
                job.name,
                job.cadence_type,
                job.cadence_value,
                job.prompt,
                job.project,
                skills_json,
                job.permission_level,
                job.max_concurrency,
                job.enabled as i32,
                job.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn update(db: &Connection, job: &CronJob) -> SqlResult<()> {
        let skills_json = serde_json::to_string(&job.skills).unwrap_or_else(|_| "[]".to_string());
        db.execute(
            "UPDATE cronjobs SET name=?1, cadence_type=?2, cadence_value=?3, prompt=?4, project=?5, skills=?6, permission_level=?7, max_concurrency=?8, enabled=?9
             WHERE id=?10",
            params![
                job.name,
                job.cadence_type,
                job.cadence_value,
                job.prompt,
                job.project,
                skills_json,
                job.permission_level,
                job.max_concurrency,
                job.enabled as i32,
                job.id,
            ],
        )?;
        Ok(())
    }

    pub fn delete(db: &Connection, id: &str) -> SqlResult<()> {
        db.execute("DELETE FROM cronjobs WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn list(db: &Connection) -> SqlResult<Vec<CronJob>> {
        let mut stmt = db.prepare("SELECT id, name, cadence_type, cadence_value, prompt, project, skills, permission_level, max_concurrency, enabled, created_at FROM cronjobs")?;
        let rows = stmt.query_map([], |row| {
            let skills_str: String = row.get(6)?;
            let skills: Vec<String> = serde_json::from_str(&skills_str).unwrap_or_default();
            let created_at_str: String = row.get(10)?;
            let created_at = DateTime::parse_from_rfc3339(&created_at_str).unwrap_or_default().with_timezone(&Utc);

            Ok(CronJob {
                id: row.get(0)?,
                name: row.get(1)?,
                cadence_type: row.get(2)?,
                cadence_value: row.get(3)?,
                prompt: row.get(4)?,
                project: row.get(5)?,
                skills,
                permission_level: row.get(7)?,
                max_concurrency: row.get(8)?,
                enabled: row.get::<_, i32>(9)? != 0,
                created_at,
            })
        })?;

        let mut jobs = Vec::new();
        for job in rows {
            jobs.push(job?);
        }
        Ok(jobs)
    }
}
