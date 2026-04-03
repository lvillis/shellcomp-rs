use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

#[derive(Clone, Debug)]
pub(crate) struct Environment {
    overrides: BTreeMap<String, Option<OsString>>,
    path_overrides: BTreeMap<PathBuf, bool>,
    file_overrides: BTreeMap<PathBuf, Option<Vec<u8>>>,
    dir_entries_overrides: BTreeMap<PathBuf, Vec<PathBuf>>,
    windows_override: Option<bool>,
    use_real_path_lookups: bool,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            overrides: BTreeMap::new(),
            path_overrides: BTreeMap::new(),
            file_overrides: BTreeMap::new(),
            dir_entries_overrides: BTreeMap::new(),
            windows_override: None,
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
        if let Some(contents) = self.file_overrides.get(path) {
            return contents.is_some();
        }
        if self.dir_entries_overrides.contains_key(path) {
            return true;
        }
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
    pub(crate) fn with_file_contents(
        mut self,
        path: impl Into<PathBuf>,
        contents: impl Into<Vec<u8>>,
    ) -> Self {
        self.file_overrides
            .insert(path.into(), Some(contents.into()));
        self
    }

    #[cfg(test)]
    pub(crate) fn with_dir_entries(
        mut self,
        path: impl Into<PathBuf>,
        entries: impl IntoIterator<Item = PathBuf>,
    ) -> Self {
        self.dir_entries_overrides
            .insert(path.into(), entries.into_iter().collect());
        self
    }

    #[cfg(test)]
    pub(crate) fn without_real_path_lookups(mut self) -> Self {
        self.use_real_path_lookups = false;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_windows_platform(mut self) -> Self {
        self.windows_override = Some(true);
        self
    }

    pub(crate) fn read_file_if_exists(&self, path: &Path) -> Result<Option<Vec<u8>>> {
        if let Some(contents) = self.file_overrides.get(path) {
            return Ok(contents.clone());
        }

        if !self.use_real_path_lookups && !self.is_user_scoped_path(path) {
            return Ok(None);
        }

        match std::fs::read(path) {
            Ok(contents) => Ok(Some(contents)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(Error::io("read file", path, source)),
        }
    }

    pub(crate) fn read_dir_entries(&self, path: &Path) -> Result<Vec<PathBuf>> {
        if let Some(entries) = self.dir_entries_overrides.get(path) {
            return Ok(entries.clone());
        }

        if !self.use_real_path_lookups && !self.is_user_scoped_path(path) {
            return Ok(Vec::new());
        }

        match std::fs::read_dir(path) {
            Ok(entries) => entries
                .map(|entry| {
                    entry
                        .map(|entry| entry.path())
                        .map_err(|source| Error::io("read directory", path, source))
                })
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(source) => Err(Error::io("read directory", path, source)),
        }
    }

    fn is_user_scoped_path(&self, path: &Path) -> bool {
        [
            self.var_os("HOME").map(PathBuf::from),
            self.var_os("USERPROFILE").map(PathBuf::from),
            self.var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            self.var_os("XDG_DATA_HOME").map(PathBuf::from),
            self.var_os("ZDOTDIR").map(PathBuf::from),
        ]
        .into_iter()
        .flatten()
        .any(|root| path.starts_with(root))
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

    pub(crate) fn powershell_default_install_dir(&self) -> Result<PathBuf> {
        if self.is_windows_platform() {
            return Ok(self
                .powershell_home_dir()?
                .join("Documents")
                .join("PowerShell")
                .join("Completions"));
        }

        Ok(self.xdg_data_home()?.join("powershell").join("completions"))
    }

    pub(crate) fn powershell_profile_path(&self) -> Result<PathBuf> {
        if self.is_windows_platform() {
            return Ok(self
                .powershell_home_dir()?
                .join("Documents")
                .join("PowerShell")
                .join("profile.ps1"));
        }

        Ok(self
            .home_dir()?
            .join(".config")
            .join("powershell")
            .join("profile.ps1"))
    }

    fn powershell_home_dir(&self) -> Result<PathBuf> {
        self.var_os("USERPROFILE")
            .map(PathBuf::from)
            .or_else(|| self.var_os("HOME").map(PathBuf::from))
            .ok_or(Error::MissingHome)
    }

    pub(crate) fn is_windows_platform(&self) -> bool {
        self.windows_override.unwrap_or(cfg!(windows))
    }
}
