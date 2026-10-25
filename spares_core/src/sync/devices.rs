//! Browsers that keep a copy of the collection: replicas `2` to `4`, enrolled with the server.
//!
//! A browser has no environment to read `SPARES_REPLICA` from, so it asks the server for a number
//! (`POST /api/sync/enroll`). The server hands out the smallest free one and records it in
//! `sync_device`; the device keeps it in its own storage, next to its copy of the database, never
//! in a table, since a full download replaces those (decision 6 in [`super`]). Then the device does
//! a full download, so that its first note ids follow the newest note it has seen (see
//! [`crate::ids`]). Each of them has its own letters for those, which is what limits them to
//! three.
//!
//! **A replica number is never shared.** A full upload keeps `sync_device`, so a number stays
//! taken until its device is unenrolled. The server refuses to
//! sync with a replica from `2` up that isn't enrolled ([`Error::NotEnrolled`]), so a device
//! that was unenrolled can't come back under a number that has since gone to another one. That
//! also holds a second laptop with `SPARES_REPLICA=2`: it has to be enrolled too (`spares sync
//! devices add`).
//!
//! Reusing an unenrolled number is safe: every push sends the replica's `id_counter` rows, which
//! the server keeps by taking the larger value, and a full download merges them the same way. So
//! the new device starts above every id the old one handed out before its last sync, deleted ones
//! too, and the ones it handed out after reached no one.

use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use sqlx::SqliteConnection;
use sqlx::SqlitePool;

use crate::Error;

/// The replica numbers devices are enrolled under. `0` is the laptop and `1` the server.
pub const DEVICE_REPLICAS: std::ops::RangeInclusive<u8> = 2..=4;

fn sqlx_error(e: sqlx::Error) -> Error {
    Error::Sqlx { source: e }
}

/// `POST /api/sync/enroll`
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnrollRequest {
    /// What the device is called in the list of devices, such as "iPhone (Safari)".
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnrollResponse {
    pub replica: u8,
}

/// `POST /api/sync/unenroll`
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UnenrollRequest {
    pub replica: u8,
}

/// `GET /api/sync/devices`: an enrolled device.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, sqlx::FromRow)]
pub struct Device {
    pub replica: u8,
    pub name: String,
    /// Unix time
    pub enrolled_at: i64,
    /// Unix time of its last push, if it has pushed. A full upload keeps it.
    pub last_sync_at: Option<i64>,
}

/// Gives a device the smallest free replica number from `2` to `4`. A number is taken while a
/// device is enrolled under it, or while `sync_client` records a replica that synced under it
/// without enrolling (from before enrollment existed).
pub async fn enroll(db: &SqlitePool, request: EnrollRequest) -> Result<EnrollResponse, Error> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err(Error::Sync("a device needs a name".to_string()));
    }
    let mut tx = db.begin().await.map_err(sqlx_error)?;
    let taken: Vec<i64> =
        sqlx::query_scalar("SELECT replica FROM sync_device UNION SELECT replica FROM sync_client")
            .fetch_all(&mut *tx)
            .await
            .map_err(sqlx_error)?;
    let replica = DEVICE_REPLICAS
        .clone()
        .find(|r| !taken.contains(&i64::from(*r)))
        .ok_or_else(|| {
            Error::Sync(format!(
                "every device number ({}–{}) is taken. Remove one that no longer keeps a copy \
                 first, in Settings → This Device or with `spares sync devices remove`.",
                DEVICE_REPLICAS.start(),
                DEVICE_REPLICAS.end()
            ))
        })?;
    sqlx::query("INSERT INTO sync_device (replica, name, enrolled_at) VALUES (?, ?, ?)")
        .bind(i64::from(replica))
        .bind(name)
        .bind(Utc::now().timestamp())
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?;
    tx.commit().await.map_err(sqlx_error)?;
    Ok(EnrollResponse { replica })
}

/// Frees a device's number. Returns whether it was enrolled.
pub async fn unenroll(db: &SqlitePool, request: UnenrollRequest) -> Result<bool, Error> {
    let mut tx = db.begin().await.map_err(sqlx_error)?;
    let removed = sqlx::query("DELETE FROM sync_device WHERE replica = ?")
        .bind(i64::from(request.replica))
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?
        .rows_affected()
        > 0;
    sqlx::query("DELETE FROM sync_client WHERE replica = ?")
        .bind(i64::from(request.replica))
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?;
    tx.commit().await.map_err(sqlx_error)?;
    Ok(removed)
}

/// The enrolled devices, by replica number.
pub async fn list(db: &SqlitePool) -> Result<Vec<Device>, Error> {
    sqlx::query_as(
        "SELECT d.replica, d.name, d.enrolled_at, c.last_sync_at
         FROM sync_device d LEFT JOIN sync_client c ON c.replica = d.replica
         ORDER BY d.replica",
    )
    .fetch_all(db)
    .await
    .map_err(sqlx_error)
}

/// Refuses a replica from `2` up that isn't enrolled. Only `2` to `4` can be.
pub(crate) async fn check_enrolled(conn: &mut SqliteConnection, replica: u8) -> Result<(), Error> {
    if replica < *DEVICE_REPLICAS.start() {
        return Ok(());
    }
    let enrolled: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM sync_device WHERE replica = ?)")
            .bind(i64::from(replica))
            .fetch_one(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    if enrolled {
        Ok(())
    } else {
        Err(Error::NotEnrolled(replica))
    }
}
