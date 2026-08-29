//! Model-role routing (spec §13): different loop stages can use different
//! models. MVP maps every role to one configured provider; the abstraction
//! lets heavier roles upgrade independently later.

use std::sync::Arc;

use super::AgentProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelRole {
    /// Repository classification / cheap summarization.
    Classification,
    /// Architecture reasoning, failure diagnosis.
    Reasoning,
    /// Code generation.
    Coding,
    /// Simple formatting tasks.
    Formatting,
}

pub struct ProviderRouter {
    default: Arc<dyn AgentProvider>,
    by_role: std::collections::HashMap<ModelRole, Arc<dyn AgentProvider>>,
}

impl ProviderRouter {
    pub fn new(default: Arc<dyn AgentProvider>) -> Self {
        Self {
            default,
            by_role: std::collections::HashMap::new(),
        }
    }

    /// Override the provider used for a specific role.
    pub fn set_role(&mut self, role: ModelRole, provider: Arc<dyn AgentProvider>) {
        self.by_role.insert(role, provider);
    }

    pub fn provider_for(&self, role: ModelRole) -> &Arc<dyn AgentProvider> {
        self.by_role.get(&role).unwrap_or(&self.default)
    }

    pub fn default_provider(&self) -> &Arc<dyn AgentProvider> {
        &self.default
    }
}
