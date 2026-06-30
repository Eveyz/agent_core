use agent_core::session::SessionManager;
use agent_core::memory::storage::Storage;

fn main() {
    let storage = Storage::new("/Users/zniverse/.agverse/memory.db").unwrap();
    let sm = SessionManager::new(storage);
    
    // First list sessions to get a valid session ID
    let sessions = sm.list(true).unwrap();
    if sessions.is_empty() {
        println!("No sessions found!");
        return;
    }
    
    let session_id = &sessions[0].id;
    println!("Found session: {}", session_id);
    
    match sm.resume(session_id) {
        Ok(Some(session)) => println!("Resumed successfully! Messages: {}", session.messages.len()),
        Ok(None) => println!("Session not found in resume!"),
        Err(e) => println!("Error in resume: {:?}", e),
    }
}
