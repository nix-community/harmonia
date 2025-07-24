use std::{error::Error, path::Path};

use actix_web::{http, web, HttpResponse};
use anyhow::Context;
use anyhow::Result;
use harmonia_store_remote::protocol::StorePath;
use serde::{Deserialize, Serialize};

use crate::config::{Config, SigningKey};
use crate::signing::convert_base16_to_nix32;
use crate::signing::{fingerprint_path, sign_string};
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
    sigs: Vec<String>,
    ca: Option<String>,
}

fn extract_filename(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .and_then(|v| v.to_str().map(ToOwned::to_owned))
}

async fn query_narinfo(
    virtual_nix_store: &str,
    store_path: &StorePath,
    hash: &str,
    sign_keys: &Vec<SigningKey>,
    settings: &web::Data<Config>,
) -> Result<Option<NarInfo>> {
    let mut daemon_guard = settings.store.get_daemon().await?;
    let daemon = daemon_guard.as_mut().unwrap();

    let path_info = match daemon.query_path_info(store_path).await? {
        Some(info) => info,
        None => {
            return Ok(None);
        }
    };
    let nar_hash =
        convert_base16_to_nix32(&path_info.hash).context("failed to convert path info hash")?;
    let mut res = NarInfo {
        store_path: store_path.to_string(),
        url: format!("nar/{}.nar?hash={}", nar_hash, hash),
        compression: "none".into(),
        nar_hash: format!("sha256:{}", nar_hash),
        nar_size: path_info.nar_size,
        references: vec![],
        deriver: path_info
            .deriver
            .as_ref()
            .and_then(|d| extract_filename(&d.to_string())),
        sigs: vec![],
        ca: path_info.content_address.clone(),
    };

    if !path_info.references.is_empty() {
        res.references = path_info
            .references
            .iter()
            .filter_map(|r| extract_filename(r.as_str()))
            .collect::<Vec<String>>();
    }

    let fingerprint = fingerprint_path(
        virtual_nix_store,
        store_path,
        &res.nar_hash,
        res.nar_size,
        &path_info.references,
    )?;
    for sk in sign_keys {
        if let Some(ref fp) = fingerprint {
            res.sigs.push(sign_string(sk, fp));
        }
    }

    if res.sigs.is_empty() {
        res.sigs = path_info.signatures.clone();
    }

    Ok(Some(res))
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

    for sig in &narinfo.sigs {
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
    let store_path = some_or_404!(nixhash(&settings, &hash)
        .await
        .context("Could not query nar hash in database")?);
    let narinfo = match query_narinfo(
        settings.store.virtual_store(),
        &store_path,
        &hash,
        &settings.secret_keys,
        &settings,
    )
    .await?
    {
        Some(narinfo) => narinfo,
        None => {
            return Ok(HttpResponse::NotFound()
                .insert_header(cache_control_max_age_1d())
                .body("missed hash"))
        }
    };

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
