use anyhow::Context as _;

use serde::Deserialize;
use wstd::http::{Body, Request, Response, StatusCode};
use wstd::io::{AsyncRead, AsyncWrite};

static UI_HTML: &str = include_str!("../ui.html");

#[wstd::http_server]
async fn main(req: Request<Body>) -> anyhow::Result<Response<Body>> {
    match req.uri().path() {
        "/" => home(req).await,
        "/task" => handle_task(req).await,
        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body("Not found\n".into())
            .map_err(Into::into),
    }
}

async fn home(_req: Request<Body>) -> anyhow::Result<Response<Body>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/html")
        .body(UI_HTML.into())
        .map_err(Into::into)
}

#[derive(Deserialize)]
struct TaskRequest {
    payload: String,
}

async fn handle_task(mut req: Request<Body>) -> anyhow::Result<Response<Body>> {
    let task_request: TaskRequest = req
        .body_mut()
        .json()
        .await
        .context("failed to parse body")?;

    let body = task_request.payload.into_bytes();

    let client = wstd::net::TcpStream::connect("127.0.0.1:7777").await?;
    // write the payload followed by a newline
    // read the response until a newline is encountered
    // with a timeout of 5 seconds
    // disconnect
    let (mut reader, mut writer) = client.split();
    writer.write_all(&body).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    let mut resp_buf = Vec::new();
    let mut buf = [0u8; 1024];

    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            return Err(anyhow::anyhow!(
                "connection closed before response was complete"
            ));
        }
        for &byte in &buf[..n] {
            if byte == b'\n' {
                let response = String::from_utf8_lossy(&resp_buf).to_string();
                let resp = Response::builder()
                    .status(StatusCode::OK)
                    .body(response.into())?;
                return Ok(resp);
            } else {
                resp_buf.push(byte);
            }
        }
    }
}
