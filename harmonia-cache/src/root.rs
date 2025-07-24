use std::error::Error;

use actix_web::{http, web, HttpResponse};

use crate::BOOTSTRAP_SOURCE;
use crate::{config, CARGO_HOME_PAGE, CARGO_NAME, CARGO_VERSION};

pub(crate) async fn get(config: web::Data<config::Config>) -> Result<HttpResponse, Box<dyn Error>> {
    Ok(HttpResponse::Ok()
        .insert_header(http::header::ContentType(mime::TEXT_HTML_UTF_8))
        .body(format!(
            r#"
<!DOCTYPE html>
<html lang="en">

<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1, shrink-to-fit=no">
  <title>Nix binary cache ({CARGO_NAME} {CARGO_VERSION})</title>
  {BOOTSTRAP_SOURCE}
</head>
<body>
  <div class="container mt-3">
    <div class="row justify-content-md-center">
      <div class="col-md-auto">
        <p class="lead">
          This service, provides a "binary cache" for the
          <a href="https://nixos.org/nix/">Nix package manager</a>
        </p>
      </div>
    </div>
    <hr>
    <div class="row">
      <div class="col text-center">
        <h4 class="mb-3">Cache Info</h4>
        <p>Store Dir: {store}</p>
        <p>Want Mass Query: 1</p>
        <p>Priority: {priority}</p>
      </div>
    </div>
    <hr>
    <div class="row">
      <div class="col text-center">
        <small class="d-block mb-3 text-muted">
          Powered by <a href="{CARGO_HOME_PAGE}">{CARGO_NAME}</a>
        </small>
      </div>
    </div>
  </div>
</body>
</html>
"#,
            store = config.store.virtual_store(),
            priority = config.priority,
        )))
}
