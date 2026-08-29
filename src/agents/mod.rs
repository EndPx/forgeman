//! Logical agents of the ForgeMan loop (spec §7–17). Each agent is exposed
//! as a `Stage` the orchestrator can run.

pub mod analyze;
pub mod coder;
pub mod inspect;
pub mod llm;
pub mod plan;
pub mod test_runner;
