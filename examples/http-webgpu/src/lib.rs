use wstd::http::body::{BodyForthcoming, IncomingBody};
use wstd::http::server::{Finished, Responder};
use wstd::http::{IntoBody, Request, Response, StatusCode};
use wstd::io::{copy, empty, AsyncWrite};
use wstd::time::{Duration, Instant};

#[wstd::http_server]
async fn main(req: Request<IncomingBody>, res: Responder) -> Finished {
    match req.uri().path_and_query().unwrap().as_str() {
        "/wait" => wait(req, res).await,
        "/echo" => echo(req, res).await,
        "/echo-headers" => echo_headers(req, res).await,
        "/echo-trailers" => echo_trailers(req, res).await,
        "/" => home(req, res).await,
        _ => not_found(req, res).await,
    }
}

async fn home(_req: Request<IncomingBody>, res: Responder) -> Finished {
    let instance = wgpu::Instance::new(Default::default());
    let adapter = instance.request_adapter(&Default::default()).await.unwrap();
    let (device, queue) = adapter.request_device(&Default::default(), None).await.unwrap();

    println!("device: {:#?}", device);
    println!("queue: {:#?}", queue);

    // To send a single string as the response body, use `res::respond`.
    res.respond(Response::new("Hello, wasi:http/proxy world!\n".into_body()))
        .await
}

async fn wait(_req: Request<IncomingBody>, res: Responder) -> Finished {
    // Get the time now
    let now = Instant::now();

    // Sleep for one second.
    wstd::task::sleep(Duration::from_secs(1)).await;

    // Compute how long we slept for.
    let elapsed = Instant::now().duration_since(now).as_millis();

    // To stream data to the response body, use `res::start_response`.
    let mut body = res.start_response(Response::new(BodyForthcoming));
    let result = body
        .write_all(format!("slept for {elapsed} millis\n").as_bytes())
        .await;
    Finished::finish(body, result, None)
}

async fn echo(mut req: Request<IncomingBody>, res: Responder) -> Finished {
    // Stream data from the req body to the response body.
    let mut body = res.start_response(Response::new(BodyForthcoming));
    let result = copy(req.body_mut(), &mut body).await;
    Finished::finish(body, result, None)
}

async fn echo_headers(req: Request<IncomingBody>, responder: Responder) -> Finished {
    let mut res = Response::builder();
    *res.headers_mut().unwrap() = req.into_parts().0.headers;
    let res = res.body(empty()).unwrap();
    responder.respond(res).await
}

async fn echo_trailers(req: Request<IncomingBody>, res: Responder) -> Finished {
    let body = res.start_response(Response::new(BodyForthcoming));
    let (trailers, result) = match req.into_body().finish().await {
        Ok(trailers) => (trailers, Ok(())),
        Err(err) => (Default::default(), Err(std::io::Error::other(err))),
    };
    Finished::finish(body, result, trailers)
}

async fn not_found(_req: Request<IncomingBody>, responder: Responder) -> Finished {
    let res = Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(empty())
        .unwrap();
    responder.respond(res).await
}
