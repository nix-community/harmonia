use std::path::PathBuf;

#[derive(Default, Debug)]
pub struct Store {
    virtual_store: String,
    real_store: Option<String>,
}

impl Store {
    pub fn new() -> Self {
        let real_store = libnixstore::get_real_store_dir();
        let virtual_store = libnixstore::get_store_dir();

        if virtual_store == real_store {
            return Self {
                virtual_store,
                real_store: None,
            };
        }
        Self {
            virtual_store,
            real_store: Some(real_store),
        }
    }
    pub fn get_real_path(&self, virtual_path: &str) -> PathBuf {
        if let Some(real_store) = &self.real_store {
            if virtual_path.starts_with(&self.virtual_store) {
                return PathBuf::from(format!(
                    "{}{}",
                    real_store,
                    &virtual_path[self.virtual_store.len()..]
                ));
            }
        }
        PathBuf::from(virtual_path)
    }
    pub fn real_store(&self) -> &str {
        self.real_store
            .as_ref()
            .map_or(&self.virtual_store, |s| s.as_str())
    }
}
