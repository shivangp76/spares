//! `accounts.sqlite`: a host's users. Collections never refer to it, so a user's directory can
//! move to another host, whose own accounts then name it.

use std::str::FromStr;

use argon2::Argon2;
use argon2::PasswordHash;
use argon2::PasswordHasher;
use argon2::PasswordVerifier;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use spares_core::Error;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;

/// The file name of a host's accounts, in its root.
pub const ACCOUNTS_FILE_NAME: &str = "accounts.sqlite";

pub(crate) fn sqlx_error(e: sqlx::Error) -> Error {
    Error::Sqlx { source: e }
}

pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::Library(spares_core::LibraryError::InvalidConfig(message.into()))
}

/// Opens (creating and migrating) the accounts database at `path`.
pub async fn open(path: &std::path::Path) -> Result<SqlitePool, Error> {
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .map_err(sqlx_error)?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .map_err(sqlx_error)?;
    sqlx::migrate!("./accounts_migrations")
        .run(&pool)
        .await
        .map_err(|e| Error::Sync(format!("Failed to migrate {}: {e}", path.display())))?;
    Ok(pool)
}

/// An account.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: String,
    pub name: String,
    pub email: Option<String>,
    pub admin: bool,
    pub created_at: i64,
    pub email_verified_at: Option<i64>,
    pub disabled_at: Option<i64>,
}

const USER_COLUMNS: &str = "id, name, email, admin, created_at, email_verified_at, disabled_at";

/// Whether `id` can name a user's directory: 1 to 64 lowercase letters, digits, `-` and `_`,
/// starting with a letter or digit.
pub fn valid_user_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        && id.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
}

/// A new user id: 16 random lowercase letters and digits.
pub fn new_user_id() -> String {
    random_string(16)
}

/// `length` random lowercase letters and digits.
pub(crate) fn random_string(length: usize) -> String {
    use rand::RngExt;
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    (0..length)
        .map(|_| char::from(ALPHABET[rng.random_range(0..ALPHABET.len())]))
        .collect()
}

fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The shortest password an account may have.
pub const MIN_PASSWORD_LENGTH: usize = 8;

pub fn hash_password(password: &str) -> Result<String, Error> {
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        return Err(invalid(format!(
            "a password needs at least {MIN_PASSWORD_LENGTH} characters"
        )));
    }
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|e| invalid(format!("Failed to hash the password: {e}")))
}

fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|hash| {
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    })
}

/// What [`create_user`] needs.
#[derive(Clone, Debug)]
pub struct NewUser<'a> {
    /// `None` for a new random id (see [`new_user_id`])
    pub id: Option<&'a str>,
    pub name: &'a str,
    pub email: Option<&'a str>,
    pub password: &'a str,
    pub admin: bool,
    /// Whether the email address is known to be the user's (an admin adding them, say)
    pub email_verified: bool,
}

/// Adds an account. Its directory is created by [`super::Host::create_user`].
pub async fn create_user(db: &SqlitePool, new: NewUser<'_>) -> Result<User, Error> {
    let id = new.id.map_or_else(new_user_id, str::to_string);
    if !valid_user_id(&id) {
        return Err(invalid(format!(
            "`{id}` can't be a user id: use up to 64 lowercase letters, digits, `-` and `_`"
        )));
    }
    let name = new.name.trim();
    if !valid_name(name) {
        return Err(invalid(format!(
            "`{name}` can't be a user name: use up to 64 letters, digits, `-`, `_` and `.`"
        )));
    }
    let email = new.email.map(str::trim).filter(|e| !e.is_empty());
    if email.is_some_and(|e| !e.contains('@')) {
        return Err(invalid("that isn't an email address"));
    }
    let hash = hash_password(new.password)?;
    let now = Utc::now().timestamp();
    let result = sqlx::query(
        "INSERT INTO user (id, name, email, password_hash, admin, created_at, email_verified_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(name)
    .bind(email)
    .bind(&hash)
    .bind(new.admin)
    .bind(now)
    .bind(new.email_verified.then_some(now))
    .execute(db)
    .await;
    match result {
        Ok(_) => {}
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(invalid(format!(
                "there is already an account with that id, name or email ({})",
                e.message()
            )));
        }
        Err(e) => return Err(sqlx_error(e)),
    }
    user(db, &id)
        .await?
        .ok_or_else(|| invalid("the new account is gone"))
}

pub async fn user(db: &SqlitePool, id: &str) -> Result<Option<User>, Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {USER_COLUMNS} FROM user WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(db)
    .await
    .map_err(sqlx_error)
}

/// The account with this id, name or email.
pub async fn find_user(db: &SqlitePool, who: &str) -> Result<Option<User>, Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {USER_COLUMNS} FROM user WHERE id = ?1 OR name = ?1 OR email = ?1
         ORDER BY id = ?1 DESC, name = ?1 DESC LIMIT 1"
    )))
    .bind(who.trim())
    .fetch_optional(db)
    .await
    .map_err(sqlx_error)
}

pub async fn users(db: &SqlitePool) -> Result<Vec<User>, Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {USER_COLUMNS} FROM user ORDER BY created_at, id"
    )))
    .fetch_all(db)
    .await
    .map_err(sqlx_error)
}

/// The account `who` names (as [`find_user`]) if `password` is its password and it isn't disabled.
/// A wrong name and a wrong password look the same.
pub async fn check_password(
    db: &SqlitePool,
    who: &str,
    password: &str,
) -> Result<Option<User>, Error> {
    let Some(user) = find_user(db, who).await? else {
        // As long as checking a real one, so timing doesn't tell which names exist
        let _ = verify_password(password, DUMMY_HASH);
        return Ok(None);
    };
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM user WHERE id = ?")
        .bind(&user.id)
        .fetch_one(db)
        .await
        .map_err(sqlx_error)?;
    let ok = verify_password(password, &hash) && user.disabled_at.is_none();
    Ok(ok.then_some(user))
}

/// An Argon2id hash of nothing in particular, for [`check_password`].
const DUMMY_HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$c3BhcmVzc3BhcmVz$MvTd8bm8w3W6kQ2a1i0K9jQ1C1n3+3l0x6o2mI8dZ3c";

pub async fn set_password(db: &SqlitePool, id: &str, password: &str) -> Result<(), Error> {
    let hash = hash_password(password)?;
    let changed = sqlx::query("UPDATE user SET password_hash = ? WHERE id = ?")
        .bind(hash)
        .bind(id)
        .execute(db)
        .await
        .map_err(sqlx_error)?
        .rows_affected();
    if changed == 0 {
        return Err(invalid(format!("no account has id {id}")));
    }
    Ok(())
}

/// Removes an account. Its directory is removed by [`super::Host::delete_user`].
pub async fn delete_user(db: &SqlitePool, id: &str) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM user WHERE id = ?")
        .bind(id)
        .execute(db)
        .await
        .map_err(sqlx_error)?
        .rows_affected()
        > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn accounts() -> SqlitePool {
        let dir = std::env::temp_dir().join(format!("spares-accounts-{}", random_string(8)));
        std::fs::create_dir_all(&dir).unwrap();
        open(&dir.join(ACCOUNTS_FILE_NAME)).await.unwrap()
    }

    #[tokio::test]
    async fn accounts_check_passwords() {
        let db = accounts().await;
        let user = create_user(
            &db,
            NewUser {
                id: Some("me"),
                name: "Shivang",
                email: Some("me@example.com"),
                password: "correct horse",
                admin: true,
                email_verified: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(user.id, "me");
        assert!(user.admin);
        for who in ["me", "shivang", "Shivang", "ME@example.com"] {
            assert_eq!(
                check_password(&db, who, "correct horse").await.unwrap(),
                Some(user.clone()),
                "{who}"
            );
        }
        assert_eq!(
            check_password(&db, "me", "wrong horse").await.unwrap(),
            None
        );
        assert_eq!(
            check_password(&db, "you", "correct horse").await.unwrap(),
            None
        );
        let duplicate = NewUser {
            id: None,
            name: "shivang",
            email: None,
            password: "another one",
            admin: false,
            email_verified: false,
        };
        assert!(create_user(&db, duplicate.clone()).await.is_err());
        assert!(
            create_user(
                &db,
                NewUser {
                    password: "short",
                    name: "x",
                    ..duplicate.clone()
                }
            )
            .await
            .is_err()
        );
        assert!(
            create_user(
                &db,
                NewUser {
                    id: Some("../x"),
                    name: "x",
                    ..duplicate.clone()
                }
            )
            .await
            .is_err()
        );
        let other = create_user(
            &db,
            NewUser {
                name: "other",
                ..duplicate
            },
        )
        .await
        .unwrap();
        assert!(valid_user_id(&other.id));
        set_password(&db, "me", "battery staple").await.unwrap();
        assert!(
            check_password(&db, "me", "battery staple")
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(users(&db).await.unwrap().len(), 2);
        assert!(delete_user(&db, &other.id).await.unwrap());
        assert_eq!(users(&db).await.unwrap().len(), 1);
    }

    #[test]
    fn user_ids_name_directories() {
        assert!(valid_user_id("shivang"));
        assert!(valid_user_id("a1-b_2"));
        for bad in ["", ".", "..", "-x", "A", "a/b", "a b", &"a".repeat(65)] {
            assert!(!valid_user_id(bad), "{bad}");
        }
    }
}
