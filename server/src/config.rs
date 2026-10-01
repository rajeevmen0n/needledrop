//! Deployment settings from `ND_BIND`, `ND_DATA_DIR` and `ND_PUBLIC_URL`.

use std::{fmt, path::PathBuf};

use jiff::civil::{Date, date};

const ENV_BIND: &str = "ND_BIND";
const ENV_DATA_DIR: &str = "ND_DATA_DIR";
const ENV_PUBLIC_URL: &str = "ND_PUBLIC_URL";

/// The store factory also exposes an in-memory backend to tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    Sqlite,
    #[cfg(test)]
    Memory,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    pub bind: String,
    /// Cached previews, cookie key and SQLite database live here.
    pub data_dir: PathBuf,
    pub launch_date: Date,
    pub public_url: Option<String>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("data_dir", &self.data_dir)
            .field("launch_date", &self.launch_date)
            .field("public_url", &self.public_url)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(
        "{setting} is {value:?}: expected the address the game is served under, \
         such as https://needledrop.example ({reason})"
    )]
    PublicUrl {
        setting: &'static str,
        value: String,
        reason: &'static str,
    },
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        Self::from_env(|name| std::env::var(name).ok())
    }

    /// Pure environment lookup, used by tests. Blank values use defaults.
    pub fn from_env(env: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let value = |name: &str| env(name).filter(|value| !value.trim().is_empty());
        let bind = value(ENV_BIND)
            .unwrap_or_else(|| "127.0.0.1:4810".to_owned())
            .trim()
            .to_owned();
        let data_dir = value(ENV_DATA_DIR)
            .map(|path| PathBuf::from(path.trim()))
            .unwrap_or_else(|| PathBuf::from("data"));
        let public_url = value(ENV_PUBLIC_URL)
            .map(|value| public_url(&value))
            .transpose()?;
        Ok(Self {
            bind,
            data_dir,
            launch_date: date(2026, 10, 1),
            public_url,
        })
    }

    pub fn store_path(&self) -> PathBuf {
        self.data_dir.join("needledrop.db")
    }
}

/// Accept only a public origin and normalize away its trailing slash.
fn public_url(value: &str) -> Result<String, ConfigError> {
    let refuse = |reason| ConfigError::PublicUrl {
        setting: ENV_PUBLIC_URL,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn defaults_need_no_file_or_environment() {
        let config = Config::from_env(env(&[])).unwrap();
        assert_eq!(config.bind, "127.0.0.1:4810");
        assert_eq!(config.data_dir, PathBuf::from("data"));
        assert_eq!(config.store_path(), PathBuf::from("data/needledrop.db"));
        assert_eq!(config.launch_date, date(2026, 10, 1));
        assert_eq!(config.public_url, None);
    }

    #[test]
    fn deployment_variables_override_defaults() {
        let config = Config::from_env(env(&[
            ("ND_BIND", " 0.0.0.0:4810 "),
            ("ND_DATA_DIR", " /var/lib/needledrop "),
            ("ND_PUBLIC_URL", "https://needledrop.example/"),
        ]))
        .unwrap();
        assert_eq!(config.bind, "0.0.0.0:4810");
        assert_eq!(config.data_dir, PathBuf::from("/var/lib/needledrop"));
        assert_eq!(
            config.store_path(),
            PathBuf::from("/var/lib/needledrop/needledrop.db")
        );
        assert_eq!(
            config.public_url.as_deref(),
            Some("https://needledrop.example")
        );
    }

    #[test]
    fn blank_values_use_defaults_and_old_variables_are_ignored() {
        let config = Config::from_env(env(&[
            ("ND_BIND", " "),
            ("ND_DATA_DIR", ""),
            ("ND_PUBLIC_URL", " "),
            ("GTS_BIND", "0.0.0.0:1234"),
            ("GTS_DATA_DIR", "/old"),
            ("GTS_PUBLIC_URL", "https://old.example"),
        ]))
        .unwrap();
        assert_eq!(config, Config::from_env(env(&[])).unwrap());
    }

    #[test]
    fn public_url_accepts_only_an_origin() {
        for value in [
            "needledrop.example",
            "ftp://example.com",
            "https://example.com/path",
            "https://example.com?q=x",
            "https://user@example.com",
        ] {
            let error = Config::from_env(env(&[("ND_PUBLIC_URL", value)])).unwrap_err();
            assert!(error.to_string().contains("ND_PUBLIC_URL"), "{error}");
            assert!(error.to_string().contains(value), "{error}");
        }
        let config = Config::from_env(env(&[("ND_PUBLIC_URL", "http://localhost:4811")])).unwrap();
        assert_eq!(config.public_url.as_deref(), Some("http://localhost:4811"));
    }
}
