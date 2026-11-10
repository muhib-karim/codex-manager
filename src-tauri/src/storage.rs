use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{Context, Result};
use argon2::Argon2;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::{DateTime, Utc};
use rand::rngs::OsRng;
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use crate::models::{Account, AccountId, UsageSnapshot};

/// Metadata key holding the base64 Argon2 salt for this database.
const SALT_KEY: &str = "kdf_salt";
/// Metadata key holding an encrypted marker used to detect a wrong master key.
const KEY_CHECK_KEY: &str = "key_check";
const KEY_CHECK_PLAINTEXT: &str = "codex-manager-key-check-v1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

/// SQLite storage for accounts and usage data. API keys are encrypted with
/// AES-256-GCM; the key is derived from the master key with Argon2id and a
/// random salt that is created once and stored next to the data.
pub struct EncryptedStore {
    conn: Mutex<Connection>,
    cipher: Aes256Gcm,
}

fn conversion_error(message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, message.into())
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS accounts (
            id TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            api_key_encrypted TEXT NOT NULL,
            org_id TEXT,
            model_scope TEXT,
            daily_limit REAL,
            monthly_limit REAL,
            priority INTEGER DEFAULT 0,
            enabled INTEGER DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            last_used TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_accounts_enabled ON accounts(enabled);
        CREATE INDEX IF NOT EXISTS idx_accounts_priority ON accounts(priority);

        CREATE TABLE IF NOT EXISTS usage_snapshots (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            account_id TEXT NOT NULL,
            tokens_used INTEGER DEFAULT 0,
            cost_estimate REAL DEFAULT 0.0,
            hard_limit REAL,
            soft_limit REAL,
            remaining_budget REAL,
            daily_usage REAL DEFAULT 0.0,
            monthly_usage REAL DEFAULT 0.0,
            timestamp TEXT NOT NULL,
            FOREIGN KEY (account_id) REFERENCES accounts(id)
        );

        CREATE INDEX IF NOT EXISTS idx_usage_account ON usage_snapshots(account_id);
        CREATE INDEX IF NOT EXISTS idx_usage_timestamp ON usage_snapshots(timestamp);

        CREATE TABLE IF NOT EXISTS metadata (
            key TEXT PRIMARY KEY,
            value TEXT
        );
        "#,
    )
    .context("Failed to initialize database schema")?;
    Ok(())
}

fn read_metadata(conn: &Connection, key: &str) -> Result<Option<String>> {
    let value = conn
        .query_row(
            "SELECT value FROM metadata WHERE key = ?1",
            params![key],
            |row| {
                let value: Option<String> = row.get(0)?;
                Ok(value)
            },
        )
        .optional()
        .context("Failed to read metadata")?;
    Ok(value.flatten())
}

fn write_metadata(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .context("Failed to write metadata")?;
    Ok(())
}

/// Read the salt stored in this database, or create and store one on first
/// use. The salt must stay the same for the lifetime of the database:
/// deriving with a fresh salt would produce a different key and make every
/// stored API key undecryptable.
fn load_or_create_salt(conn: &Connection) -> Result<Vec<u8>> {
    if let Some(encoded) = read_metadata(conn, SALT_KEY)? {
        let salt = BASE64
            .decode(encoded.as_bytes())
            .context("Stored key-derivation salt is not valid base64")?;
        if salt.len() < 8 {
            anyhow::bail!("Stored key-derivation salt is too short");
        }
        return Ok(salt);
    }

    let mut salt = vec![0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    write_metadata(conn, SALT_KEY, &BASE64.encode(&salt))?;
    Ok(salt)
}

/// Derive the AES-256-GCM cipher from the master key with Argon2id.
fn derive_cipher(master_key: &str, salt: &[u8]) -> Result<Aes256Gcm> {
    let mut key_bytes = [0u8; 32];
    Argon2::default()
        .hash_password_into(master_key.as_bytes(), salt, &mut key_bytes)
        .map_err(|e| anyhow::anyhow!("Key derivation failed: {e}"))?;
    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    Ok(Aes256Gcm::new(key))
}

impl EncryptedStore {
    /// Initialize or open the database file
    pub fn open(db_path: &Path, master_key: &str) -> Result<Self> {
        let conn = Connection::open(db_path).context("Failed to open database")?;
        Self::from_connection(conn, master_key)
    }

    /// Create an in-memory database (for testing)
    pub fn open_in_memory(master_key: &str) -> Result<Self> {
        let conn = Connection::open_in_memory().context("Failed to open in-memory database")?;
        Self::from_connection(conn, master_key)
    }

    fn from_connection(conn: Connection, master_key: &str) -> Result<Self> {
        init_schema(&conn)?;
        let salt = load_or_create_salt(&conn)?;
        let cipher = derive_cipher(master_key, &salt)?;
        let store = Self {
            conn: Mutex::new(conn),
            cipher,
        };
        store.verify_or_init_key_check()?;
        Ok(store)
    }

    /// Lock the connection. A poisoned lock is recovered: the connection
    /// itself stays usable after a panic in another thread.
    fn lock(&self) -> MutexGuard<'_, Connection> {
        match self.conn.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// On first use store an encrypted marker; on later opens require that
    /// the derived key can decrypt it, so a wrong master key is reported
    /// when the database is opened instead of when an account is read.
    fn verify_or_init_key_check(&self) -> Result<()> {
        let conn = self.lock();
        let existing = read_metadata(&conn, KEY_CHECK_KEY)?;
        drop(conn);

        match existing {
            Some(token) => {
                let matches = self
                    .decrypt(&token)
                    .is_ok_and(|plaintext| plaintext == KEY_CHECK_PLAINTEXT);
                if !matches {
                    anyhow::bail!("The master key does not match this database");
                }
            }
            None => {
                let token = self.encrypt(KEY_CHECK_PLAINTEXT)?;
                let conn = self.lock();
                write_metadata(&conn, KEY_CHECK_KEY, &token)?;
            }
        }
        Ok(())
    }

    /// Encrypt with AES-256-GCM. Output is base64(nonce || ciphertext) with
    /// a fresh random 96-bit nonce per call.
    fn encrypt(&self, plaintext: &str) -> Result<String> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|_| anyhow::anyhow!("Encryption failed"))?;

        let mut combined = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        combined.extend_from_slice(&nonce_bytes);
        combined.extend_from_slice(&ciphertext);
        Ok(BASE64.encode(&combined))
    }

    /// Decrypt a value produced by `encrypt`
    fn decrypt(&self, ciphertext_b64: &str) -> Result<String> {
        let combined = BASE64
            .decode(ciphertext_b64.as_bytes())
            .context("Invalid base64 encoding")?;

        if combined.len() <= NONCE_LEN {
            anyhow::bail!("Ciphertext too short");
        }

        let nonce = Nonce::from_slice(&combined[..NONCE_LEN]);
        let plaintext = self
            .cipher
            .decrypt(nonce, &combined[NONCE_LEN..])
            .map_err(|_| anyhow::anyhow!("Decryption failed"))?;

        String::from_utf8(plaintext).context("Invalid UTF-8 in decrypted data")
    }

    /// Build an account from a database row. Every conversion failure is
    /// returned as an error; nothing in here may panic, because the caller
    /// holds the connection lock.
    fn row_to_account(&self, row: &Row<'_>) -> rusqlite::Result<Account> {
        let id_text: String = row.get("id")?;
        let label: String = row.get("label")?;
        let encrypted_key: String = row.get("api_key_encrypted")?;
        let org_id: Option<String> = row.get("org_id")?;
        let model_scope_json: Option<String> = row.get("model_scope")?;
        let daily_limit: Option<f64> = row.get("daily_limit")?;
        let monthly_limit: Option<f64> = row.get("monthly_limit")?;
        let priority: Option<i32> = row.get("priority")?;
        let enabled: Option<i64> = row.get("enabled")?;
        let created_text: String = row.get("created_at")?;
        let updated_text: String = row.get("updated_at")?;
        let last_used_text: Option<String> = row.get("last_used")?;

        let id: AccountId = id_text
            .parse::<AccountId>()
            .map_err(|e| conversion_error(format!("invalid account id: {e}")))?;
        let api_key = self
            .decrypt(&encrypted_key)
            .map_err(|e| conversion_error(format!("cannot decrypt API key: {e}")))?;
        let model_scope: Vec<String> = match model_scope_json {
            Some(json) => serde_json::from_str(&json)
                .map_err(|e| conversion_error(format!("invalid model scope: {e}")))?,
            None => Vec::new(),
        };
        let created_at: DateTime<Utc> = created_text
            .parse::<DateTime<Utc>>()
            .map_err(|e| conversion_error(format!("invalid created_at: {e}")))?;
        let updated_at: DateTime<Utc> = updated_text
            .parse::<DateTime<Utc>>()
            .map_err(|e| conversion_error(format!("invalid updated_at: {e}")))?;
        let last_used: Option<DateTime<Utc>> = match last_used_text {
            Some(text) => Some(
                text.parse::<DateTime<Utc>>()
                    .map_err(|e| conversion_error(format!("invalid last_used: {e}")))?,
            ),
            None => None,
        };

        Ok(Account {
            id,
            label,
            api_key,
            org_id,
            model_scope,
            daily_limit,
            monthly_limit,
            priority: priority.unwrap_or(0),
            enabled: enabled.unwrap_or(1) != 0,
            created_at,
            updated_at,
            last_used,
        })
    }

    /// Save or update an account
    pub fn save_account(&self, account: &Account) -> Result<()> {
        let encrypted_key = self.encrypt(&account.api_key)?;
        let model_scope_json = serde_json::to_string(&account.model_scope)?;
        let last_used = account.last_used.map(|t| t.to_rfc3339());

        let conn = self.lock();
        conn.execute(
            r#"
            INSERT INTO accounts (
                id, label, api_key_encrypted, org_id, model_scope,
                daily_limit, monthly_limit, priority, enabled,
                created_at, updated_at, last_used
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(id) DO UPDATE SET
                label = excluded.label,
                api_key_encrypted = excluded.api_key_encrypted,
                org_id = excluded.org_id,
                model_scope = excluded.model_scope,
                daily_limit = excluded.daily_limit,
                monthly_limit = excluded.monthly_limit,
                priority = excluded.priority,
                enabled = excluded.enabled,
                updated_at = excluded.updated_at,
                last_used = excluded.last_used
            "#,
            params![
                account.id.to_string(),
                account.label,
                encrypted_key,
                account.org_id,
                model_scope_json,
                account.daily_limit,
                account.monthly_limit,
                account.priority,
                i64::from(account.enabled),
                account.created_at.to_rfc3339(),
                account.updated_at.to_rfc3339(),
                last_used,
            ],
        )
        .context("Failed to save account")?;

        Ok(())
    }

    /// Load all accounts
    pub fn load_accounts(&self) -> Result<Vec<Account>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT * FROM accounts ORDER BY priority DESC, created_at ASC")
            .context("Failed to prepare account query")?;
        let rows = stmt
            .query_map([], |row| self.row_to_account(row))
            .context("Failed to query accounts")?;

        let mut accounts = Vec::new();
        for row in rows {
            let account = row.context("Failed to load account")?;
            accounts.push(account);
        }
        Ok(accounts)
    }

    /// Load a single account by ID
    pub fn load_account(&self, id: AccountId) -> Result<Option<Account>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT * FROM accounts WHERE id = ?1")
            .context("Failed to prepare account query")?;
        let account = stmt
            .query_row(params![id.to_string()], |row| self.row_to_account(row))
            .optional()
            .context("Failed to load account")?;
        Ok(account)
    }

    /// Delete an account and its usage history
    pub fn delete_account(&self, id: AccountId) -> Result<bool> {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM usage_snapshots WHERE account_id = ?1",
            params![id.to_string()],
        )
        .context("Failed to delete usage snapshots")?;
        let rows = conn
            .execute("DELETE FROM accounts WHERE id = ?1", params![id.to_string()])
            .context("Failed to delete account")?;
        Ok(rows > 0)
    }

    /// Save a usage snapshot
    pub fn save_usage_snapshot(&self, snapshot: &UsageSnapshot) -> Result<()> {
        let tokens_used = i64::try_from(snapshot.tokens_used).unwrap_or(i64::MAX);
        let conn = self.lock();
        conn.execute(
            r#"
            INSERT INTO usage_snapshots (
                account_id, tokens_used, cost_estimate, hard_limit,
                soft_limit, remaining_budget, daily_usage, monthly_usage, timestamp
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                snapshot.account_id.to_string(),
                tokens_used,
                snapshot.cost_estimate,
                snapshot.hard_limit,
                snapshot.soft_limit,
                snapshot.remaining_budget,
                snapshot.daily_usage,
                snapshot.monthly_usage,
                snapshot.timestamp.to_rfc3339(),
            ],
        )
        .context("Failed to save usage snapshot")?;

        Ok(())
    }

    /// Load the latest usage snapshot for an account
    pub fn load_latest_usage(&self, account_id: AccountId) -> Result<Option<UsageSnapshot>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT * FROM usage_snapshots WHERE account_id = ?1 \
                 ORDER BY id DESC LIMIT 1",
            )
            .context("Failed to prepare usage query")?;

        let snapshot = stmt
            .query_row(params![account_id.to_string()], |row| {
                let tokens_used: Option<i64> = row.get("tokens_used")?;
                let cost_estimate: Option<f64> = row.get("cost_estimate")?;
                let hard_limit: Option<f64> = row.get("hard_limit")?;
                let soft_limit: Option<f64> = row.get("soft_limit")?;
                let remaining_budget: Option<f64> = row.get("remaining_budget")?;
                let daily_usage: Option<f64> = row.get("daily_usage")?;
                let monthly_usage: Option<f64> = row.get("monthly_usage")?;
                let timestamp_text: String = row.get("timestamp")?;
                let timestamp: DateTime<Utc> = timestamp_text
                    .parse::<DateTime<Utc>>()
                    .map_err(|e| conversion_error(format!("invalid timestamp: {e}")))?;

                Ok(UsageSnapshot {
                    account_id,
                    tokens_used: u64::try_from(tokens_used.unwrap_or(0)).unwrap_or(0),
                    cost_estimate: cost_estimate.unwrap_or(0.0),
                    hard_limit,
                    soft_limit,
                    remaining_budget,
                    daily_usage: daily_usage.unwrap_or(0.0),
                    monthly_usage: monthly_usage.unwrap_or(0.0),
                    timestamp,
                })
            })
            .optional()
            .context("Failed to load usage snapshot")?;

        Ok(snapshot)
    }

    /// Get database metadata
    pub fn get_metadata(&self, key: &str) -> Result<Option<String>> {
        let conn = self.lock();
        read_metadata(&conn, key)
    }

    /// Set database metadata
    pub fn set_metadata(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.lock();
        write_metadata(&conn, key, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const API_KEY: &str = "sk-test-key-0123456789abcdef";

    /// Unique database path in the OS temp dir; removed when dropped.
    struct TempDb {
        path: PathBuf,
    }

    impl TempDb {
        fn new() -> Self {
            let name = format!("codex-manager-test-{}.db", uuid::Uuid::new_v4());
            Self {
                path: std::env::temp_dir().join(name),
            }
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn sample_account() -> Account {
        Account::new("Primary".to_string(), API_KEY.to_string())
            .with_org_id("org-123".to_string())
            .with_model_scope(vec!["gpt-4o".to_string(), "gpt-4*".to_string()])
            .with_limits(Some(5.0), Some(100.0))
            .with_priority(7)
    }

    #[test]
    fn account_round_trips_through_the_store() {
        let store = EncryptedStore::open_in_memory("master").unwrap();
        let account = sample_account();
        store.save_account(&account).unwrap();

        let loaded = store.load_account(account.id).unwrap().unwrap();
        assert_eq!(loaded.id, account.id);
        assert_eq!(loaded.label, "Primary");
        assert_eq!(loaded.api_key, API_KEY);
        assert_eq!(loaded.org_id.as_deref(), Some("org-123"));
        assert_eq!(loaded.model_scope, account.model_scope);
        assert_eq!(loaded.daily_limit, Some(5.0));
        assert_eq!(loaded.monthly_limit, Some(100.0));
        assert_eq!(loaded.priority, 7);
        assert!(loaded.enabled);
        assert_eq!(store.load_accounts().unwrap().len(), 1);
    }

    // Regression: the salt used to be regenerated on every open, so keys
    // written in one session could not be decrypted in the next.
    #[test]
    fn reopening_the_database_with_the_same_master_key_decrypts_accounts() {
        let db = TempDb::new();
        let account = sample_account();
        {
            let store = EncryptedStore::open(&db.path, "correct horse").unwrap();
            store.save_account(&account).unwrap();
        }

        let reopened = EncryptedStore::open(&db.path, "correct horse").unwrap();
        let accounts = reopened.load_accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].api_key, API_KEY);
    }

    #[test]
    fn reopening_with_a_different_master_key_is_rejected() {
        let db = TempDb::new();
        {
            let store = EncryptedStore::open(&db.path, "correct horse").unwrap();
            store.save_account(&sample_account()).unwrap();
        }

        let result = EncryptedStore::open(&db.path, "wrong horse");
        assert!(result.is_err());
    }

    #[test]
    fn api_key_is_not_stored_in_plaintext() {
        let store = EncryptedStore::open_in_memory("master").unwrap();
        let account = sample_account();
        store.save_account(&account).unwrap();

        let conn = store.lock();
        let stored: String = conn
            .query_row(
                "SELECT api_key_encrypted FROM accounts WHERE id = ?1",
                params![account.id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        drop(conn);
        assert!(!stored.contains(API_KEY));
        assert!(!stored.contains("0123456789"));
        assert_ne!(stored, API_KEY);
    }

    #[test]
    fn encrypting_the_same_value_twice_gives_different_ciphertexts() {
        let store = EncryptedStore::open_in_memory("master").unwrap();
        let first = store.encrypt(API_KEY).unwrap();
        let second = store.encrypt(API_KEY).unwrap();
        assert_ne!(first, second);
        assert_eq!(store.decrypt(&first).unwrap(), API_KEY);
        assert_eq!(store.decrypt(&second).unwrap(), API_KEY);
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let store = EncryptedStore::open_in_memory("master").unwrap();
        let token = store.encrypt(API_KEY).unwrap();
        let mut raw = BASE64.decode(token.as_bytes()).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0x01;
        let tampered = BASE64.encode(&raw);
        assert!(store.decrypt(&tampered).is_err());
        assert!(store.decrypt("not base64 !!").is_err());
        assert!(store.decrypt("").is_err());
    }

    // Regression: bad rows used to panic (`unwrap`) while the lock was held.
    #[test]
    fn a_corrupt_row_is_an_error_not_a_panic() {
        let store = EncryptedStore::open_in_memory("master").unwrap();
        let conn = store.lock();
        conn.execute(
            "INSERT INTO accounts (id, label, api_key_encrypted, created_at, updated_at) \
             VALUES ('not-a-uuid', 'broken', 'not-ciphertext', 'yesterday', 'today')",
            [],
        )
        .unwrap();
        drop(conn);

        assert!(store.load_accounts().is_err());
        // The store is still usable afterwards.
        let account = sample_account();
        store.save_account(&account).unwrap();
        assert!(store.load_account(account.id).unwrap().is_some());
    }

    #[test]
    fn updating_an_account_keeps_a_single_row() {
        let store = EncryptedStore::open_in_memory("master").unwrap();
        let mut account = sample_account();
        store.save_account(&account).unwrap();
        account.label = "Renamed".to_string();
        account.enabled = false;
        store.save_account(&account).unwrap();

        let accounts = store.load_accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].label, "Renamed");
        assert!(!accounts[0].enabled);
    }

    #[test]
    fn latest_usage_snapshot_wins_and_delete_removes_history() {
        let store = EncryptedStore::open_in_memory("master").unwrap();
        let account = sample_account();
        store.save_account(&account).unwrap();
        assert!(store.load_latest_usage(account.id).unwrap().is_none());

        let mut first = UsageSnapshot::new(account.id);
        first.monthly_usage = 10.0;
        store.save_usage_snapshot(&first).unwrap();
        let mut second = UsageSnapshot::new(account.id);
        second.monthly_usage = 25.5;
        second.hard_limit = Some(120.0);
        store.save_usage_snapshot(&second).unwrap();

        let latest = store.load_latest_usage(account.id).unwrap().unwrap();
        assert_eq!(latest.monthly_usage, 25.5);
        assert_eq!(latest.hard_limit, Some(120.0));

        assert!(store.delete_account(account.id).unwrap());
        assert!(store.load_account(account.id).unwrap().is_none());
        assert!(store.load_latest_usage(account.id).unwrap().is_none());
        assert!(!store.delete_account(account.id).unwrap());
    }

    #[test]
    fn metadata_round_trips() {
        let store = EncryptedStore::open_in_memory("master").unwrap();
        assert!(store.get_metadata("missing").unwrap().is_none());
        store.set_metadata("schema", "1").unwrap();
        store.set_metadata("schema", "2").unwrap();
        assert_eq!(store.get_metadata("schema").unwrap().as_deref(), Some("2"));
    }
}
