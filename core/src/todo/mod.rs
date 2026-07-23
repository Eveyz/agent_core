use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::memory::storage::Storage;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TodoStatus::Pending => write!(f, "pending"),
            TodoStatus::InProgress => write!(f, "in_progress"),
            TodoStatus::Completed => write!(f, "completed"),
            TodoStatus::Blocked => write!(f, "blocked"),
        }
    }
}

impl TodoStatus {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Active,
    Parked,
    Finished,
    Cancelled,
}

impl std::fmt::Display for PlanStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanStatus::Active => write!(f, "active"),
            PlanStatus::Parked => write!(f, "parked"),
            PlanStatus::Finished => write!(f, "finished"),
            PlanStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl PlanStatus {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "parked" => Some(Self::Parked),
            "finished" => Some(Self::Finished),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub description: String,
    pub status: TodoStatus,
    pub depends_on: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl TodoItem {
    pub fn new(id: &str, description: &str) -> Self {
        Self {
            id: id.to_string(),
            description: description.to_string(),
            status: TodoStatus::Pending,
            depends_on: Vec::new(),
            created_at: Utc::now(),
            completed_at: None,
        }
    }

    pub fn with_depends_on(mut self, deps: Vec<String>) -> Self {
        self.depends_on = deps;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoList {
    pub items: Vec<TodoItem>,
    #[serde(skip)]
    storage_path: Option<PathBuf>,
}

impl Default for TodoList {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoList {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            storage_path: None,
        }
    }

    pub fn with_storage(path: PathBuf) -> Self {
        let items = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|content| serde_json::from_str(&content).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        Self {
            items,
            storage_path: Some(path),
        }
    }

    pub fn add(&mut self, item: TodoItem) {
        self.items.push(item);
        self.persist();
    }

    /// Replace the entire list with new items. IDs are auto-assigned (1, 2, 3, ...).
    /// **Destructive** — wipes all statuses. Prefer [`Self::merge_replace`] for replans.
    pub fn replace_all(&mut self, descriptions: Vec<String>) {
        self.items = descriptions
            .into_iter()
            .enumerate()
            .map(|(i, desc)| TodoItem::new(&(i + 1).to_string(), &desc))
            .collect();
        self.persist();
    }

    /// Merge a new plan description list with existing progress.
    pub fn merge_replace(&mut self, descriptions: Vec<String>) {
        let old = std::mem::take(&mut self.items);
        let mut used_old: Vec<bool> = vec![false; old.len()];

        for (i, desc) in descriptions.into_iter().enumerate() {
            let id = (i + 1).to_string();
            let norm = normalize_desc(&desc);
            let mut item = TodoItem::new(&id, &desc);

            if let Some((idx, prev)) = old.iter().enumerate().find(|(j, o)| {
                !used_old[*j] && normalize_desc(&o.description) == norm
            }) {
                used_old[idx] = true;
                item.status = prev.status;
                item.completed_at = prev.completed_at;
                item.depends_on = prev.depends_on.clone();
            }
            self.items.push(item);
        }

        let has_ip = self.items.iter().any(|i| i.status == TodoStatus::InProgress);
        if !has_ip {
            if let Some(ready_id) = self
                .ready_items()
                .first()
                .map(|i| i.id.clone())
                .or_else(|| {
                    self.items
                        .iter()
                        .find(|i| i.status == TodoStatus::Pending)
                        .map(|i| i.id.clone())
                })
            {
                let _ = self.update_status(&ready_id, TodoStatus::InProgress);
            }
        }

        self.persist();
    }

    pub fn ensure_active_step(&mut self) -> Option<String> {
        if let Some(ip) = self.in_progress().first() {
            return Some(ip.id.clone());
        }
        let next = self
            .ready_items()
            .first()
            .map(|i| i.id.clone())
            .or_else(|| {
                self.items
                    .iter()
                    .find(|i| i.status == TodoStatus::Pending)
                    .map(|i| i.id.clone())
            })?;
        let _ = self.update_status(&next, TodoStatus::InProgress);
        Some(next)
    }

    pub fn complete_and_advance(&mut self, id: &str) -> Result<(), String> {
        self.update_status(id, TodoStatus::Completed)?;
        let _ = self.ensure_active_step();
        Ok(())
    }

    pub fn update_status(&mut self, id: &str, status: TodoStatus) -> Result<(), String> {
        let item = self
            .items
            .iter_mut()
            .find(|i| i.id == id)
            .ok_or_else(|| format!("Todo item '{}' not found", id))?;

        if status == TodoStatus::Completed {
            item.completed_at = Some(Utc::now());
        }
        item.status = status;
        self.persist();
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&TodoItem> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn pending(&self) -> Vec<&TodoItem> {
        self.items
            .iter()
            .filter(|i| i.status == TodoStatus::Pending)
            .collect()
    }

    pub fn in_progress(&self) -> Vec<&TodoItem> {
        self.items
            .iter()
            .filter(|i| i.status == TodoStatus::InProgress)
            .collect()
    }

    pub fn ready_items(&self) -> Vec<&TodoItem> {
        self.items
            .iter()
            .filter(|i| {
                i.status == TodoStatus::Pending
                    && i.depends_on.iter().all(|dep| {
                        self.items
                            .iter()
                            .any(|d| d.id == *dep && d.status == TodoStatus::Completed)
                    })
            })
            .collect()
    }

    pub fn completion_rate(&self) -> f64 {
        if self.items.is_empty() {
            return 1.0;
        }
        let completed = self
            .items
            .iter()
            .filter(|i| i.status == TodoStatus::Completed)
            .count();
        completed as f64 / self.items.len() as f64
    }

    pub fn progress_counts(&self) -> (usize, usize) {
        let total = self.items.len();
        let completed = self
            .items
            .iter()
            .filter(|i| i.status == TodoStatus::Completed)
            .count();
        (completed, total)
    }

    pub fn all_completed(&self) -> bool {
        !self.items.is_empty()
            && self
                .items
                .iter()
                .all(|i| i.status == TodoStatus::Completed)
    }

    pub fn has_incomplete(&self) -> bool {
        self.items
            .iter()
            .any(|i| i.status != TodoStatus::Completed)
    }

    pub fn summary(&self) -> String {
        let total = self.items.len();
        let completed = self
            .items
            .iter()
            .filter(|i| i.status == TodoStatus::Completed)
            .count();
        let in_progress = self
            .items
            .iter()
            .filter(|i| i.status == TodoStatus::InProgress)
            .count();
        let pending = self
            .items
            .iter()
            .filter(|i| i.status == TodoStatus::Pending)
            .count();
        let blocked = self
            .items
            .iter()
            .filter(|i| i.status == TodoStatus::Blocked)
            .count();

        format!(
            "Todo: {total} total | {completed} done | {in_progress} in progress | {pending} pending | {blocked} blocked"
        )
    }

    pub fn to_context_string(&self) -> String {
        if self.items.is_empty() {
            return "No todo items.".to_string();
        }

        let mut out = String::from("== Current Plan ==\n");
        for item in &self.items {
            let status_icon = match item.status {
                TodoStatus::Pending => "[ ]",
                TodoStatus::InProgress => "[~]",
                TodoStatus::Completed => "[x]",
                TodoStatus::Blocked => "[!]",
            };
            out.push_str(&format!(
                "{} {} {}: {}\n",
                status_icon, item.id, item.status, item.description
            ));
        }
        out.push_str(&format!("== {} ==\n", self.summary()));
        out
    }

    fn persist(&self) {
        if let Some(ref path) = self.storage_path {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(&self.items) {
                let _ = std::fs::write(path, json);
            }
        }
    }
}

fn normalize_desc(s: &str) -> String {
    s.trim().to_lowercase()
}

fn title_from_items(items: &[String]) -> String {
    items
        .first()
        .map(|s| {
            let t = s.trim();
            if t.chars().count() > 60 {
                format!("{}…", t.chars().take(57).collect::<String>())
            } else {
                t.to_string()
            }
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Untitled plan".into())
}

/// A checklist instance with lifecycle status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub title: String,
    pub status: PlanStatus,
    pub source_prompt_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub items: TodoList,
}

impl Plan {
    pub fn new(title: impl Into<String>, items: TodoList) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title: title.into(),
            status: PlanStatus::Active,
            source_prompt_id: None,
            created_at: now,
            updated_at: now,
            items,
        }
    }

    pub fn progress_label(&self) -> String {
        let (done, total) = self.items.progress_counts();
        format!("{done}/{total}")
    }
}

/// Lightweight parked-plan row for UI / injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParkedPlanSummary {
    pub id: String,
    pub title: String,
    pub completed: usize,
    pub total: usize,
    pub updated_at: String,
    #[serde(default)]
    pub source_prompt_id: Option<String>,
}

/// Full plan row for Overview / history UI (active, parked, finished, cancelled).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanDetail {
    pub id: String,
    pub title: String,
    pub status: String,
    #[serde(default)]
    pub source_prompt_id: Option<String>,
    pub updated_at: String,
    pub items: Vec<TodoItem>,
}

/// Snapshot of active + parked plans for events / UI hydrate.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlansSnapshot {
    pub active_plan_id: Option<String>,
    pub active_plan_title: Option<String>,
    pub items: Vec<TodoItem>,
    pub parked: Vec<ParkedPlanSummary>,
    /// All plans with items (for Overview prompt grouping).
    #[serde(default)]
    pub plans: Vec<PlanDetail>,
}

/// Result of resolving a continue/resume intent.
#[derive(Debug, Clone)]
pub enum ContinueResolution {
    NothingParked,
    Activated { plan_id: String, title: String },
    Choose(Vec<ParkedPlanSummary>),
}

struct SessionPlanState {
    plans: Vec<Plan>,
    /// True once we've attempted a DB load for this session key.
    loaded: bool,
}

impl SessionPlanState {
    fn empty() -> Self {
        Self {
            plans: Vec::new(),
            loaded: false,
        }
    }
}

/// Per-session multi-plan store (active / parked / finished / cancelled).
///
/// Brain holds one store; each chat session gets its own plan list.
/// Optional [`Storage`] persists plans to SQLite.
pub struct SessionPlanStore {
    by_session: Mutex<HashMap<String, SessionPlanState>>,
    storage: Option<Storage>,
}

/// Back-compat alias — callers historically used SessionTodoStore.
pub type SessionTodoStore = SessionPlanStore;

impl Default for SessionPlanStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionPlanStore {
    pub fn new() -> Self {
        Self {
            by_session: Mutex::new(HashMap::new()),
            storage: None,
        }
    }

    pub fn with_storage(storage: Option<Storage>) -> Self {
        Self {
            by_session: Mutex::new(HashMap::new()),
            storage,
        }
    }

    fn scope_key(session_id: Option<&str>) -> String {
        session_id.unwrap_or("").to_string()
    }

    fn persistable(session_id: &str) -> bool {
        !session_id.is_empty()
    }

    fn prompt_key(prompt_id: Option<&str>) -> String {
        prompt_id.unwrap_or("").to_string()
    }

    fn same_prompt(plan: &Plan, prompt_id: Option<&str>) -> bool {
        let want = Self::prompt_key(prompt_id);
        let got = plan
            .source_prompt_id
            .as_deref()
            .unwrap_or("")
            .to_string();
        want == got
    }

    /// Ensure session state is loaded from DB (and demote stale `active` → `parked`
    /// on process reopen so the first message is not hijacked).
    fn ensure_loaded(&self, key: &str, state: &mut SessionPlanState) {
        if state.loaded {
            return;
        }
        state.loaded = true;
        if !Self::persistable(key) {
            return;
        }
        let Some(ref storage) = self.storage else {
            return;
        };
        if let Ok(mut plans) = load_plans_from_db(storage, key) {
            // On cold load: any DB-active plan becomes parked (crash / reopen).
            for p in &mut plans {
                if p.status == PlanStatus::Active {
                    p.status = PlanStatus::Parked;
                    p.updated_at = Utc::now();
                }
            }
            // Persist demotion so next load is consistent.
            for p in &plans {
                if p.status == PlanStatus::Parked {
                    let _ = upsert_plan(storage, key, p);
                }
            }
            state.plans = plans;
        }
    }

    fn with_session_mut<R>(
        &self,
        session_id: Option<&str>,
        f: impl FnOnce(&str, &mut SessionPlanState) -> R,
    ) -> R {
        let key = Self::scope_key(session_id);
        let mut map = self.by_session.lock();
        let state = map.entry(key.clone()).or_insert_with(SessionPlanState::empty);
        self.ensure_loaded(&key, state);
        f(&key, state)
    }

    fn flush_plan(&self, session_id: &str, plan: &Plan) {
        if !Self::persistable(session_id) {
            return;
        }
        if let Some(ref storage) = self.storage {
            let _ = upsert_plan(storage, session_id, plan);
        }
    }

    fn flush_delete_session(&self, session_id: &str) {
        if !Self::persistable(session_id) {
            return;
        }
        if let Some(ref storage) = self.storage {
            let _ = delete_session_plans(storage, session_id);
        }
    }

    /// Snapshot for UI / events.
    pub fn snapshot(&self, session_id: Option<&str>) -> PlansSnapshot {
        self.with_session_mut(session_id, |_, state| snapshot_of(state))
    }

    /// Active plan's todo list (clone of items container). Empty if none.
    pub fn active_list(&self, session_id: Option<&str>) -> TodoList {
        self.with_session_mut(session_id, |_, state| {
            state
                .plans
                .iter()
                .find(|p| p.status == PlanStatus::Active)
                .map(|p| p.items.clone())
                .unwrap_or_default()
        })
    }

    /// Mutate the active plan's items; creates nothing if no active plan.
    pub fn with_active_mut<R>(
        &self,
        session_id: Option<&str>,
        f: impl FnOnce(&mut TodoList) -> R,
    ) -> Option<R> {
        self.with_session_mut(session_id, |key, state| {
            let idx = state
                .plans
                .iter()
                .position(|p| p.status == PlanStatus::Active)?;
            let result = f(&mut state.plans[idx].items);
            state.plans[idx].updated_at = Utc::now();
            let plan = state.plans[idx].clone();
            self.flush_plan(key, &plan);
            Some(result)
        })
    }

    /// Read-only access to active items via callback.
    pub fn with_active<R>(
        &self,
        session_id: Option<&str>,
        f: impl FnOnce(&TodoList) -> R,
    ) -> R {
        self.with_session_mut(session_id, |_, state| {
            if let Some(p) = state.plans.iter().find(|p| p.status == PlanStatus::Active) {
                f(&p.items)
            } else {
                let empty = TodoList::new();
                f(&empty)
            }
        })
    }

    /// Compatibility: return Arc<Mutex<TodoList>> mirroring the active plan.
    ///
    /// Mutations through this Arc are **not** auto-persisted; prefer store methods
    /// from tools. Used by legacy CLI paths and Run helpers that sync then persist
    /// via [`Self::replace_active_items`] / tool APIs.
    pub fn for_session(&self, session_id: Option<&str>) -> Arc<Mutex<TodoList>> {
        let list = self.active_list(session_id);
        Arc::new(Mutex::new(list))
    }

    /// Park the session-active plan when it belongs to a different prompt.
    /// Returns true if something was parked.
    pub fn park_active_if_other_prompt(
        &self,
        session_id: Option<&str>,
        prompt_id: Option<&str>,
    ) -> bool {
        self.with_session_mut(session_id, |key, state| {
            let Some(idx) = state
                .plans
                .iter()
                .position(|p| p.status == PlanStatus::Active)
            else {
                return false;
            };
            if Self::same_prompt(&state.plans[idx], prompt_id) {
                return false;
            }
            if state.plans[idx].items.items.is_empty() {
                state.plans[idx].status = PlanStatus::Cancelled;
            } else {
                state.plans[idx].status = PlanStatus::Parked;
            }
            state.plans[idx].updated_at = Utc::now();
            let plan = state.plans[idx].clone();
            self.flush_plan(key, &plan);
            true
        })
    }

    /// True if session has an incomplete active plan or any parked plan.
    pub fn has_resumable_work(&self, session_id: Option<&str>) -> bool {
        self.with_session_mut(session_id, |_, state| {
            let active_incomplete = state.plans.iter().any(|p| {
                p.status == PlanStatus::Active && p.items.has_incomplete()
            });
            let any_parked = state.plans.iter().any(|p| p.status == PlanStatus::Parked);
            active_incomplete || any_parked
        })
    }

    /// Source prompt id of the session-active plan, if any.
    pub fn active_source_prompt_id(&self, session_id: Option<&str>) -> Option<String> {
        self.with_session_mut(session_id, |_, state| {
            state
                .plans
                .iter()
                .find(|p| p.status == PlanStatus::Active)
                .and_then(|p| p.source_prompt_id.clone())
        })
    }

    /// Latest parked plan summary (newest first), if any.
    pub fn latest_parked(&self, session_id: Option<&str>) -> Option<ParkedPlanSummary> {
        self.parked(session_id).into_iter().next()
    }

    /// Park the active plan (if any). Returns true if something was parked.
    pub fn park_active(&self, session_id: Option<&str>) -> bool {
        self.with_session_mut(session_id, |key, state| {
            let Some(idx) = state
                .plans
                .iter()
                .position(|p| p.status == PlanStatus::Active)
            else {
                return false;
            };
            // Don't park empty plans — cancel them instead.
            if state.plans[idx].items.items.is_empty() {
                state.plans[idx].status = PlanStatus::Cancelled;
            } else {
                state.plans[idx].status = PlanStatus::Parked;
            }
            state.plans[idx].updated_at = Utc::now();
            let plan = state.plans[idx].clone();
            self.flush_plan(key, &plan);
            true
        })
    }

    /// Activate a parked plan by id (parks any current active first).
    pub fn activate(&self, session_id: Option<&str>, plan_id: &str) -> Result<(), String> {
        self.with_session_mut(session_id, |key, state| {
            let target = state
                .plans
                .iter()
                .position(|p| p.id == plan_id)
                .ok_or_else(|| format!("Plan '{plan_id}' not found"))?;
            if !matches!(
                state.plans[target].status,
                PlanStatus::Parked | PlanStatus::Active
            ) {
                return Err(format!(
                    "Plan '{plan_id}' is {} — only parked plans can be resumed",
                    state.plans[target].status
                ));
            }
            // Park other actives.
            for (i, p) in state.plans.iter_mut().enumerate() {
                if i != target && p.status == PlanStatus::Active {
                    p.status = PlanStatus::Parked;
                    p.updated_at = Utc::now();
                    let cloned = p.clone();
                    self.flush_plan(key, &cloned);
                }
            }
            state.plans[target].status = PlanStatus::Active;
            state.plans[target].updated_at = Utc::now();
            let _ = state.plans[target].items.ensure_active_step();
            let plan = state.plans[target].clone();
            self.flush_plan(key, &plan);
            Ok(())
        })
    }

    /// Activate a parked plan whose title equals (or uniquely contains) `title`.
    pub fn activate_by_title(&self, session_id: Option<&str>, title: &str) -> Result<String, String> {
        let needle = title.trim();
        if needle.is_empty() {
            return Err("empty plan title".into());
        }
        let parked = self.parked(session_id);
        let exact: Vec<_> = parked
            .iter()
            .filter(|p| p.title == needle)
            .collect();
        let id = if exact.len() == 1 {
            exact[0].id.clone()
        } else {
            let partial: Vec<_> = parked
                .iter()
                .filter(|p| p.title.contains(needle) || needle.contains(p.title.as_str()))
                .collect();
            match partial.len() {
                1 => partial[0].id.clone(),
                0 => return Err(format!("No parked plan titled '{needle}'")),
                _ => {
                    return Err(format!(
                        "Multiple parked plans match '{needle}' — pick one with /plan resume <id>"
                    ))
                }
            }
        };
        self.activate(session_id, &id)?;
        Ok(id)
    }

    /// Cancel a plan by id.
    pub fn cancel(&self, session_id: Option<&str>, plan_id: &str) -> Result<(), String> {
        self.with_session_mut(session_id, |key, state| {
            let plan = state
                .plans
                .iter_mut()
                .find(|p| p.id == plan_id)
                .ok_or_else(|| format!("Plan '{plan_id}' not found"))?;
            plan.status = PlanStatus::Cancelled;
            plan.updated_at = Utc::now();
            let cloned = plan.clone();
            self.flush_plan(key, &cloned);
            Ok(())
        })
    }

    /// Mark active plan finished when all items completed.
    pub fn finish_active_if_done(&self, session_id: Option<&str>) -> bool {
        self.with_session_mut(session_id, |key, state| {
            let Some(idx) = state
                .plans
                .iter()
                .position(|p| p.status == PlanStatus::Active)
            else {
                return false;
            };
            if !state.plans[idx].items.all_completed() {
                return false;
            }
            state.plans[idx].status = PlanStatus::Finished;
            state.plans[idx].updated_at = Utc::now();
            let plan = state.plans[idx].clone();
            self.flush_plan(key, &plan);
            true
        })
    }

    /// Wipe all plans for a session (goal clear / session delete).
    pub fn clear_session(&self, session_id: &str) {
        {
            let mut map = self.by_session.lock();
            if let Some(state) = map.get_mut(session_id) {
                state.plans.clear();
                state.loaded = true;
            }
        }
        self.flush_delete_session(session_id);
    }

    /// Drop the session entry entirely (frees memory); also deletes DB rows.
    pub fn remove_session(&self, session_id: &str) {
        self.by_session.lock().remove(session_id);
        self.flush_delete_session(session_id);
    }

    /// Parked plans newest-first.
    pub fn parked(&self, session_id: Option<&str>) -> Vec<ParkedPlanSummary> {
        self.with_session_mut(session_id, |_, state| parked_summaries(state))
    }

    /// Resolve continue/resume: activate sole parked, or ask to choose.
    pub fn resolve_continue(&self, session_id: Option<&str>) -> ContinueResolution {
        let parked = self.parked(session_id);
        match parked.len() {
            0 => ContinueResolution::NothingParked,
            1 => {
                let id = parked[0].id.clone();
                let title = parked[0].title.clone();
                match self.activate(session_id, &id) {
                    Ok(()) => ContinueResolution::Activated {
                        plan_id: id,
                        title,
                    },
                    Err(_) => ContinueResolution::NothingParked,
                }
            }
            _ => ContinueResolution::Choose(parked),
        }
    }

    /// Create or update the active plan from todo_write.
    ///
    /// - `force=true`: replace active in place (or create if none).
    /// - else if active has progress and descriptions look like a replan of same work:
    ///   merge into active.
    /// - else if active exists with incomplete work and this looks like a **new** plan
    ///   (force not set): park active, create new.
    /// - else: create / replace empty active.
    pub fn write_plan(
        &self,
        session_id: Option<&str>,
        descriptions: Vec<String>,
        force: bool,
        source_prompt_id: Option<&str>,
    ) -> Result<String, String> {
        if descriptions.is_empty() {
            return Err("items must not be empty".into());
        }
        let title = title_from_items(&descriptions);

        self.with_session_mut(session_id, |key, state| {
            // Prompt-bucket rule: if the session-active plan belongs to another
            // prompt, park it first so this write lands in the current prompt.
            if let Some(idx) = state
                .plans
                .iter()
                .position(|p| p.status == PlanStatus::Active)
            {
                if !Self::same_prompt(&state.plans[idx], source_prompt_id) {
                    if state.plans[idx].items.items.is_empty() {
                        state.plans[idx].status = PlanStatus::Cancelled;
                    } else {
                        state.plans[idx].status = PlanStatus::Parked;
                    }
                    state.plans[idx].updated_at = Utc::now();
                    let parked = state.plans[idx].clone();
                    self.flush_plan(key, &parked);
                }
            }

            let active_idx = state
                .plans
                .iter()
                .position(|p| p.status == PlanStatus::Active);

            if force {
                if let Some(idx) = active_idx {
                    state.plans[idx].items.replace_all(descriptions);
                    let _ = state.plans[idx].items.ensure_active_step();
                    state.plans[idx].title = title;
                    state.plans[idx].updated_at = Utc::now();
                    if let Some(pid) = source_prompt_id {
                        state.plans[idx].source_prompt_id = Some(pid.to_string());
                    }
                    let plan = state.plans[idx].clone();
                    self.flush_plan(key, &plan);
                    return Ok(format!(
                        "[plan replaced force=true]\n{}",
                        plan.items.to_context_string()
                    ));
                }
                // No active — create.
                let mut list = TodoList::new();
                list.replace_all(descriptions);
                let _ = list.ensure_active_step();
                let mut plan = Plan::new(title, list);
                if let Some(pid) = source_prompt_id {
                    plan.source_prompt_id = Some(pid.to_string());
                }
                let ctx = plan.items.to_context_string();
                self.flush_plan(key, &plan);
                state.plans.push(plan);
                return Ok(format!("[plan created]\n{ctx}"));
            }

            if let Some(idx) = active_idx {
                let had_progress = state.plans[idx].items.items.iter().any(|i| {
                    matches!(
                        i.status,
                        TodoStatus::Completed | TodoStatus::InProgress
                    )
                });
                let incomplete = state.plans[idx].items.has_incomplete();

                // Same-plan merge when there is progress and caller didn't force a new plan.
                // Heuristic: if every new description matches an existing one (or vice versa
                // substantial overlap), merge; otherwise park + create.
                let overlap = description_overlap(
                    &state.plans[idx].items.items,
                    &descriptions,
                );
                if had_progress && incomplete && overlap < 0.4 {
                    // New job — park current, create new active.
                    state.plans[idx].status = PlanStatus::Parked;
                    state.plans[idx].updated_at = Utc::now();
                    let parked = state.plans[idx].clone();
                    self.flush_plan(key, &parked);

                    let mut list = TodoList::new();
                    list.replace_all(descriptions);
                    let _ = list.ensure_active_step();
                    let mut plan = Plan::new(title, list);
                    if let Some(pid) = source_prompt_id {
                        plan.source_prompt_id = Some(pid.to_string());
                    }
                    let ctx = plan.items.to_context_string();
                    self.flush_plan(key, &plan);
                    state.plans.push(plan);
                    return Ok(format!(
                        "[prior plan parked; new plan created]\n{ctx}"
                    ));
                }

                if had_progress {
                    state.plans[idx].items.merge_replace(descriptions);
                    state.plans[idx].updated_at = Utc::now();
                    let plan = state.plans[idx].clone();
                    self.flush_plan(key, &plan);
                    return Ok(format!(
                        "[plan merged — prior progress preserved]\n{}",
                        plan.items.to_context_string()
                    ));
                }

                state.plans[idx].items.replace_all(descriptions);
                let _ = state.plans[idx].items.ensure_active_step();
                state.plans[idx].title = title;
                state.plans[idx].updated_at = Utc::now();
                let plan = state.plans[idx].clone();
                self.flush_plan(key, &plan);
                return Ok(format!("[plan created]\n{}", plan.items.to_context_string()));
            }

            let mut list = TodoList::new();
            list.replace_all(descriptions);
            let _ = list.ensure_active_step();
            let mut plan = Plan::new(title, list);
            if let Some(pid) = source_prompt_id {
                plan.source_prompt_id = Some(pid.to_string());
            }
            let ctx = plan.items.to_context_string();
            self.flush_plan(key, &plan);
            state.plans.push(plan);
            Ok(format!("[plan created]\n{ctx}"))
        })
    }

    /// Update a todo item on the active plan.
    pub fn update_item(
        &self,
        session_id: Option<&str>,
        id: &str,
        status: TodoStatus,
    ) -> Result<String, String> {
        self.with_session_mut(session_id, |key, state| {
            let idx = state
                .plans
                .iter()
                .position(|p| p.status == PlanStatus::Active)
                .ok_or_else(|| {
                    "No active plan. Call todo_read, resume a parked plan (say continue), \
                     or todo_write a new plan. Stale item ids from chat history are ignored."
                        .to_string()
                })?;

            let list = &mut state.plans[idx].items;
            if list.get(id).is_none() {
                return Err(format!(
                    "Todo item '{id}' not found on the active plan. \
                     Call todo_read to see current ids, or resume a parked plan if this \
                     step belongs to an older checklist."
                ));
            }

            if status == TodoStatus::Completed {
                list.complete_and_advance(id)?;
            } else {
                list.update_status(id, status)?;
                if status == TodoStatus::InProgress {
                    let others: Vec<String> = list
                        .items
                        .iter()
                        .filter(|i| i.id != id && i.status == TodoStatus::InProgress)
                        .map(|i| i.id.clone())
                        .collect();
                    for oid in others {
                        let _ = list.update_status(&oid, TodoStatus::Pending);
                    }
                }
            }

            let all_done = list.all_completed();
            let ctx = list.to_context_string();
            let desc = list
                .get(id)
                .map(|i| i.description.clone())
                .unwrap_or_default();

            state.plans[idx].updated_at = Utc::now();
            if all_done {
                state.plans[idx].status = PlanStatus::Finished;
            }
            let plan = state.plans[idx].clone();
            self.flush_plan(key, &plan);

            Ok(format!(
                "Todo '{}': \"{}\" updated to {}\n\n{}",
                id, desc, status, ctx
            ))
        })
    }

    /// Parked + active summary line for Segment 7.
    pub fn parked_injection_line(&self, session_id: Option<&str>) -> Option<String> {
        let parked = self.parked(session_id);
        if parked.is_empty() {
            return None;
        }
        let latest = &parked[0];
        Some(format!(
            "Parked plans: {} (say continue / /plan resume to resume). Latest: \"{}\" {}.",
            parked.len(),
            latest.title,
            format!("{}/{}", latest.completed, latest.total)
        ))
    }
}

fn description_overlap(existing: &[TodoItem], new_descs: &[String]) -> f64 {
    if existing.is_empty() || new_descs.is_empty() {
        return 0.0;
    }
    let old_norms: Vec<String> = existing
        .iter()
        .map(|i| normalize_desc(&i.description))
        .collect();
    let mut hits = 0usize;
    for d in new_descs {
        let n = normalize_desc(d);
        if old_norms.iter().any(|o| o == &n) {
            hits += 1;
        }
    }
    hits as f64 / new_descs.len().max(old_norms.len()) as f64
}

fn parked_summaries(state: &SessionPlanState) -> Vec<ParkedPlanSummary> {
    let mut parked: Vec<ParkedPlanSummary> = state
        .plans
        .iter()
        .filter(|p| p.status == PlanStatus::Parked)
        .map(|p| {
            let (completed, total) = p.items.progress_counts();
            ParkedPlanSummary {
                id: p.id.clone(),
                title: p.title.clone(),
                completed,
                total,
                updated_at: p.updated_at.to_rfc3339(),
                source_prompt_id: p.source_prompt_id.clone(),
            }
        })
        .collect();
    parked.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    parked
}

fn plan_details(state: &SessionPlanState) -> Vec<PlanDetail> {
    let mut plans: Vec<PlanDetail> = state
        .plans
        .iter()
        // Skip empty cancelled shells that never received items.
        .filter(|p| !(p.status == PlanStatus::Cancelled && p.items.items.is_empty()))
        .map(|p| PlanDetail {
            id: p.id.clone(),
            title: p.title.clone(),
            status: p.status.to_string(),
            source_prompt_id: p.source_prompt_id.clone(),
            updated_at: p.updated_at.to_rfc3339(),
            items: p.items.items.clone(),
        })
        .collect();
    // Newest first so Overview groups feel recent-first within a prompt.
    plans.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    plans
}

fn snapshot_of(state: &SessionPlanState) -> PlansSnapshot {
    let active = state.plans.iter().find(|p| p.status == PlanStatus::Active);
    PlansSnapshot {
        active_plan_id: active.map(|p| p.id.clone()),
        active_plan_title: active.map(|p| p.title.clone()),
        items: active.map(|p| p.items.items.clone()).unwrap_or_default(),
        parked: parked_summaries(state),
        plans: plan_details(state),
    }
}

/// Detect continue / resume cues in user text (legacy — prefer classifiers below).
pub fn is_continue_cue(text: &str) -> bool {
    is_bare_continue(text) || is_explicit_plan_resume(text)
}

/// Exact/near-exact continue phrases with no extra object.
pub fn is_bare_continue(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    let t = t.trim_end_matches(['.', '。', '!', '！']);
    matches!(
        t,
        "continue"
            | "resume"
            | "keep going"
            | "continue please"
            | "please continue"
            | "please resume"
            | "继续"
            | "接着"
            | "接着做"
            | "继续吧"
            | "继续啊"
    )
}

/// Explicit plan-resume intent (slash, UI phrasing, or 「继续…计划」).
pub fn is_explicit_plan_resume(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    if t == "/plan resume" || t.starts_with("/plan resume ") {
        return true;
    }
    if t.contains("resume the plan")
        || t.contains("resume plan")
        || t.contains("continue the plan")
        || t.contains("continue with the plan")
        || t.contains("continue executing the plan")
        || t.contains("continue the todo")
        || t.contains("resume the todo")
    {
        return true;
    }
    // Chinese: 继续…计划 / 继续todo / 接着做计划
    let lower = text.trim().to_lowercase();
    if (lower.contains("继续") || lower.contains("接着") || lower.contains("resume"))
        && (lower.contains("计划")
            || lower.contains("todo")
            || lower.contains("plan")
            || lower.contains("清单"))
    {
        return true;
    }
    false
}

/// What a resume cue is asking to restore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeTarget {
    /// `/plan resume <uuid>`
    PlanId(String),
    /// Natural language with a plan title, e.g. 「继续执行计划：Auth」.
    Title(String),
    /// Resume intent without naming which plan.
    Unspecified,
}

/// Parse `/plan resume <id>` or natural "continue the plan: Title" into a target.
pub fn parse_resume_target(text: &str) -> ResumeTarget {
    let trimmed = text.trim();
    if let Some(rest) = trimmed.strip_prefix("/plan resume") {
        let rest = rest.trim();
        if rest.is_empty() {
            return ResumeTarget::Unspecified;
        }
        return ResumeTarget::PlanId(rest.to_string());
    }
    if !is_explicit_plan_resume(trimmed) {
        return ResumeTarget::Unspecified;
    }

    // Longest prefixes first so "继续执行计划：" wins over "继续执行计划".
    const PREFIXES: &[&str] = &[
        "继续执行计划：",
        "继续执行计划:",
        "继续执行计划",
        "继续刚才的计划：",
        "继续刚才的计划:",
        "继续刚才的计划",
        "继续这个计划：",
        "继续这个计划:",
        "继续这个计划",
        "继续计划：",
        "继续计划:",
        "继续计划",
        "continue executing the plan:",
        "continue executing the plan -",
        "continue executing the plan —",
        "continue executing the plan",
        "continue with the plan:",
        "continue with the plan -",
        "continue with the plan —",
        "continue with the plan",
        "continue the plan:",
        "continue the plan -",
        "continue the plan —",
        "continue the plan",
        "resume the plan:",
        "resume the plan -",
        "resume the plan —",
        "resume the plan",
        "resume plan:",
        "resume plan",
    ];

    let lower = trimmed.to_lowercase();
    for prefix in PREFIXES {
        let p = prefix.to_lowercase();
        if lower.starts_with(&p) {
            let rest = trimmed[prefix.len()..].trim();
            let rest = rest
                .trim_matches(['「', '」', '《', '》', '"', '"', '"', '\'', '“', '”'])
                .trim();
            if rest.is_empty() {
                return ResumeTarget::Unspecified;
            }
            return ResumeTarget::Title(rest.to_string());
        }
    }
    ResumeTarget::Unspecified
}

/// "Continue X" where X is a concrete object (not a plan resume).
pub fn is_object_bearing_continue(text: &str) -> bool {
    if is_explicit_plan_resume(text) || is_bare_continue(text) {
        return false;
    }
    let t = text.trim().to_lowercase();
    let prefixes = [
        "continue ",
        "continue,",
        "keep going ",
        "resume ",
        "继续",
        "接着",
    ];
    prefixes.iter().any(|p| t.starts_with(p) && t.len() > p.len() + 1)
}

pub fn is_plan_park_cmd(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    t == "/plan park" || t == "/plan pause"
}

pub fn is_plan_clear_cmd(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    t == "/plan clear" || t == "/plan cancel" || t == "/plan stop"
}

pub fn is_plan_resume_cmd(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    t == "/plan resume" || t.starts_with("/plan resume ")
}

/// Softened: only explicit side-channel cues (not every short question).
pub fn looks_like_detour(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t.starts_with('/') {
        return false;
    }
    if is_continue_cue(t) || is_object_bearing_continue(t) {
        return false;
    }
    let lower = t.to_lowercase();
    lower.starts_with("btw")
        || lower.starts_with("by the way")
        || lower.starts_with("quick question")
        || lower.starts_with("unrelated")
        || lower.starts_with("aside:")
        || lower.starts_with("one sec")
}

// ── SQLite persistence ─────────────────────────────────────────────

fn load_plans_from_db(storage: &Storage, session_id: &str) -> anyhow::Result<Vec<Plan>> {
    let db = storage.conn();
    let mut stmt = db.prepare(
        "SELECT id, title, status, source_prompt_id, created_at, updated_at \
         FROM session_plans WHERE session_id = ?1 ORDER BY updated_at DESC",
    )?;
    let plan_rows: Vec<(String, String, String, String, String, String)> = stmt
        .query_map(rusqlite::params![session_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    let mut plans = Vec::new();
    for (id, title, status, source_prompt_id, created_at, updated_at) in plan_rows {
        let status = PlanStatus::parse(&status).unwrap_or(PlanStatus::Parked);
        let mut item_stmt = db.prepare(
            "SELECT item_id, description, status, depends_on, created_at, completed_at \
             FROM session_plan_items WHERE plan_id = ?1",
        )?;
        let items: Vec<TodoItem> = item_stmt
            .query_map(rusqlite::params![id], |row| {
                let item_id: String = row.get(0)?;
                let description: String = row.get(1)?;
                let st: String = row.get(2)?;
                let depends_on_json: String = row.get(3)?;
                let created: String = row.get(4)?;
                let completed: Option<String> = row.get(5)?;
                let depends_on: Vec<String> =
                    serde_json::from_str(&depends_on_json).unwrap_or_default();
                Ok(TodoItem {
                    id: item_id,
                    description,
                    status: TodoStatus::parse(&st).unwrap_or(TodoStatus::Pending),
                    depends_on,
                    created_at: DateTime::parse_from_rfc3339(&created)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    completed_at: completed.and_then(|c| {
                        DateTime::parse_from_rfc3339(&c)
                            .ok()
                            .map(|d| d.with_timezone(&Utc))
                    }),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        drop(item_stmt);

        let mut list = TodoList::new();
        list.items = items;
        plans.push(Plan {
            id,
            title,
            status,
            source_prompt_id: if source_prompt_id.is_empty() {
                None
            } else {
                Some(source_prompt_id)
            },
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            updated_at: DateTime::parse_from_rfc3339(&updated_at)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            items: list,
        });
    }
    Ok(plans)
}

fn upsert_plan(storage: &Storage, session_id: &str, plan: &Plan) -> anyhow::Result<()> {
    let db = storage.conn();
    db.execute(
        "INSERT INTO session_plans (id, session_id, title, status, source_prompt_id, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(id) DO UPDATE SET \
           title=excluded.title, status=excluded.status, \
           source_prompt_id=excluded.source_prompt_id, updated_at=excluded.updated_at",
        rusqlite::params![
            plan.id,
            session_id,
            plan.title,
            plan.status.to_string(),
            plan.source_prompt_id.as_deref().unwrap_or(""),
            plan.created_at.to_rfc3339(),
            plan.updated_at.to_rfc3339(),
        ],
    )?;
    db.execute(
        "DELETE FROM session_plan_items WHERE plan_id = ?1",
        rusqlite::params![plan.id],
    )?;
    for item in &plan.items.items {
        db.execute(
            "INSERT INTO session_plan_items \
             (plan_id, item_id, description, status, depends_on, created_at, completed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                plan.id,
                item.id,
                item.description,
                item.status.to_string(),
                serde_json::to_string(&item.depends_on).unwrap_or_else(|_| "[]".into()),
                item.created_at.to_rfc3339(),
                item.completed_at.map(|t| t.to_rfc3339()),
            ],
        )?;
    }
    Ok(())
}

fn delete_session_plans(storage: &Storage, session_id: &str) -> anyhow::Result<()> {
    let db = storage.conn();
    // Items cascade via FK when plans deleted — but delete items first if FK
    // path isn't available on older DBs without CASCADE wired for this table.
    db.execute(
        "DELETE FROM session_plan_items WHERE plan_id IN \
         (SELECT id FROM session_plans WHERE session_id = ?1)",
        rusqlite::params![session_id],
    )?;
    db.execute(
        "DELETE FROM session_plans WHERE session_id = ?1",
        rusqlite::params![session_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_get() {
        let mut list = TodoList::new();
        list.add(TodoItem::new("1", "First task"));
        list.add(TodoItem::new("2", "Second task"));

        assert_eq!(list.items.len(), 2);
        assert!(list.get("1").is_some());
        assert!(list.get("3").is_none());
    }

    #[test]
    fn session_plan_store_isolates_sessions() {
        let store = SessionPlanStore::new();
        store
            .write_plan(Some("s1"), vec!["from s1".into()], false, None)
            .unwrap();
        store
            .write_plan(Some("s2"), vec!["from s2".into()], false, None)
            .unwrap();
        assert_eq!(
            store.active_list(Some("s1")).items[0].description,
            "from s1"
        );
        store.clear_session("s1");
        assert!(store.active_list(Some("s1")).items.is_empty());
        assert_eq!(
            store.active_list(Some("s2")).items[0].description,
            "from s2"
        );
    }

    #[test]
    fn park_and_continue_single() {
        let store = SessionPlanStore::new();
        store
            .write_plan(
                Some("s1"),
                vec!["A".into(), "B".into()],
                false,
                None,
            )
            .unwrap();
        assert!(store.park_active(Some("s1")));
        assert!(store.active_list(Some("s1")).items.is_empty());
        assert_eq!(store.parked(Some("s1")).len(), 1);
        match store.resolve_continue(Some("s1")) {
            ContinueResolution::Activated { .. } => {}
            other => panic!("expected Activated, got {other:?}"),
        }
        assert!(!store.active_list(Some("s1")).items.is_empty());
    }

    #[test]
    fn continue_choose_when_multiple_parked() {
        let store = SessionPlanStore::new();
        store
            .write_plan(Some("s"), vec!["Plan A step".into()], false, None)
            .unwrap();
        store.park_active(Some("s"));
        store
            .write_plan(Some("s"), vec!["Plan B step".into()], false, None)
            .unwrap();
        store.park_active(Some("s"));
        match store.resolve_continue(Some("s")) {
            ContinueResolution::Choose(list) => assert_eq!(list.len(), 2),
            other => panic!("expected Choose, got {other:?}"),
        }
    }

    #[test]
    fn write_new_plan_parks_incomplete_active() {
        let store = SessionPlanStore::new();
        store
            .write_plan(
                Some("s"),
                vec!["Auth: models".into(), "Auth: routes".into()],
                false,
                None,
            )
            .unwrap();
        store
            .update_item(Some("s"), "1", TodoStatus::Completed)
            .unwrap();
        store
            .write_plan(
                Some("s"),
                vec!["Charts: extract".into(), "Charts: plot".into()],
                false,
                None,
            )
            .unwrap();
        assert_eq!(store.parked(Some("s")).len(), 1);
        assert!(store
            .active_list(Some("s"))
            .items
            .iter()
            .any(|i| i.description.contains("Charts")));
    }

    #[test]
    fn test_update_status() {
        let mut list = TodoList::new();
        list.add(TodoItem::new("1", "Task"));

        list.update_status("1", TodoStatus::InProgress).unwrap();
        assert_eq!(list.get("1").unwrap().status, TodoStatus::InProgress);

        list.update_status("1", TodoStatus::Completed).unwrap();
        assert!(list.get("1").unwrap().completed_at.is_some());
    }

    #[test]
    fn test_ready_items_with_dependencies() {
        let mut list = TodoList::new();
        list.add(TodoItem::new("1", "First"));
        list.add(TodoItem::new("2", "Second").with_depends_on(vec!["1".to_string()]));
        list.add(TodoItem::new("3", "Third").with_depends_on(vec!["2".to_string()]));

        let ready = list.ready_items();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "1");

        list.update_status("1", TodoStatus::Completed).unwrap();
        let ready = list.ready_items();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "2");
    }

    #[test]
    fn test_completion_rate() {
        let mut list = TodoList::new();
        list.add(TodoItem::new("1", "A"));
        list.add(TodoItem::new("2", "B"));

        assert_eq!(list.completion_rate(), 0.0);

        list.update_status("1", TodoStatus::Completed).unwrap();
        assert!((list.completion_rate() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn merge_replace_preserves_completed() {
        let mut list = TodoList::new();
        list.replace_all(vec!["Write models".into(), "Write tests".into()]);
        list.update_status("1", TodoStatus::Completed).unwrap();
        list.update_status("2", TodoStatus::InProgress).unwrap();

        list.merge_replace(vec![
            "Write models".into(),
            "Write tests".into(),
            "Write README".into(),
        ]);

        assert_eq!(list.items.len(), 3);
        assert_eq!(list.get("1").unwrap().status, TodoStatus::Completed);
        assert_eq!(list.get("2").unwrap().status, TodoStatus::InProgress);
        assert_eq!(list.get("3").unwrap().status, TodoStatus::Pending);
    }

    #[test]
    fn merge_replace_promotes_next_when_none_in_progress() {
        let mut list = TodoList::new();
        list.replace_all(vec!["A".into(), "B".into()]);
        list.update_status("1", TodoStatus::Completed).unwrap();
        list.merge_replace(vec!["A".into(), "B".into()]);
        assert_eq!(list.get("1").unwrap().status, TodoStatus::Completed);
        assert_eq!(list.get("2").unwrap().status, TodoStatus::InProgress);
    }

    #[test]
    fn persist_reload_demotes_active() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.db");
        let storage = Storage::new(db_path.to_str().unwrap()).unwrap();

        // Need a sessions row for FK.
        {
            let db = storage.conn();
            db.execute(
                "INSERT INTO sessions (id, title, start_time, created_at, updated_at) \
                 VALUES ('sess1', 't', datetime('now'), datetime('now'), datetime('now'))",
                [],
            )
            .unwrap();
        }

        let store = SessionPlanStore::with_storage(Some(storage.clone()));
        store
            .write_plan(
                Some("sess1"),
                vec!["Step one".into(), "Step two".into()],
                false,
                None,
            )
            .unwrap();
        assert_eq!(
            store.snapshot(Some("sess1")).active_plan_id.is_some(),
            true
        );

        // New store instance = process restart.
        let store2 = SessionPlanStore::with_storage(Some(storage));
        let snap = store2.snapshot(Some("sess1"));
        assert!(snap.active_plan_id.is_none());
        assert_eq!(snap.parked.len(), 1);
        assert_eq!(snap.parked[0].total, 2);
    }

    #[test]
    fn is_continue_cue_detects_phrases() {
        assert!(is_bare_continue("continue"));
        assert!(is_bare_continue("  Keep Going  "));
        assert!(is_bare_continue("继续"));
        assert!(is_explicit_plan_resume("/plan resume"));
        assert!(is_explicit_plan_resume("继续刚才的计划"));
        assert!(is_explicit_plan_resume("继续执行计划"));
        assert!(is_explicit_plan_resume("Continue the plan: Auth"));
        assert!(is_object_bearing_continue("继续算斐波那契"));
        assert!(!is_bare_continue("继续算斐波那契"));
        assert!(!is_continue_cue("what is rust?"));
    }

    #[test]
    fn parse_resume_target_natural_and_slash() {
        assert_eq!(
            parse_resume_target("/plan resume abc-123"),
            ResumeTarget::PlanId("abc-123".into())
        );
        assert_eq!(
            parse_resume_target("继续执行计划"),
            ResumeTarget::Unspecified
        );
        assert_eq!(
            parse_resume_target("继续执行计划：建立协议安全分级体系"),
            ResumeTarget::Title("建立协议安全分级体系".into())
        );
        assert_eq!(
            parse_resume_target("Continue the plan: Auth models"),
            ResumeTarget::Title("Auth models".into())
        );
    }

    #[test]
    fn activate_by_title_picks_matching_parked() {
        let store = SessionPlanStore::new();
        store
            .write_plan(Some("s"), vec!["Alpha step".into()], false, Some("p1"))
            .unwrap();
        store.park_active(Some("s"));
        store
            .write_plan(Some("s"), vec!["Beta step".into()], false, Some("p2"))
            .unwrap();
        store.park_active(Some("s"));
        store.activate_by_title(Some("s"), "Alpha step").unwrap();
        assert_eq!(
            store.active_list(Some("s")).items[0].description,
            "Alpha step"
        );
        assert_eq!(store.parked(Some("s")).len(), 1);
        assert_eq!(store.parked(Some("s"))[0].title, "Beta step");
    }

    #[test]
    fn looks_like_detour_only_explicit_side_channels() {
        assert!(looks_like_detour("btw who invented rust"));
        assert!(!looks_like_detour("what time is it?"));
        assert!(!looks_like_detour("continue"));
    }

    #[test]
    fn write_plan_parks_active_from_other_prompt() {
        let store = SessionPlanStore::new();
        store
            .write_plan(
                Some("s"),
                vec!["A1".into(), "A2".into()],
                false,
                Some("prompt-a"),
            )
            .unwrap();
        store
            .write_plan(
                Some("s"),
                vec!["B1".into()],
                false,
                Some("prompt-b"),
            )
            .unwrap();
        assert_eq!(store.parked(Some("s")).len(), 1);
        assert_eq!(
            store.parked(Some("s"))[0].source_prompt_id.as_deref(),
            Some("prompt-a")
        );
        assert_eq!(
            store.active_source_prompt_id(Some("s")).as_deref(),
            Some("prompt-b")
        );
        assert!(store
            .active_list(Some("s"))
            .items
            .iter()
            .any(|i| i.description == "B1"));
    }

    #[test]
    fn park_active_if_other_prompt_only_when_different() {
        let store = SessionPlanStore::new();
        store
            .write_plan(Some("s"), vec!["X".into()], false, Some("p1"))
            .unwrap();
        assert!(!store.park_active_if_other_prompt(Some("s"), Some("p1")));
        assert!(store.park_active_if_other_prompt(Some("s"), Some("p2")));
        assert!(store.active_list(Some("s")).items.is_empty());
        assert_eq!(store.parked(Some("s")).len(), 1);
    }

    #[test]
    fn continue_sees_parked_across_prompts() {
        let store = SessionPlanStore::new();
        store
            .write_plan(Some("s"), vec!["Old".into()], false, Some("prompt-a"))
            .unwrap();
        store.park_active(Some("s"));
        store
            .write_plan(Some("s"), vec!["New".into()], false, Some("prompt-b"))
            .unwrap();
        store.park_active(Some("s"));
        match store.resolve_continue(Some("s")) {
            ContinueResolution::Choose(list) => {
                assert_eq!(list.len(), 2);
                let prompts: Vec<_> = list
                    .iter()
                    .filter_map(|p| p.source_prompt_id.as_deref())
                    .collect();
                assert!(prompts.contains(&"prompt-a"));
                assert!(prompts.contains(&"prompt-b"));
                // Newest first.
                assert_eq!(list[0].source_prompt_id.as_deref(), Some("prompt-b"));
            }
            other => panic!("expected Choose, got {other:?}"),
        }

        // Single parked across prompts → auto-activate.
        let store2 = SessionPlanStore::new();
        store2
            .write_plan(Some("s"), vec!["Only".into()], false, Some("p1"))
            .unwrap();
        store2.park_active(Some("s"));
        match store2.resolve_continue(Some("s")) {
            ContinueResolution::Activated { .. } => {
                assert_eq!(
                    store2.active_list(Some("s")).items[0].description,
                    "Only"
                );
            }
            other => panic!("expected Activated, got {other:?}"),
        }
    }

    #[test]
    fn persist_reload_restores_prompt_buckets() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("buckets.db");
        let storage = Storage::new(db_path.to_str().unwrap()).unwrap();
        {
            let db = storage.conn();
            db.execute(
                "INSERT INTO sessions (id, title, start_time, created_at, updated_at) \
                 VALUES ('sess1', 't', datetime('now'), datetime('now'), datetime('now'))",
                [],
            )
            .unwrap();
        }

        let store = SessionPlanStore::with_storage(Some(storage.clone()));
        store
            .write_plan(
                Some("sess1"),
                vec!["A1".into()],
                false,
                Some("prompt-a"),
            )
            .unwrap();
        store.park_active(Some("sess1"));
        store
            .write_plan(
                Some("sess1"),
                vec!["B1".into(), "B2".into()],
                false,
                Some("prompt-b"),
            )
            .unwrap();

        let store2 = SessionPlanStore::with_storage(Some(storage));
        let snap = store2.snapshot(Some("sess1"));
        // Crash demote: active B becomes parked; A already parked.
        assert!(snap.active_plan_id.is_none());
        assert_eq!(snap.parked.len(), 2);
        let prompts: Vec<_> = snap
            .parked
            .iter()
            .filter_map(|p| p.source_prompt_id.as_deref())
            .collect();
        assert!(prompts.contains(&"prompt-a"));
        assert!(prompts.contains(&"prompt-b"));
    }

    #[test]
    fn has_resumable_work_detects_parked_or_active() {
        let store = SessionPlanStore::new();
        assert!(!store.has_resumable_work(Some("s")));
        store
            .write_plan(Some("s"), vec!["X".into()], false, Some("p1"))
            .unwrap();
        assert!(store.has_resumable_work(Some("s")));
        store.park_active(Some("s"));
        assert!(store.has_resumable_work(Some("s")));
    }
}
