use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

#[derive(Clone, Debug)]
pub(crate) struct Environment {
    overrides: BTreeMap<String, Option<OsString>>,
    path_overrides: BTreeMap<PathBuf, bool>,
    use_real_path_lookups: bool,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            overrides: BTreeMap::new(),
            path_overrides: BTreeMap::new(),
            use_real_path_lookups: true,
        }
    }
}

impl Environment {
    pub(crate) fn system() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn test() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn with_var(mut self, key: &str, value: impl Into<OsString>) -> Self {
        self.overrides.insert(key.to_owned(), Some(value.into()));
        self
    }

    #[cfg(test)]
    pub(crate) fn without_var(mut self, key: &str) -> Self {
        self.overrides.insert(key.to_owned(), None);
        self
    }

    pub(crate) fn var_os(&self, key: &str) -> Option<OsString> {
        if let Some(value) = self.overrides.get(key) {
            return value.clone();
        }
        std::env::var_os(key)
    }

    pub(crate) fn path_exists(&self, path: &Path) -> bool {
        if let Some(exists) = self.path_overrides.get(path) {
            return *exists;
        }
        self.use_real_path_lookups && path.exists()
    }

    #[cfg(test)]
    pub(crate) fn with_existing_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path_overrides.insert(path.into(), true);
        self
    }

    #[cfg(test)]
    pub(crate) fn without_existing_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path_overrides.insert(path.into(), false);
        self
    }

    #[cfg(test)]
    pub(crate) fn without_real_path_lookups(mut self) -> Self {
        self.use_real_path_lookups = false;
        self
    }

    pub(crate) fn home_dir(&self) -> Result<PathBuf> {
        self.var_os("HOME")
            .map(PathBuf::from)
            .ok_or(Error::MissingHome)
    }

    pub(crate) fn xdg_config_home(&self) -> Result<PathBuf> {
        if let Some(path) = self.var_os("XDG_CONFIG_HOME") {
            return Ok(PathBuf::from(path));
        }
        Ok(self.home_dir()?.join(".config"))
    }

    pub(crate) fn xdg_data_home(&self) -> Result<PathBuf> {
        if let Some(path) = self.var_os("XDG_DATA_HOME") {
            return Ok(PathBuf::from(path));
        }
        Ok(self.home_dir()?.join(".local").join("share"))
    }

    pub(crate) fn zdotdir(&self) -> Result<PathBuf> {
        if let Some(path) = self.var_os("ZDOTDIR") {
            return Ok(PathBuf::from(path));
        }
        self.home_dir()
    }
}
