use std::fs;
use std::path::Path;

use crate::error::{Error, Result};
use crate::model::FileChange;

pub(crate) fn write_if_changed(path: &Path, contents: &[u8]) -> Result<FileChange> {
    let parent = path.parent().ok_or_else(|| Error::PathHasNoParent {
        path: path.to_path_buf(),
    })?;

    fs::create_dir_all(parent)
        .map_err(|source| Error::io("create parent directory for", parent, source))?;

    match fs::read(path) {
        Ok(existing) if existing == contents => Ok(FileChange::Unchanged),
        Ok(_) => {
            fs::write(path, contents).map_err(|source| Error::io("write file", path, source))?;
            Ok(FileChange::Updated)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::write(path, contents).map_err(|source| Error::io("write file", path, source))?;
            Ok(FileChange::Created)
        }
        Err(source) => Err(Error::io("read file", path, source)),
    }
}

pub(crate) fn remove_file_if_exists(path: &Path) -> Result<FileChange> {
    match fs::remove_file(path) {
        Ok(()) => Ok(FileChange::Removed),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FileChange::Absent),
        Err(source) => Err(Error::io("remove file", path, source)),
    }
}

pub(crate) fn file_exists(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::write_if_changed;
    use crate::model::FileChange;

    #[test]
    fn write_if_changed_distinguishes_created_updated_and_unchanged() {
        let temp_root = crate::tests::temp_dir("write-if-changed");
        let target = temp_root.join("file.txt");

        let created = write_if_changed(&target, b"one").expect("create should succeed");
        let unchanged = write_if_changed(&target, b"one").expect("unchanged write should succeed");
        let updated = write_if_changed(&target, b"two").expect("update should succeed");

        assert_eq!(created, FileChange::Created);
        assert_eq!(unchanged, FileChange::Unchanged);
        assert_eq!(updated, FileChange::Updated);
        assert_eq!(fs::read(&target).expect("target should exist"), b"two");
    }
}
