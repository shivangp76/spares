//! Who can create an account on a host, `signups` in `host.toml` ([`super::Signups`]):
//!
//! - `closed`: no one but the admin (`spares host add-user`). The laptop and a personal VPS are
//!   closed, which is what keeps them to one user.
//! - `invite`: anyone with a code from an admin (`spares host invite`), used once.
//! - `open`: anyone, with an email address they verify by a link sent there. An unverified
//!   account can't log in. Email is sent with `[email] command` (such as `sendmail -t`), which
//!   gets the message on its standard input; without it, open signups are refused.
//!
//! The same email links reset forgotten passwords.

use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use spares_core::Error;
use sqlx::SqlitePool;

use super::accounts::invalid;
use super::accounts::random_string;
use super::accounts::sqlx_error;

fn hash(secret: &str) -> String {
    blake3::hash(secret.as_bytes()).to_hex().to_string()
}

/// `[email]` in `host.toml`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmailConfig {
    /// A shell command that sends the message on its standard input, headers and all, such as
    /// `sendmail -t`. Without it, the host sends no email, and refuses open signups.
    pub command: Option<String>,
    /// The `From:` address
    pub from: String,
}

/// Sends an email with `[email] command`.
pub fn send_email(config: &EmailConfig, to: &str, subject: &str, body: &str) -> Result<(), Error> {
    use std::io::Write;
    let Some(command) = &config.command else {
        return Err(invalid(
            "this host sends no email ([email] command in host.toml)",
        ));
    };
    if [to, subject, &config.from]
        .iter()
        .any(|header| header.contains(['\r', '\n']))
    {
        return Err(invalid("a header can't have a line break"));
    }
    let message = format!(
        "From: {}\r\nTo: {to}\r\nSubject: {subject}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n",
        config.from
    );
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| Error::Io {
            description: format!("Failed to run `{command}`"),
            source: e,
        })?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(message.as_bytes()).map_err(|e| Error::Io {
            description: format!("Failed to send email with `{command}`"),
            source: e,
        })?;
    }
    let status = child.wait().map_err(|e| Error::Io {
        description: format!("Failed to send email with `{command}`"),
        source: e,
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(invalid(format!(
            "`{command}` failed ({status}) to send email"
        )))
    }
}

/// Makes an invite, valid for `days`, and returns its code.
pub async fn create_invite(db: &SqlitePool, days: i64) -> Result<String, Error> {
    let code = random_string(20);
    let now = Utc::now().timestamp();
    sqlx::query("INSERT INTO invite (code_hash, created_at, expires_at) VALUES (?, ?, ?)")
        .bind(hash(&code))
        .bind(now)
        .bind(now + days.max(1) * 86_400)
        .execute(db)
        .await
        .map_err(sqlx_error)?;
    Ok(code)
}

/// Uses an invite for `user_id`. Returns whether it was valid and unused.
pub async fn use_invite(db: &SqlitePool, code: &str, user_id: &str) -> Result<bool, Error> {
    let now = Utc::now().timestamp();
    Ok(sqlx::query(
        "UPDATE invite SET used_by = ?, used_at = ?
         WHERE code_hash = ? AND used_at IS NULL AND expires_at > ?",
    )
    .bind(user_id)
    .bind(now)
    .bind(hash(code.trim()))
    .bind(now)
    .execute(db)
    .await
    .map_err(sqlx_error)?
    .rows_affected()
        > 0)
}

/// Whether an invite is valid and unused, without using it.
pub async fn invite_is_valid(db: &SqlitePool, code: &str) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM invite
         WHERE code_hash = ? AND used_at IS NULL AND expires_at > ?)",
    )
    .bind(hash(code.trim()))
    .bind(Utc::now().timestamp())
    .fetch_one(db)
    .await
    .map_err(sqlx_error)
}

/// What an email token is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq, strum::Display)]
#[strum(serialize_all = "lowercase")]
pub enum Purpose {
    Verify,
    Reset,
}

/// A token for a link sent to `user_id`'s address, valid for a day.
pub async fn create_email_token(
    db: &SqlitePool,
    user_id: &str,
    purpose: Purpose,
) -> Result<String, Error> {
    let token = random_string(40);
    sqlx::query(
        "INSERT INTO email_token (token_hash, user_id, purpose, expires_at) VALUES (?, ?, ?, ?)",
    )
    .bind(hash(&token))
    .bind(user_id)
    .bind(purpose.to_string())
    .bind(Utc::now().timestamp() + 86_400)
    .execute(db)
    .await
    .map_err(sqlx_error)?;
    Ok(token)
}

/// The user a token was made for, using it up.
pub async fn use_email_token(
    db: &SqlitePool,
    token: &str,
    purpose: Purpose,
) -> Result<Option<String>, Error> {
    let user: Option<String> = sqlx::query_scalar(
        "DELETE FROM email_token WHERE token_hash = ? AND purpose = ? AND expires_at > ?
         RETURNING user_id",
    )
    .bind(hash(token.trim()))
    .bind(purpose.to_string())
    .bind(Utc::now().timestamp())
    .fetch_optional(db)
    .await
    .map_err(sqlx_error)?;
    Ok(user)
}

pub async fn mark_verified(db: &SqlitePool, user_id: &str) -> Result<(), Error> {
    sqlx::query("UPDATE user SET email_verified_at = ? WHERE id = ? AND email_verified_at IS NULL")
        .bind(Utc::now().timestamp())
        .bind(user_id)
        .execute(db)
        .await
        .map_err(sqlx_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::tests::test_host;

    #[tokio::test]
    async fn invites_and_email_tokens_work_once() {
        let (host, _root) = test_host().await;
        let db = &host.accounts;
        let code = create_invite(db, 7).await.unwrap();
        assert!(invite_is_valid(db, &code).await.unwrap());
        assert!(use_invite(db, &code, "me").await.unwrap());
        assert!(!use_invite(db, &code, "me").await.unwrap());
        assert!(!invite_is_valid(db, "nope").await.unwrap());

        let token = create_email_token(db, "me", Purpose::Verify).await.unwrap();
        assert_eq!(
            use_email_token(db, &token, Purpose::Reset).await.unwrap(),
            None
        );
        assert_eq!(
            use_email_token(db, &token, Purpose::Verify)
                .await
                .unwrap()
                .as_deref(),
            Some("me")
        );
        assert_eq!(
            use_email_token(db, &token, Purpose::Verify).await.unwrap(),
            None
        );
    }

    #[test]
    fn email_goes_through_the_command() {
        let out = std::env::temp_dir().join(format!("spares-email-{}", random_string(6)));
        let config = EmailConfig {
            command: Some(format!("cat > {}", out.display())),
            from: "spares@example.com".to_string(),
        };
        send_email(&config, "me@example.com", "Hi", "Body").unwrap();
        let sent = std::fs::read_to_string(&out).unwrap();
        assert!(sent.contains("To: me@example.com\r\n"), "{sent}");
        assert!(sent.ends_with("Body\r\n"));
        assert!(send_email(&config, "me@example.com\r\nBcc: x", "Hi", "Body").is_err());
        assert!(send_email(&EmailConfig::default(), "a@b", "s", "b").is_err());
        std::fs::remove_file(out).unwrap();
    }
}
