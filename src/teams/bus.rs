use super::TeamMessage;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct MessageBus {
    inboxes: Arc<Mutex<HashMap<String, Vec<TeamMessage>>>>,
}

impl Default for MessageBus {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageBus {
    pub fn new() -> Self {
        Self {
            inboxes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register(&self, agent_id: &str) {
        let mut inboxes = self.inboxes.lock().unwrap();
        inboxes.entry(agent_id.to_string()).or_default();
    }

    pub fn unregister(&self, agent_id: &str) {
        let mut inboxes = self.inboxes.lock().unwrap();
        inboxes.remove(agent_id);
    }

    pub fn send(&self, msg: TeamMessage) {
        let mut inboxes = self.inboxes.lock().unwrap();
        inboxes.entry(msg.to.clone()).or_default().push(msg);
    }

    pub fn receive(&self, agent_id: &str) -> Vec<TeamMessage> {
        let mut inboxes = self.inboxes.lock().unwrap();
        inboxes
            .get_mut(agent_id)
            .map(std::mem::take)
            .unwrap_or_default()
    }

    pub fn peek(&self, agent_id: &str) -> Vec<TeamMessage> {
        let inboxes = self.inboxes.lock().unwrap();
        inboxes.get(agent_id).cloned().unwrap_or_default()
    }

    pub fn message_count(&self, agent_id: &str) -> usize {
        let inboxes = self.inboxes.lock().unwrap();
        inboxes.get(agent_id).map(|i| i.len()).unwrap_or(0)
    }

    pub fn total_messages(&self) -> usize {
        let inboxes = self.inboxes.lock().unwrap();
        inboxes.values().map(|i| i.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::{TeamMessage, TeamMessageType};
    use chrono::Utc;

    fn make_msg(from: &str, to: &str, content: &str) -> TeamMessage {
        TeamMessage {
            id: "test".to_string(),
            from: from.to_string(),
            to: to.to_string(),
            content: content.to_string(),
            msg_type: TeamMessageType::Request,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_register_and_send() {
        let bus = MessageBus::new();
        bus.register("agent1");
        bus.register("agent2");

        bus.send(make_msg("agent1", "agent2", "hello"));
        assert_eq!(bus.message_count("agent2"), 1);
        assert_eq!(bus.message_count("agent1"), 0);
    }

    #[test]
    fn test_receive_drains_inbox() {
        let bus = MessageBus::new();
        bus.register("a");
        bus.send(make_msg("system", "a", "msg1"));
        bus.send(make_msg("system", "a", "msg2"));

        let msgs = bus.receive("a");
        assert_eq!(msgs.len(), 2);
        assert_eq!(bus.message_count("a"), 0);
    }

    #[test]
    fn test_peek_does_not_drain() {
        let bus = MessageBus::new();
        bus.register("a");
        bus.send(make_msg("system", "a", "msg"));

        let peeked = bus.peek("a");
        assert_eq!(peeked.len(), 1);
        assert_eq!(bus.message_count("a"), 1);
    }
}
