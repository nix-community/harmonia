use std::error::Error;

use actix_web::{http, web, HttpResponse};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::Metadata;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::config::Config;
use crate::{cache_control_max_age_1y, nixhash, some_or_404};

use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs::symlink_metadata;

fn is_false(b: &bool) -> bool {
    !b
}

#[derive(Debug, Serialize, Deserialize, Clone, Eq, PartialEq)]
#[serde(tag = "type")]
enum NarEntry {
    #[serde(rename = "directory")]
    Directory { entries: HashMap<String, NarEntry> },
    #[serde(rename = "regular")]
    Regular {
        #[serde(rename = "narOffset")]
        nar_offset: Option<u64>,
        size: u64,

        #[serde(default, skip_serializing_if = "is_false")]
        executable: bool,
    },
    #[serde(rename = "symlink")]
    Symlink { target: String },
}

#[derive(Debug, Serialize, Eq, PartialEq)]
struct NarList {
    version: u16,
    root: NarEntry,
}

struct Frame {
    path: PathBuf,
    nar_entry: NarEntry,
    dir_entry: tokio::fs::ReadDir,
}

fn file_entry(metadata: Metadata) -> NarEntry {
    NarEntry::Regular {
        size: metadata.len(),
        executable: metadata.permissions().mode() & 0o111 != 0,
        nar_offset: None,
    }
}

async fn symlink_entry(path: &Path) -> Result<NarEntry> {
    let target = tokio::fs::read_link(&path).await?;
    Ok(NarEntry::Symlink {
        target: target.to_string_lossy().into_owned(),
    })
}

async fn get_nar_list(path: PathBuf) -> Result<NarList> {
    let st = symlink_metadata(&path).await?;

    let file_type = st.file_type();
    let root = if file_type.is_file() {
        file_entry(st)
    } else if file_type.is_symlink() {
        symlink_entry(&path)
            .await
            .with_context(|| format!("Failed to read symlink {:?}", path))?
    } else if file_type.is_dir() {
        let dir_entry = tokio::fs::read_dir(&path)
            .await
            .with_context(|| format!("Failed to read directory {:?}", path))?;
        let mut stack = vec![Frame {
            path,
            dir_entry,
            nar_entry: NarEntry::Directory {
                entries: HashMap::new(),
            },
        }];

        let mut root: Option<NarEntry> = None;

        while let Some(frame) = stack.last_mut() {
            if let Some(entry) = frame.dir_entry.next_entry().await? {
                let name = entry.file_name().to_string_lossy().into_owned();
                let entry_path = entry.path();
                let entry_st = symlink_metadata(&entry_path).await?;
                let entry_file_type = entry_st.file_type();

                let entries = match &mut frame.nar_entry {
                    NarEntry::Directory { entries, .. } => entries,
                    _ => unreachable!(),
                };
                if entry_file_type.is_file() {
                    entries.insert(name, file_entry(entry_st));
                } else if entry_file_type.is_symlink() {
                    entries.insert(
                        name,
                        symlink_entry(&entry_path)
                            .await
                            .with_context(|| format!("Failed to read symlink {:?}", entry_path))?,
                    );
                } else if entry_file_type.is_dir() {
                    let dir_entry = tokio::fs::read_dir(&entry_path).await?;
                    stack.push(Frame {
                        path: entry_path,
                        dir_entry,
                        nar_entry: NarEntry::Directory {
                            entries: HashMap::new(),
                        },
                    });
                }
            } else {
                let entry = stack.pop().unwrap();
                if let Some(frame) = stack.last_mut() {
                    let name = match entry.path.file_name() {
                        Some(name) => name.to_string_lossy().into_owned(),
                        None => bail!("Failed to get file name {:?}", entry.path),
                    };
                    let entries = match &mut frame.nar_entry {
                        NarEntry::Directory { entries, .. } => entries,
                        _ => unreachable!(),
                    };
                    entries.insert(name, entry.nar_entry);
                } else {
                    root = Some(entry.nar_entry);
                }
            }
        }

        root.unwrap()
    } else {
        return Err(anyhow::anyhow!("Unsupported file type {:?}", path));
    };

    Ok(NarList { version: 1, root })
}

pub(crate) async fn get(
    hash: web::Path<String>,
    settings: web::Data<Config>,
) -> Result<HttpResponse, Box<dyn Error>> {
    let store_path = PathBuf::from(some_or_404!(nixhash(&hash)));

    let nar_list = get_nar_list(settings.store.get_real_path(&store_path)).await?;
    Ok(HttpResponse::Ok()
        .insert_header(cache_control_max_age_1y())
        .insert_header(http::header::ContentType(mime::APPLICATION_JSON))
        .body(serde_json::to_string(&nar_list)?))
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs;
    use std::process::Command;

    pub fn unset_nar_offset(entry: &mut NarEntry) {
        match entry {
            NarEntry::Regular { nar_offset, .. } => {
                *nar_offset = None;
            }
            NarEntry::Directory { entries } => {
                for (_, entry) in entries.iter_mut() {
                    unset_nar_offset(entry);
                }
            }
            _ => {}
        }
    }

    #[tokio::test]
    async fn test_get_nar_list() -> Result<()> {
        let temp_dir = tempfile::tempdir()
            .context("Failed to create temp dir")
            .expect("Failed to create temp dir");
        let dir = temp_dir.path().join("store");
        fs::create_dir(&dir)
            .context("Failed to create temp dir")
            .unwrap();
        fs::write(dir.join("file"), b"somecontent")
            .context("Failed to write file")
            .unwrap();

        fs::create_dir(dir.join("some_empty_dir"))
            .context("Failed to create dir")
            .unwrap();

        let some_dir = dir.join("some_dir");
        fs::create_dir(&some_dir)
            .context("Failed to create dir")
            .unwrap();

        let executable_path = some_dir.join("executable");
        fs::write(&executable_path, b"somescript")
            .context("Failed to write file")
            .unwrap();
        fs::set_permissions(&executable_path, fs::Permissions::from_mode(0o755))
            .context("Failed to set permissions")
            .unwrap();

        std::os::unix::fs::symlink("sometarget", dir.join("symlink"))
            .context("Failed to create symlink")
            .unwrap();

        let json = get_nar_list(dir.to_owned()).await.unwrap();

        //let nar_dump = dump_to_vec(dir.to_str().unwrap().to_owned()).await?;
        let nar_file = temp_dir.path().join("store.nar");
        let res = Command::new("nix-store")
            .arg("--dump")
            .arg(dir)
            .stdout(
                fs::File::create(&nar_file)
                    .context("Failed to create nar file")
                    .unwrap(),
            )
            .status()
            .context("Failed to run nix-store --dump")
            .unwrap();
        assert!(res.success());
        // nix nar ls --json --recursive
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
            .context("Failed to run nix nar ls --json --recursive")
            .unwrap();
        let parsed_json: serde_json::Value = serde_json::from_slice(&res2.stdout).unwrap();
        let pretty_string = serde_json::to_string_pretty(&parsed_json).unwrap();
        assert!(res2.status.success());
        let mut reference_json: NarEntry = serde_json::from_str(&pretty_string).unwrap();

        // our posix implementation does not support narOffset
        unset_nar_offset(&mut reference_json);

        println!("get_nar_list:");
        println!("{}", serde_json::to_string_pretty(&json.root).unwrap());
        println!("nix nar ls --json --recursive:");
        println!("{}", pretty_string);
        assert_eq!(json.root, reference_json);

        Ok(())
    }
}
