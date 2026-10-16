//! Settings from `config.toml` ([`SparesExternalConfig`]), the environment, and where spares keeps
//! its files.
//!
//! # Directories
//!
//! Rendered files are written to a per-parser directory, chosen by the first of these that
//! applies:
//!
//! 1. The parser's own environment variable, if it points to an existing directory:
//!    `MARKDOWN_OUT_DIR`, `LATEX_OUT_DIR`, or `TYPST_OUT_DIR`.
//! 2. `SPARES_CACHE_DIR`, if set ([`get_cache_dir`]).
//! 3. The default cache directory, `$XDG_CACHE_HOME/spares` (`~/.cache/spares` if
//!    `XDG_CACHE_HOME` is unset).
//!
//! The more specific variable wins, so `SPARES_CACHE_DIR` does **not** override a parser's
//! `*_OUT_DIR`. If you run a separate instance of spares (e.g. for testing) next to your main one,
//! unset the `*_OUT_DIR` variables for it too, or its rendered files will overwrite your main
//! instance's. `spares_server` logs the directory each parser renders to when it starts (run it
//! with `RUST_LOG=info` to see this).
//!
//! The other directories can be overridden with `SPARES_CONFIG_DIR` (config file and parser
//! templates, [`get_config_dir`]), `SPARES_DATA_DIR` (database, and note and card text files,
//! [`get_data_dir`]), and `DATABASE_URL` (the database alone, [`get_env_config`]).

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::create_dir_all;
use std::fs::read_to_string;
use std::fs::write;
use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Duration;
use chrono::NaiveDate;
use chrono::Utc;
use chrono::Weekday;
use etcetera::AppStrategy;
use etcetera::AppStrategyArgs;
use etcetera::choose_app_strategy;
use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use sqlx::SqlitePool;
use toml_edit::DocumentMut;

use crate::ALLOWED_F64_ERROR;
use crate::Error;
use crate::LibraryError;
use crate::parsers::image_occlusion::ImageOcclusionConfig;
use crate::parsers::impls::markdown::MarkdownParserConfig;
use crate::parsers::overlapper::OverlapperConfig;
use crate::sync::SyncConfig;

const SPARES: &str = "spares";

/// True when running under a test harness. `cfg!(test)` alone is not enough: it only reflects
/// whether *this crate* was compiled in test mode. A dependent crate's test binary (e.g.
/// `spares_cli`'s tests) compiles `spares_core` as a normal, non-test dependency, so `cfg!(test)`
/// is `false` there even though real tests are running — which previously caused those tests to
/// read/write the user's actual config, cache, and data directories. `SPARES_TEST_MODE` lets such
/// tests opt in explicitly.
pub fn is_test_mode() -> bool {
    cfg!(test) || std::env::var_os("SPARES_TEST_MODE").is_some()
}

#[allow(clippy::missing_panics_doc)]
pub fn get_config_dir() -> PathBuf {
    if is_test_mode() {
        let mut tmp_dir = PathBuf::from("/tmp");
        tmp_dir.push(SPARES);
        tmp_dir.push("config");
        create_dir_all(&tmp_dir).unwrap();
        return tmp_dir;
    }
    if let Ok(p) = std::env::var("SPARES_CONFIG_DIR") {
        let path = PathBuf::from(p);
        create_dir_all(&path).unwrap();
        return path;
    }
    let strategy: etcetera::app_strategy::Xdg = choose_app_strategy(AppStrategyArgs {
        top_level_domain: "org".to_string(),
        author: SPARES.to_string(),
        app_name: SPARES.to_string(),
    })
    .unwrap();
    strategy.config_dir().push(SPARES);
    create_dir_all(strategy.config_dir()).unwrap();
    strategy.config_dir()
}

#[allow(clippy::missing_panics_doc)]
pub fn get_cache_dir() -> PathBuf {
    if is_test_mode() {
        let mut tmp_dir = PathBuf::from("/tmp");
        tmp_dir.push(SPARES);
        tmp_dir.push("cache");
        create_dir_all(&tmp_dir).unwrap();
        return tmp_dir;
    }
    if let Ok(p) = std::env::var("SPARES_CACHE_DIR") {
        let path = PathBuf::from(p);
        create_dir_all(&path).unwrap();
        return path;
    }
    let strategy: etcetera::app_strategy::Xdg = choose_app_strategy(AppStrategyArgs {
        top_level_domain: "org".to_string(),
        author: SPARES.to_string(),
        app_name: SPARES.to_string(),
    })
    .unwrap();
    strategy.cache_dir().push(SPARES);
    create_dir_all(strategy.cache_dir()).unwrap();
    strategy.cache_dir()
}

/// The directory the server serves at `/files`. Paths in API responses are relative to it, so
/// web clients can request them and local clients can [resolve](resolve_in_files_dir) them.
/// Defaults to the rendered files, never the data directory, which holds the database.
pub fn get_files_dir() -> PathBuf {
    std::env::var("SPARES_FILES_DIR").map_or_else(|_| get_cache_dir(), PathBuf::from)
}

/// The local path of a path from an API response. Paths outside the files directory are
/// already absolute and returned as is.
pub fn resolve_in_files_dir(path: &Path) -> PathBuf {
    get_files_dir().join(path)
}

#[allow(clippy::missing_panics_doc)]
pub fn get_data_dir() -> PathBuf {
    if is_test_mode() {
        let mut tmp_dir = PathBuf::from("/tmp");
        tmp_dir.push(SPARES);
        tmp_dir.push("data");
        create_dir_all(&tmp_dir).unwrap();
        return tmp_dir;
    }
    if let Ok(p) = std::env::var("SPARES_DATA_DIR") {
        let path = PathBuf::from(p);
        create_dir_all(&path).unwrap();
        return path;
    }
    let strategy: etcetera::app_strategy::Xdg = choose_app_strategy(AppStrategyArgs {
        top_level_domain: "org".to_string(),
        author: SPARES.to_string(),
        app_name: SPARES.to_string(),
    })
    .unwrap();
    strategy.data_dir().push(SPARES);
    create_dir_all(strategy.data_dir()).unwrap();
    strategy.data_dir()
}

#[derive(Clone, Copy, Debug, strum::EnumString, strum::Display, strum_macros::EnumIter)]
pub enum Environment {
    Production,
    Development,
}

#[derive(Debug, Clone)]
pub struct EnvironmentConfig {
    pub socket_address: String,
    pub database_url: String,
}

pub fn get_env_config(env: Environment) -> EnvironmentConfig {
    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        let mut database_path = get_data_dir();
        database_path.push(match env {
            Environment::Production => "spares-main.sqlite",
            Environment::Development => "spares-dev.sqlite",
        });
        format!("sqlite://{}", database_path.display())
    });
    let socket_address = std::env::var("SPARES_SOCKET_ADDRESS").unwrap_or_else(|_| match env {
        Environment::Production => "127.0.0.1:8080".to_string(),
        Environment::Development => "127.0.0.1:8081".to_string(),
    });

    EnvironmentConfig {
        socket_address,
        database_url,
    }
}

/// The local server's API key, from `SPARES_API_KEY`. The server then refuses requests without it,
/// so its clients send it too.
pub fn local_api_key() -> Option<String> {
    std::env::var("SPARES_API_KEY")
        .ok()
        .filter(|key| !key.is_empty())
}

/// Which replica this instance is, from `SPARES_REPLICA`: `0` is the laptop, `1` the server, and a
/// later replica takes the next unused number. It decides which form of note id this instance
/// writes (see [`crate::ids`]). It comes from the environment, never the database, since a full
/// sync copies the other replica's database.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Replica(u8);

impl Replica {
    /// The largest replica number. Each replica owns one residue modulo [`crate::ids::ID_STEP`].
    pub const MAX: u8 = crate::ids::ID_STEP as u8 - 1;

    pub(crate) const fn new_unchecked(number: u8) -> Self {
        Self(number)
    }

    pub const fn number(self) -> u8 {
        self.0
    }
}

impl std::str::FromStr for Replica {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.trim()
            .parse::<u8>()
            .ok()
            .filter(|n| *n <= Self::MAX)
            .map(Self)
            .ok_or_else(|| {
                format!(
                    "expected a replica number from 0 to {}, got `{s}`",
                    Self::MAX
                )
            })
    }
}

/// Whether this instance answers sync requests or runs the client side, from `SPARES_SYNC_ROLE`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, strum::EnumString, strum::Display)]
#[strum(serialize_all = "lowercase")]
pub enum SyncRole {
    Server,
    Client,
}

pub const REPLICA_ENV: &str = "SPARES_REPLICA";
pub const SYNC_ROLE_ENV: &str = "SPARES_SYNC_ROLE";

#[cfg(test)]
tokio::task_local! {
    /// Lets a test act as another replica (for example the server in a sync test) for the code it
    /// runs inside [`with_test_replica`], since `SPARES_REPLICA` is shared by every test.
    static TEST_REPLICA: (Replica, SyncRole);
}

/// Runs `f` as replica `replica` with sync role `role`. Tests only.
#[cfg(test)]
pub(crate) async fn with_test_replica<F: std::future::Future>(
    replica: Replica,
    role: SyncRole,
    f: F,
) -> F::Output {
    TEST_REPLICA.scope((replica, role), f).await
}

/// This instance's replica. Required, with no default, except under tests, which are replica `0`.
pub fn get_replica() -> Result<Replica, Error> {
    #[cfg(test)]
    if let Ok((replica, _)) = TEST_REPLICA.try_with(|test| *test) {
        return Ok(replica);
    }
    match std::env::var(REPLICA_ENV) {
        Ok(value) => value
            .parse()
            .map_err(|e| invalid_config(format!("{REPLICA_ENV}: {e}"))),
        Err(_) if is_test_mode() => Ok(Replica(0)),
        Err(_) => Err(invalid_config(format!(
            "{REPLICA_ENV} is not set. Set it to 0 on the laptop and 1 on the server."
        ))),
    }
}

/// This instance's sync role. Required, with no default, except under tests, which are clients.
pub fn get_sync_role() -> Result<SyncRole, Error> {
    #[cfg(test)]
    if let Ok((_, role)) = TEST_REPLICA.try_with(|test| *test) {
        return Ok(role);
    }
    match std::env::var(SYNC_ROLE_ENV) {
        Ok(value) => value.trim().parse().map_err(|_| {
            invalid_config(format!(
                "{SYNC_ROLE_ENV}: expected `server` or `client`, got `{value}`"
            ))
        }),
        Err(_) if is_test_mode() => Ok(SyncRole::Client),
        Err(_) => Err(invalid_config(format!(
            "{SYNC_ROLE_ENV} is not set. Set it to `client` on the laptop and `server` on the \
             server."
        ))),
    }
}

fn invalid_config(message: String) -> Error {
    Error::Library(LibraryError::InvalidConfig(message))
}

#[serde_with::serde_as]
#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct SparesInternalConfig {
    pub(crate) last_unburied: DateTime<Utc>,
    pub(crate) linked_notes_generated: bool,
}

impl Default for SparesInternalConfig {
    fn default() -> Self {
        Self {
            last_unburied: DateTime::<Utc>::MIN_UTC,
            linked_notes_generated: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct EasyDaysConfig {
    /// Deprecated: use [`SparesExternalConfig::load_balance`]. `false` is read as
    /// `load_balance = false`, which is what it used to control.
    #[serde(skip_serializing)]
    pub enabled: Option<bool>,
    /// Mapping from days of the week to a percentage describing their workload.
    /// Between 0% (0.0) and 100% (1.0). For example, if this is 0.2, then 20% of cards will be scheduled on that day, relative to normal days (which are set to 1.0).
    ///
    /// Each percentage is relative to the rest. For example, if all days are set to 0.1, then each day will be treated normally, since 0.1/(0.1 * 7) = 1/7, so each day will have 1/7 of the workload which is the default behavior.
    ///
    /// Only days within a card's fuzz range can be chosen, so this shifts reviews by a few days
    /// at most.
    pub days_to_workload_percentage: HashMap<Weekday, f64>,
    /// Specific easy dates. Useful when you are going on vacation, for example, and want minimal workload on those days. These days will have a workload percentage of 0%.
    pub specific_dates: HashSet<NaiveDate>,
}

impl Default for EasyDaysConfig {
    fn default() -> Self {
        let mut days_to_workload_percentage = HashMap::new();
        for weekday in [
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
            Weekday::Sat,
            Weekday::Sun,
        ] {
            days_to_workload_percentage.insert(weekday, 1.);
        }
        Self {
            enabled: None,
            days_to_workload_percentage,
            specific_dates: HashSet::default(),
        }
    }
}

/// The `[queue]` table of `config.toml`: the order review shows the cards it can show today.
///
/// `spares card review`, the web app, and the queue the web app downloads for offline review all
/// use this order. Which cards can be shown at all (what is due, and the daily limits) is set
/// elsewhere, such as [`SparesExternalConfig::new_cards_daily_limit`].
///
/// Cards come in these groups, one group after another:
///
/// 1. Learning and relearning cards whose current step is over, the earliest first. They go first
///    so their steps (see [`SparesExternalConfig::learning_steps`]) are not stretched by the cards
///    around them.
/// 2. New cards and review cards, each in their own order ([`Self::new_order`] and
///    [`Self::review_order`]), combined as [`Self::new_review_mix`] says. A review query that
///    sorts, like `spares card review -q 'tag=chess sort_by_desc=c.difficulty'`, orders them by
///    its sort instead (see "Sorting" in [`crate::search`]).
/// 3. Learning and relearning cards whose current step ends within [`Self::learn_ahead_limit`],
///    the earliest first.
/// 4. Cards buried until later today, in the order they were buried.
///
/// Learning and relearning cards whose step ends later than that are not shown until it is
/// within the limit. When they are all that is left, `spares card review` and the web app wait
/// for the next one, showing when it will be.
///
/// For example, to review the cards you are most likely to have forgotten first, and to learn new
/// cards a whole note at a time, in a random order of notes:
///
/// ```toml
/// [queue]
/// review_order = "retrievability"
/// new_order = "random_note"
/// new_review_mix = "interleaved"
/// learn_ahead_limit = 1200
/// ```
///
/// The cards of notes with some tags can have orders, limits and steps of their own, through
/// [`Self::presets`]. See [`QueuePreset`].
#[serde_with::serde_as]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct QueueConfig {
    /// The order of review cards (cards that graduated from their learning steps). Defaults to
    /// [`ReviewOrder::Due`].
    pub review_order: ReviewOrder,
    /// The order of new cards. Defaults to [`NewCardOrder::Created`].
    pub new_order: NewCardOrder,
    /// How new cards and review cards are combined. Defaults to [`NewReviewMix::Interleaved`].
    pub new_review_mix: NewReviewMix,
    /// How early a learning or relearning card may be shown when nothing else is left, in
    /// seconds. Defaults to 1200 (20 minutes), as in Anki.
    ///
    /// While other cards are left, a learning card waits until its step is over. Once nothing
    /// else is left, a card whose step ends within this limit is shown early, so a session does
    /// not end with waiting out a short step. A card whose step ends later than that is not shown
    /// until it is within the limit, and the review waits for it.
    ///
    /// With the default and steps of 1 and 10 minutes, cards on those steps come back as soon as
    /// nothing else is left, while a card on a 1 hour step waits until 20 minutes of it are left.
    /// Set it to 0 to never show a card before its step is over, or to 86400 (a day) to show any
    /// learning card due today as soon as nothing else is left, as spares did before this
    /// setting.
    ///
    /// Filtered tags are not affected: they show every card in them, whenever it is due. Offline
    /// review (see [`crate::api::offline_review`]) applies the limit the server had when the
    /// cards were downloaded.
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    pub learn_ahead_limit: Duration,
    /// Whether rating a card buries the new cards of its note until the next day. Defaults to
    /// `false`.
    ///
    /// Cards of the same note often give each other away, so seeing one makes the next easier
    /// than it would be on its own. With this set, a new note's cards are learned one a day.
    ///
    /// Siblings are only buried when the card is rated on the day it is reviewed, outside a
    /// filtered tag (the cards of a filtered tag were picked to be studied together), and
    /// undoing the rating unburies them. Buried siblings are listed by `c.scheduler_buried`
    /// queries and unburied with `spares card unbury`.
    pub bury_new_siblings: bool,
    /// Whether rating a card buries the review cards of its note that are due today until the
    /// next day. Defaults to `false`. See [`Self::bury_new_siblings`].
    ///
    /// [`SparesExternalConfig::disperse_siblings`] keeps siblings from being due on the same day
    /// to begin with; this handles the ones that are anyway, such as after a backlog. Learning
    /// and relearning siblings are never buried, since that would break their steps.
    pub bury_review_siblings: bool,
    /// Settings for the cards of notes with some tags, in place of the top-level ones, by name:
    /// `[queue.presets.<name>]`. Empty by default. See [`QueuePreset`].
    ///
    /// The order they are listed in is their priority: a card whose note matches several presets
    /// uses the first.
    pub presets: IndexMap<String, QueuePreset>,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            review_order: ReviewOrder::default(),
            new_order: NewCardOrder::default(),
            new_review_mix: NewReviewMix::default(),
            learn_ahead_limit: Duration::minutes(20),
            bury_new_siblings: false,
            bury_review_siblings: false,
            presets: IndexMap::new(),
        }
    }
}

/// A `[queue.presets.<name>]` table of `config.toml`: settings for the cards of notes with some
/// tags, like a deck preset in Anki, where tags play the role of decks. See
/// [`QueueConfig::presets`].
///
/// Subjects often want different settings. Vocabulary does well with many new cards a day in a
/// random order, while a math course wants few new cards, in the order the notes were written,
/// with longer steps. With one set of settings, one of them is always set wrong:
///
/// ```toml
/// [queue]
/// review_order = "due"
///
/// [queue.presets.languages]
/// tags = ["lang"]                  # notes tagged `lang`, or `lang:german`, `lang:french:verbs`...
/// new_cards_daily_limit = 40
/// new_order = "random"
/// review_order = "random"
/// learning_steps = [60, 300, 1800] # 1, 5 and 30 minutes
///
/// [queue.presets.exam]
/// tags = ["math:analysis", "math:algebra"]
/// new_cards_daily_limit = 5
/// review_order = "retrievability"
/// relearning_steps = [600, 3600]
/// bury_new_siblings = true
/// ```
///
/// A note tagged `lang:german` gets up to 40 new cards a day, in a random order, with steps of 1,
/// 5 and 30 minutes. One tagged `math:analysis` gets 5 new cards a day and its reviews by
/// retrievability. One tagged both uses `languages`, since it is listed first. Notes matching no
/// preset use the top-level settings.
///
/// # Which preset a card uses
///
/// A card uses the first preset, in the order of `config.toml`, one of whose [`Self::tags`] its
/// note has, either exactly or as a parent: `lang` matches the tags `lang` and `lang:german` but
/// not `language` or `old:lang`. Only the note's own tags count, not the filtered tags its cards
/// are in. Changing a note's tags changes its cards' preset right away, for the counts of what was
/// studied today too (see [`Self::new_cards_daily_limit`]).
///
/// # What a preset can set
///
/// Each setting left out is taken from the top level. A preset can set:
///
/// - Its own daily limits, [`Self::new_cards_daily_limit`] and [`Self::reviews_daily_limit`].
/// - The order of its cards, [`Self::new_order`] and [`Self::review_order`].
/// - Its steps, [`Self::learning_steps`] and [`Self::relearning_steps`].
/// - Sibling burying, [`Self::bury_new_siblings`] and [`Self::bury_review_siblings`].
///
/// Everything else stays global. The FSRS parameters are fitted to the whole review log, so
/// there is one set of them. [`QueueConfig::new_review_mix`] and
/// [`QueueConfig::learn_ahead_limit`] shape a whole review session, which can mix the cards of
/// several presets. A setting a preset cannot set, or a misspelled one, is an error rather than
/// silently ignored.
#[serde_with::serde_as]
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct QueuePreset {
    /// The tags whose notes use this preset, along with their child tags. Must not be empty.
    pub tags: Vec<String>,
    /// The most new cards of this preset to study each day. Like
    /// [`SparesExternalConfig::new_cards_daily_limit`], which still caps the total.
    ///
    /// For example, with a top-level limit of 50 and 40 here, a day can have 40 new cards of this
    /// preset and 10 others, or 50 others, but not 40 and 20. The cards studied today count
    /// toward the preset their note matches now, so retagging a note moves its reviews of today
    /// to its new preset.
    pub new_cards_daily_limit: Option<u32>,
    /// The most review cards of this preset to study each day. Like
    /// [`SparesExternalConfig::reviews_daily_limit`], which still caps the total, and with
    /// [`SparesExternalConfig::new_cards_ignore_review_limit`] applying to it the same way: unless
    /// that is set, this preset's new cards count toward it too, and only fill the room this
    /// preset's reviews due today leave under it.
    pub reviews_daily_limit: Option<u32>,
    /// The order of this preset's review cards, in place of [`QueueConfig::review_order`].
    ///
    /// In a review mixing presets that order cards differently, cards are gathered by order: the
    /// cards of the first preset listed come first (with those of any later preset that orders
    /// them the same way, top level included), then those of the next preset with another order,
    /// and so on. To study one preset in its own order, review its tag, as in `spares card review
    /// -q 'tag=lang'`.
    pub review_order: Option<ReviewOrder>,
    /// The order of this preset's new cards, in place of [`QueueConfig::new_order`]. Cards of
    /// presets with different orders are gathered as for [`Self::review_order`].
    pub new_order: Option<NewCardOrder>,
    /// This preset's learning steps, in seconds, in place of
    /// [`SparesExternalConfig::learning_steps`]. The same rules apply.
    ///
    /// They are used when a card is rated, and when its history is replayed, as by `spares card
    /// reschedule`, by a sync, or by an offline review inserted into the history. A card that
    /// changes preset in the middle of its steps continues on the new preset's steps.
    #[serde_as(as = "Option<Vec<serde_with::DurationSeconds<i64>>>")]
    pub learning_steps: Option<Vec<Duration>>,
    /// This preset's relearning steps, in seconds, in place of
    /// [`SparesExternalConfig::relearning_steps`]. See [`Self::learning_steps`]. `spares card
    /// optimize` still assumes the top-level number of relearning steps.
    #[serde_as(as = "Option<Vec<serde_with::DurationSeconds<i64>>>")]
    pub relearning_steps: Option<Vec<Duration>>,
    /// Whether rating a card of this preset buries the new cards of its note, in place of
    /// [`QueueConfig::bury_new_siblings`]. Siblings are cards of the same note, so they always
    /// share the preset.
    pub bury_new_siblings: Option<bool>,
    /// Whether rating a card of this preset buries its note's review cards due today, in place
    /// of [`QueueConfig::bury_review_siblings`].
    pub bury_review_siblings: Option<bool>,
}

impl QueuePreset {
    /// Whether this preset sets anything that changes how its cards are scheduled or buried, as
    /// [`SparesExternalConfig::for_preset`] applies.
    fn changes_rating(&self) -> bool {
        self.learning_steps.is_some()
            || self.relearning_steps.is_some()
            || self.bury_new_siblings.is_some()
            || self.bury_review_siblings.is_some()
    }

    /// Whether this preset has a daily limit of its own.
    pub(crate) fn has_limits(&self) -> bool {
        self.new_cards_daily_limit.is_some() || self.reviews_daily_limit.is_some()
    }
}

/// The order of review cards. See [`QueueConfig::review_order`].
///
/// Cards that tie are shown in the order of their ids, so the order is the same every time it is
/// computed.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewOrder {
    /// `"due"`: the card that has been due the longest first, then cards of older notes first.
    /// Older notes come first since newer notes may build on them.
    #[default]
    Due,
    /// `"retrievability"`: the card you are least likely to remember first, as FSRS predicts from
    /// its stability and the time since its last review. When you are behind, this saves the
    /// cards that are slipping before the ones that would keep, at the cost of cards with short
    /// intervals waiting longer than their due date.
    Retrievability,
    /// `"relative_overdueness"`: the card that is the most overdue compared to its interval
    /// first. A card 3 days late on a 3 day interval (100% overdue) comes before one 10 days late
    /// on a 100 day interval (10% overdue). Similar to [`Self::Retrievability`], from the due date
    /// rather than the memory model.
    RelativeOverdueness,
    /// `"difficulty_descending"`: the hardest card first, by its FSRS difficulty.
    DifficultyDescending,
    /// `"difficulty_ascending"`: the easiest card first, by its FSRS difficulty.
    DifficultyAscending,
    /// `"random"`: a random order. It is the same all day, so a session picked up again later in
    /// the day continues where it left off, and changes the next day.
    Random,
    /// `"note_created"`: cards of older notes first, regardless of when they are due. Useful when
    /// notes build on earlier ones, like the notes of a course.
    NoteCreated,
}

/// The order of new cards. See [`QueueConfig::new_order`].
///
/// Cards that tie are shown in the order of their ids.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NewCardOrder {
    /// `"created"`: cards of the oldest notes first, and a note's cards in the order they appear
    /// in it. Since newer notes may build on older ones, this learns them in that order.
    #[default]
    Created,
    /// `"newest_first"`: cards of the newest notes first, and a note's cards in the order they
    /// appear in it. Useful to learn what you just wrote down while it is still fresh.
    NewestFirst,
    /// `"random"`: a random order of cards. It is the same all day and changes the next day.
    Random,
    /// `"random_note"`: a random order of notes, with each note's cards together, in the order
    /// they appear in it. It is the same all day and changes the next day.
    RandomNote,
}

/// How new cards and review cards are combined. See [`QueueConfig::new_review_mix`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NewReviewMix {
    /// `"interleaved"`: new cards are spread evenly among review cards. With 100 reviews and 20
    /// new cards left, every sixth card is new. Only new cards that today's limit still allows are
    /// counted, so they are spread over the whole session rather than shown at its start.
    #[default]
    Interleaved,
    /// `"new_first"`: all new cards, then all review cards.
    NewFirst,
    /// `"reviews_first"`: all review cards, then all new cards.
    ReviewsFirst,
}

/// The `[leech]` table of `config.toml`: cards you keep forgetting.
///
/// A card lapses when it is rated Again after it graduated from its learning steps (Again during
/// learning or relearning steps does not count). A card that has lapsed
/// [`Self::lapses_threshold`] times is a leech: reviewing it again and again costs more time than
/// it is worth, and the note is usually better rewritten, split up, or dropped. `spares card
/// leeches` lists them.
///
/// ```toml
/// [leech]
/// lapses_threshold = 8
/// action = "suspend"
/// tag_name = "leech"
/// ```
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct LeechConfig {
    /// The number of lapses at which a card becomes a leech. Defaults to 8.
    pub lapses_threshold: u32,
    /// What to do with a card once it becomes a leech. Defaults to [`LeechAction::Tag`].
    ///
    /// It is done on the lapse that reaches [`Self::lapses_threshold`], and again every half of
    /// it after (every 4 lapses with the default of 8), in case the card was let back in without
    /// being fixed. Undoing the rating undoes it too. An offline rating placed before a later
    /// review of the card, which cannot be undone, gets it as an undo event of its own (see
    /// [`crate::api::offline_review`]).
    pub action: LeechAction,
    /// The tag [`LeechAction::Tag`] and [`LeechAction::Suspend`] add to a leech's note, so you can
    /// find the notes to fix with `tag=leech`. Tags belong to notes, so the note's other cards
    /// get it too. Defaults to `"leech"`.
    pub tag_name: String,
}

/// What to do with a card once it becomes a leech. See [`LeechConfig::action`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LeechAction {
    /// `"tag"`: add [`LeechConfig::tag_name`] to the card's note, and keep reviewing it.
    #[default]
    Tag,
    /// `"suspend"`: add [`LeechConfig::tag_name`] to the card's note, and suspend the card, so it
    /// is not reviewed again until you unsuspend it, presumably after fixing it.
    Suspend,
    /// `"none"`: nothing. Leeches are still listed by `spares card leeches`.
    None,
}

impl Default for LeechConfig {
    fn default() -> Self {
        Self {
            lapses_threshold: 8,
            action: LeechAction::default(),
            tag_name: "leech".to_string(),
        }
    }
}

impl LeechConfig {
    /// Whether a card that now has `lapses` lapses, the last of them just now, should get
    /// [`Self::action`].
    pub(crate) fn acts_on(&self, lapses: u32) -> bool {
        let interval = (self.lapses_threshold / 2).max(1);
        self.action != LeechAction::None
            && lapses >= self.lapses_threshold
            && (lapses - self.lapses_threshold).is_multiple_of(interval)
    }
}

/// The `[statistics]` table of `config.toml`. It changes only how statistics are shown, not how
/// cards are scheduled, so changing it needs no `spares card reschedule`: every past review is
/// counted again under it the next time statistics load.
///
/// ```toml
/// [statistics]
/// mature_threshold_days = 21
/// ```
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct StatisticsConfig {
    /// The interval, in whole days, from which a card is mature rather than young. Defaults to
    /// 21, as in Anki.
    ///
    /// A review is mature when the card's interval going into it, the interval its previous
    /// review gave it, was at least this long; a card is mature when the time from its last
    /// review to its due date is. With `mature_threshold_days = 7`, a card reviewed on Monday and
    /// given an interval of 7 days is mature, and so is its review the next Monday. Statistics
    /// split Reviews, True retention, Answer buttons and Card counts by it, and label their
    /// columns with it, e.g. "Mature (≥ 7 days)".
    pub mature_threshold_days: u32,
}

impl Default for StatisticsConfig {
    fn default() -> Self {
        Self {
            mature_threshold_days: 21,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct ParserConfig {
    pub markdown: MarkdownParserConfig,
}

#[serde_with::serde_as]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent setting of config.toml"
)]
pub struct SparesExternalConfig {
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    pub maximum_interval: Duration,
    /// Cards due in less than this duration will not be rescheduled to respect their desired retention.
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    pub minimum_interval: Duration,
    /// The most new cards to study each day. A card counts once it is first rated, whether or not
    /// it graduates that day. Defaults to 20.
    ///
    /// Unless [`Self::new_cards_ignore_review_limit`] is set, new cards are also held back by
    /// [`Self::reviews_daily_limit`].
    ///
    /// A [preset](QueuePreset::new_cards_daily_limit) can have a limit of its own for its cards,
    /// within this one.
    pub new_cards_daily_limit: u32,
    /// The most review cards to study each day. A card counts once it is rated while in review,
    /// that is, after it graduated from its learning steps. Cards in learning or relearning steps
    /// are never held back, so a card forgotten today can still be relearned once the limit is
    /// reached. Defaults to 9999, which in practice means no limit.
    ///
    /// When the limit is reached, review cards still due wait until the next day, when they are
    /// shown in [`QueueConfig::review_order`]. Ordering by
    /// [`ReviewOrder::Retrievability`] then makes sure the cards held back are the ones most
    /// likely to still be remembered.
    ///
    /// Like the new card limit, this applies to `spares card review` with or without `--query`,
    /// and not to filtered tags, which show every card in them. A query with `limit` (a review
    /// snapshot) leaves out review cards once the limit is reached, but is otherwise limited by
    /// its own `limit`s.
    ///
    /// A [preset](QueuePreset::reviews_daily_limit) can have a limit of its own for its cards,
    /// within this one.
    pub reviews_daily_limit: u32,
    /// Whether new cards may be studied regardless of [`Self::reviews_daily_limit`]. Defaults to
    /// `false`, as in Anki: new cards then count toward the review limit too, and are only shown
    /// while the reviews due today still leave room under it. With a limit of 200 and 190 reviews
    /// due, at most 10 new cards are shown, however high [`Self::new_cards_daily_limit`] is, so
    /// a backlog of reviews stops new cards from adding to it.
    pub new_cards_ignore_review_limit: bool,
    pub flagged_tag_name: String,
    /// Delays between the same-day reviews of a new card before it graduates to review, in
    /// seconds. Each must be shorter than a day, in increasing order. Empty to graduate on the
    /// first answer. A [preset](QueuePreset::learning_steps) can have its own.
    #[serde_as(as = "Vec<serde_with::DurationSeconds<i64>>")]
    pub learning_steps: Vec<Duration>,
    /// Like [`Self::learning_steps`], for a card in review that was forgotten. Empty to keep it
    /// in review with a short interval instead. A [preset](QueuePreset::relearning_steps) can
    /// have its own.
    #[serde_as(as = "Vec<serde_with::DurationSeconds<i64>>")]
    pub relearning_steps: Vec<Duration>,
    /// The FSRS model's parameters: 21 for FSRS-6, or 19 or 17 from FSRS-5 or FSRS-4.5, so
    /// parameters from Anki work too. Empty to use the FSRS-6 defaults, which are fitted to many
    /// people's reviews. `spares card optimize --apply` sets these from your review history. Run
    /// `spares card reschedule` after changing them by hand.
    pub fsrs_parameters: Vec<f32>,
    /// Spread reviews so that each day has a similar number of cards due, in proportion to its
    /// easy-day workload percentage.
    pub load_balance: bool,
    /// Avoid scheduling cards of the same note on the same or nearby days, so that reviewing one
    /// does not give away the answer to another.
    pub disperse_siblings: bool,
    pub easy_days: EasyDaysConfig,
    /// The order review shows cards in. See [`QueueConfig`].
    pub queue: QueueConfig,
    pub leech: LeechConfig,
    pub statistics: StatisticsConfig,
    pub parser: ParserConfig,
    pub image_occlusion: ImageOcclusionConfig,
    pub overlapper: OverlapperConfig,
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    pub set_card_due_date_duration: Duration,
    pub sync: SyncConfig,
    /// Compile the notes and cards of parsers that render in the browser, and write their cards'
    /// raw files. Web clients compile these cards themselves from the database, so a remote server
    /// can turn this off to save compute. Keep it on where notes are rendered before
    /// `spares sync cloud`, since rendering is what records the files their sources read.
    pub render_browser_parsers: bool,
    /// The command `spares card review` plays audio clips with, followed by each clip's path.
    /// Defaults to `afplay` on macOS and `mpv --no-video` elsewhere.
    pub audio_player: String,
}

impl Default for SparesExternalConfig {
    fn default() -> Self {
        Self {
            maximum_interval: Duration::days(180),
            minimum_interval: Duration::days(2),
            new_cards_daily_limit: 20,
            reviews_daily_limit: 9999,
            new_cards_ignore_review_limit: false,
            flagged_tag_name: "flagged".to_string(),
            learning_steps: vec![Duration::minutes(1), Duration::minutes(10)],
            relearning_steps: vec![Duration::minutes(10)],
            fsrs_parameters: Vec::new(),
            load_balance: true,
            disperse_siblings: true,
            easy_days: EasyDaysConfig::default(),
            queue: QueueConfig::default(),
            leech: LeechConfig::default(),
            statistics: StatisticsConfig::default(),
            parser: ParserConfig::default(),
            image_occlusion: ImageOcclusionConfig::default(),
            overlapper: OverlapperConfig::default(),
            set_card_due_date_duration: Duration::weeks(1),
            sync: SyncConfig::default(),
            render_browser_parsers: true,
            audio_player: if cfg!(target_os = "macos") {
                "afplay"
            } else {
                "mpv --no-video"
            }
            .to_string(),
        }
    }
}

impl SparesExternalConfig {
    pub(crate) fn validate(&mut self) -> Result<(), String> {
        validate_steps("learning_steps", &self.learning_steps)?;
        validate_steps("relearning_steps", &self.relearning_steps)?;
        for (name, preset) in &self.queue.presets {
            let key = |setting: &str| format!("queue.presets.{name}.{setting}");
            if preset.tags.is_empty() {
                return Err(format!(
                    "`{}` must list at least one tag, or the preset applies to no card.",
                    key("tags")
                ));
            }
            if preset.tags.iter().any(|tag| tag.trim().is_empty()) {
                return Err(format!("`{}` must not have empty tags.", key("tags")));
            }
            for (setting, steps) in [
                ("learning_steps", &preset.learning_steps),
                ("relearning_steps", &preset.relearning_steps),
            ] {
                if let Some(steps) = steps {
                    validate_steps(&key(setting), steps)?;
                }
            }
        }
        if self.queue.learn_ahead_limit < Duration::zero() {
            return Err("`queue.learn_ahead_limit` must not be negative.".to_string());
        }
        if fsrs_rs::check_and_fill_parameters(&self.fsrs_parameters).is_err() {
            return Err(format!(
                "`fsrs_parameters` must be empty or 17, 19 or 21 finite numbers, not {} values.",
                self.fsrs_parameters.len()
            ));
        }
        if self.easy_days.enabled == Some(false) && self.load_balance {
            log::warn!(
                "`easy_days.enabled` is deprecated. Replace `enabled = false` with \
                 `load_balance = false` at the top level of the config."
            );
            self.load_balance = false;
        }
        for (weekday, workload_percentage) in &self.easy_days.days_to_workload_percentage {
            if !(&0_f64..=&1.).contains(&workload_percentage) {
                return Err(format!(
                    "{:?}'s workload percentage must be between 0% (0.0) and 100% (1.0).",
                    weekday
                ));
            }
        }
        if self
            .easy_days
            .days_to_workload_percentage
            .values()
            .all(|x| x.abs() < ALLOWED_F64_ERROR)
        {
            return Err("Each day cannot have 0 workload.".to_string());
        }

        // Add missing days
        for weekday in [
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
            Weekday::Sat,
            Weekday::Sun,
        ] {
            self.easy_days
                .days_to_workload_percentage
                .entry(weekday)
                .or_insert(1.);
        }

        // Renormalize weights
        let total: f64 = self.easy_days.days_to_workload_percentage.values().sum();
        self.easy_days
            .days_to_workload_percentage
            .iter_mut()
            .for_each(|(_, value)| {
                *value /= total;
            });
        Ok(())
    }
}

impl SparesExternalConfig {
    /// The settings a card of the preset at index `preset` of [`QueueConfig::presets`] is rated
    /// with: these, with the preset's steps and sibling burying in place of the top-level ones.
    /// Borrowed when nothing changes, as for `None` (a card matching no preset).
    pub(crate) fn for_preset(&self, preset: Option<usize>) -> Cow<'_, Self> {
        let Some(preset) = preset
            .and_then(|index| self.queue.presets.get_index(index))
            .map(|(_, preset)| preset)
            .filter(|preset| preset.changes_rating())
        else {
            return Cow::Borrowed(self);
        };
        let mut config = self.clone();
        if let Some(steps) = &preset.learning_steps {
            config.learning_steps.clone_from(steps);
        }
        if let Some(steps) = &preset.relearning_steps {
            config.relearning_steps.clone_from(steps);
        }
        if let Some(bury) = preset.bury_new_siblings {
            config.queue.bury_new_siblings = bury;
        }
        if let Some(bury) = preset.bury_review_siblings {
            config.queue.bury_review_siblings = bury;
        }
        Cow::Owned(config)
    }
}

/// Checks `steps`, the value of `name`, as [`SparesExternalConfig::learning_steps`] describes.
fn validate_steps(name: &str, steps: &[Duration]) -> Result<(), String> {
    if steps
        .iter()
        .any(|step| *step <= Duration::zero() || *step >= Duration::days(1))
    {
        return Err(format!(
            "Each of `{name}` must be longer than 0 seconds and shorter than a day. Longer \
             intervals are FSRS's to choose."
        ));
    }
    if !steps.is_sorted_by(|a, b| a < b) {
        return Err(format!("`{name}` must be in increasing order."));
    }
    Ok(())
}

pub(crate) async fn read_internal_config(pool: &SqlitePool) -> Result<SparesInternalConfig, Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM app_state WHERE key IN ('last_unburied', 'linked_notes_generated')",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    let defaults = SparesInternalConfig::default();
    let mut last_unburied = defaults.last_unburied;
    let mut linked_notes_generated = defaults.linked_notes_generated;

    for (key, value) in rows {
        match key.as_str() {
            "last_unburied" => {
                if let Ok(dt) = value.parse::<DateTime<Utc>>() {
                    last_unburied = dt;
                }
            }
            "linked_notes_generated" => {
                if let Ok(b) = value.parse::<bool>() {
                    linked_notes_generated = b;
                }
            }
            _ => {}
        }
    }

    Ok(SparesInternalConfig {
        last_unburied,
        linked_notes_generated,
    })
}

pub(crate) async fn write_internal_config(
    pool: &SqlitePool,
    config: &SparesInternalConfig,
) -> Result<(), Error> {
    let last_unburied = config.last_unburied.to_rfc3339();
    let linked_notes_generated = config.linked_notes_generated.to_string();
    sqlx::query(
        "INSERT INTO app_state (key, value) VALUES ('last_unburied', ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(last_unburied)
    .execute(pool)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    sqlx::query(
        "INSERT INTO app_state (key, value) VALUES ('linked_notes_generated', ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(linked_notes_generated)
    .execute(pool)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    Ok(())
}

pub fn get_external_config_file() -> PathBuf {
    let mut config_file_path = get_config_dir();
    config_file_path.push("config.toml");
    config_file_path
}

/// Sets `fsrs_parameters` in the config file, leaving the rest of the file, comments included, as
/// it is.
pub(crate) fn write_fsrs_parameters(parameters: &[f32]) -> Result<(), Error> {
    let config_file_path = get_external_config_file();
    if !config_file_path.exists() {
        write_external_config(&SparesExternalConfig::default())?;
    }
    let file_contents = read_to_string(&config_file_path).map_err(|e| Error::Io {
        description: format!("Failed to read {}.", config_file_path.display()),
        source: e,
    })?;
    let new_contents = set_fsrs_parameters(&file_contents, parameters)?;
    write(&config_file_path, new_contents).map_err(|e| Error::Io {
        description: "Failed to write config".to_string(),
        source: e,
    })?;
    Ok(())
}

fn set_fsrs_parameters(file_contents: &str, parameters: &[f32]) -> Result<String, Error> {
    let mut doc = file_contents
        .parse::<DocumentMut>()
        .map_err(|e| Error::Library(LibraryError::InvalidConfig(e.to_string())))?;
    // Through the shortest decimal that reads back as the same `f32`, so that `0.212` is not
    // written as `0.21199999749660492`.
    let array = parameters
        .iter()
        .map(|parameter| parameter.to_string().parse::<f64>().unwrap())
        .collect::<toml_edit::Array>();
    // Top-level keys must come before the first table, which `insert` takes care of.
    doc.insert("fsrs_parameters", toml_edit::value(array));
    Ok(doc.to_string())
}

// The `toml_edit` package was used in place of `confy` since `confy` does not support default values when serializing. For example, if a user had an existing config file and then `spares` was changed to add a new config key, deserialization would fail since a key was missing and not defaulted.
pub fn read_external_config() -> Result<SparesExternalConfig, Error> {
    let config_file_path = get_external_config_file();
    if !config_file_path.exists() {
        let config = SparesExternalConfig::default();
        write_external_config(&config)?;
        return Ok(config);
    }
    let file_contents = read_to_string(&config_file_path).map_err(|e| Error::Io {
        description: format!("Failed to read {}.", config_file_path.display()),
        source: e,
    })?;
    let doc = file_contents
        .parse::<DocumentMut>()
        .map_err(|e| Error::Library(LibraryError::InvalidConfig(e.to_string())))?;
    let mut config: SparesExternalConfig = toml_edit::de::from_document(doc)
        .map_err(|e| Error::Library(LibraryError::InvalidConfig(e.to_string())))?;
    let () = &mut config
        .validate()
        .map_err(|x| Error::Library(LibraryError::InvalidConfig(x)))?;
    Ok(config)
}

/// The `config.toml` for the sync server, made from this machine's (`contents`), so reviews and
/// rendering there use the same settings: the same, without the `[sync]` table, which only means
/// something on the machine that syncs, and with `render_browser_parsers = false`, since web
/// clients compile those parsers' cards.
pub fn server_config(contents: &str) -> Result<String, Error> {
    let mut doc = contents
        .parse::<DocumentMut>()
        .map_err(|e| Error::Library(LibraryError::InvalidConfig(e.to_string())))?;
    doc.remove("sync");
    doc.insert("render_browser_parsers", toml_edit::value(false));
    Ok(format!(
        "# Copied from the config.toml of the machine that syncs by `spares sync cloud`, which \
         overwrites\n# changes made here. Change that one instead.\n{doc}"
    ))
}

/// `contents` (a `config.toml`) with these keys of its `[sync]` table set, leaving the rest of the
/// file, comments included, as it is. The table is added if it is missing.
pub fn set_sync_values(
    contents: &str,
    values: impl IntoIterator<Item = (&'static str, toml_edit::Value)>,
) -> Result<String, Error> {
    let mut doc = contents
        .parse::<DocumentMut>()
        .map_err(|e| Error::Library(LibraryError::InvalidConfig(e.to_string())))?;
    let sync = doc
        .entry("sync")
        .or_insert_with(toml_edit::table)
        .as_table_like_mut()
        .ok_or_else(|| {
            Error::Library(LibraryError::InvalidConfig(
                "`sync` is not a table".to_string(),
            ))
        })?;
    for (key, value) in values {
        sync.insert(key, toml_edit::Item::Value(value));
    }
    Ok(doc.to_string())
}

pub(crate) fn write_external_config(config: &SparesExternalConfig) -> Result<(), Error> {
    let config_file_path = get_external_config_file();
    let config_string = toml_edit::ser::to_string_pretty(&config).map_err(|e| {
        Error::Library(LibraryError::InvalidConfig(format!(
            "Failed to serialize config: {}",
            e
        )))
    })?;
    write(&config_file_path, config_string).map_err(|e| Error::Io {
        description: "Failed to write config".to_string(),
        source: e,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml: &str) -> SparesExternalConfig {
        let mut config: SparesExternalConfig = toml_edit::de::from_str(toml).unwrap();
        config.validate().unwrap();
        config
    }

    #[test]
    fn server_config_keeps_settings_but_not_how_to_reach_the_server() {
        let laptop = "maximum_interval = 100 # days, roughly\n\n\
                      [sync]\nremote_host = \"me@vps\"\nurl = \"https://vps\"\n\
                      interval_minutes = 15\n\n\
                      [leech]\nlapses_threshold = 8\n";
        let server = server_config(laptop).unwrap();
        let config = parse(&server);
        assert_eq!(config.maximum_interval, Duration::seconds(100));
        assert_eq!(config.leech.lapses_threshold, 8);
        assert!(!config.render_browser_parsers);
        assert_eq!(config.sync.remote_host, None);
        assert_eq!(config.sync.url, None);
        assert_eq!(config.sync.interval_minutes, 0);
        assert!(!server.contains("[sync]"), "{server}");
        assert!(server.contains("# days, roughly"), "{server}");
        // A top-level key after a table would belong to the table.
        assert!(
            server.find("render_browser_parsers") < server.find("[leech]"),
            "{server}"
        );
        // An existing value is replaced.
        let server = server_config("render_browser_parsers = true\n").unwrap();
        assert!(!parse(&server).render_browser_parsers);
        assert_eq!(server.matches("render_browser_parsers").count(), 1);
    }

    #[test]
    fn set_sync_values_keeps_the_rest_of_the_file() {
        let values = || {
            [
                ("remote_host", "me@vps".into()),
                ("url", "https://vps".into()),
                ("interval_minutes", 15.into()),
            ]
        };
        let original = "maximum_interval = 100 # days, roughly\n\n\
                        [sync] # how to reach the server\nurl = \"https://old\" # replaced\n\
                        remote_rendered_dir = \"rendered\"\n\n[leech]\nlapses_threshold = 8\n";
        let written = set_sync_values(original, values()).unwrap();
        let config = parse(&written);
        assert_eq!(config.sync.remote_host.as_deref(), Some("me@vps"));
        assert_eq!(config.sync.url.as_deref(), Some("https://vps"));
        assert_eq!(config.sync.interval_minutes, 15);
        assert_eq!(config.sync.remote_rendered_dir, "rendered");
        assert_eq!(config.leech.lapses_threshold, 8);
        assert!(written.contains("# days, roughly"), "{written}");
        assert!(written.contains("# how to reach the server"), "{written}");
        assert_eq!(written.matches("url =").count(), 1, "{written}");
        assert!(
            written.find("interval_minutes") < written.find("[leech]"),
            "{written}"
        );

        // Without a [sync] table, or a file
        let written = set_sync_values("[leech]\nlapses_threshold = 8\n", values()).unwrap();
        assert_eq!(parse(&written).sync.url.as_deref(), Some("https://vps"));
        assert_eq!(parse(&written).leech.lapses_threshold, 8);
        let written = set_sync_values("", values()).unwrap();
        assert_eq!(parse(&written).sync.interval_minutes, 15);
    }

    #[test]
    fn placement_defaults_to_enabled() {
        let config = parse("");
        assert!(config.load_balance);
        assert!(config.disperse_siblings);
    }

    #[test]
    fn deprecated_easy_days_enabled_turns_off_load_balancing() {
        let config = parse("[easy_days]\nenabled = false\n");
        assert!(!config.load_balance);
        assert!(config.disperse_siblings, "it never controlled dispersal");

        assert!(parse("[easy_days]\nenabled = true\n").load_balance);
    }

    #[test]
    fn deprecated_easy_days_enabled_is_not_written_back() {
        let config = parse("[easy_days]\nenabled = false\n");
        let written = toml_edit::ser::to_string_pretty(&config).unwrap();
        assert!(!written.contains("enabled"), "{written}");
        assert!(written.contains("load_balance = false"), "{written}");
    }

    #[test]
    fn learning_steps_are_validated() {
        let steps = parse("learning_steps = [30, 600, 3600]\nrelearning_steps = []\n");
        assert_eq!(steps.learning_steps[1], Duration::minutes(10));
        assert_eq!(steps.relearning_steps, Vec::<Duration>::new());
        for invalid in [
            "learning_steps = [0]",
            "learning_steps = [86400]",
            "learning_steps = [600, 60]",
            "relearning_steps = [600, 600]",
        ] {
            let mut config: SparesExternalConfig = toml_edit::de::from_str(invalid).unwrap();
            assert!(config.validate().is_err(), "{invalid} must be rejected");
        }
    }

    #[test]
    fn fsrs_parameters_are_validated() {
        assert_eq!(parse("").fsrs_parameters, Vec::<f32>::new());
        let defaults = fsrs_rs::DEFAULT_PARAMETERS
            .iter()
            .map(|parameter| parameter.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(
            parse(&format!("fsrs_parameters = [{defaults}]"))
                .fsrs_parameters
                .len(),
            21
        );
        for invalid in ["fsrs_parameters = [1.0, 2.0]", "fsrs_parameters = [nan]"] {
            let mut config: SparesExternalConfig = toml_edit::de::from_str(invalid).unwrap();
            assert!(config.validate().is_err(), "{invalid} must be rejected");
        }
    }

    #[test]
    fn setting_fsrs_parameters_keeps_the_rest_of_the_file() {
        let original = indoc::indoc! {r"
            # My settings
            load_balance = false # keep this

            [leech]
            lapses_threshold = 4
        "};
        let parameters = fsrs_rs::DEFAULT_PARAMETERS.to_vec();
        let written = set_fsrs_parameters(original, &parameters).unwrap();
        assert!(written.contains("# My settings"), "{written}");
        assert!(written.contains("# keep this"), "{written}");
        let config = parse(&written);
        assert!(!config.load_balance);
        assert_eq!(config.leech.lapses_threshold, 4);
        assert_eq!(config.fsrs_parameters, parameters);

        // Setting them again replaces them rather than adding a second key.
        let rewritten = set_fsrs_parameters(&written, &parameters[..19]).unwrap();
        assert_eq!(parse(&rewritten).fsrs_parameters, parameters[..19]);
    }

    #[test]
    fn leech_action_repeats_every_half_threshold() {
        let leech = LeechConfig::default();
        let acted_on = (1..=20)
            .filter(|&lapses| leech.acts_on(lapses))
            .collect::<Vec<_>>();
        assert_eq!(acted_on, [8, 12, 16, 20]);

        let every_lapse = LeechConfig {
            lapses_threshold: 1,
            ..LeechConfig::default()
        };
        assert!((1..=3).all(|lapses| every_lapse.acts_on(lapses)));

        let none = LeechConfig {
            action: LeechAction::None,
            ..LeechConfig::default()
        };
        assert!(!none.acts_on(8));
    }

    /// The example on [`LeechConfig`].
    #[test]
    fn documented_leech_example_parses() {
        let config = parse(indoc::indoc! {r#"
            [leech]
            lapses_threshold = 8
            action = "suspend"
            tag_name = "leech"
        "#});
        assert_eq!(config.leech.action, LeechAction::Suspend);
        assert_eq!(config.leech.tag_name, "leech");
    }

    /// The example on [`StatisticsConfig`].
    #[test]
    fn documented_statistics_example_parses() {
        let config = parse(indoc::indoc! {r"
            [statistics]
            mature_threshold_days = 7
        "});
        assert_eq!(config.statistics.mature_threshold_days, 7);
        assert_eq!(parse("").statistics.mature_threshold_days, 21);
    }

    /// The example on [`QueueConfig`].
    #[test]
    fn documented_queue_example_parses() {
        let config = parse(indoc::indoc! {r#"
            [queue]
            review_order = "retrievability"
            new_order = "random_note"
            new_review_mix = "interleaved"
            learn_ahead_limit = 1200
        "#});
        assert_eq!(config.queue.review_order, ReviewOrder::Retrievability);
        assert_eq!(config.queue.new_order, NewCardOrder::RandomNote);
        assert_eq!(config.queue.new_review_mix, NewReviewMix::Interleaved);
        assert_eq!(config.queue.learn_ahead_limit, Duration::minutes(20));
    }

    /// The example in `docs/src/concepts.md`.
    #[test]
    fn documented_scheduling_example_parses() {
        let config = parse(indoc::indoc! {r#"
            learning_steps = [60, 600]  # 1 and 10 minutes
            relearning_steps = [600]    # 10 minutes
            load_balance = true
            disperse_siblings = true
            fsrs_parameters = [0.212, 1.2931, 2.3065, 8.2956, 6.4133, 0.8334, 3.0194, 0.001, 1.8722, 0.1666, 0.796, 1.4835, 0.0614, 0.2629, 1.6483, 0.6014, 1.8729, 0.5425, 0.0912, 0.0658, 0.1542]

            [easy_days]
            days_to_workload_percentage = { Mon = 1.0, Tue = 1.0, Wed = 1.0, Thu = 1.0, Fri = 1.0, Sat = 1.0, Sun = 0.5 }
            specific_dates = ["2026-12-25"]
        "#});
        assert_eq!(
            config.easy_days.specific_dates,
            [NaiveDate::from_ymd_opt(2026, 12, 25).unwrap()].into()
        );
        assert_eq!(config.fsrs_parameters.len(), 21);
        let workload = &config.easy_days.days_to_workload_percentage;
        assert!((workload[&Weekday::Sun] * 2.0 - workload[&Weekday::Mon]).abs() < 1e-9);
    }
}
