pub mod bus;

pub use bus::MessageBus;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMessage {
    pub id: String,
    pub from: String,
    pub to: String,
    pub content: String,
    pub msg_type: TeamMessageType,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TeamMessageType {
    Request,
    Reply,
    Notification,
    Shutdown,
}

pub struct AgentTeam {
    pub name: String,
    agents: Vec<String>,
    bus: MessageBus,
}

impl AgentTeam {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            agents: Vec::new(),
            bus: MessageBus::new(),
        }
    }

    pub fn add_agent(&mut self, agent_id: &str) {
        self.agents.push(agent_id.to_string());
        self.bus.register(agent_id);
    }

    pub fn remove_agent(&mut self, agent_id: &str) {
        self.agents.retain(|a| a != agent_id);
        self.bus.unregister(agent_id);
    }

    pub fn send(&self, msg: TeamMessage) {
        self.bus.send(msg);
    }

    pub fn receive(&self, agent_id: &str) -> Vec<TeamMessage> {
        self.bus.receive(agent_id)
    }

    pub fn agents(&self) -> &[String] {
        &self.agents
    }

    pub fn bus(&self) -> &MessageBus {
        &self.bus
    }

    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }
}

pub fn create_request(from: &str, to: &str, content: &str, msg_id: &str) -> TeamMessage {
    TeamMessage {
        id: msg_id.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        content: content.to_string(),
        msg_type: TeamMessageType::Request,
        timestamp: chrono::Utc::now(),
    }
}

pub fn create_reply(from: &str, to: &str, content: &str, msg_id: &str) -> TeamMessage {
    TeamMessage {
        id: msg_id.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        content: content.to_string(),
        msg_type: TeamMessageType::Reply,
        timestamp: chrono::Utc::now(),
    }
}
