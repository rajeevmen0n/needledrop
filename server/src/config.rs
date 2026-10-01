//! Settings from `config.toml` and the environment: bind address, data dir, launch date, public address, store backend.
//!
//! Three layers, the later winning over the earlier: `config.toml`, which is
//! committed and holds what every machine shares; `config.local.toml` next to
//! it, which is git-ignored and holds what belongs to one machine (the public
//! hostname, above all); and the environment, where `GTS_BIND`,
//! `GTS_DATA_DIR`, `GTS_PUBLIC_URL`, `GTS_STORE_PATH` and `GTS_SECRET` override or add to
//! what the files say.
//! Keys the server does not know are
//! ignored, which is what lets a config file from before the song database
//! (with its `track_id` and `playlists`) still start the server.

use std::{
    fmt, io,
    path::{Path, PathBuf},
};

use anyhow::Context;
use jiff::civil::Date;
use serde::Deserialize;

/// Where the settings are read from unless `GTS_CONFIG` names another file.
/// Relative to the working directory, which the `just` recipes make the repo root.
const DEFAULT_PATH: &str = "config.toml";

const ENV_CONFIG: &str = "GTS_CONFIG";
const ENV_BIND: &str = "GTS_BIND";
const ENV_DATA_DIR: &str = "GTS_DATA_DIR";
const ENV_PUBLIC_URL: &str = "GTS_PUBLIC_URL";
const ENV_STORE_PATH: &str = "GTS_STORE_PATH";

/// The database file of the SQLite backend when no path is configured, under
/// the data directory.
const DATABASE_FILE: &str = "needledrop.db";
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
    /// Directory for cached previews, the cookie key and, unless a separate
    /// store path is configured, the database. `GTS_DATA_DIR` overrides
    /// `data_dir` in the files. Created on demand.
    pub data_dir: PathBuf,
    /// Day 1 of the game, a UTC date.
    pub launch_date: Date,
    /// The address players reach the game under, when it has one beyond
    /// loopback: scheme and host, with a port if it is not the default, and
    /// no trailing slash (`https://needledrop.example`). This is the one
    /// place a deployment's hostname is written down. The server puts it in
    /// the user agent it shows Deezer; the Vite dev server reads the same
    /// setting to know which host to answer and where hot reload connects.
    pub public_url: Option<String>,
    /// The backend that keeps the song pool, the games, the picks and the
    /// day offset.
    pub store: StoreKind,
    /// The database file of the SQLite backend: `[store] path` or
    /// `GTS_STORE_PATH` when one is set, and otherwise `needledrop.db` under
    /// the effective data directory. The server creates and seeds it on its
    /// first start.
    /// Relative paths are relative to the working directory, like
    /// `data_dir`. The in-memory backend does not use it.
    pub store_path: PathBuf,
    /// `GTS_SECRET` when set: the cookie key (see `player::session_key` for
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
            .field("public_url", &self.public_url)
            .field("store", &self.store)
            .field("store_path", &self.store_path)
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// A setting that is missing or cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0}")]
    Toml(#[from] toml::de::Error),
    #[error("in the local settings file: {0}")]
    LocalToml(#[source] toml::de::Error),
    #[error("`bind` is empty: expected an address such as 127.0.0.1:4810")]
    EmptyBind,
    #[error("`store.kind` is {0:?}: expected \"sqlite\" or \"memory\"")]
    UnknownStoreKind(String),
    #[error(
        "{setting} is {value:?}: expected the address the game is served under, \
         such as https://needledrop.example ({reason})"
    )]
    PublicUrl {
        /// `` `public_url` `` or the variable that overrode it.
        setting: &'static str,
        value: String,
        reason: &'static str,
    },
}

/// `config.toml` as written. Everything but the launch date can be left out.
#[derive(Deserialize)]
struct FileConfig {
    #[serde(default = "default_bind")]
    bind: String,
    #[serde(default = "default_data_dir")]
    data_dir: PathBuf,
    launch_date: Date,
    public_url: Option<String>,
    #[serde(default)]
    store: FileStore,
}

/// The `[store]` table. Without it, or without `kind`, the store is SQLite;
/// without `path`, its file is under the data directory.
#[derive(Default, Deserialize)]
struct FileStore {
    kind: Option<String>,
    path: Option<PathBuf>,
}

fn default_bind() -> String {
    "127.0.0.1:4810".to_owned()
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("data")
}

impl Config {
    /// Reads the config file, the local settings file next to it if there is
    /// one, and applies the process environment on top.
    pub fn load() -> anyhow::Result<Self> {
        let path = env_var(ENV_CONFIG).unwrap_or_else(|| DEFAULT_PATH.to_owned());
        let text = std::fs::read_to_string(&path).with_context(|| {
            format!("reading the config file {path:?} (set {ENV_CONFIG} to use another path)")
        })?;
        let local_path = local_path(Path::new(&path));
        let local = match std::fs::read_to_string(&local_path) {
            Ok(local) => Some(local),
            // Most machines have none: the committed file is the whole config.
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("reading the local settings file {}", local_path.display())
                });
            }
        };
        Self::from_layers(&text, local.as_deref(), env_var)
            .with_context(|| format!("loading the config {path:?} with {}", local_path.display()))
    }

    /// Builds the config from one file's text and an environment lookup:
    /// [`from_layers`](Self::from_layers) without a local settings file.
    #[cfg(test)]
    pub fn from_sources(
        toml_text: &str,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        Self::from_layers(toml_text, None, env)
    }

    /// Builds the config from the committed file's text, the local settings
    /// file's text if there is one, and an environment lookup.
    ///
    /// A key the local file sets replaces the same key of the committed file;
    /// a table (`[store]`) is merged key by key. The lookup is passed in so
    /// the precedence rules can be tested without touching the process
    /// environment. A variable that is unset or blank leaves the files' value
    /// alone.
    pub fn from_layers(
        toml_text: &str,
        local_toml_text: Option<&str>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let env = |var: &str| env(var).filter(|value| !value.trim().is_empty());
        let file: FileConfig = match local_toml_text {
            // The common case keeps the parser's own errors, which point at
            // the line.
            None => toml::from_str(toml_text)?,
            Some(local) => {
                let mut merged: toml::Table = toml::from_str(toml_text)?;
                let local: toml::Table = toml::from_str(local).map_err(ConfigError::LocalToml)?;
                overlay(&mut merged, local);
                merged.try_into()?
            }
        };

        let bind = env(ENV_BIND).unwrap_or(file.bind).trim().to_owned();
        if bind.is_empty() {
            return Err(ConfigError::EmptyBind);
        }
        let data_dir = env(ENV_DATA_DIR)
            .map(|path| PathBuf::from(path.trim()))
            .unwrap_or(file.data_dir);

        // Blank in the file means the same as leaving the key out: a server
        // that is only reached on loopback.
        let public_url = match env(ENV_PUBLIC_URL) {
            Some(value) => Some(public_url(ENV_PUBLIC_URL, &value)?),
            None => file
                .public_url
                .filter(|value| !value.trim().is_empty())
                .map(|value| public_url("`public_url`", &value))
                .transpose()?,
        };

        let store = match file.store.kind {
            Some(kind) => StoreKind::from_name(&kind).ok_or(ConfigError::UnknownStoreKind(kind))?,
            None => StoreKind::default(),
        };

        // Blank means the same as leaving it out, in the file as in the
        // environment: the database the server makes under the data directory.
        let store_path = env(ENV_STORE_PATH)
            .map(|path| PathBuf::from(path.trim()))
            .or(file
                .store
                .path
                .filter(|path| !path.as_os_str().to_string_lossy().trim().is_empty()))
            .unwrap_or_else(|| data_dir.join(DATABASE_FILE));

        Ok(Self {
            bind,
            data_dir,
            launch_date: file.launch_date,
            public_url,
            store,
            store_path,
            secret: env(ENV_SECRET),
        })
    }
}

/// Where the local settings that go with the config file at `path` are:
/// `config.toml` → `config.local.toml`, in the same directory. The Vite dev
/// server works the name out the same way (`web/dev-server.ts`).
fn local_path(path: &Path) -> PathBuf {
    path.with_extension("local.toml")
}

/// Lays `over` on top of `base`: a key of `over` replaces the same key of
/// `base`, except that two tables are merged key by key, so a local file can
/// change `[store] kind` without repeating the rest of the table.
fn overlay(base: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(below)), toml::Value::Table(above)) => overlay(below, above),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// Checks a public address and writes it the one way it is used: scheme,
/// host and, when it is not the scheme's default, the port; nothing after.
///
/// Anything else is refused rather than trimmed. An address with a path would
/// say the game lives under a prefix, which neither the server nor the client
/// supports, and a typo here should stop the server, not be guessed at.
fn public_url(setting: &'static str, value: &str) -> Result<String, ConfigError> {
    let refuse = |reason| ConfigError::PublicUrl {
        setting,
        value: value.to_owned(),
        reason,
    };
    let url = reqwest::Url::parse(value.trim())
        .map_err(|_| refuse("it is not a URL; it needs https:// or http:// in front"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(refuse("the scheme has to be https or http"));
    }
    let Some(host) = url.host_str() else {
        return Err(refuse("it has no host"));
    };
    if !url.username().is_empty() || url.password().is_some() {
        return Err(refuse("it must not carry a user name or password"));
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(refuse(
            "it must end after the host: no path, query or fragment",
        ));
    }
    Ok(match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    })
}

/// A process environment variable, if set to valid Unicode.
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    /// The shape of the real `config.toml`.
    const FILE: &str = r#"
        bind = "127.0.0.1:4810"
        data_dir = "data"
        launch_date = "2026-10-01"

        [store]
        kind = "sqlite"
    "#;

    /// A config file from before the song database: one hard-coded track and
    /// the playlists the daily song was once going to come from.
    const OLD_FILE: &str = r#"
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
    const MINIMAL: &str = "launch_date = \"2026-10-01\"\n";

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
    fn reads_the_file() {
        let config = Config::from_sources(FILE, no_env).unwrap();
        assert_eq!(
            config,
            Config {
                bind: "127.0.0.1:4810".to_owned(),
                data_dir: PathBuf::from("data"),
                launch_date: date(2026, 10, 1),
                public_url: None,
                store: StoreKind::Sqlite,
                store_path: PathBuf::from("data/needledrop.db"),
                secret: None,
            }
        );
    }

    #[test]
    fn the_public_address_is_optional_and_written_one_way() {
        // Left out or blank: a server reached on loopback only.
        for file in [
            MINIMAL.to_owned(),
            format!("{MINIMAL}public_url = \"\"\n"),
            format!("{MINIMAL}public_url = \"  \"\n"),
        ] {
            let config = Config::from_sources(&file, no_env).unwrap();
            assert_eq!(config.public_url, None, "{file}");
        }

        for (written, kept) in [
            ("https://needledrop.example", "https://needledrop.example"),
            ("https://needledrop.example/", "https://needledrop.example"),
            (
                "  https://Needledrop.Example  ",
                "https://needledrop.example",
            ),
            // The default port is not written; another one is kept.
            (
                "https://needledrop.example:443",
                "https://needledrop.example",
            ),
            (
                "https://needledrop.example:8443",
                "https://needledrop.example:8443",
            ),
            ("http://192.168.1.20:4811", "http://192.168.1.20:4811"),
        ] {
            let file = format!("{MINIMAL}public_url = \"{written}\"\n");
            let config = Config::from_sources(&file, no_env).unwrap();
            assert_eq!(config.public_url.as_deref(), Some(kept), "{written}");
        }
    }

    #[test]
    fn the_local_file_wins_over_the_committed_one_key_by_key() {
        let committed = r#"
            bind = "127.0.0.1:4810"
            data_dir = "data"
            launch_date = "2026-10-01"
            public_url = "https://committed.example"

            [store]
            kind = "sqlite"
        "#;

        // No local file, and an empty one, change nothing.
        let alone = Config::from_layers(committed, None, no_env).unwrap();
        assert_eq!(
            Config::from_layers(committed, Some(""), no_env).unwrap(),
            alone
        );
        assert_eq!(
            alone.public_url.as_deref(),
            Some("https://committed.example")
        );

        // What it sets replaces the committed value; the rest stays.
        let local = "public_url = \"https://this-machine.example\"\nbind = \"127.0.0.1:9999\"\n";
        let config = Config::from_layers(committed, Some(local), no_env).unwrap();
        assert_eq!(
            config.public_url.as_deref(),
            Some("https://this-machine.example")
        );
        assert_eq!(config.bind, "127.0.0.1:9999");
        assert_eq!(config.data_dir, PathBuf::from("data"));
        assert_eq!(config.launch_date, date(2026, 10, 1));
        assert_eq!(config.store, StoreKind::Sqlite);

        // A table is merged, not replaced.
        let local = "[store]\nkind = \"memory\"\n";
        let config = Config::from_layers(committed, Some(local), no_env).unwrap();
        assert_eq!(config.store, StoreKind::Memory);
        assert_eq!(
            config.public_url.as_deref(),
            Some("https://committed.example")
        );

        // A blank address in the local file says "none" on this machine.
        let config = Config::from_layers(committed, Some("public_url = \"\"\n"), no_env).unwrap();
        assert_eq!(config.public_url, None);

        // It can supply what the committed file leaves out.
        let config = Config::from_layers(
            "bind = \"127.0.0.1:4810\"\n",
            Some("launch_date = \"2026-10-01\"\n"),
            no_env,
        )
        .unwrap();
        assert_eq!(config.launch_date, date(2026, 10, 1));

        // And the environment wins over both files.
        let env = env_of(&[("GTS_PUBLIC_URL", "https://from-the-environment.example")]);
        let local = "public_url = \"https://this-machine.example\"\n";
        let config = Config::from_layers(committed, Some(local), env).unwrap();
        assert_eq!(
            config.public_url.as_deref(),
            Some("https://from-the-environment.example")
        );
    }

    #[test]
    fn a_mistake_in_the_local_file_is_an_error_that_says_which_file() {
        // Not TOML at all.
        let error = Config::from_layers(MINIMAL, Some("public_url = "), no_env).unwrap_err();
        assert!(matches!(error, ConfigError::LocalToml(_)), "{error}");
        assert!(error.to_string().contains("local settings file"), "{error}");

        // TOML, but not a usable setting: the same errors as in the committed file.
        let error =
            Config::from_layers(MINIMAL, Some("public_url = \"nowhere\"\n"), no_env).unwrap_err();
        assert!(matches!(error, ConfigError::PublicUrl { .. }), "{error}");
        let error = Config::from_layers(MINIMAL, Some("bind = 4810\n"), no_env).unwrap_err();
        assert!(matches!(error, ConfigError::Toml(_)), "{error}");
        assert!(error.to_string().contains("bind"), "{error}");
        let error = Config::from_layers(MINIMAL, Some("[store]\nkind = \"postgres\"\n"), no_env)
            .unwrap_err();
        assert!(matches!(error, ConfigError::UnknownStoreKind(_)), "{error}");
    }

    #[test]
    fn the_local_file_sits_next_to_the_config_file() {
        for (config, local) in [
            ("config.toml", "config.local.toml"),
            (
                "/etc/needledrop/prod.toml",
                "/etc/needledrop/prod.local.toml",
            ),
            ("/repo/settings.dev.toml", "/repo/settings.dev.local.toml"),
            ("/repo/settings.conf", "/repo/settings.local.toml"),
            ("/repo.d/settings", "/repo.d/settings.local.toml"),
        ] {
            assert_eq!(local_path(Path::new(config)), Path::new(local), "{config}");
        }
    }

    #[test]
    fn the_environment_names_the_public_address_over_the_file() {
        let file = format!("{MINIMAL}public_url = \"https://from-the-file.example\"\n");
        let env = env_of(&[("GTS_PUBLIC_URL", "https://from-the-environment.example/")]);
        let config = Config::from_sources(&file, env).unwrap();
        assert_eq!(
            config.public_url.as_deref(),
            Some("https://from-the-environment.example")
        );

        // The variable alone is enough, and a blank one leaves the file's.
        let env = env_of(&[("GTS_PUBLIC_URL", "http://localhost:4811")]);
        let config = Config::from_sources(MINIMAL, env).unwrap();
        assert_eq!(config.public_url.as_deref(), Some("http://localhost:4811"));
        let config = Config::from_sources(&file, env_of(&[("GTS_PUBLIC_URL", " ")])).unwrap();
        assert_eq!(
            config.public_url.as_deref(),
            Some("https://from-the-file.example")
        );
    }

    #[test]
    fn a_public_address_that_is_not_one_names_the_setting_and_the_value() {
        for value in [
            "needledrop.example",
            "ftp://needledrop.example",
            "https://",
            "https://needledrop.example/game",
            "https://needledrop.example/?x=1",
            "https://needledrop.example/#top",
            "https://user:secret@needledrop.example",
        ] {
            let file = format!("{MINIMAL}public_url = \"{value}\"\n");
            let error = Config::from_sources(&file, no_env).unwrap_err();
            assert!(matches!(error, ConfigError::PublicUrl { .. }), "{error}");
            let message = error.to_string();
            assert!(message.contains("`public_url`"), "{message}");
            assert!(message.contains(&format!("{value:?}")), "{message}");
        }

        let env = env_of(&[("GTS_PUBLIC_URL", "needledrop.example")]);
        let error = Config::from_sources(MINIMAL, env).unwrap_err();
        assert!(error.to_string().contains("GTS_PUBLIC_URL"), "{error}");
    }

    #[test]
    fn an_old_config_file_with_a_track_and_playlists_still_loads() {
        // The song now comes from the database; the keys that used to choose
        // it are unknown keys like any other, and are ignored whatever they
        // hold.
        let expected = Config::from_sources(FILE, no_env).unwrap();
        assert_eq!(Config::from_sources(OLD_FILE, no_env).unwrap(), expected);
        for leftover in ["track_id = 0", "track_id = \"abc\"", "playlists = 7"] {
            let file = format!("{MINIMAL}{leftover}\n");
            let config = Config::from_sources(&file, no_env).unwrap();
            assert_eq!(config.launch_date, date(2026, 10, 1), "{leftover}");
        }
        // The variable that used to override the track is not read either.
        let env = env_of(&[("GTS_TRACK_ID", "starboy")]);
        assert_eq!(Config::from_sources(OLD_FILE, env).unwrap(), expected);
    }

    #[test]
    fn the_repo_config_file_loads() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../config.toml"))
            .unwrap();
        let config = Config::from_sources(&text, no_env).unwrap();
        assert_eq!(config.bind, "127.0.0.1:4810");
        assert_eq!(config.store, StoreKind::Sqlite);
        // Run from the repo, the server uses the database it bootstraps
        // itself: the path is there, blank, for a deployment to fill in.
        assert!(text.contains("path = \"\""), "{text}");
        assert_eq!(config.store_path, config.data_dir.join("needledrop.db"));
        // The keys that went with the hard-coded track are gone from it.
        assert!(!text.contains("track_id"), "{text}");
        assert!(!text.contains("playlists"), "{text}");
        // It is committed, so it names no host: the key is there, blank, to
        // be filled in in a copy of the file, `config.local.toml`.
        assert!(text.contains("public_url = \"\""), "{text}");
        assert_eq!(config.public_url, None);

        // A copy of it with the address filled in is such a local file.
        let local = text.replace(
            "public_url = \"\"",
            "public_url = \"https://needledrop.example\"",
        );
        let config = Config::from_layers(&text, Some(&local), no_env).unwrap();
        assert_eq!(
            config.public_url.as_deref(),
            Some("https://needledrop.example")
        );
    }

    #[test]
    fn bind_and_data_dir_have_defaults() {
        let config = Config::from_sources(MINIMAL, no_env).unwrap();
        assert_eq!(config.bind, "127.0.0.1:4810");
        assert_eq!(config.data_dir, PathBuf::from("data"));
        assert_eq!(config.store_path, PathBuf::from("data/needledrop.db"));
        assert_eq!(config.launch_date, date(2026, 10, 1));
    }

    #[test]
    fn data_dir_follows_file_local_and_environment_precedence() {
        let committed = format!("{MINIMAL}data_dir = \"data\"\n[store]\npath = \"\"\n");
        let local = "data_dir = \"/srv/needledrop\"\n";

        let config = Config::from_layers(&committed, Some(local), no_env).unwrap();
        assert_eq!(config.data_dir, PathBuf::from("/srv/needledrop"));
        assert_eq!(config.store_path, config.data_dir.join("needledrop.db"));

        let config = Config::from_layers(
            &committed,
            Some(local),
            env_of(&[("GTS_DATA_DIR", " /mnt/persistent/needledrop ")]),
        )
        .unwrap();
        assert_eq!(config.data_dir, PathBuf::from("/mnt/persistent/needledrop"));
        assert_eq!(config.store_path, config.data_dir.join("needledrop.db"));

        // Blank variables leave the local value in place.
        let config =
            Config::from_layers(&committed, Some(local), env_of(&[("GTS_DATA_DIR", "  ")]))
                .unwrap();
        assert_eq!(config.data_dir, PathBuf::from("/srv/needledrop"));
        assert_eq!(config.store_path, config.data_dir.join("needledrop.db"));
    }

    #[test]
    fn explicit_store_path_is_independent_of_the_data_dir_override() {
        let committed = format!("{MINIMAL}[store]\npath = \"/srv/database/game.sqlite\"\n");
        let env = env_of(&[("GTS_DATA_DIR", "/mnt/needledrop")]);
        let config = Config::from_sources(&committed, env).unwrap();
        assert_eq!(config.data_dir, PathBuf::from("/mnt/needledrop"));
        assert_eq!(
            config.store_path,
            PathBuf::from("/srv/database/game.sqlite")
        );

        let env = env_of(&[
            ("GTS_DATA_DIR", "/mnt/needledrop"),
            ("GTS_STORE_PATH", "/mnt/database/game.sqlite"),
        ]);
        let config = Config::from_sources(&committed, env).unwrap();
        assert_eq!(config.data_dir, PathBuf::from("/mnt/needledrop"));
        assert_eq!(
            config.store_path,
            PathBuf::from("/mnt/database/game.sqlite")
        );
    }

    #[test]
    fn environment_wins_over_the_file() {
        let env = env_of(&[("GTS_BIND", "0.0.0.0:9999"), ("GTS_SECRET", "abc")]);
        let config = Config::from_sources(FILE, env).unwrap();
        assert_eq!(config.bind, "0.0.0.0:9999");
        assert_eq!(config.secret.as_deref(), Some("abc"));
        // What the environment does not mention still comes from the file.
        assert_eq!(config.launch_date, date(2026, 10, 1));
        assert_eq!(config.data_dir, PathBuf::from("data"));
    }

    #[test]
    fn blank_variables_count_as_unset() {
        let env = env_of(&[("GTS_BIND", ""), ("GTS_SECRET", "  ")]);
        let config = Config::from_sources(FILE, env).unwrap();
        assert_eq!(config.bind, "127.0.0.1:4810");
        assert_eq!(config.secret, None);
    }

    #[test]
    fn a_missing_or_malformed_launch_date_names_the_key() {
        let missing = Config::from_sources("bind = \"127.0.0.1:1\"", no_env).unwrap_err();
        assert!(missing.to_string().contains("launch_date"), "{missing}");

        let malformed = Config::from_sources("launch_date = \"1 Oct 2026\"", no_env).unwrap_err();
        assert!(malformed.to_string().contains("launch_date"), "{malformed}");
    }

    #[test]
    fn a_wrongly_typed_value_is_an_error() {
        let file = "launch_date = \"2026-10-01\"\nbind = 4810";
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
    fn an_empty_bind_is_an_error() {
        for file in [
            format!("{MINIMAL}bind = \"\"\n"),
            format!("{MINIMAL}bind = \"   \"\n"),
        ] {
            assert!(matches!(
                Config::from_sources(&file, no_env),
                Err(ConfigError::EmptyBind)
            ));
        }
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
    fn the_database_is_under_the_data_directory_unless_a_path_is_set() {
        // Nothing said, or said blank: the file the server makes for itself.
        for file in [
            MINIMAL.to_owned(),
            format!("{MINIMAL}[store]\nkind = \"sqlite\"\n"),
            format!("{MINIMAL}[store]\npath = \"\"\n"),
            format!("{MINIMAL}[store]\npath = \"  \"\n"),
        ] {
            let config = Config::from_sources(&file, no_env).unwrap();
            assert_eq!(
                config.store_path,
                PathBuf::from("data/needledrop.db"),
                "{file}"
            );
        }
        // It follows the data directory.
        let file = format!("{MINIMAL}data_dir = \"/var/lib/needledrop\"\n");
        let config = Config::from_sources(&file, no_env).unwrap();
        assert_eq!(
            config.store_path,
            PathBuf::from("/var/lib/needledrop/needledrop.db")
        );

        // A path of its own, absolute or relative, wherever the data directory is.
        for path in ["/srv/needledrop/game.sqlite", "db/game.sqlite"] {
            let file = format!(
                "{MINIMAL}data_dir = \"/var/lib/needledrop\"\n[store]\npath = \"{path}\"\n"
            );
            let config = Config::from_sources(&file, no_env).unwrap();
            assert_eq!(config.store_path, PathBuf::from(path));
            assert_eq!(config.data_dir, PathBuf::from("/var/lib/needledrop"));
            assert_eq!(config.store, StoreKind::Sqlite);
        }
    }

    #[test]
    fn the_database_path_comes_from_the_local_file_or_the_environment_first() {
        let committed = format!("{MINIMAL}[store]\nkind = \"sqlite\"\npath = \"\"\n");

        // The local file names it without repeating the rest of the table.
        let local = "[store]\npath = \"/srv/needledrop/game.sqlite\"\n";
        let config = Config::from_layers(&committed, Some(local), no_env).unwrap();
        assert_eq!(
            config.store_path,
            PathBuf::from("/srv/needledrop/game.sqlite")
        );
        assert_eq!(config.store, StoreKind::Sqlite);

        // The variable wins over both files; a blank one changes nothing.
        let env = env_of(&[("GTS_STORE_PATH", " /mnt/volume/needledrop.db ")]);
        let config = Config::from_layers(&committed, Some(local), env).unwrap();
        assert_eq!(
            config.store_path,
            PathBuf::from("/mnt/volume/needledrop.db")
        );
        let env = env_of(&[("GTS_STORE_PATH", "  ")]);
        let config = Config::from_layers(&committed, Some(local), env).unwrap();
        assert_eq!(
            config.store_path,
            PathBuf::from("/srv/needledrop/game.sqlite")
        );
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
            ("[store]\npath = 3\n", "path"),
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
