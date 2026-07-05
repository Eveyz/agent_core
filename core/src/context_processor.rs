//! Context processors — lightweight "BeforeModel" transforms.
//!
//! Each [`ContextProcessor`] is a named function that transforms the outgoing
//! message list just before each LLM call. Multiple processors run in
//! registration order; each receives the output of the previous one.

use crate::types::Message;

/// Callback type for transform_context: receives messages, returns transformed messages.
pub type TransformContextFn = Box<dyn Fn(Vec<Message>) -> Vec<Message> + Send + Sync>;

/// A named, ordered context processor applied to the outgoing message list
/// just before each LLM call. Multiple processors run in registration order;
/// each receives the output of the previous one. This is the lightweight
/// "BeforeModel processor" seam — no trait, no control-flow change, just an
/// ordered list of transforms.
pub struct ContextProcessor {
    pub name: String,
    pub transform: TransformContextFn,
}
