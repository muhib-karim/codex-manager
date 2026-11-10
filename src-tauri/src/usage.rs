use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{NaiveDate, Utc};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, warn};

use crate::models::{Account, AccountId, UsageSnapshot, ValidationResult};

const DEFAULT_BASE_URL: &str = "https://api.openai.com";
const SECONDS_PER_DAY: i64 = 86_400;

/// Date range for the billing usage query: from the first of the month up
/// to and including `today`. The upstream `end_date` is exclusive, so it
/// has to be the day after `today`; using `today` would leave out today's
/// spend and send an empty range on the first of the month.
pub fn billing_window(today: NaiveDate) -> (String, String) {
    let start = today.format("%Y-%m-01").to_string();
    let end = today
        .succ_opt()
        .unwrap_or(today)
        .format("%Y-%m-%d")
        .to_string();
    (start, end)
}

/// Days since the Unix epoch for a Unix timestamp in seconds (UTC).
pub fn day_index(unix_seconds: i64) -> i64 {
    unix_seconds.div_euclid(SECONDS_PER_DAY)
}

/// Month-to-date and today's spend in dollars from a billing usage
/// response. The API reports amounts in cents.
pub fn summarize_billing(response: &BillingUsageResponse, today_index: i64) -> (f64, f64) {
    let monthly = response.total_usage / 100.0;
    let mut today_cents = 0.0;
    for day in response.daily_costs.iter() {
        if day_index(day.timestamp as i64) != today_index {
            continue;
        }
        for item in day.line_items.iter() {
            today_cents += item.cost;
        }
    }
    (monthly, today_cents / 100.0)
}

/// Combine what the billing endpoints returned into a snapshot. Returns
/// `None` when neither source produced data, so a failed refresh is never
/// recorded as "zero usage".
pub fn build_snapshot(
    account_id: AccountId,
    spend: Option<(f64, f64)>,
    limits: Option<(Option<f64>, Option<f64>)>,
) -> Option<UsageSnapshot> {
    if spend.is_none() && limits.is_none() {
        return None;
    }

    let mut snapshot = UsageSnapshot::new(account_id);
    if let Some((monthly, daily)) = spend {
        snapshot.monthly_usage = monthly;
        snapshot.daily_usage = daily;
        snapshot.cost_estimate = monthly;
    }
    if let Some((hard_limit, soft_limit)) = limits {
        snapshot.hard_limit = hard_limit;
        snapshot.soft_limit = soft_limit;
        // Remaining budget is only meaningful when the spend is known too.
        if spend.is_some() {
            if let Some(hard) = hard_limit {
                snapshot.remaining_budget = Some(hard - snapshot.monthly_usage);
            }
        }
    }
    Some(snapshot)
}

/// OpenAI API client for fetching usage and billing information
pub struct OpenAIClient {
    http: Client,
    base_url: String,
}

impl OpenAIClient {
    pub fn new() -> Self {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            http,
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url.trim_end_matches('/').to_string();
        self
    }

    /// Build an authenticated GET request for an account
    fn get(&self, account: &Account, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let mut request = self.http.get(url).bearer_auth(&account.api_key);
        if let Some(org_id) = account.org_id.as_deref() {
            if !org_id.is_empty() {
                request = request.header("OpenAI-Organization", org_id);
            }
        }
        request
    }

    /// Fetch the current usage snapshot for an account. Fails when no
    /// billing data could be read at all.
    pub async fn fetch_usage(&self, account: &Account) -> Result<UsageSnapshot> {
        let now = Utc::now();
        let today_index = day_index(now.timestamp());

        let spend = match self.fetch_billing_usage(account, now.date_naive()).await {
            Ok(response) => Some(summarize_billing(&response, today_index)),
            Err(e) => {
                warn!("Failed to fetch billing usage for {}: {}", account.label, e);
                None
            }
        };

        let limits = match self.fetch_subscription(account).await {
            Ok(subscription) => Some((subscription.hard_limit_usd, subscription.soft_limit_usd)),
            Err(e) => {
                warn!("Failed to fetch subscription for {}: {}", account.label, e);
                None
            }
        };

        match build_snapshot(account.id, spend, limits) {
            Some(snapshot) => {
                debug!(
                    "Fetched usage for {}: ${:.2} this month",
                    account.label, snapshot.monthly_usage
                );
                Ok(snapshot)
            }
            None => anyhow::bail!(
                "Usage data is unavailable for {}: the billing endpoints returned no data",
                account.label
            ),
        }
    }

    /// Fetch billing usage for the current month
    async fn fetch_billing_usage(
        &self,
        account: &Account,
        today: NaiveDate,
    ) -> Result<BillingUsageResponse> {
        let (start_date, end_date) = billing_window(today);

        let response = self
            .get(account, "/v1/dashboard/billing/usage")
            .query(&[("start_date", start_date), ("end_date", end_date)])
            .send()
            .await
            .context("Failed to send billing usage request")?;

        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("Billing usage API returned status {}", status.as_u16());
        }

        let usage: BillingUsageResponse = response
            .json()
            .await
            .context("Failed to parse billing usage response")?;
        Ok(usage)
    }

    /// Fetch subscription limits
    async fn fetch_subscription(&self, account: &Account) -> Result<SubscriptionResponse> {
        let response = self
            .get(account, "/v1/dashboard/billing/subscription")
            .send()
            .await
            .context("Failed to send subscription request")?;

        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("Subscription API returned status {}", status.as_u16());
        }

        let subscription: SubscriptionResponse = response
            .json()
            .await
            .context("Failed to parse subscription response")?;
        Ok(subscription)
    }

    /// Validate that an API key is working
    pub async fn validate_key(
        &self,
        api_key: &str,
        org_id: Option<&str>,
    ) -> Result<ValidationResult> {
        let url = format!("{}/v1/models", self.base_url);
        let mut request = self.http.get(url).bearer_auth(api_key);
        if let Some(org) = org_id {
            if !org.is_empty() {
                request = request.header("OpenAI-Organization", org);
            }
        }

        let response = request.send().await.context("Failed to validate API key")?;
        let status = response.status();
        if !status.is_success() {
            return Ok(ValidationResult {
                valid: false,
                org_id: None,
                error: Some(format!("API returned status {}", status.as_u16())),
            });
        }

        let org_id = response
            .headers()
            .get("openai-organization")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.to_string());

        Ok(ValidationResult {
            valid: true,
            org_id,
            error: None,
        })
    }
}

impl Default for OpenAIClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Response of the billing usage endpoint. Amounts are in cents.
#[derive(Debug, Deserialize)]
pub struct BillingUsageResponse {
    #[serde(default)]
    pub total_usage: f64,
    #[serde(default)]
    pub daily_costs: Vec<DailyCost>,
}

#[derive(Debug, Deserialize)]
pub struct DailyCost {
    #[serde(default)]
    pub timestamp: f64,
    #[serde(default)]
    pub line_items: Vec<LineItem>,
}

#[derive(Debug, Deserialize)]
pub struct LineItem {
    #[serde(default)]
    pub cost: f64,
}

#[derive(Debug, Deserialize)]
struct SubscriptionResponse {
    #[serde(default)]
    soft_limit_usd: Option<f64>,
    #[serde(default)]
    hard_limit_usd: Option<f64>,
}

/// Computes how long to wait between usage refreshes
pub struct UsagePoller {
    client: OpenAIClient,
    min_interval: Duration,
    max_interval: Duration,
}

impl UsagePoller {
    pub fn new() -> Self {
        Self {
            client: OpenAIClient::new(),
            min_interval: Duration::from_secs(60),
            max_interval: Duration::from_secs(3600),
        }
    }

    pub async fn poll_account(&self, account: &Account) -> Result<UsageSnapshot> {
        self.client.fetch_usage(account).await
    }

    /// Back-off: the base interval plus 2^errors seconds, capped
    pub fn next_interval(&self, consecutive_errors: u32) -> Duration {
        let backoff = Duration::from_secs(2_u64.pow(consecutive_errors.min(10)));
        std::cmp::min(self.min_interval + backoff, self.max_interval)
    }
}

impl Default for UsagePoller {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::{Json, Router};
    use std::net::SocketAddr;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    // Regression: `end_date` used to be today, which the API treats as
    // exclusive, so today's spend was never counted.
    #[test]
    fn billing_window_ends_the_day_after_today() {
        let (start, end) = billing_window(date(2026, 11, 10));
        assert_eq!(start, "2026-11-01");
        assert_eq!(end, "2026-11-11");
    }

    #[test]
    fn billing_window_on_the_first_of_the_month_is_not_empty() {
        let (start, end) = billing_window(date(2026, 11, 1));
        assert_eq!(start, "2026-11-01");
        assert_eq!(end, "2026-11-02");
    }

    #[test]
    fn billing_window_rolls_over_month_and_year_ends() {
        let (start, end) = billing_window(date(2026, 12, 31));
        assert_eq!(start, "2026-12-01");
        assert_eq!(end, "2027-01-01");
        let (_, leap_end) = billing_window(date(2028, 2, 29));
        assert_eq!(leap_end, "2028-03-01");
    }

    #[test]
    fn day_index_groups_timestamps_by_utc_day() {
        assert_eq!(day_index(0), 0);
        assert_eq!(day_index(86_399), 0);
        assert_eq!(day_index(86_400), 1);
        assert_eq!(day_index(-1), -1);
    }

    fn billing_fixture() -> BillingUsageResponse {
        // Day 100 and day 101 after the epoch, amounts in cents.
        let json = serde_json::json!({
            "object": "list",
            "total_usage": 1234.5,
            "daily_costs": [
                { "timestamp": 8_640_000.0, "line_items": [
                    { "name": "GPT-4", "cost": 900.0 }
                ]},
                { "timestamp": 8_726_400.0, "line_items": [
                    { "name": "GPT-4", "cost": 300.0 },
                    { "name": "Embeddings", "cost": 34.5 }
                ]}
            ]
        });
        serde_json::from_value(json).unwrap()
    }

    // Regression: `daily_usage` was never filled in, so daily limits could
    // not trigger.
    #[test]
    fn billing_summary_converts_cents_and_sums_todays_items() {
        let (monthly, daily) = summarize_billing(&billing_fixture(), 101);
        assert!((monthly - 12.345).abs() < 1e-9);
        assert!((daily - 3.345).abs() < 1e-9);

        let (_, yesterday) = summarize_billing(&billing_fixture(), 100);
        assert!((yesterday - 9.0).abs() < 1e-9);

        let (_, no_data) = summarize_billing(&billing_fixture(), 102);
        assert_eq!(no_data, 0.0);
    }

    #[test]
    fn billing_response_tolerates_missing_fields() {
        let parsed: BillingUsageResponse = serde_json::from_str("{}").unwrap();
        let (monthly, daily) = summarize_billing(&parsed, 0);
        assert_eq!(monthly, 0.0);
        assert_eq!(daily, 0.0);
    }

    #[test]
    fn snapshot_combines_spend_and_limits() {
        let id = uuid::Uuid::new_v4();
        let snapshot =
            build_snapshot(id, Some((12.5, 3.0)), Some((Some(120.0), Some(100.0)))).unwrap();
        assert_eq!(snapshot.account_id, id);
        assert_eq!(snapshot.monthly_usage, 12.5);
        assert_eq!(snapshot.daily_usage, 3.0);
        assert_eq!(snapshot.hard_limit, Some(120.0));
        assert_eq!(snapshot.soft_limit, Some(100.0));
        assert_eq!(snapshot.remaining_budget, Some(107.5));
    }

    #[test]
    fn snapshot_without_spend_has_no_remaining_budget() {
        let id = uuid::Uuid::new_v4();
        let snapshot = build_snapshot(id, None, Some((Some(120.0), None))).unwrap();
        assert_eq!(snapshot.hard_limit, Some(120.0));
        assert_eq!(snapshot.remaining_budget, None);
    }

    // Regression: when every request failed, an all-zero snapshot was
    // still saved as the latest reading.
    #[test]
    fn snapshot_is_none_when_nothing_was_fetched() {
        assert!(build_snapshot(uuid::Uuid::new_v4(), None, None).is_none());
    }

    #[test]
    fn poll_interval_backs_off_and_is_capped() {
        let poller = UsagePoller::new();
        assert_eq!(poller.next_interval(0), Duration::from_secs(61));
        assert_eq!(poller.next_interval(3), Duration::from_secs(68));
        assert_eq!(poller.next_interval(40), Duration::from_secs(1084));
        assert!(poller.next_interval(u32::MAX) <= Duration::from_secs(3600));
    }

    async fn spawn_mock(app: Router) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        addr
    }

    async fn mock_usage() -> Json<serde_json::Value> {
        Json(serde_json::json!({ "total_usage": 2500.0, "daily_costs": [] }))
    }

    async fn mock_subscription() -> Json<serde_json::Value> {
        Json(serde_json::json!({ "hard_limit_usd": 100.0, "soft_limit_usd": 80.0 }))
    }

    #[tokio::test]
    async fn fetch_usage_reads_both_billing_endpoints() {
        let app = Router::new()
            .route("/v1/dashboard/billing/usage", get(mock_usage))
            .route("/v1/dashboard/billing/subscription", get(mock_subscription));
        let addr = spawn_mock(app).await;

        let client = OpenAIClient::new().with_base_url(format!("http://{addr}/"));
        let account = Account::new("mock".to_string(), "sk-mock-0123456789".to_string());
        let snapshot = client.fetch_usage(&account).await.unwrap();

        assert_eq!(snapshot.monthly_usage, 25.0);
        assert_eq!(snapshot.hard_limit, Some(100.0));
        assert_eq!(snapshot.soft_limit, Some(80.0));
        assert_eq!(snapshot.remaining_budget, Some(75.0));
    }

    #[tokio::test]
    async fn fetch_usage_fails_when_no_endpoint_answers() {
        // A server with no routes answers 404 to everything.
        let addr = spawn_mock(Router::new()).await;
        let client = OpenAIClient::new().with_base_url(format!("http://{addr}"));
        let account = Account::new("mock".to_string(), "sk-mock-0123456789".to_string());
        assert!(client.fetch_usage(&account).await.is_err());
    }
}
