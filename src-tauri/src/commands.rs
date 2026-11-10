//! Tauri commands. Every command lives in this module and is registered in
//! `lib.rs` by its module path (`commands::add_account`).
//!
//! Accounts returned to the UI are redacted (`Account::redacted`): the API
//! key is masked. `export_accounts` is the only command that returns keys.

use std::collections::HashMap;

use tauri::State;
use tracing::info;
use uuid::Uuid;

use crate::config;
use crate::models::{
    Account, AccountExport, AccountId, AccountStatus, AppConfig, CreateAccountRequest,
    ProxyServerConfig, ProxyStatus, RoutingConfig, RoutingStats, RoutingStrategy,
    UpdateAccountRequest, UsageRefreshResult, UsageSnapshot, ValidationResult,
};
use crate::proxy::ProxyServer;
use crate::usage::OpenAIClient;
use crate::AppState;

// ============================================================================
// Pure helpers (unit tested below)
// ============================================================================

/// A new account needs a label and an API key
pub fn validate_new_account(label: &str, api_key: &str) -> Result<(), String> {
    if label.trim().is_empty() {
        return Err("Account label must not be empty".to_string());
    }
    if api_key.trim().is_empty() {
        return Err("API key must not be empty".to_string());
    }
    Ok(())
}

/// Build an account from a creation request
pub fn account_from_request(request: CreateAccountRequest) -> Result<Account, String> {
    validate_new_account(&request.label, &request.api_key)?;

    let mut account = Account::new(
        request.label.trim().to_string(),
        request.api_key.trim().to_string(),
    )
    .with_limits(request.daily_limit, request.monthly_limit);

    if let Some(org_id) = request.org_id {
        if !org_id.trim().is_empty() {
            account = account.with_org_id(org_id.trim().to_string());
        }
    }
    if let Some(models) = request.model_scope {
        account = account.with_model_scope(models);
    }
    if let Some(priority) = request.priority {
        account = account.with_priority(priority);
    }
    Ok(account)
}

/// Apply the fields present in an update request
pub fn apply_update(account: &mut Account, request: UpdateAccountRequest) {
    if let Some(label) = request.label {
        account.label = label;
    }
    if let Some(org_id) = request.org_id {
        account.org_id = Some(org_id);
    }
    if let Some(models) = request.model_scope {
        account.model_scope = models;
    }
    if let Some(daily) = request.daily_limit {
        account.daily_limit = Some(daily);
    }
    if let Some(monthly) = request.monthly_limit {
        account.monthly_limit = Some(monthly);
    }
    if let Some(priority) = request.priority {
        account.priority = priority;
    }
    if let Some(enabled) = request.enabled {
        account.enabled = enabled;
    }
    account.updated_at = chrono::Utc::now();
}

/// Turn imported accounts into new accounts: entries without a label or
/// key and entries whose key already exists are dropped, and every kept
/// entry gets a fresh id and timestamps.
pub fn prepare_import(existing_keys: &[String], accounts: Vec<Account>) -> Vec<Account> {
    let mut seen: Vec<String> = existing_keys.to_vec();
    let mut prepared = Vec::new();
    for mut account in accounts {
        if validate_new_account(&account.label, &account.api_key).is_err() {
            continue;
        }
        if seen.contains(&account.api_key) {
            continue;
        }
        seen.push(account.api_key.clone());

        let now = chrono::Utc::now();
        account.id = Uuid::new_v4();
        account.created_at = now;
        account.updated_at = now;
        account.last_used = None;
        prepared.push(account);
    }
    prepared
}

/// Reload accounts and usage from the store into the routing engine
fn refresh_routing_engine(state: &AppState) -> Result<(), String> {
    let accounts = state.store.load_accounts().map_err(|e| e.to_string())?;

    let mut usage_map = HashMap::new();
    for account in &accounts {
        if let Ok(Some(usage)) = state.store.load_latest_usage(account.id) {
            usage_map.insert(account.id, usage);
        }
    }

    state.routing_engine.update_accounts(accounts, usage_map);
    Ok(())
}

// ============================================================================
// Account management
// ============================================================================

/// Add a new OpenAI account
#[tauri::command]
pub async fn add_account(
    state: State<'_, AppState>,
    request: CreateAccountRequest,
) -> Result<Account, String> {
    let account = account_from_request(request)?;

    state
        .store
        .save_account(&account)
        .map_err(|e| format!("Failed to save account: {e}"))?;
    refresh_routing_engine(&state)?;

    info!("Added account: {} ({})", account.label, account.id);
    Ok(account.redacted())
}

/// Update an existing account
#[tauri::command]
pub async fn update_account(
    state: State<'_, AppState>,
    request: UpdateAccountRequest,
) -> Result<Account, String> {
    let mut account = state
        .store
        .load_account(request.id)
        .map_err(|e| format!("Failed to load account: {e}"))?
        .ok_or_else(|| "Account not found".to_string())?;

    apply_update(&mut account, request);

    state
        .store
        .save_account(&account)
        .map_err(|e| format!("Failed to save account: {e}"))?;
    refresh_routing_engine(&state)?;

    Ok(account.redacted())
}

/// Remove an account by ID
#[tauri::command]
pub async fn remove_account(state: State<'_, AppState>, id: AccountId) -> Result<bool, String> {
    let deleted = state
        .store
        .delete_account(id)
        .map_err(|e| format!("Failed to delete account: {e}"))?;

    if deleted {
        refresh_routing_engine(&state)?;
        info!("Removed account: {id}");
    }
    Ok(deleted)
}

/// Get a single account by ID
#[tauri::command]
pub async fn get_account(
    state: State<'_, AppState>,
    id: AccountId,
) -> Result<Option<Account>, String> {
    let account = state.store.load_account(id).map_err(|e| e.to_string())?;
    Ok(account.map(|account| account.redacted()))
}

/// List all accounts
#[tauri::command]
pub async fn list_accounts(state: State<'_, AppState>) -> Result<Vec<Account>, String> {
    let accounts = state.store.load_accounts().map_err(|e| e.to_string())?;
    Ok(accounts.iter().map(Account::redacted).collect())
}

/// List all accounts with their current routing status
#[tauri::command]
pub async fn list_account_statuses(
    state: State<'_, AppState>,
) -> Result<Vec<AccountStatus>, String> {
    let mut statuses = state.routing_engine.account_statuses();
    for status in statuses.iter_mut() {
        status.account = status.account.redacted();
    }
    Ok(statuses)
}

/// Toggle account enabled state
#[tauri::command]
pub async fn toggle_account_enabled(
    state: State<'_, AppState>,
    id: AccountId,
) -> Result<Account, String> {
    let mut account = state
        .store
        .load_account(id)
        .map_err(|e| format!("Failed to load account: {e}"))?
        .ok_or_else(|| "Account not found".to_string())?;

    account.enabled = !account.enabled;
    account.updated_at = chrono::Utc::now();

    state
        .store
        .save_account(&account)
        .map_err(|e| format!("Failed to save account: {e}"))?;
    refresh_routing_engine(&state)?;

    Ok(account.redacted())
}

// ============================================================================
// Usage data
// ============================================================================

/// Get the latest stored usage snapshot for an account
#[tauri::command]
pub async fn get_account_usage(
    state: State<'_, AppState>,
    id: AccountId,
) -> Result<Option<UsageSnapshot>, String> {
    state.store.load_latest_usage(id).map_err(|e| e.to_string())
}

/// Refresh usage data for all accounts
#[tauri::command]
pub async fn refresh_all_usage(
    state: State<'_, AppState>,
) -> Result<Vec<UsageRefreshResult>, String> {
    let accounts = state.store.load_accounts().map_err(|e| e.to_string())?;
    let client = OpenAIClient::new();
    let mut results = Vec::new();

    for account in accounts {
        let result = match client.fetch_usage(&account).await {
            Ok(usage) => match state.store.save_usage_snapshot(&usage) {
                Ok(()) => UsageRefreshResult {
                    account_id: account.id,
                    usage: Some(usage),
                    error: None,
                },
                Err(e) => UsageRefreshResult {
                    account_id: account.id,
                    usage: None,
                    error: Some(format!("Failed to save usage: {e}")),
                },
            },
            Err(e) => UsageRefreshResult {
                account_id: account.id,
                usage: None,
                error: Some(e.to_string()),
            },
        };
        results.push(result);
    }

    refresh_routing_engine(&state)?;
    Ok(results)
}

/// Refresh usage for a single account
#[tauri::command]
pub async fn refresh_account_usage(
    state: State<'_, AppState>,
    id: AccountId,
) -> Result<UsageSnapshot, String> {
    let account = state
        .store
        .load_account(id)
        .map_err(|e| format!("Failed to load account: {e}"))?
        .ok_or_else(|| "Account not found".to_string())?;

    let client = OpenAIClient::new();
    let usage = client
        .fetch_usage(&account)
        .await
        .map_err(|e| e.to_string())?;

    state
        .store
        .save_usage_snapshot(&usage)
        .map_err(|e| format!("Failed to save usage: {e}"))?;
    refresh_routing_engine(&state)?;

    Ok(usage)
}

// ============================================================================
// Routing
// ============================================================================

/// Get current routing statistics
#[tauri::command]
pub async fn get_routing_stats(state: State<'_, AppState>) -> Result<RoutingStats, String> {
    Ok(state.routing_engine.stats())
}

/// Set the routing strategy: applies to the running engine and is saved
#[tauri::command]
pub async fn set_routing_strategy(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    strategy: RoutingStrategy,
) -> Result<(), String> {
    state.routing_engine.set_strategy(strategy);

    let mut app_config = config::load_config(&app_handle).map_err(|e| e.to_string())?;
    app_config.routing.strategy = strategy;
    config::save_config(&app_handle, &app_config).map_err(|e| e.to_string())?;

    info!("Routing strategy set to {strategy:?}");
    Ok(())
}

/// Clear all sticky-session mappings
#[tauri::command]
pub async fn clear_routing_sessions(state: State<'_, AppState>) -> Result<(), String> {
    state.routing_engine.clear_sessions();
    Ok(())
}

// ============================================================================
// Proxy server
// ============================================================================

/// Start the local proxy server
#[tauri::command]
pub async fn start_proxy_server(
    state: State<'_, AppState>,
    config: ProxyServerConfig,
) -> Result<ProxyStatus, String> {
    let mut guard = state.proxy.lock().await;
    if guard.is_some() {
        return Err("Proxy server already running".to_string());
    }

    let server = ProxyServer::start(state.routing_engine.clone(), config).await?;
    let status = server.status();
    *guard = Some(server);
    Ok(status)
}

/// Stop the local proxy server
#[tauri::command]
pub async fn stop_proxy_server(state: State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.proxy.lock().await;
    match guard.take() {
        Some(server) => {
            server.stop();
            Ok(())
        }
        None => Err("Proxy server not running".to_string()),
    }
}

/// Get proxy status
#[tauri::command]
pub async fn get_proxy_status(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<ProxyStatus, String> {
    let guard = state.proxy.lock().await;
    if let Some(server) = guard.as_ref() {
        return Ok(server.status());
    }
    drop(guard);

    let bind_addr = match config::load_config(&app_handle) {
        Ok(app_config) => app_config.proxy.bind_addr,
        Err(_) => ProxyServerConfig::default().bind_addr,
    };
    Ok(ProxyStatus {
        running: false,
        bind_addr,
        request_count: 0,
        uptime_seconds: 0,
    })
}

// ============================================================================
// Import / export
// ============================================================================

/// Export all accounts, including their API keys
#[tauri::command]
pub async fn export_accounts(state: State<'_, AppState>) -> Result<AccountExport, String> {
    let accounts = state.store.load_accounts().map_err(|e| e.to_string())?;

    Ok(AccountExport {
        version: env!("CARGO_PKG_VERSION").to_string(),
        exported_at: chrono::Utc::now(),
        accounts,
    })
}

/// Import accounts from an export. Returns the accounts that were added.
#[tauri::command]
pub async fn import_accounts(
    state: State<'_, AppState>,
    export: AccountExport,
) -> Result<Vec<Account>, String> {
    let existing = state.store.load_accounts().map_err(|e| e.to_string())?;
    let existing_keys: Vec<String> = existing
        .iter()
        .map(|account| account.api_key.clone())
        .collect();

    let prepared = prepare_import(&existing_keys, export.accounts);
    for account in &prepared {
        state
            .store
            .save_account(account)
            .map_err(|e| format!("Failed to import account {}: {}", account.label, e))?;
    }
    refresh_routing_engine(&state)?;

    info!("Imported {} accounts", prepared.len());
    Ok(prepared.iter().map(Account::redacted).collect())
}

/// Validate an API key without saving it
#[tauri::command]
pub async fn validate_api_key(
    api_key: String,
    org_id: Option<String>,
) -> Result<ValidationResult, String> {
    let client = OpenAIClient::new();
    client
        .validate_key(&api_key, org_id.as_deref())
        .await
        .map_err(|e| e.to_string())
}

// ============================================================================
// Configuration
// ============================================================================

/// Load configuration
#[tauri::command]
pub async fn load_app_config(app_handle: tauri::AppHandle) -> Result<AppConfig, String> {
    config::load_config(&app_handle).map_err(|e| e.to_string())
}

/// Save configuration and apply the routing strategy
#[tauri::command]
pub async fn save_app_config(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    config: AppConfig,
) -> Result<(), String> {
    // `config` is the argument here, so the module is named by its full path.
    crate::config::save_config(&app_handle, &config).map_err(|e| e.to_string())?;
    state.routing_engine.set_strategy(config.routing.strategy);
    Ok(())
}

/// Update proxy configuration (applies the next time the proxy starts)
#[tauri::command]
pub async fn update_proxy_configuration(
    app_handle: tauri::AppHandle,
    proxy: ProxyServerConfig,
) -> Result<(), String> {
    config::update_proxy_config(&app_handle, proxy).map_err(|e| e.to_string())
}

/// Update routing configuration and apply the strategy
#[tauri::command]
pub async fn update_routing_configuration(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    routing: RoutingConfig,
) -> Result<(), String> {
    let strategy = routing.strategy;
    config::update_routing_config(&app_handle, routing).map_err(|e| e.to_string())?;
    state.routing_engine.set_strategy(strategy);
    Ok(())
}

/// Get the data directory path
#[tauri::command]
pub async fn get_data_directory(app_handle: tauri::AppHandle) -> Result<String, String> {
    let path = config::get_data_dir(&app_handle).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(label: &str, api_key: &str) -> CreateAccountRequest {
        CreateAccountRequest {
            label: label.to_string(),
            api_key: api_key.to_string(),
            org_id: None,
            model_scope: None,
            daily_limit: None,
            monthly_limit: None,
            priority: None,
        }
    }

    #[test]
    fn new_accounts_need_a_label_and_a_key() {
        assert!(validate_new_account("Work", "sk-abc").is_ok());
        assert!(validate_new_account("  ", "sk-abc").is_err());
        assert!(validate_new_account("Work", "").is_err());
        assert!(account_from_request(request("", "sk-abc")).is_err());
        assert!(account_from_request(request("Work", "   ")).is_err());
    }

    // Regression: a monthly limit was dropped unless a daily limit was
    // also given.
    #[test]
    fn monthly_limit_is_kept_without_a_daily_limit() {
        let mut create = request("Work", "sk-abc");
        create.monthly_limit = Some(50.0);
        let account = account_from_request(create).unwrap();
        assert_eq!(account.daily_limit, None);
        assert_eq!(account.monthly_limit, Some(50.0));
    }

    #[test]
    fn creation_request_fields_are_applied_and_trimmed() {
        let mut create = request("  Work  ", "  sk-abc  ");
        create.org_id = Some("org-1".to_string());
        create.model_scope = Some(vec!["gpt-4*".to_string()]);
        create.daily_limit = Some(5.0);
        create.priority = Some(3);

        let account = account_from_request(create).unwrap();
        assert_eq!(account.label, "Work");
        assert_eq!(account.api_key, "sk-abc");
        assert_eq!(account.org_id.as_deref(), Some("org-1"));
        assert_eq!(account.model_scope, vec!["gpt-4*".to_string()]);
        assert_eq!(account.daily_limit, Some(5.0));
        assert_eq!(account.priority, 3);
        assert!(account.enabled);
    }

    #[test]
    fn blank_org_id_is_not_stored() {
        let mut create = request("Work", "sk-abc");
        create.org_id = Some("  ".to_string());
        let account = account_from_request(create).unwrap();
        assert_eq!(account.org_id, None);
    }

    #[test]
    fn update_changes_only_the_given_fields() {
        let mut account = account_from_request(request("Work", "sk-abc")).unwrap();
        let update = UpdateAccountRequest {
            id: account.id,
            label: Some("Renamed".to_string()),
            org_id: None,
            model_scope: None,
            daily_limit: None,
            monthly_limit: Some(20.0),
            priority: None,
            enabled: Some(false),
        };
        apply_update(&mut account, update);

        assert_eq!(account.label, "Renamed");
        assert_eq!(account.api_key, "sk-abc");
        assert_eq!(account.monthly_limit, Some(20.0));
        assert_eq!(account.priority, 0);
        assert!(!account.enabled);
    }

    #[test]
    fn import_skips_blank_and_duplicate_keys_and_assigns_new_ids() {
        let existing = ["sk-existing".to_string()];
        let fresh = Account::new("Fresh".to_string(), "sk-fresh".to_string());
        let fresh_id = fresh.id;
        let incoming = vec![
            fresh,
            Account::new("Already there".to_string(), "sk-existing".to_string()),
            Account::new("Twice".to_string(), "sk-fresh".to_string()),
            Account::new("No key".to_string(), String::new()),
            Account::new(String::new(), "sk-nolabel".to_string()),
        ];

        let prepared = prepare_import(&existing, incoming);
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].label, "Fresh");
        assert_eq!(prepared[0].api_key, "sk-fresh");
        assert_ne!(prepared[0].id, fresh_id);
        assert!(prepared[0].last_used.is_none());
    }
}
