//! Local-profile auth commands (brief.md FR-1 / FR-2 / FR-3).
//!
//! There is no account server: the profile is a username plus an Argon2 PHC
//! string in `settings.json`. Argon2 is deliberately CPU-heavy, so every hash
//! and verify runs on the blocking pool — never on the async runtime that also
//! services device polling.

use std::time::Instant;

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::store::{self, AuthConfig};

/// Shortest accepted secret. Low on purpose: FR-1 allows a PIN, and this is a
/// LAN tool on a machine the user already controls.
const MIN_SECRET_LEN: usize = 4;

/// A valid Argon2 hash of a value nobody can type, used to burn the same CPU on
/// an unknown-username login as on a real one (no user-enumeration timing gap).
const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$dGhpc2lzbm90YXJlYWxoYXNodmFsdWVvaw";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStateDto {
    /// A profile exists — otherwise the setup wizard runs (FR-1).
    pub configured: bool,
    /// The login screen must be shown before the app is usable (FR-2).
    pub requires_login: bool,
    /// False once FR-3's "remove password" ran: configured, but no prompt.
    pub has_password: bool,
    pub username: Option<String>,
    /// Mirrors the "remember me" checkbox so Settings can render its state.
    pub remember_me: bool,
}

// ---------------------------------------------------------------- hashing --

/// Hash a secret into a PHC string. Blocking and CPU-bound — call from
/// `spawn_blocking`.
fn hash_secret(secret: &str) -> AppResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(secret.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::Internal(format!("password hash failed: {e}")))
}

/// Verify a secret against a PHC string. A malformed stored hash verifies as
/// false rather than erroring — a corrupted config must not lock the app into
/// an unrecoverable error state.
fn verify_secret(hash: &str, secret: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| Argon2::default().verify_password(secret.as_bytes(), &parsed).is_ok())
        .unwrap_or(false)
}

async fn hash_secret_async(secret: String) -> AppResult<String> {
    tauri::async_runtime::spawn_blocking(move || hash_secret(&secret))
        .await
        .map_err(|e| AppError::Internal(format!("hash task failed: {e}")))?
}

async fn verify_secret_async(hash: String, secret: String) -> AppResult<bool> {
    tauri::async_runtime::spawn_blocking(move || verify_secret(&hash, &secret))
        .await
        .map_err(|e| AppError::Internal(format!("verify task failed: {e}")))
}

fn validate(username: &str, secret: &str) -> AppResult<()> {
    if username.trim().is_empty() {
        return Err(AppError::InvalidInput("Please choose a username.".into()));
    }
    validate_secret(secret)
}

fn validate_secret(secret: &str) -> AppResult<()> {
    if secret.chars().count() < MIN_SECRET_LEN {
        return Err(AppError::InvalidInput(format!(
            "Password or PIN must be at least {MIN_SECRET_LEN} characters."
        )));
    }
    Ok(())
}

/// Verify `secret` against the stored profile. A profile with no password set
/// (post-`remove_password`) has nothing to check, so anything passes — there is
/// no secret left to protect.
async fn verify_current(app: &AppHandle, secret: String) -> AppResult<()> {
    let auth = store::get(app)
        .auth
        .ok_or_else(|| AppError::Auth("No profile is configured.".into()))?;
    let Some(hash) = auth.password_hash else {
        return Ok(());
    };
    if verify_secret_async(hash, secret).await? {
        Ok(())
    } else {
        Err(AppError::Auth("Current password is incorrect.".into()))
    }
}

// --------------------------------------------------------------- commands --

/// What the frontend router needs on boot: wizard, login, or straight in.
#[tauri::command]
pub fn auth_state(app: AppHandle, state: State<'_, AppState>) -> AuthStateDto {
    let config = store::get(&app);
    let configured = config.auth.is_some();
    let has_password = config.auth.as_ref().is_some_and(|a| a.password_hash.is_some());

    // "Remember me" is a standing grant on this machine, so honouring it just
    // means opening the session now; every later check is a plain session read.
    // A profile with no password has nothing to grant — it is always open.
    if configured && (config.remember_me || !has_password) {
        state.set_logged_in(true);
    }

    AuthStateDto {
        configured,
        requires_login: has_password && !state.logged_in(),
        has_password,
        username: config.auth.map(|a| a.username),
        remember_me: config.remember_me,
    }
}

/// FR-1: create the one local profile. Refuses to overwrite an existing one —
/// that path is `set_password`, which requires the current secret.
#[tauri::command]
pub async fn create_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    username: String,
    password: String,
) -> AppResult<()> {
    validate(&username, &password)?;
    if store::get(&app).auth.is_some() {
        return Err(AppError::Auth("A profile already exists on this machine.".into()));
    }

    let password_hash = hash_secret_async(password).await?;
    let username = username.trim().to_string();
    store::update(&app, move |config| {
        // Re-check under the write lock: two wizards can't race a profile in.
        if config.auth.is_some() {
            return Err(AppError::Auth("A profile already exists on this machine.".into()));
        }
        config.auth = Some(AuthConfig { username, password_hash: Some(password_hash) });
        Ok(())
    })?;

    state.set_logged_in(true);
    Ok(())
}

/// FR-2. Returns `false` for a wrong username/password; errors only when the
/// throttle is engaged or nothing is configured.
#[tauri::command]
pub async fn login(
    app: AppHandle,
    state: State<'_, AppState>,
    username: String,
    password: String,
) -> AppResult<bool> {
    let now = Instant::now();
    if let Some(remaining) = state
        .throttle
        .lock()
        .expect("throttle lock poisoned")
        .remaining(now)
    {
        return Err(AppError::LockedOut { secs: remaining.as_secs() + 1 });
    }

    let auth = store::get(&app)
        .auth
        .ok_or_else(|| AppError::Auth("No profile is configured.".into()))?;
    let Some(stored) = auth.password_hash else {
        // No password set (FR-3 removal) — nothing to log in against.
        state.set_logged_in(true);
        return Ok(true);
    };

    // Hash even when the username is wrong so both failures cost the same.
    let username_ok = auth.username.eq_ignore_ascii_case(username.trim());
    let hash = if username_ok { stored } else { DUMMY_HASH.to_string() };
    let ok = verify_secret_async(hash, password).await? && username_ok;

    let mut throttle = state.throttle.lock().expect("throttle lock poisoned");
    if ok {
        throttle.reset();
        drop(throttle);
        state.set_logged_in(true);
    } else {
        throttle.record_failure(Instant::now());
    }
    Ok(ok)
}

/// Ends the session and clears the standing "remember me" grant — otherwise the
/// next `auth_state` would silently log the user back in.
#[tauri::command]
pub fn logout(app: AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    store::update(&app, |config| {
        config.remember_me = false;
        Ok(())
    })?;
    state.set_logged_in(false);
    Ok(())
}

/// FR-3: change the password/PIN from Settings.
#[tauri::command]
pub async fn set_password(app: AppHandle, current: String, next: String) -> AppResult<()> {
    validate_secret(&next)?;
    verify_current(&app, current).await?;

    let password_hash = hash_secret_async(next).await?;
    store::update(&app, move |config| match config.auth.as_mut() {
        Some(auth) => {
            auth.password_hash = Some(password_hash);
            Ok(())
        }
        None => Err(AppError::Auth("No profile is configured.".into())),
    })
}

/// FR-3: drop the password entirely. The profile (and its username) survives so
/// this is never mistaken for first launch; the app just opens straight to the
/// dashboard, so "remember me" is set to keep `auth_state` consistent.
#[tauri::command]
pub async fn remove_password(
    app: AppHandle,
    state: State<'_, AppState>,
    current: String,
) -> AppResult<()> {
    verify_current(&app, current).await?;
    store::update(&app, |config| {
        if let Some(auth) = config.auth.as_mut() {
            auth.password_hash = None;
        }
        config.remember_me = true;
        Ok(())
    })?;
    state.set_logged_in(true);
    Ok(())
}

/// FR-2: persist the "skip login on this machine" choice.
#[tauri::command]
pub fn set_remember_me(app: AppHandle, state: State<'_, AppState>, value: bool) -> AppResult<()> {
    store::update(&app, |config| {
        config.remember_me = value;
        Ok(())
    })?;
    if value {
        state.set_logged_in(true);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_verify_roundtrip() {
        let hash = hash_secret("1234").expect("hashing must succeed");
        assert!(hash.starts_with("$argon2"), "expected a PHC string, got {hash}");
        assert!(verify_secret(&hash, "1234"));
        assert!(!verify_secret(&hash, "4321"));
    }

    #[test]
    fn salt_makes_hashes_unique() {
        let a = hash_secret("hunter2").unwrap();
        let b = hash_secret("hunter2").unwrap();
        assert_ne!(a, b, "each hash must carry a fresh salt");
    }

    #[test]
    fn corrupt_hash_verifies_false_instead_of_panicking() {
        assert!(!verify_secret("not-a-phc-string", "1234"));
    }

    #[test]
    fn dummy_hash_is_parseable_so_timing_matches_a_real_verify() {
        assert!(PasswordHash::new(DUMMY_HASH).is_ok());
    }

    #[test]
    fn validation_rejects_empty_username_and_short_secret() {
        assert!(validate("  ", "1234").is_err());
        assert!(validate("andrea", "123").is_err());
        assert!(validate("andrea", "1234").is_ok());
    }
}
