use std::{collections::HashMap, sync::Arc};

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::model::{ArtifactRef, EffectPolicy, NodeKey, RunId, RunScope};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityDescriptor {
    pub kind: String,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct ActivityInvocation {
    pub run_id: RunId,
    pub node: NodeKey,
    pub node_instance_id: String,
    pub attempt_id: String,
    pub attempt: u32,
    pub effect_key: String,
    pub effect: EffectPolicy,
    pub config: Value,
    pub input: Value,
    pub scope: RunScope,
    pub cancel_token: CancellationToken,
}

#[derive(Debug, Clone)]
pub enum ActivityOutcome {
    Completed {
        output: Value,
        artifacts: Vec<ArtifactRef>,
    },
    Failed {
        error: String,
        retryable: bool,
    },
    Suspended {
        signal: String,
    },
    OutcomeUnknown {
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub enum RecoveryDisposition {
    Retry,
    Wait,
    NeedsAttention { reason: String },
}

#[async_trait]
pub trait ActivityAdapter: Send + Sync {
    fn descriptor(&self) -> ActivityDescriptor;

    async fn invoke(&self, invocation: ActivityInvocation) -> Result<ActivityOutcome>;

    async fn recover(&self, invocation: ActivityInvocation) -> Result<RecoveryDisposition> {
        Ok(match invocation.effect {
            EffectPolicy::Pure | EffectPolicy::ReadOnly => RecoveryDisposition::Retry,
            EffectPolicy::WorkspaceWrite | EffectPolicy::External => {
                RecoveryDisposition::NeedsAttention {
                    reason: "activity outcome is unknown after interruption".to_string(),
                }
            }
        })
    }
}

#[derive(Clone, Default)]
pub struct ActivityRegistry {
    adapters: Arc<HashMap<String, Arc<dyn ActivityAdapter>>>,
}

impl ActivityRegistry {
    pub fn new(adapters: impl IntoIterator<Item = Arc<dyn ActivityAdapter>>) -> Result<Self> {
        let mut by_kind = HashMap::new();
        for adapter in adapters {
            let descriptor = adapter.descriptor();
            if by_kind.insert(descriptor.kind.clone(), adapter).is_some() {
                bail!("duplicate workflow activity adapter: {}", descriptor.kind);
            }
        }
        Ok(Self {
            adapters: Arc::new(by_kind),
        })
    }

    pub fn get(&self, kind: &str) -> Result<Arc<dyn ActivityAdapter>> {
        self.adapters
            .get(kind)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("workflow activity adapter not found: {kind}"))
    }

    pub fn versions(&self) -> std::collections::BTreeMap<String, String> {
        self.adapters
            .iter()
            .map(|(kind, adapter)| (kind.clone(), adapter.descriptor().version))
            .collect()
    }
}
