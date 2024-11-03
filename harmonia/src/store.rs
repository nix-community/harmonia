use std::path::Path;
use std::path::PathBuf;

#[derive(Default, Debug)]
pub struct Store {
    virtual_store: PathBuf,
    real_store: Option<PathBuf>,
}

impl Store {
    pub fn new() -> Self {
        let real_store = libnixstore::get_real_store_dir();
        let virtual_store = libnixstore::get_store_dir();

        if virtual_store == real_store {
            return Self {
                virtual_store: PathBuf::from(virtual_store),
                real_store: None,
            };
        }
        Self {
            virtual_store: PathBuf::from(virtual_store),
            real_store: Some(PathBuf::from(real_store)),
        }
    }
    pub fn get_real_path(&self, virtual_path: &Path) -> PathBuf {
        if let Some(real_store) = &self.real_store {
            if virtual_path.starts_with(&self.virtual_store) {
                return real_store.join(virtual_path.strip_prefix(&self.virtual_store).unwrap());
            }
        }
        PathBuf::from(virtual_path)
    }

    pub fn real_store(&self) -> &Path {
        self.real_store.as_ref().unwrap_or(&self.virtual_store)
    }
}
