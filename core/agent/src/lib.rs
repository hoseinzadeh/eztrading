//! The engine behind the `ezt-agent` command: an LLM agent held to the real SML
//! compiler. Also used by Strategy Studio (`core/studio`) for its AI features.

pub mod agent;
pub mod compiler;
pub mod llm;
