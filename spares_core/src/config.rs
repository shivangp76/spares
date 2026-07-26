use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::create_dir_all;
use std::fs::read_to_string;
use std::fs::write;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Duration;
use chrono::NaiveDate;
use chrono::Utc;
use chrono::Weekday;
use etcetera::AppStrategy;
use etcetera::AppStrategyArgs;
use etcetera::choose_app_strategy;
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

const SPARES: &str = "spares";

#[allow(clippy::missing_panics_doc)]
pub fn get_config_dir() -> PathBuf {
    if cfg!(test) {
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
    if cfg!(test) {
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

#[allow(clippy::missing_panics_doc)]
pub fn get_data_dir() -> PathBuf {
    if cfg!(test) {
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

#[serde_with::serde_as]
#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct SparesInternalConfig {
    pub(crate) last_unburied: DateTime<Utc>,
    pub(crate) linked_notes_generated: bool,
    // #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    // pub fuzz_range: Duration,
    // #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    // pub reschedule_range: Duration,
}

impl Default for SparesInternalConfig {
    fn default() -> Self {
        Self {
            last_unburied: DateTime::<Utc>::MIN_UTC,
            linked_notes_generated: false,
            // fuzz_range: Duration::days(4),
            // reschedule_range: Duration::weeks(1),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct EasyDaysConfig {
    /// With default settings for [`self.days_to_workload_percentage`], this makes your workload (reviews per day) more consistent.
    // Equivalent to `fsrs4anki-helper`'s `load_balance` option
    pub enabled: bool,
    // Always enabled, unlike `fsrs4anki-helper`'s `auto_easy_days`. Can be disabled by setting `days` and `specific_dates` to default values.
    /// Mapping from days of the week to a percentage describing their workload.
    /// Between 0% (0.0) and 100% (1.0). For example, if this is 0.2, then 20% of cards will be scheduled on that day, relative to normal days (which are set to 1.0).
    ///
    /// Note that if all each percentage is treated relative to the rest. For example, if all days are set to 0.1, then each day will be treated normally , since 0.1/(0.1 * 7) = 1/7, so each day will have 1/7 of the workload which is the default behavior.
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
            enabled: true,
            days_to_workload_percentage,
            specific_dates: HashSet::default(),
        }
    }
}

// #[derive(Debug, Serialize, Deserialize)]
// #[serde(default)]
// pub struct DisperseSiblingsConfig {
//     // NOTE: There is no upside to disabling this option, so therefore it is not provided.
//     // This alleviates interference between siblings. Disabling it will only decrease the efficiency of spaced repetition.
//     // Replaces `fsrs4anki-helper`'s `auto_disperse_when_review`.
//     // pub auto_after_review: bool,
//     // Note that this breaks load balancing.
//     // Replaces `fsrs4anki-helper`'s `auto_disperse_after_reschedule`.
//     // pub auto_after_reschedule: bool,
// }
//
// impl Default for DisperseSiblingsConfig {
//     fn default() -> Self {
//         Self {
//             // auto_after_review: true,
//             auto_after_reschedule: false,
//         }
//     }
// }

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct LeechConfig {
    // pub auto_tag: bool,
    // pub tag_name: String,
    /// The number of lapses after which to tag a card as a leech.
    pub lapses_threshold: u32,
}

impl Default for LeechConfig {
    fn default() -> Self {
        Self {
            // auto_tag: true,
            // tag_name: "leech".to_string(),
            lapses_threshold: 8,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct ParserConfig {
    pub markdown: MarkdownParserConfig,
}

#[serde_with::serde_as]
#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct SparesExternalConfig {
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    pub maximum_interval: Duration,
    /// Cards due in less than this duration will not be rescheduled to respect their desired retention.
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    pub minimum_interval: Duration,
    pub new_cards_daily_limit: u32,
    pub flagged_tag_name: String,
    pub easy_days: EasyDaysConfig,
    // pub disperse_siblings: DisperseSiblingsConfig,
    pub leech: LeechConfig,
    pub parser: ParserConfig,
    pub image_occlusion: ImageOcclusionConfig,
    pub overlapper: OverlapperConfig,
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    pub set_card_due_date_duration: Duration,
    pub remote_host: Option<String>,
}

impl Default for SparesExternalConfig {
    fn default() -> Self {
        Self {
            maximum_interval: Duration::days(180),
            minimum_interval: Duration::days(2),
            new_cards_daily_limit: 20,
            flagged_tag_name: "flagged".to_string(),
            easy_days: EasyDaysConfig::default(),
            // disperse_siblings: DisperseSiblingsConfig::default(),
            leech: LeechConfig::default(),
            parser: ParserConfig::default(),
            image_occlusion: ImageOcclusionConfig::default(),
            overlapper: OverlapperConfig::default(),
            set_card_due_date_duration: Duration::weeks(1),
            remote_host: None,
        }
    }
}

impl SparesExternalConfig {
    fn validate(&mut self) -> Result<(), String> {
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

fn get_external_config_file() -> PathBuf {
    let mut config_file_path = get_config_dir();
    config_file_path.push("config.toml");
    config_file_path
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
