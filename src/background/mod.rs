use anyhow::Result;
use std::collections::HashMap;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum Notification {
    Completed {
        task_id: String,
        output: String,
    },
    Failed {
        task_id: String,
        error: String,
    },
    Progress {
        task_id: String,
        message: String,
    },
}

pub struct BackgroundTask {
    pub id: String,
    pub description: String,
    pub status: BackgroundStatus,
}

#[derive(Debug, Clone)]
pub enum BackgroundStatus {
    Running,
    Completed(String),
    Failed(String),
}

pub struct BackgroundPool {
    tasks: HashMap<String, BackgroundStatus>,
    rx: mpsc::UnboundedReceiver<Notification>,
    tx: mpsc::UnboundedSender<Notification>,
}

impl BackgroundPool {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tasks: HashMap::new(),
            rx,
            tx,
        }
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<Notification> {
        self.tx.clone()
    }

    pub fn spawn<F, Fut>(&mut self, task_id: &str, _description: &str, f: F)
    where
        F: FnOnce(mpsc::UnboundedSender<Notification>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<String>> + Send + 'static,
    {
        let id = task_id.to_string();
        let tx = self.tx.clone();

        self.tasks
            .insert(id.clone(), BackgroundStatus::Running);

        tokio::spawn(async move {
            let result = f(tx.clone()).await;
            match result {
                Ok(output) => {
                    let _ = tx.send(Notification::Completed {
                        task_id: id.clone(),
                        output: output.clone(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(Notification::Failed {
                        task_id: id.clone(),
                        error: e.to_string(),
                    });
                }
            }
        });
    }

    pub fn poll_notifications(&mut self) -> Vec<Notification> {
        let mut notifications = Vec::new();
        while let Ok(notification) = self.rx.try_recv() {
            match &notification {
                Notification::Completed { task_id, output } => {
                    self.tasks
                        .insert(task_id.clone(), BackgroundStatus::Completed(output.clone()));
                }
                Notification::Failed { task_id, error } => {
                    self.tasks
                        .insert(task_id.clone(), BackgroundStatus::Failed(error.clone()));
                }
                _ => {}
            }
            notifications.push(notification);
        }
        notifications
    }

    pub fn status(&self, task_id: &str) -> Option<&BackgroundStatus> {
        self.tasks.get(task_id)
    }

    pub fn list_tasks(&self) -> Vec<(&str, &BackgroundStatus)> {
        self.tasks
            .iter()
            .map(|(id, status)| (id.as_str(), status))
            .collect()
    }

    pub fn running_count(&self) -> usize {
        self.tasks
            .values()
            .filter(|s| matches!(s, BackgroundStatus::Running))
            .count()
    }
}

impl Default for BackgroundPool {
    fn default() -> Self {
        Self::new()
    }
}
