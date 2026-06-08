use rustyline::completion::{Completer, extract_word};
use rustyline::hint::Hinter;
use rustyline::highlight::Highlighter;
use rustyline::validate::Validator;
use rustyline::{Context, Result};

#[derive(rustyline::Helper)]
pub struct CommandCompleter {
    commands: Vec<String>,
}

impl CommandCompleter {
    pub fn new() -> Self {
        Self {
            commands: vec![
                "/quit".into(), "/exit".into(), "/help".into(), "/models".into(),
                "/clear".into(), "/memory".into(), "/tokens".into(), "/permission".into(),
                "/perm".into(), "/hooks".into(), "/todo".into(), "/tasks".into(),
                "/skills".into(), "/status".into(), "/abort".into(), "/state".into(),
                "/tool-mode".into(), "/clear-queues".into(), "/model ".into(),
                "/temp ".into(), "/max-tokens ".into(), "/steer ".into(), "/follow-up ".into(),
                "/skill ".into()
            ]
        }
    }
}

impl Completer for CommandCompleter {
    type Candidate = String;

    fn complete(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Result<(usize, Vec<Self::Candidate>)> {
        let (start, word) = extract_word(line, pos, None, |c| c == ' ');
        if word.starts_with('/') {
            let matches: Vec<String> = self.commands
                .iter()
                .filter(|c| c.starts_with(word))
                .map(|c| c.clone())
                .collect();
            Ok((start, matches))
        } else {
            Ok((0, Vec::with_capacity(0)))
        }
    }
}

impl Hinter for CommandCompleter {
    type Hint = String;
}

impl Highlighter for CommandCompleter {}

impl Validator for CommandCompleter {}
