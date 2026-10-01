//! Settings from `config.toml` and the environment: bind address, data dir, launch date, track ID, store backend.
//!
//! The file is the base and the environment wins: `GTS_BIND`, `GTS_TRACK_ID`
//! and `GTS_SECRET` override or add to what the file says. Keys the server
//! does not know (`playlists`, until the daily pick is built) are ignored.

use std::{fmt, path::PathBuf};

use anyhow::Context;
use jiff::civil::Date;
use serde::Deserialize;

/// Where the settings are read from unless `GTS_CONFIG` names another file.
/// Relative to the working directory, which the `just` recipes make the repo root.
const DEFAULT_PATH: &str = "config.toml";

const ENV_CONFIG: &str = "GTS_CONFIG";
const ENV_BIND: &str = "GTS_BIND";
const ENV_TRACK_ID: &str = "GTS_TRACK_ID";
const ENV_SECRET: &str = "GTS_SECRET";

/// Which backend keeps the persistent data: `[store] kind` in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StoreKind {
    /// `"sqlite"`: one database file under the data directory. The default.
    #[default]
    Sqlite,
    /// `"memory"`: nothing is kept; every start begins with the seed songs.
    Memory,
}

impl StoreKind {
    /// The kind `[store] kind` names, if it is one.
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "sqlite" => Some(Self::Sqlite),
            "memory" => Some(Self::Memory),
            _ => None,
        }
    }
}

/// Everything the server needs to start.
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    /// Address to listen on, `host:port`.
    pub bind: String,
    /// Directory for cached previews, the cookie key and the database.
    /// Created on demand.
    pub data_dir: PathBuf,
    /// Day 1 of the game, a UTC date.
    pub launch_date: Date,
    /// Deezer ID of the song the server plays.
    pub track_id: u64,
    /// The backend that keeps the song pool.
    pub store: StoreKind,
    /// `GTS_SECRET` when set: the cookie key (see `routes::session_key` for
    /// the format). Only ever taken from the environment, because
    /// `config.toml` is committed.
    pub secret: Option<String>,
}

// Written by hand so the secret cannot end up in a log line.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("data_dir", &self.data_dir)
            .field("launch_date", &self.launch_date)
            .field("track_id", &self.track_id)
            .field("store", &self.store)
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// A setting that is missing or cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0}")]
    Toml(#[from] toml::de::Error),
    #[error("{var}={value:?} is not valid: {reason}")]
    Env {
        var: &'static str,
        value: String,
        reason: String,
    },
    #[error("`track_id` is not set: add it to the config file or set {ENV_TRACK_ID}")]
    MissingTrackId,
    #[error("`track_id` must be a Deezer track ID greater than zero")]
    ZeroTrackId,
    #[error("`bind` is empty: expected an address such as 127.0.0.1:4810")]
    EmptyBind,
    #[error("`store.kind` is {0:?}: expected \"sqlite\" or \"memory\"")]
    UnknownStoreKind(String),
}

/// `config.toml` as written. Everything but the launch date can be left out.
#[derive(Deserialize)]
struct FileConfig {
    #[serde(default = "default_bind")]
    bind: String,
    #[serde(default = "default_data_dir")]
    data_dir: PathBuf,
    launch_date: Date,
    track_id: Option<u64>,
    #[serde(default)]
    store: FileStore,
}

/// The `[store]` table. Without it, or without `kind`, the store is SQLite.
#[derive(Default, Deserialize)]
struct FileStore {
    kind: Option<String>,
}

fn default_bind() -> String {
    "127.0.0.1:4810".to_owned()
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("data")
}

impl Config {
    /// Reads the config file and applies the process environment on top.
    pub fn load() -> anyhow::Result<Self> {
        let path = env_var(ENV_CONFIG).unwrap_or_else(|| DEFAULT_PATH.to_owned());
        let text = std::fs::read_to_string(&path).with_context(|| {
            format!("reading the config file {path:?} (set {ENV_CONFIG} to use another path)")
        })?;
        Self::from_sources(&text, env_var).with_context(|| format!("loading the config {path:?}"))
    }

    /// Builds the config from the file's text and an environment lookup.
    ///
    /// The lookup is passed in so the precedence rules can be tested without
    /// touching the process environment. A variable that is unset or blank
    /// leaves the file's value alone.
    pub fn from_sources(
        toml_text: &str,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let env = |var: &str| env(var).filter(|value| !value.trim().is_empty());
        let file: FileConfig = toml::from_str(toml_text)?;

        let track_id = match env(ENV_TRACK_ID) {
            Some(value) => value.trim().parse().map_err(|error| ConfigError::Env {
                var: ENV_TRACK_ID,
                reason: format!("expected a Deezer track ID ({error})"),
                value,
            })?,
            None => file.track_id.ok_or(ConfigError::MissingTrackId)?,
        };
        if track_id == 0 {
            return Err(ConfigError::ZeroTrackId);
        }

        let bind = env(ENV_BIND).unwrap_or(file.bind).trim().to_owned();
        if bind.is_empty() {
            return Err(ConfigError::EmptyBind);
        }

        let store = match file.store.kind {
            Some(kind) => StoreKind::from_name(&kind).ok_or(ConfigError::UnknownStoreKind(kind))?,
            None => StoreKind::default(),
        };

        Ok(Self {
            bind,
            data_dir: file.data_dir,
            launch_date: file.launch_date,
            track_id,
            store,
            secret: env(ENV_SECRET),
        })
    }
}

/// A process environment variable, if set to valid Unicode.
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    /// The shape of the real `config.toml`, `playlists` included.
    const FILE: &str = r#"
        bind = "127.0.0.1:4810"
        data_dir = "data"
        launch_date = "2026-10-01"
        track_id = 136889400
        playlists = [
          11535307124, # a comment
          5123717724,
        ]

        [store]
        kind = "sqlite"
    "#;

    /// The smallest file that loads.
    const MINIMAL: &str = "launch_date = \"2026-10-01\"\ntrack_id = 7\n";

    fn no_env(_: &str) -> Option<String> {
        None
    }

    /// An environment holding exactly `vars`.
    fn env_of(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            vars.iter()
                .find(|(var, _)| *var == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn reads_the_file_and_ignores_unknown_keys() {
        let config = Config::from_sources(FILE, no_env).unwrap();
        assert_eq!(
            config,
            Config {
                bind: "127.0.0.1:4810".to_owned(),
                data_dir: PathBuf::from("data"),
                launch_date: date(2026, 10, 1),
                track_id: 136_889_400,
                store: StoreKind::Sqlite,
                secret: None,
            }
        );
    }

    #[test]
    fn the_repo_config_file_loads() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../config.toml"))
            .unwrap();
        let config = Config::from_sources(&text, no_env).unwrap();
        assert_eq!(config.bind, "127.0.0.1:4810");
        assert!(config.track_id > 0);
        assert_eq!(config.store, StoreKind::Sqlite);
    }

    #[test]
    fn bind_and_data_dir_have_defaults() {
        let config =
            Config::from_sources("launch_date = \"2026-10-01\"\ntrack_id = 7", no_env).unwrap();
        assert_eq!(config.bind, "127.0.0.1:4810");
        assert_eq!(config.data_dir, PathBuf::from("data"));
        assert_eq!(config.track_id, 7);
    }

    #[test]
    fn environment_wins_over_the_file() {
        let env = env_of(&[
            ("GTS_BIND", "0.0.0.0:9999"),
            ("GTS_TRACK_ID", " 3135556 "),
            ("GTS_SECRET", "abc"),
        ]);
        let config = Config::from_sources(FILE, env).unwrap();
        assert_eq!(config.bind, "0.0.0.0:9999");
        assert_eq!(config.track_id, 3_135_556);
        assert_eq!(config.secret.as_deref(), Some("abc"));
        // What the environment does not mention still comes from the file.
        assert_eq!(config.launch_date, date(2026, 10, 1));
        assert_eq!(config.data_dir, PathBuf::from("data"));
    }

    #[test]
    fn blank_variables_count_as_unset() {
        let env = env_of(&[("GTS_BIND", ""), ("GTS_TRACK_ID", "  "), ("GTS_SECRET", "")]);
        let config = Config::from_sources(FILE, env).unwrap();
        assert_eq!(config.bind, "127.0.0.1:4810");
        assert_eq!(config.track_id, 136_889_400);
        assert_eq!(config.secret, None);
    }

    #[test]
    fn track_id_may_come_from_the_environment_alone() {
        let file = "launch_date = \"2026-10-01\"";
        assert!(matches!(
            Config::from_sources(file, no_env),
            Err(ConfigError::MissingTrackId)
        ));
        let config = Config::from_sources(file, env_of(&[("GTS_TRACK_ID", "42")])).unwrap();
        assert_eq!(config.track_id, 42);
    }

    #[test]
    fn a_bad_track_id_variable_is_named_in_the_error() {
        let error = Config::from_sources(FILE, env_of(&[("GTS_TRACK_ID", "starboy")])).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("GTS_TRACK_ID"), "{message}");
        assert!(message.contains("starboy"), "{message}");
    }

    #[test]
    fn a_zero_track_id_is_rejected() {
        assert!(matches!(
            Config::from_sources(FILE, env_of(&[("GTS_TRACK_ID", "0")])),
            Err(ConfigError::ZeroTrackId)
        ));
        assert!(matches!(
            Config::from_sources("launch_date = \"2026-10-01\"\ntrack_id = 0", no_env),
            Err(ConfigError::ZeroTrackId)
        ));
    }

    #[test]
    fn a_missing_or_malformed_launch_date_names_the_key() {
        let missing = Config::from_sources("track_id = 1", no_env).unwrap_err();
        assert!(missing.to_string().contains("launch_date"), "{missing}");

        let malformed =
            Config::from_sources("launch_date = \"1 Oct 2026\"\ntrack_id = 1", no_env).unwrap_err();
        assert!(malformed.to_string().contains("launch_date"), "{malformed}");
    }

    #[test]
    fn a_wrongly_typed_value_is_an_error() {
        let file = "launch_date = \"2026-10-01\"\ntrack_id = \"abc\"";
        assert!(matches!(
            Config::from_sources(file, no_env),
            Err(ConfigError::Toml(_))
        ));
        assert!(matches!(
            Config::from_sources("launch_date = ", no_env),
            Err(ConfigError::Toml(_))
        ));
    }

    #[test]
    fn the_store_is_sqlite_unless_the_file_says_otherwise() {
        // No `[store]` table, an empty one, and one that names the default.
        for file in [
            MINIMAL.to_owned(),
            format!("{MINIMAL}[store]\n"),
            format!("{MINIMAL}[store]\nkind = \"sqlite\"\n"),
        ] {
            let config = Config::from_sources(&file, no_env).unwrap();
            assert_eq!(config.store, StoreKind::Sqlite, "{file}");
        }

        let file = format!("{MINIMAL}[store]\nkind = \"memory\"\n");
        let config = Config::from_sources(&file, no_env).unwrap();
        assert_eq!(config.store, StoreKind::Memory);
    }

    #[test]
    fn an_unknown_store_kind_names_the_key_and_the_value() {
        for kind in ["postgres", "SQLite", ""] {
            let file = format!("{MINIMAL}[store]\nkind = \"{kind}\"\n");
            let error = Config::from_sources(&file, no_env).unwrap_err();
            assert!(
                matches!(&error, ConfigError::UnknownStoreKind(found) if found == kind),
                "{error}"
            );
            let message = error.to_string();
            assert!(message.contains("store.kind"), "{message}");
            assert!(message.contains(&format!("{kind:?}")), "{message}");
            assert!(message.contains("\"sqlite\" or \"memory\""), "{message}");
        }
    }

    #[test]
    fn a_store_setting_of_the_wrong_type_is_an_error() {
        for (store, key) in [
            ("[store]\nkind = 3\n", "kind"),
            ("store = \"sqlite\"\n", "store"),
        ] {
            let file = format!("{MINIMAL}{store}");
            let error = Config::from_sources(&file, no_env).unwrap_err();
            assert!(matches!(error, ConfigError::Toml(_)), "{error}");
            assert!(error.to_string().contains(key), "{error}");
        }
    }

    #[test]
    fn debug_output_hides_the_secret() {
        let config = Config::from_sources(FILE, env_of(&[("GTS_SECRET", "hunter2")])).unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(debug.contains("redacted"), "{debug}");
    }
}
