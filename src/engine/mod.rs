//! Derives an agent's status from its screen, its session store and its own OSC 7501 reports.
//! Nothing here names an agent: differences come from its `AgentProfile`.

pub(crate) mod message;
pub(crate) mod regions;
pub(crate) mod report;
pub(crate) mod rules;
pub(crate) mod screen;
pub(crate) mod stabilizer;
pub(crate) mod store;
