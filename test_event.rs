use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentEvent {
    AgentStart,
    TurnStart { turn_index: usize },
}

fn main() {
    let event = AgentEvent::TurnStart { turn_index: 0 };
    println!("{}", serde_json::to_string(&event).unwrap());
}
