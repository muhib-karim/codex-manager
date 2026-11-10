use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for accounts
pub type AccountId = Uuid;

/// Mask an API key for display and logs: keeps a short prefix and suffix of
/// long keys and hides short keys completely.
pub fn mask_api_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() < 12 {
        return "****".to_string();
    }
    let prefix: String = chars[..3].iter().collect();
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    format!("{prefix}****{suffix}")
}

/// Account model representing a single OpenAI API tenant
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Account {
    pub id: AccountId,
    pub label: String,
    pub api_key: String,
    pub org_id: Option<String>,
    pub model_scope: Vec<String>,
    pub daily_limit: Option<f64>,
    pub monthly_limit: Option<f64>,
    pub priority: i32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_used: Option<DateTime<Utc>>,
}

// Manual Debug so an account can never put its API key into a log line.
impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("api_key", &mask_api_key(&self.api_key))
            .field("org_id", &self.org_id)
            .field("model_scope", &self.model_scope)
            .field("daily_limit", &self.daily_limit)
            .field("monthly_limit", &self.monthly_limit)
            .field("priority", &self.priority)
            .field("enabled", &self.enabled)
            .finish()
    }
}

impl Account {
    pub fn new(label: String, api_key: String) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            label,
            api_key,
            org_id: None,
            model_scope: vec![],
            daily_limit: None,
            monthly_limit: None,
            priority: 0,
            enabled: true,
            created_at: now,
            updated_at: now,
            last_used: None,
        }
    }

    pub fn with_org_id(mut self, org_id: String) -> Self {
        self.org_id = Some(org_id);
        self
    }

    pub fn with_model_scope(mut self, models: Vec<String>) -> Self {
        self.model_scope = models;
        self
    }

    pub fn with_limits(mut self, daily: Option<f64>, monthly: Option<f64>) -> Self {
        self.daily_limit = daily;
        self.monthly_limit = monthly;
        self
    }

    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Copy of this account that is safe to hand to the UI: the API key is
    /// replaced by its masked form.
    pub fn redacted(&self) -> Account {
        let mut copy = self.clone();
        copy.api_key = mask_api_key(&self.api_key);
        copy
    }
}

/// Account status combining account config with usage data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountStatus {
    pub account: Account,
    pub usage: UsageSnapshot,
    pub is_available: bool,
    pub disable_reason: Option<String>,
}

/// Usage snapshot for an account at a point in time
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub account_id: AccountId,
    pub tokens_used: u64,
    pub cost_estimate: f64,
    pub hard_limit: Option<f64>,
    pub soft_limit: Option<f64>,
    pub remaining_budget: Option<f64>,
    pub daily_usage: f64,
    pub monthly_usage: f64,
    pub timestamp: DateTime<Utc>,
}

fn ratio_of(used: f64, limit: Option<f64>) -> f64 {
    match limit {
        Some(value) if value > 0.0 => used / value,
        _ => 0.0,
    }
}

impl UsageSnapshot {
    pub fn new(account_id: AccountId) -> Self {
        Self {
            account_id,
            tokens_used: 0,
            cost_estimate: 0.0,
            hard_limit: None,
            soft_limit: None,
            remaining_budget: None,
            daily_usage: 0.0,
            monthly_usage: 0.0,
            timestamp: Utc::now(),
        }
    }

    /// Utilization ratio (0.0 - 1.0): the tightest of the limits that apply
    /// to this account. Takes the provider hard limit and the limits the
    /// user configured on the account into consideration, so an account
    /// without a provider limit is still ranked by its own budget.
    pub fn utilization_ratio(&self, account: &Account) -> f64 {
        let by_hard_limit = ratio_of(self.monthly_usage, self.hard_limit);
        let by_monthly_limit = ratio_of(self.monthly_usage, account.monthly_limit);
        let by_daily_limit = ratio_of(self.daily_usage, account.daily_limit);
        by_hard_limit
            .max(by_monthly_limit)
            .max(by_daily_limit)
            .clamp(0.0, 1.0)
    }

    /// Check if account is over its limits
    pub fn is_over_limit(&self, account: &Account) -> bool {
        if let Some(daily) = account.daily_limit {
            if self.daily_usage >= daily {
                return true;
            }
        }
        if let Some(monthly) = account.monthly_limit {
            if self.monthly_usage >= monthly {
                return true;
            }
        }
        if let Some(remaining) = self.remaining_budget {
            if remaining <= 0.0 {
                return true;
            }
        }
        false
    }
}

/// Request context passed to routing engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestContext {
    /// Requested model. Empty when the request does not name one.
    pub model: String,
    pub estimated_tokens: Option<u64>,
    pub session_id: Option<String>,
    pub priority: Option<i32>,
}

impl RequestContext {
    pub fn new(model: String) -> Self {
        Self {
            model,
            estimated_tokens: None,
            session_id: None,
            priority: None,
        }
    }

    pub fn with_session(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }
}

/// Account creation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAccountRequest {
    pub label: String,
    pub api_key: String,
    pub org_id: Option<String>,
    pub model_scope: Option<Vec<String>>,
    pub daily_limit: Option<f64>,
    pub monthly_limit: Option<f64>,
    pub priority: Option<i32>,
}

/// Account update request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateAccountRequest {
    pub id: AccountId,
    pub label: Option<String>,
    pub org_id: Option<String>,
    pub model_scope: Option<Vec<String>>,
    pub daily_limit: Option<f64>,
    pub monthly_limit: Option<f64>,
    pub priority: Option<i32>,
    pub enabled: Option<bool>,
}

/// Routing strategy types
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    #[default]
    LeastUtilized,
    RoundRobin,
    Priority,
    Sticky,
}

/// Routing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    pub strategy: RoutingStrategy,
    pub min_request_interval_ms: u64,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            strategy: RoutingStrategy::LeastUtilized,
            min_request_interval_ms: 100,
        }
    }
}

/// Proxy server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyServerConfig {
    pub bind_addr: String,
    pub api_key: String,
    pub openai_base_url: String,
}

impl Default for ProxyServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:8080".to_string(),
            api_key: "sk-codex-manager".to_string(),
            openai_base_url: "https://api.openai.com".to_string(),
        }
    }
}

/// App configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    pub proxy: ProxyServerConfig,
    pub routing: RoutingConfig,
}

/// Routing decision. The credentials travel with the decision inside the
/// backend but are never serialized or printed.
#[derive(Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub account_id: AccountId,
    pub account_label: String,
    pub reason: String,
    pub utilization_ratio: f64,
    pub remaining_budget: Option<f64>,
    #[serde(skip)]
    pub api_key: String,
    #[serde(skip)]
    pub org_id: Option<String>,
}

impl std::fmt::Debug for RoutingDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoutingDecision")
            .field("account_id", &self.account_id)
            .field("account_label", &self.account_label)
            .field("reason", &self.reason)
            .field("utilization_ratio", &self.utilization_ratio)
            .finish()
    }
}

/// Routing statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingStats {
    pub total_accounts: usize,
    pub available_accounts: usize,
    pub strategy: RoutingStrategy,
    pub open_circuits: usize,
    pub active_sessions: usize,
}

/// Proxy server status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyStatus {
    pub running: bool,
    pub bind_addr: String,
    pub request_count: u64,
    pub uptime_seconds: u64,
}

/// Import/Export data structure. This is the only shape that carries
/// plaintext API keys out of the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountExport {
    pub version: String,
    pub exported_at: DateTime<Utc>,
    pub accounts: Vec<Account>,
}

/// Validation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub valid: bool,
    pub org_id: Option<String>,
    pub error: Option<String>,
}

/// Outcome of a usage refresh for one account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRefreshResult {
    pub account_id: AccountId,
    pub usage: Option<UsageSnapshot>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> Account {
        Account::new("test".to_string(), "sk-test-0123456789abcdef".to_string())
    }

    #[test]
    fn mask_api_key_hides_the_middle_of_long_keys() {
        assert_eq!(mask_api_key("sk-test-0123456789abcdef"), "sk-****cdef");
    }

    #[test]
    fn mask_api_key_hides_short_keys_completely() {
        assert_eq!(mask_api_key("short"), "****");
        assert_eq!(mask_api_key("elevenchars"), "****");
        assert_eq!(mask_api_key(""), "****");
    }

    #[test]
    fn redacted_account_does_not_contain_the_key() {
        let original = account();
        let redacted = original.redacted();
        assert_eq!(redacted.id, original.id);
        assert_eq!(redacted.api_key, "sk-****cdef");
        let json = serde_json::to_string(&redacted).unwrap();
        assert!(!json.contains("0123456789"));
    }

    #[test]
    fn debug_output_never_contains_the_key() {
        let printed = format!("{:?}", account());
        assert!(!printed.contains("0123456789"));
        assert!(printed.contains("sk-****cdef"));
    }

    #[test]
    fn utilization_is_zero_without_any_limit() {
        let acc = account();
        let mut usage = UsageSnapshot::new(acc.id);
        usage.monthly_usage = 50.0;
        assert_eq!(usage.utilization_ratio(&acc), 0.0);
    }

    #[test]
    fn utilization_uses_the_account_monthly_limit_when_there_is_no_hard_limit() {
        let acc = account().with_limits(None, Some(200.0));
        let mut usage = UsageSnapshot::new(acc.id);
        usage.monthly_usage = 50.0;
        assert!((usage.utilization_ratio(&acc) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn utilization_takes_the_tightest_limit() {
        let acc = account().with_limits(Some(10.0), Some(1000.0));
        let mut usage = UsageSnapshot::new(acc.id);
        usage.hard_limit = Some(500.0);
        usage.monthly_usage = 100.0;
        usage.daily_usage = 9.0;
        // hard: 0.2, monthly: 0.1, daily: 0.9
        assert!((usage.utilization_ratio(&acc) - 0.9).abs() < 1e-9);
    }

    #[test]
    fn utilization_is_clamped_to_one() {
        let acc = account().with_limits(None, Some(10.0));
        let mut usage = UsageSnapshot::new(acc.id);
        usage.monthly_usage = 25.0;
        assert_eq!(usage.utilization_ratio(&acc), 1.0);
    }

    #[test]
    fn over_limit_checks_daily_monthly_and_remaining_budget() {
        let acc = account().with_limits(Some(5.0), Some(100.0));
        let mut usage = UsageSnapshot::new(acc.id);
        assert!(!usage.is_over_limit(&acc));
        usage.daily_usage = 5.0;
        assert!(usage.is_over_limit(&acc));
        usage.daily_usage = 0.0;
        usage.monthly_usage = 100.0;
        assert!(usage.is_over_limit(&acc));
        usage.monthly_usage = 0.0;
        usage.remaining_budget = Some(0.0);
        assert!(usage.is_over_limit(&acc));
    }

    #[test]
    fn routing_decision_never_serializes_or_prints_credentials() {
        let decision = RoutingDecision {
            account_id: Uuid::new_v4(),
            account_label: "primary".to_string(),
            reason: "least_utilized".to_string(),
            utilization_ratio: 0.5,
            remaining_budget: None,
            api_key: "sk-secret-value-123456".to_string(),
            org_id: Some("org-secret".to_string()),
        };
        let json = serde_json::to_string(&decision).unwrap();
        assert!(!json.contains("sk-secret-value-123456"));
        assert!(!json.contains("org-secret"));
        let printed = format!("{decision:?}");
        assert!(!printed.contains("sk-secret-value-123456"));
    }

    #[test]
    fn routing_strategy_uses_snake_case_names_on_the_wire() {
        let json = serde_json::to_string(&RoutingStrategy::LeastUtilized).unwrap();
        assert_eq!(json, "\"least_utilized\"");
        let parsed: RoutingStrategy = serde_json::from_str("\"round_robin\"").unwrap();
        assert_eq!(parsed, RoutingStrategy::RoundRobin);
        assert_eq!(RoutingStrategy::default(), RoutingStrategy::LeastUtilized);
    }
}
