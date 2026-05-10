use crate::ServerResult;
use crate::error::{CacheError, ServeError};
use actix_web::{HttpResponse, http, web};
use cap_tokio::ambient_authority;
use cap_tokio::fs::Dir;
use harmonia_file_core::FileTree;
use harmonia_file_fd::DirSource;
use harmonia_file_io_pure::{Stat, list_deep};
use serde::Serialize;

use crate::config::Config;
use crate::{cache_control_max_age_1y, nixhash, some_or_404};

use std::path::PathBuf;

#[derive(Debug, Serialize)]
struct NarList {
    version: u16,
    root: FileTree<Stat>,
}

async fn get_nar_list(path: PathBuf) -> crate::error::Result<NarList> {
    let dir = Dir::open_ambient_dir(&path, ambient_authority())
        .await
        .map_err(|e| ServeError::ServeFailed {
            source: std::io::Error::other(format!(
                "Failed to open directory {}: {e}",
                path.display()
            )),
        })?;
    let source = DirSource::open(dir)
        .await
        .map_err(|e| ServeError::ServeFailed {
            source: std::io::Error::other(e),
        })?;
    let root = list_deep(&source)
        .await
        .map_err(|e| ServeError::ServeFailed {
            source: std::io::Error::other(e.to_string()),
        })?;
    Ok(NarList { version: 1, root })
}

pub(crate) async fn get(hash: web::Path<String>, settings: web::Data<Config>) -> ServerResult {
    let store_path = some_or_404!(nixhash(&settings, hash.as_bytes())?);

    let nar_list = get_nar_list(settings.store.get_real_path(&store_path)).await?;
    Ok(HttpResponse::Ok()
        .insert_header(cache_control_max_age_1y())
        .insert_header(http::header::ContentType(mime::APPLICATION_JSON))
        .body(serde_json::to_string(&nar_list).map_err(|e| {
            CacheError::from(ServeError::ServeFailed {
                source: std::io::Error::other(e),
            })
        })?))
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::error::{IoErrorContext, Result};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    #[tokio::test]
    async fn test_get_nar_list() -> Result<()> {
        let temp_dir = harmonia_utils_test::CanonicalTempDir::new()
            .io_context("Failed to create canonical temp dir")?;
        let dir = temp_dir.path().join("store");
        fs::create_dir(&dir).io_context("Failed to create temp dir")?;
        fs::write(dir.join("file"), b"somecontent").io_context("Failed to write file")?;

        fs::create_dir(dir.join("some_empty_dir")).io_context("Failed to create dir")?;

        let some_dir = dir.join("some_dir");
        fs::create_dir(&some_dir).io_context("Failed to create dir")?;

        let executable_path = some_dir.join("executable");
        fs::write(&executable_path, b"somescript").io_context("Failed to write file")?;
        fs::set_permissions(&executable_path, fs::Permissions::from_mode(0o755))
            .io_context("Failed to set permissions")?;

        std::os::unix::fs::symlink("sometarget", dir.join("symlink"))
            .io_context("Failed to create symlink")?;

        let json = get_nar_list(dir.to_owned()).await.unwrap();

        // Compare against nix's own listing
        let nar_file = temp_dir.path().join("store.nar");
        let res = Command::new("nix-store")
            .arg("--dump")
            .arg(&dir)
            .stdout(
                fs::File::create(&nar_file)
                    .io_context("Failed to create nar file")
                    .unwrap(),
            )
            .status()
            .io_context("Failed to run nix-store --dump")
            .unwrap();
        assert!(res.success());

        let res2 = Command::new("nix")
            .arg("--extra-experimental-features")
            .arg("nix-command")
            .arg("nar")
            .arg("ls")
            .arg("--json")
            .arg("--recursive")
            .arg(&nar_file)
            .arg("/")
            .output()
            .io_context("Failed to run nix nar ls --json --recursive")
            .unwrap();
        assert!(res2.status.success());

        let reference: serde_json::Value = serde_json::from_slice(&res2.stdout).unwrap();
        let ours: serde_json::Value = serde_json::to_value(&json.root).unwrap();

        // Strip fields that may differ between nix versions:
        // - narOffset: present in NAR listings but not filesystem listings
        // - executable: false may be absent in older nix (pre-NixOS/nix#15834)
        fn normalize(v: &mut serde_json::Value) {
            if let Some(obj) = v.as_object_mut() {
                obj.remove("narOffset");
                if obj.get("executable") == Some(&serde_json::Value::Bool(false)) {
                    obj.remove("executable");
                }
                if let Some(entries) = obj.get_mut("entries")
                    && let Some(map) = entries.as_object_mut()
                {
                    for (_, child) in map.iter_mut() {
                        normalize(child);
                    }
                }
            }
        }
        let mut ours_normalized = ours;
        normalize(&mut ours_normalized);
        let mut reference_normalized = reference;
        normalize(&mut reference_normalized);

        assert_eq!(ours_normalized, reference_normalized);

        Ok(())
    }
}
