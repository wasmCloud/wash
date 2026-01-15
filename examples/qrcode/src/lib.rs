use std::io::Cursor;

use image::{ImageFormat, Luma};
use qrcode::QrCode;
use serde::Deserialize;
use wstd::http::{Body, Request, Response, StatusCode, error::Context};

static UI_HTML: &str = include_str!("../ui.html");

#[wstd::http_server]
async fn main(req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    match router(req).await {
        Ok(resp) => Ok(resp),
        Err(e) => {
            eprintln!("Error handling request: {:?}", e);
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body("sadness. go check logs.\n".into())
                .map_err(Into::into)
        }
    }
}

async fn router(req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    match req.uri().path() {
        "/" => home(req).await,
        "/qrcode" => qrcode(req).await,
        _ => not_found(req).await,
    }
}

async fn home(_req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    Response::builder()
        .status(StatusCode::OK)
        .body(UI_HTML.into())
        .map_err(Into::into)
}

#[derive(Deserialize)]
struct QrRequest {
    payload: String,
}

async fn qrcode(mut req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    let js_req: QrRequest = req
        .body_mut()
        .json()
        .await
        .context("failed to parse body")?;

    let code = QrCode::new(&js_req.payload)?;

    let img = code.render::<Luma<u8>>().build();

    let mut body = vec![];
    img.write_to(&mut Cursor::new(&mut body), ImageFormat::Png)?;

    Response::builder()
        .header("Content-Type", "image/png")
        .status(StatusCode::OK)
        .body(body.into())
        .map_err(Into::into)
}

async fn not_found(_req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body("Not found\n".into())
        .map_err(Into::into)
}
