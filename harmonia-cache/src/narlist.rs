use crate::ServerResult;
use crate::error::{CacheError, IoErrorContext, Result, ServeError};
use actix_web::{HttpResponse, http, web};
use harmonia_file_core::{Directory, FileSystemObject, FileTree, Regular, Symlink};
use harmonia_file_nar::NarFileInfo;
use serde::Serialize;
use std::fs::Metadata;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::config::Config;
use crate::{cache_control_max_age_1y, nixhash, some_or_404};

use std::path::PathBuf;
use tokio::fs::symlink_metadata;

#[derive(Debug, Serialize)]
struct NarList {
    version: u16,
    root: FileTree<NarFileInfo>,
}

fn file_entry(metadata: Metadata) -> FileTree<NarFileInfo> {
    FileTree(FileSystemObject::Regular(Regular {
        executable: metadata.permissions().mode() & 0o111 != 0,
        contents: NarFileInfo {
            size: metadata.len(),
            nar_offset: None,
        },
    }))
}

async fn symlink_entry(path: &Path) -> Result<FileTree<NarFileInfo>> {
    let target = tokio::fs::read_link(&path)
        .await
        .io_context(format!("Failed to read link {}", path.display()))?;
    Ok(FileTree(FileSystemObject::Symlink(Symlink {
        target: target.to_string_lossy().into_owned(),
    })))
}

struct Frame {
    path: PathBuf,
    entries: std::collections::BTreeMap<String, Box<FileTree<NarFileInfo>>>,
    dir_entry: tokio::fs::ReadDir,
}

async fn get_nar_list(path: PathBuf) -> Result<NarList> {
    let st = symlink_metadata(&path).await.io_context(format!(
        "Failed to get symlink metadata for {}",
        path.display()
    ))?;

    let file_type = st.file_type();
    let root = if file_type.is_file() {
        file_entry(st)
    } else if file_type.is_symlink() {
        symlink_entry(&path).await?
    } else if file_type.is_dir() {
        let dir_entry = tokio::fs::read_dir(&path)
            .await
            .io_context(format!("Failed to read directory {}", path.display()))?;
        let mut stack = vec![Frame {
            path,
            entries: std::collections::BTreeMap::new(),
            dir_entry,
        }];

        let mut root: Option<FileTree<NarFileInfo>> = None;

        while let Some(frame) = stack.last_mut() {
            if let Some(entry) = frame
                .dir_entry
                .next_entry()
                .await
                .io_context("Failed to read next directory entry")?
            {
                let name = entry.file_name().to_string_lossy().into_owned();
                let entry_path = entry.path();
                let entry_st = symlink_metadata(&entry_path).await.io_context(format!(
                    "Failed to get metadata for {}",
                    entry_path.display()
                ))?;
                let entry_file_type = entry_st.file_type();

                if entry_file_type.is_file() {
                    frame.entries.insert(name, Box::new(file_entry(entry_st)));
                } else if entry_file_type.is_symlink() {
                    frame
                        .entries
                        .insert(name, Box::new(symlink_entry(&entry_path).await?));
                } else if entry_file_type.is_dir() {
                    let dir_entry = tokio::fs::read_dir(&entry_path)
                        .await
                        .io_context(format!("Failed to read directory {}", entry_path.display()))?;
                    stack.push(Frame {
                        path: entry_path,
                        entries: std::collections::BTreeMap::new(),
                        dir_entry,
                    });
                }
            } else {
                let frame = stack
                    .pop()
                    .expect("stack should not be empty inside loop iteration");
                let dir_tree = FileTree(FileSystemObject::Directory(Directory {
                    entries: frame.entries,
                }));
                if let Some(parent) = stack.last_mut() {
                    let name = match frame.path.file_name() {
                        Some(name) => name.to_string_lossy().into_owned(),
                        None => {
                            return Err(ServeError::AccessDenied {
                                path: frame.path.display().to_string(),
                            }
                            .into());
                        }
                    };
                    parent.entries.insert(name, Box::new(dir_tree));
                } else {
                    root = Some(dir_tree);
                }
            }
        }

        root.expect("root should be set after processing directory stack")
    } else {
        return Err(ServeError::ServeFailed {
            source: std::io::Error::other(format!(
                "Unsupported file type for path: {}",
                path.display()
            )),
        }
        .into());
    };

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
    use std::fs;
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

        // nix's output may include narOffset; strip for comparison
        fn strip_nar_offset(v: &mut serde_json::Value) {
            if let Some(obj) = v.as_object_mut() {
                obj.remove("narOffset");
                if let Some(entries) = obj.get_mut("entries")
                    && let Some(map) = entries.as_object_mut()
                {
                    for (_, child) in map.iter_mut() {
                        strip_nar_offset(child);
                    }
                }
            }
        }
        let mut reference_stripped = reference;
        strip_nar_offset(&mut reference_stripped);

        assert_eq!(ours, reference_stripped);

        Ok(())
    }
}
