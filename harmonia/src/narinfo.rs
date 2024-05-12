use std::{error::Error, path::Path};

use actix_web::{http, web, HttpResponse};
use libnixstore::Radix;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::{cache_control_max_age_1d, nixhash, some_or_404};

#[derive(Debug, Deserialize)]
pub struct Param {
    json: Option<String>,
}

#[derive(Debug, Serialize)]
struct NarInfo {
    store_path: String,
    url: String,
    compression: String,
    nar_hash: String,
    nar_size: u64,
    references: Vec<String>,
    deriver: Option<String>,
    system: Option<String>,
    sig: Option<String>,
    ca: Option<String>,
}

fn fingerprint_path(
    store_path: &str,
    nar_hash: &str,
    nar_size: u64,
    refs: &[String],
) -> Result<Option<String>, Box<dyn Error>> {
    let root_store_dir = libnixstore::get_store_dir();
    if store_path[0..root_store_dir.len()] != root_store_dir || &nar_hash[0..7] != "sha256:" {
        return Ok(None);
    }

    let mut nar_hash = nar_hash.to_owned();
    if nar_hash.len() == 71 {
        nar_hash = format!(
            "sha256:{}",
            libnixstore::convert_hash("sha256", &nar_hash[7..], Radix::default())?
        );
    }

    if nar_hash.len() != 59 {
        return Ok(None);
    }

    for r in refs {
        if r[0..root_store_dir.len()] != root_store_dir {
            return Ok(None);
        }
    }

    Ok(Some(format!(
        "1;{};{};{};{}",
        store_path,
        nar_hash,
        nar_size,
        refs.join(",")
    )))
}

fn extract_filename(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .and_then(|v| v.to_str().map(ToOwned::to_owned))
}

fn query_narinfo(
    store_path: &str,
    hash: &str,
    sign_keys: &Vec<String>,
) -> Result<NarInfo, Box<dyn Error>> {
    let path_info = libnixstore::query_path_info(store_path, Radix::default())?;
    let mut res = NarInfo {
        store_path: store_path.into(),
        url: format!(
            "nar/{}.nar?hash={}",
            path_info.narhash.split_once(':').map_or(hash, |x| x.1),
            hash
        ),
        compression: "none".into(),
        nar_hash: path_info.narhash,
        nar_size: path_info.size,
        references: vec![],
        deriver: None,
        system: None,
        sig: None,
        ca: path_info.ca,
    };

    let refs = path_info.refs.clone();
    if !path_info.refs.is_empty() {
        res.references = path_info
            .refs
            .into_iter()
            .filter_map(|r| extract_filename(&r))
            .collect::<Vec<String>>();
    }

    if let Some(drv) = path_info.drv {
        res.deriver = extract_filename(&drv);

        if libnixstore::is_valid_path(&drv) {
            res.system = Some(libnixstore::derivation_from_path(&drv)?.platform);
        }
    }

    for sk in sign_keys {
        let fingerprint = fingerprint_path(store_path, &res.nar_hash, res.nar_size, &refs)?;
        if let Some(fp) = fingerprint {
            res.sig = Some(libnixstore::sign_string(sk, &fp)?);
        }
    }

    Ok(res)
}

fn format_narinfo_txt(narinfo: &NarInfo) -> String {
    let mut res = vec![
        format!("StorePath: {}", narinfo.store_path),
        format!("URL: {}", narinfo.url),
        format!("Compression: {}", narinfo.compression),
        format!("FileHash: {}", narinfo.nar_hash),
        format!("FileSize: {}", narinfo.nar_size),
        format!("NarHash: {}", narinfo.nar_hash),
        format!("NarSize: {}", narinfo.nar_size),
    ];

    if !narinfo.references.is_empty() {
        res.push(format!("References: {}", &narinfo.references.join(" ")));
    }

    if let Some(drv) = &narinfo.deriver {
        res.push(format!("Deriver: {}", drv));
    }

    if let Some(sys) = &narinfo.system {
        res.push(format!("System: {}", sys));
    }

    if let Some(sig) = &narinfo.sig {
        res.push(format!("Sig: {}", sig));
    }

    if let Some(ca) = &narinfo.ca {
        res.push(format!("CA: {}", ca));
    }

    res.push("".into());
    res.join("\n")
}

pub(crate) async fn get(
    hash: web::Path<String>,
    param: web::Query<Param>,
    settings: web::Data<Config>,
) -> Result<HttpResponse, Box<dyn Error>> {
    let hash = hash.into_inner();
    let store_path = some_or_404!(nixhash(&hash));
    let narinfo = query_narinfo(&store_path, &hash, &settings.secret_keys)?;

    if param.json.is_some() {
        Ok(HttpResponse::Ok()
            .insert_header(cache_control_max_age_1d())
            .json(narinfo))
    } else {
        let res = format_narinfo_txt(&narinfo);
        Ok(HttpResponse::Ok()
            .insert_header((http::header::CONTENT_TYPE, "text/x-nix-narinfo"))
            .insert_header(("Nix-Link", narinfo.url))
            .insert_header(cache_control_max_age_1d())
            .body(res))
    }
}
