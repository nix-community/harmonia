use std::path::{Path, PathBuf};

use crate::error::IoErrorContext;
use actix_files::NamedFile;
use actix_web::Responder;
use actix_web::{HttpRequest, HttpResponse, web};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

use crate::template::{
    DIRECTORY_ROW_TEMPLATE, DIRECTORY_TEMPLATE, html_escape, render, render_page,
};
use crate::{
    CARGO_NAME, CARGO_VERSION, ServerResult, TAILWIND_CSS, config::Config, nixhash, some_or_404,
};

/// Percent-encode set for path segments placed inside `href="..."`: CONTROLS
/// plus the bytes that would break out of the attribute, alter the URL, or
/// collide with the `[[ ]]` template syntax.
const HREF_PATH_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'\'')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'&')
    .add(b'[')
    .add(b']')
    .add(b'?')
    .add(b'#');

// human readable file size
fn file_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.2} KiB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.2} MiB", bytes as f64 / 1024.0 / 1024.0)
    } else {
        format!("{:.2} GiB", bytes as f64 / 1024.0 / 1024.0 / 1024.0)
    }
}

pub(crate) fn directory_listing(
    url_prefix: &Path,
    fs_path: &Path,
    real_store: &Path,
) -> ServerResult {
    let path_without_store = fs_path.strip_prefix(real_store).unwrap_or(fs_path);
    let index_of = format!(
        "Index of {}",
        html_escape(&path_without_store.to_string_lossy())
    );
    let mut rows = String::new();

    for entry in fs_path
        .read_dir()
        .io_context(format!("cannot read directory: {}", fs_path.display()))?
    {
        let entry = entry.io_context(format!(
            "failed to read directory entry in: {}",
            fs_path.display()
        ))?;
        let p = match entry.path().strip_prefix(fs_path) {
            Ok(p) => url_prefix.join(p).to_string_lossy().into_owned(),
            Err(_) => continue,
        };

        // if file is a directory, add '/' to the end of the name
        if let Ok(metadata) = entry.metadata() {
            let mut row_vars = std::collections::HashMap::new();
            // Percent-encode for the URL, then HTML-escape for the attribute.
            let url = utf8_percent_encode(&p, HREF_PATH_SET).to_string();
            row_vars.insert("url", html_escape(&url));

            let name = html_escape(&entry.file_name().to_string_lossy());
            if metadata.is_dir() {
                row_vars.insert("name", format!("{name}/"));
                row_vars.insert("size", "-".to_string());
            } else {
                row_vars.insert("name", name);
                row_vars.insert("size", file_size(metadata.len()));
            }

            rows.push_str(&render(DIRECTORY_ROW_TEMPLATE, row_vars));
        } else {
            continue;
        }
    }

    let mut vars = std::collections::HashMap::new();
    vars.insert("index_of", index_of);
    vars.insert("rows", rows);

    let content = render(DIRECTORY_TEMPLATE, vars);
    let html = render_page(
        &format!("Nix binary cache ({CARGO_NAME} {CARGO_VERSION})"),
        TAILWIND_CSS,
        &content,
    );

    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html))
}

pub(crate) async fn get(
    path: web::Path<(String, PathBuf)>,
    req: HttpRequest,
    settings: web::Data<Config>,
) -> ServerResult {
    let (hash, dir) = path.into_inner();
    let dir = dir.strip_prefix("/").unwrap_or(&dir);

    let store_path_obj = some_or_404!(nixhash(&settings, hash.as_bytes())?);
    let store_path = settings.store.get_real_path(&store_path_obj);
    let full_path = if dir == Path::new("") {
        store_path.clone()
    } else {
        store_path.join(dir)
    };
    let full_path = match full_path.canonicalize() {
        Ok(p) => p,
        // A missing sub-path under a valid store path is a client lookup miss,
        // not a server fault; short-circuit before the generic Io mapping so
        // the response carries the same no-store 404 as `some_or_404!`.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(HttpResponse::NotFound()
                .insert_header(crate::cache_control_no_store())
                .body("missed hash"));
        }
        Err(e) => Err(e).io_context(format!(
            "cannot resolve nix store path: {}",
            full_path.display()
        ))?,
    };

    let real_store = settings
        .store
        .real_store()
        .canonicalize()
        .io_context(format!(
            "cannot resolve real nix store path: {}",
            settings.store.real_store().display()
        ))?;

    if !full_path.starts_with(&real_store) {
        return Ok(HttpResponse::NotFound().finish());
    }

    if full_path.is_dir() {
        let index_file = full_path.join("index.html");
        if index_file.metadata().is_ok_and(|stat| stat.is_file()) {
            return Ok(NamedFile::open_async(&index_file)
                .await
                .io_context(format!("cannot open {}", index_file.display()))?
                .respond_to(&req));
        }

        let url_prefix = PathBuf::from("/serve").join(&hash);
        let url_prefix = if dir == Path::new("") {
            url_prefix
        } else {
            url_prefix.join(dir)
        };
        directory_listing(&url_prefix, &full_path, &real_store)
    } else {
        Ok(NamedFile::open_async(&full_path)
            .await
            .io_context(format!("cannot open file: {}", full_path.display()))?
            .respond_to(&req))
    }
}
