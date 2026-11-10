use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tracing::{debug, warn};
use uuid::Uuid;

use crate::models::{
    Account, AccountStatus, RequestContext, RoutingDecision, RoutingStats, RoutingStrategy,
    UsageSnapshot,
};

/// Consecutive account-level failures before the circuit opens.
const FAILURE_THRESHOLD: u32 = 3;
/// How long an open circuit keeps an account out of rotation.
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(60);
/// Upper bound for remembered sticky sessions. The map is cleared when it
/// is reached, so it cannot grow without limit.
const MAX_SESSIONS: usize = 10_000;
/// Two utilization ratios closer than this count as a tie.
const RATIO_EPSILON: f64 = 1e-9;

/// Whether an upstream HTTP status means the *account* is unusable right
/// now (bad or revoked key, no permission, rate limited, provider error),
/// as opposed to a problem with the individual request.
pub fn is_account_failure(status: u16) -> bool {
    status == 401 || status == 403 || status == 429 || status >= 500
}

/// Circuit breaker state for one account
#[derive(Debug, Clone, Copy)]
enum CircuitState {
    /// Normal operation
    Closed,
    /// Failing: skipped until the cool-down has passed
    Open { since: Instant },
    /// Cool-down passed: requests are let through to probe the account
    HalfOpen,
}

#[derive(Debug, Clone, Copy)]
struct AccountRouteState {
    circuit: CircuitState,
    consecutive_errors: u32,
}

impl AccountRouteState {
    fn new() -> Self {
        Self {
            circuit: CircuitState::Closed,
            consecutive_errors: 0,
        }
    }
}

/// Plain copy of the values a strategy needs to compare accounts.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    index: usize,
    id: Uuid,
    ratio: f64,
    priority: i32,
    last_selected: u64,
}

struct Inner {
    strategy: RoutingStrategy,
    accounts: Vec<AccountStatus>,
    sessions: HashMap<String, Uuid>,
    circuits: HashMap<Uuid, AccountRouteState>,
    last_selected: HashMap<Uuid, u64>,
    selection_counter: u64,
    round_robin_next: usize,
}

/// The routing engine decides which account serves a request.
///
/// All state sits behind one mutex and every method is synchronous, so a
/// lock is never held across an `.await`.
pub struct RoutingEngine {
    inner: Mutex<Inner>,
    cooldown: Duration,
}

fn supports_model(account: &Account, model: &str) -> bool {
    if model.is_empty() || account.model_scope.is_empty() {
        return true;
    }
    account
        .model_scope
        .iter()
        .any(|pattern| match pattern.strip_suffix('*') {
            Some(prefix) => model.starts_with(prefix),
            None => pattern.as_str() == model,
        })
}

/// Lowest utilization wins; ties go to the account selected longest ago.
fn pick_least_utilized(candidates: &[Candidate]) -> Candidate {
    let mut best = candidates[0];
    for candidate in candidates.iter().skip(1) {
        let lower = candidate.ratio < best.ratio - RATIO_EPSILON;
        let tied = (candidate.ratio - best.ratio).abs() <= RATIO_EPSILON;
        if lower || (tied && candidate.last_selected < best.last_selected) {
            best = *candidate;
        }
    }
    best
}

/// Highest priority wins; ties go to the account selected longest ago.
fn pick_by_priority(candidates: &[Candidate]) -> Candidate {
    let mut best = candidates[0];
    for candidate in candidates.iter().skip(1) {
        let higher = candidate.priority > best.priority;
        let tied = candidate.priority == best.priority;
        if higher || (tied && candidate.last_selected < best.last_selected) {
            best = *candidate;
        }
    }
    best
}

impl RoutingEngine {
    /// Create a new routing engine
    pub fn new(strategy: RoutingStrategy) -> Self {
        Self::with_cooldown(strategy, DEFAULT_COOLDOWN)
    }

    /// Create an engine with a custom circuit-breaker cool-down
    pub fn with_cooldown(strategy: RoutingStrategy, cooldown: Duration) -> Self {
        Self {
            inner: Mutex::new(Inner {
                strategy,
                accounts: Vec::new(),
                sessions: HashMap::new(),
                circuits: HashMap::new(),
                last_selected: HashMap::new(),
                selection_counter: 0,
                round_robin_next: 0,
            }),
            cooldown,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Change the routing strategy for subsequent requests
    pub fn set_strategy(&self, strategy: RoutingStrategy) {
        let mut inner = self.lock();
        inner.strategy = strategy;
    }

    /// Current strategy
    pub fn strategy(&self) -> RoutingStrategy {
        let inner = self.lock();
        inner.strategy
    }

    /// Replace the account list and usage data
    pub fn update_accounts(&self, accounts: Vec<Account>, usage_map: HashMap<Uuid, UsageSnapshot>) {
        let mut statuses = Vec::with_capacity(accounts.len());
        for account in accounts {
            let usage = match usage_map.get(&account.id) {
                Some(snapshot) => snapshot.clone(),
                None => UsageSnapshot::new(account.id),
            };
            let over_limit = usage.is_over_limit(&account);
            let disable_reason = if !account.enabled {
                Some("Account disabled".to_string())
            } else if over_limit {
                Some("Over usage limit".to_string())
            } else {
                None
            };
            statuses.push(AccountStatus {
                is_available: account.enabled && !over_limit,
                account,
                usage,
                disable_reason,
            });
        }

        let mut inner = self.lock();
        let known: Vec<Uuid> = statuses.iter().map(|status| status.account.id).collect();
        inner.circuits.retain(|id, _| known.contains(id));
        inner.last_selected.retain(|id, _| known.contains(id));
        inner.sessions.retain(|_, id| known.contains(id));
        inner.accounts = statuses;
        debug!("Updated {} accounts in routing engine", inner.accounts.len());
    }

    /// Whether the circuit lets a request through. An open circuit whose
    /// cool-down has passed moves to half-open so the account is probed.
    fn circuit_allows(inner: &mut Inner, id: Uuid, cooldown: Duration) -> bool {
        let state = match inner.circuits.get_mut(&id) {
            Some(state) => state,
            None => return true,
        };
        match state.circuit {
            CircuitState::Closed | CircuitState::HalfOpen => true,
            CircuitState::Open { since } => {
                if since.elapsed() >= cooldown {
                    state.circuit = CircuitState::HalfOpen;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Resolve which account to use for a request
    pub fn resolve_account(&self, ctx: &RequestContext) -> Result<RoutingDecision, String> {
        let mut guard = self.lock();
        let inner: &mut Inner = &mut guard;

        // Pass 1: accounts that are enabled, within limits and in scope.
        let mut eligible: Vec<Candidate> = Vec::new();
        for (index, status) in inner.accounts.iter().enumerate() {
            if !status.is_available || !supports_model(&status.account, &ctx.model) {
                continue;
            }
            let id = status.account.id;
            eligible.push(Candidate {
                index,
                id,
                ratio: status.usage.utilization_ratio(&status.account),
                priority: status.account.priority,
                last_selected: inner.last_selected.get(&id).copied().unwrap_or(0),
            });
        }

        // Pass 2: drop accounts whose circuit breaker is open.
        let mut candidates: Vec<Candidate> = Vec::new();
        for candidate in eligible {
            if Self::circuit_allows(inner, candidate.id, self.cooldown) {
                candidates.push(candidate);
            }
        }

        if candidates.is_empty() {
            if ctx.model.is_empty() {
                return Err("No available accounts".to_string());
            }
            return Err(format!("No available accounts for model {}", ctx.model));
        }

        let session: Option<String> = ctx
            .session_id
            .clone()
            .filter(|value| !value.is_empty());

        let (selected, reason) = match inner.strategy {
            RoutingStrategy::LeastUtilized => {
                (pick_least_utilized(&candidates), "least_utilized".to_string())
            }
            RoutingStrategy::RoundRobin => {
                let position = inner.round_robin_next % candidates.len();
                inner.round_robin_next = inner.round_robin_next.wrapping_add(1);
                (candidates[position], format!("round_robin:{position}"))
            }
            RoutingStrategy::Priority => {
                let picked = pick_by_priority(&candidates);
                (picked, format!("priority:{}", picked.priority))
            }
            RoutingStrategy::Sticky => match session {
                Some(session_id) => {
                    let remembered = inner.sessions.get(&session_id).copied();
                    let existing = remembered.and_then(|account_id| {
                        candidates.iter().find(|c| c.id == account_id).copied()
                    });
                    match existing {
                        Some(candidate) => (candidate, "sticky".to_string()),
                        None => {
                            let picked = pick_least_utilized(&candidates);
                            if inner.sessions.len() >= MAX_SESSIONS {
                                inner.sessions.clear();
                            }
                            inner.sessions.insert(session_id, picked.id);
                            (picked, "sticky:new_session".to_string())
                        }
                    }
                }
                None => (pick_least_utilized(&candidates), "sticky:no_session".to_string()),
            },
        };

        inner.selection_counter += 1;
        let sequence = inner.selection_counter;
        inner.last_selected.insert(selected.id, sequence);

        let status = &inner.accounts[selected.index];
        Ok(RoutingDecision {
            account_id: status.account.id,
            account_label: status.account.label.clone(),
            reason,
            utilization_ratio: selected.ratio,
            remaining_budget: status.usage.remaining_budget,
            api_key: status.account.api_key.clone(),
            org_id: status.account.org_id.clone(),
        })
    }

    /// Report a successful upstream call: closes the circuit
    pub fn report_success(&self, account_id: Uuid) {
        let mut inner = self.lock();
        inner.circuits.insert(account_id, AccountRouteState::new());
    }

    /// Report a failed upstream call. `account_failure` says whether the
    /// failure counts against the account (see `is_account_failure`).
    pub fn report_error(&self, account_id: Uuid, account_failure: bool) {
        if !account_failure {
            return;
        }
        let mut inner = self.lock();
        let state = inner
            .circuits
            .entry(account_id)
            .or_insert_with(AccountRouteState::new);
        state.consecutive_errors += 1;

        let probing = matches!(state.circuit, CircuitState::HalfOpen);
        if probing || state.consecutive_errors >= FAILURE_THRESHOLD {
            warn!(
                "Opening circuit breaker for account {} after {} errors",
                account_id, state.consecutive_errors
            );
            state.circuit = CircuitState::Open {
                since: Instant::now(),
            };
        }
    }

    fn is_circuit_open(inner: &Inner, id: Uuid) -> bool {
        inner
            .circuits
            .get(&id)
            .is_some_and(|state| matches!(state.circuit, CircuitState::Open { .. }))
    }

    /// Current routing statistics
    pub fn stats(&self) -> RoutingStats {
        let inner = self.lock();
        let mut available = 0;
        let mut open_circuits = 0;
        for status in inner.accounts.iter() {
            let open = Self::is_circuit_open(&inner, status.account.id);
            if open {
                open_circuits += 1;
            }
            if status.is_available && !open {
                available += 1;
            }
        }
        RoutingStats {
            total_accounts: inner.accounts.len(),
            available_accounts: available,
            strategy: inner.strategy,
            open_circuits,
            active_sessions: inner.sessions.len(),
        }
    }

    /// Forget all sticky sessions
    pub fn clear_sessions(&self) {
        let mut inner = self.lock();
        inner.sessions.clear();
    }

    /// Account statuses with the live circuit-breaker state applied
    pub fn account_statuses(&self) -> Vec<AccountStatus> {
        let inner = self.lock();
        let mut statuses = Vec::with_capacity(inner.accounts.len());
        for status in inner.accounts.iter() {
            let mut copy = status.clone();
            if copy.is_available && Self::is_circuit_open(&inner, status.account.id) {
                copy.is_available = false;
                copy.disable_reason = Some("Circuit breaker open".to_string());
            }
            statuses.push(copy);
        }
        statuses
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(label: &str) -> Account {
        Account::new(label.to_string(), format!("sk-{label}-0123456789abcdef"))
    }

    fn engine_with(strategy: RoutingStrategy, accounts: Vec<Account>) -> RoutingEngine {
        let engine = RoutingEngine::new(strategy);
        engine.update_accounts(accounts, HashMap::new());
        engine
    }

    fn ctx(model: &str) -> RequestContext {
        RequestContext::new(model.to_string())
    }

    fn usage(account: &Account, monthly_usage: f64) -> UsageSnapshot {
        let mut snapshot = UsageSnapshot::new(account.id);
        snapshot.monthly_usage = monthly_usage;
        snapshot
    }

    #[test]
    fn no_accounts_is_an_error() {
        let engine = engine_with(RoutingStrategy::LeastUtilized, vec![]);
        assert!(engine.resolve_account(&ctx("gpt-4o")).is_err());
    }

    #[test]
    fn decision_carries_the_selected_accounts_credentials() {
        let only = account("alpha").with_org_id("org-alpha".to_string());
        let engine = engine_with(RoutingStrategy::LeastUtilized, vec![only.clone()]);
        let decision = engine.resolve_account(&ctx("gpt-4o")).unwrap();
        assert_eq!(decision.account_id, only.id);
        assert_eq!(decision.api_key, only.api_key);
        assert_eq!(decision.org_id.as_deref(), Some("org-alpha"));
    }

    // Regression: utilization ignored the per-account limits, so without a
    // provider hard limit every ratio was 0 and the first account always won.
    #[test]
    fn least_utilized_ranks_by_the_account_limits() {
        let busy = account("busy").with_limits(None, Some(100.0));
        let idle = account("idle").with_limits(None, Some(100.0));
        let mut usage_map = HashMap::new();
        usage_map.insert(busy.id, usage(&busy, 80.0));
        usage_map.insert(idle.id, usage(&idle, 10.0));

        let engine = RoutingEngine::new(RoutingStrategy::LeastUtilized);
        engine.update_accounts(vec![busy.clone(), idle.clone()], usage_map);

        for _ in 0..3 {
            let decision = engine.resolve_account(&ctx("gpt-4o")).unwrap();
            assert_eq!(decision.account_id, idle.id);
            assert_eq!(decision.reason, "least_utilized");
        }
    }

    #[test]
    fn least_utilized_spreads_ties_across_accounts() {
        let a = account("a");
        let b = account("b");
        let c = account("c");
        let engine = engine_with(
            RoutingStrategy::LeastUtilized,
            vec![a.clone(), b.clone(), c.clone()],
        );

        let mut picked = Vec::new();
        for _ in 0..3 {
            picked.push(engine.resolve_account(&ctx("gpt-4o")).unwrap().account_id);
        }
        assert!(picked.contains(&a.id));
        assert!(picked.contains(&b.id));
        assert!(picked.contains(&c.id));
    }

    // Regression: round-robin used `blocking_read()` inside async code and
    // panicked on every request.
    #[test]
    fn round_robin_cycles_through_all_accounts() {
        let a = account("a");
        let b = account("b");
        let c = account("c");
        let engine = engine_with(
            RoutingStrategy::RoundRobin,
            vec![a.clone(), b.clone(), c.clone()],
        );

        let mut picked = Vec::new();
        for _ in 0..6 {
            picked.push(engine.resolve_account(&ctx("gpt-4o")).unwrap().account_id);
        }
        assert_eq!(picked, vec![a.id, b.id, c.id, a.id, b.id, c.id]);
    }

    #[test]
    fn priority_picks_the_highest_priority_account() {
        let low = account("low").with_priority(1);
        let high = account("high").with_priority(10);
        let mid = account("mid").with_priority(5);
        let engine = engine_with(
            RoutingStrategy::Priority,
            vec![low.clone(), high.clone(), mid.clone()],
        );

        for _ in 0..3 {
            let decision = engine.resolve_account(&ctx("gpt-4o")).unwrap();
            assert_eq!(decision.account_id, high.id);
            assert_eq!(decision.reason, "priority:10");
        }
    }

    #[test]
    fn priority_falls_back_when_the_top_account_is_disabled() {
        let low = account("low").with_priority(1);
        let mut high = account("high").with_priority(10);
        high.enabled = false;
        let engine = engine_with(RoutingStrategy::Priority, vec![low.clone(), high]);
        let decision = engine.resolve_account(&ctx("gpt-4o")).unwrap();
        assert_eq!(decision.account_id, low.id);
    }

    #[test]
    fn sticky_keeps_a_session_on_one_account() {
        let a = account("a");
        let b = account("b");
        let engine = engine_with(RoutingStrategy::Sticky, vec![a, b]);

        let session = ctx("gpt-4o").with_session("session-1".to_string());
        let first = engine.resolve_account(&session).unwrap().account_id;
        for _ in 0..5 {
            let again = engine.resolve_account(&session).unwrap();
            assert_eq!(again.account_id, first);
            assert_eq!(again.reason, "sticky");
        }
        assert_eq!(engine.stats().active_sessions, 1);

        engine.clear_sessions();
        assert_eq!(engine.stats().active_sessions, 0);
    }

    // Regression: a missing session used to be turned into "" and every
    // session-less request stuck to the same account.
    #[test]
    fn sticky_does_not_pin_requests_without_a_session() {
        let a = account("a");
        let b = account("b");
        let engine = engine_with(RoutingStrategy::Sticky, vec![a.clone(), b.clone()]);

        let mut picked = Vec::new();
        for _ in 0..2 {
            picked.push(engine.resolve_account(&ctx("gpt-4o")).unwrap().account_id);
        }
        let empty = ctx("gpt-4o").with_session(String::new());
        picked.push(engine.resolve_account(&empty).unwrap().account_id);

        assert!(picked.contains(&a.id));
        assert!(picked.contains(&b.id));
        assert_eq!(engine.stats().active_sessions, 0);
    }

    #[test]
    fn sticky_moves_a_session_when_its_account_goes_away() {
        let a = account("a");
        let b = account("b");
        let engine = engine_with(RoutingStrategy::Sticky, vec![a.clone(), b.clone()]);
        let session = ctx("gpt-4o").with_session("session-1".to_string());
        let first = engine.resolve_account(&session).unwrap().account_id;

        let remaining = if first == a.id { b.clone() } else { a.clone() };
        engine.update_accounts(vec![remaining.clone()], HashMap::new());
        let moved = engine.resolve_account(&session).unwrap();
        assert_eq!(moved.account_id, remaining.id);
    }

    // Regression: `set_routing_strategy` was a no-op.
    #[test]
    fn the_strategy_can_be_changed_at_runtime() {
        let low = account("low").with_priority(1);
        let high = account("high").with_priority(10);
        let engine = engine_with(RoutingStrategy::RoundRobin, vec![low, high.clone()]);
        assert_eq!(engine.strategy(), RoutingStrategy::RoundRobin);

        engine.set_strategy(RoutingStrategy::Priority);
        assert_eq!(engine.strategy(), RoutingStrategy::Priority);
        assert_eq!(engine.stats().strategy, RoutingStrategy::Priority);
        let decision = engine.resolve_account(&ctx("gpt-4o")).unwrap();
        assert_eq!(decision.account_id, high.id);
    }

    #[test]
    fn model_scope_filters_accounts_and_supports_prefix_wildcards() {
        let gpt4_only = account("gpt4").with_model_scope(vec!["gpt-4*".to_string()]);
        let embeddings =
            account("embed").with_model_scope(vec!["text-embedding-3-small".to_string()]);
        let engine = engine_with(
            RoutingStrategy::RoundRobin,
            vec![gpt4_only.clone(), embeddings.clone()],
        );

        for _ in 0..2 {
            let decision = engine.resolve_account(&ctx("gpt-4o-mini")).unwrap();
            assert_eq!(decision.account_id, gpt4_only.id);
        }
        let decision = engine
            .resolve_account(&ctx("text-embedding-3-small"))
            .unwrap();
        assert_eq!(decision.account_id, embeddings.id);
        assert!(engine.resolve_account(&ctx("dall-e-3")).is_err());
        // A request that names no model is not filtered by scope.
        assert!(engine.resolve_account(&ctx("")).is_ok());
    }

    #[test]
    fn disabled_and_over_limit_accounts_are_skipped() {
        let mut disabled = account("disabled");
        disabled.enabled = false;
        let capped = account("capped").with_limits(None, Some(10.0));
        let healthy = account("healthy");
        let mut usage_map = HashMap::new();
        usage_map.insert(capped.id, usage(&capped, 10.0));

        let engine = RoutingEngine::new(RoutingStrategy::RoundRobin);
        engine.update_accounts(vec![disabled, capped, healthy.clone()], usage_map);

        for _ in 0..3 {
            let decision = engine.resolve_account(&ctx("gpt-4o")).unwrap();
            assert_eq!(decision.account_id, healthy.id);
        }
        let stats = engine.stats();
        assert_eq!(stats.total_accounts, 3);
        assert_eq!(stats.available_accounts, 1);
    }

    #[test]
    fn circuit_opens_after_three_account_failures() {
        let flaky = account("flaky");
        let healthy = account("healthy");
        let engine = engine_with(
            RoutingStrategy::RoundRobin,
            vec![flaky.clone(), healthy.clone()],
        );

        engine.report_error(flaky.id, true);
        engine.report_error(flaky.id, true);
        assert_eq!(engine.stats().open_circuits, 0);
        engine.report_error(flaky.id, true);
        assert_eq!(engine.stats().open_circuits, 1);
        assert_eq!(engine.stats().available_accounts, 1);

        for _ in 0..4 {
            let decision = engine.resolve_account(&ctx("gpt-4o")).unwrap();
            assert_eq!(decision.account_id, healthy.id);
        }

        let statuses = engine.account_statuses();
        let flaky_status = statuses
            .iter()
            .find(|status| status.account.id == flaky.id)
            .unwrap();
        assert!(!flaky_status.is_available);
        assert_eq!(
            flaky_status.disable_reason.as_deref(),
            Some("Circuit breaker open")
        );
    }

    #[test]
    fn request_level_errors_do_not_open_the_circuit() {
        let only = account("only");
        let engine = engine_with(RoutingStrategy::RoundRobin, vec![only.clone()]);
        for _ in 0..10 {
            engine.report_error(only.id, false);
        }
        assert_eq!(engine.stats().open_circuits, 0);
        assert!(engine.resolve_account(&ctx("gpt-4o")).is_ok());
    }

    #[test]
    fn a_success_resets_the_failure_count() {
        let only = account("only");
        let engine = engine_with(RoutingStrategy::RoundRobin, vec![only.clone()]);
        engine.report_error(only.id, true);
        engine.report_error(only.id, true);
        engine.report_success(only.id);
        engine.report_error(only.id, true);
        engine.report_error(only.id, true);
        assert_eq!(engine.stats().open_circuits, 0);
    }

    // Regression: a tripped account could stay out of rotation forever
    // because the half-open state was never entered.
    #[test]
    fn an_open_circuit_recovers_after_the_cooldown() {
        let only = account("only");
        let engine = RoutingEngine::with_cooldown(RoutingStrategy::RoundRobin, Duration::ZERO);
        engine.update_accounts(vec![only.clone()], HashMap::new());

        for _ in 0..3 {
            engine.report_error(only.id, true);
        }
        assert_eq!(engine.stats().open_circuits, 1);

        // Cool-down (zero here) has passed: the account is probed again.
        let probe = engine.resolve_account(&ctx("gpt-4o")).unwrap();
        assert_eq!(probe.account_id, only.id);
        engine.report_success(only.id);
        assert_eq!(engine.stats().open_circuits, 0);
        assert_eq!(engine.stats().available_accounts, 1);
    }

    #[test]
    fn an_open_circuit_blocks_until_the_cooldown_passes() {
        let only = account("only");
        let engine =
            RoutingEngine::with_cooldown(RoutingStrategy::RoundRobin, Duration::from_secs(3600));
        engine.update_accounts(vec![only.clone()], HashMap::new());
        for _ in 0..3 {
            engine.report_error(only.id, true);
        }
        assert!(engine.resolve_account(&ctx("gpt-4o")).is_err());
    }

    #[test]
    fn a_failed_probe_reopens_the_circuit() {
        let only = account("only");
        let engine = RoutingEngine::with_cooldown(RoutingStrategy::RoundRobin, Duration::ZERO);
        engine.update_accounts(vec![only.clone()], HashMap::new());
        for _ in 0..3 {
            engine.report_error(only.id, true);
        }
        assert!(engine.resolve_account(&ctx("gpt-4o")).is_ok());
        engine.report_error(only.id, true);
        assert_eq!(engine.stats().open_circuits, 1);
    }

    #[test]
    fn account_failure_statuses() {
        assert!(is_account_failure(401));
        assert!(is_account_failure(403));
        assert!(is_account_failure(429));
        assert!(is_account_failure(500));
        assert!(is_account_failure(503));
        assert!(!is_account_failure(200));
        assert!(!is_account_failure(400));
        assert!(!is_account_failure(404));
        assert!(!is_account_failure(422));
    }

    #[test]
    fn removed_accounts_are_forgotten() {
        let a = account("a");
        let b = account("b");
        let engine = engine_with(RoutingStrategy::RoundRobin, vec![a.clone(), b.clone()]);
        for _ in 0..3 {
            engine.report_error(a.id, true);
        }
        assert_eq!(engine.stats().open_circuits, 1);

        engine.update_accounts(vec![b.clone()], HashMap::new());
        let stats = engine.stats();
        assert_eq!(stats.total_accounts, 1);
        assert_eq!(stats.open_circuits, 0);
    }
}
