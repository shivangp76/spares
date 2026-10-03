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

#[derive(Debug, Deserialize, Serialize)]
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
    /// Delays between the same-day reviews of a new card before it graduates to review, in
    /// seconds. Each must be shorter than a day, in increasing order. Empty to graduate on the
    /// first answer.
    #[serde_as(as = "Vec<serde_with::DurationSeconds<i64>>")]
    pub learning_steps: Vec<Duration>,
    /// Like [`Self::learning_steps`], for a card in review that was forgotten. Empty to keep it
    /// in review with a short interval instead.
    #[serde_as(as = "Vec<serde_with::DurationSeconds<i64>>")]
    pub relearning_steps: Vec<Duration>,
    /// The FSRS model's parameters: 21 for FSRS-6, or 19 or 17 from FSRS-5 or FSRS-4.5. Empty to
    /// use the FSRS-6 defaults. `spares card optimize --apply` sets these from your review
    /// history.
    pub fsrs_parameters: Vec<f32>,
    /// Spread reviews so that each day has a similar number of cards due, in proportion to its
    /// easy-day workload percentage.
    pub load_balance: bool,
    /// Avoid scheduling cards of the same note on the same or nearby days, so that reviewing one
    /// does not give away the answer to another.
    pub disperse_siblings: bool,
    pub easy_days: EasyDaysConfig,
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
            learning_steps: vec![Duration::minutes(1), Duration::minutes(10)],
            relearning_steps: vec![Duration::minutes(10)],
            fsrs_parameters: Vec::new(),
            load_balance: true,
            disperse_siblings: true,
            easy_days: EasyDaysConfig::default(),
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
    pub(crate) fn validate(&mut self) -> Result<(), String> {
        for (name, steps) in [
            ("learning_steps", &self.learning_steps),
            ("relearning_steps", &self.relearning_steps),
        ] {
            if steps
                .iter()
                .any(|step| *step <= Duration::zero() || *step >= Duration::days(1))
            {
                return Err(format!(
                    "Each of `{name}` must be longer than 0 seconds and shorter than a day. \
                     Longer intervals are FSRS's to choose."
                ));
            }
            if !steps.is_sorted_by(|a, b| a < b) {
                return Err(format!("`{name}` must be in increasing order."));
            }
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
