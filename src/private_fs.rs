use std::{
    fs::{DirBuilder, OpenOptions},
    io::{self, Write},
    path::Path,
};

/// Create a data directory without granting access to other local users.
pub(crate) fn create_private_dir_all(path: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }

    builder.create(path)
}

/// Build file options whose creation mode is private on Unix.
pub(crate) fn private_open_options() -> OpenOptions {
    let mut options = OpenOptions::new();

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    options
}

/// Replace a private data file while preserving creation-time confidentiality.
pub(crate) fn write_private(path: &Path, content: &[u8]) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        create_private_dir_all(parent)?;
    }

    let mut options = private_open_options();
    let mut file = options.create(true).truncate(true).write(true).open(path)?;
    file.write_all(content)
}
