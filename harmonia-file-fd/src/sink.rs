use cap_tokio::fs::Dir;
use harmonia_file_io_pure::{DirectorySink, FileSystemSink, RegularFileSink};

/// A slot in a directory where a new node will be created.
pub struct DirSlotSink {
    parent: Dir,
    name: String,
}

impl DirSlotSink {
    /// Create a sink for a new entry in the given directory.
    pub fn new(parent: Dir, name: String) -> Self {
        Self { parent, name }
    }
}

impl FileSystemSink for DirSlotSink {
    type Error = std::io::Error;
    type Directory = DirDirSink;
    type File = DirFileSink;

    async fn create_directory(self) -> Result<Self::Directory, Self::Error> {
        self.parent.create_dir(&self.name)?;
        let dir = self.parent.open_dir(&self.name).await?;
        Ok(DirDirSink { dir })
    }

    async fn create_regular_file(self, executable: bool) -> Result<Self::File, Self::Error> {
        use cap_tokio::fs::OpenOptions;
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        let file = self.parent.open_with(&self.name, &opts).await?;
        #[cfg(unix)]
        {
            use cap_tokio::fs::PermissionsExt;
            let mode = if executable { 0o755 } else { 0o644 };
            file.set_permissions(cap_tokio::fs::Permissions::from_mode(mode))
                .await?;
        }
        Ok(DirFileSink { file })
    }

    async fn create_symlink(self, target: &str) -> Result<(), Self::Error> {
        self.parent.symlink(target, &self.name).await
    }
}

/// An open directory being populated with children.
pub struct DirDirSink {
    dir: Dir,
}

impl DirectorySink for DirDirSink {
    type Error = std::io::Error;
    type Child<'a> = DirSlotSink;

    async fn create_child(&mut self, name: &str) -> Result<DirSlotSink, Self::Error> {
        Ok(DirSlotSink {
            parent: self.dir.clone(),
            name: name.to_owned(),
        })
    }
}

/// An open file being written.
pub struct DirFileSink {
    file: cap_tokio::fs::File,
}

impl tokio::io::AsyncWrite for DirFileSink {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.file).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.file).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.file).poll_shutdown(cx)
    }
}

impl RegularFileSink for DirFileSink {
    type Error = std::io::Error;
}
