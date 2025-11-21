use anyhow::{anyhow, bail, Result};
use futures::{SinkExt, StreamExt};
use url::Url;

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::{
    exports::wasi::http::incoming_handler::Guest,
    wasi::http::types::{
        Fields, IncomingRequest, IncomingResponse, Method, OutgoingBody, OutgoingRequest,
        OutgoingResponse, ResponseOutparam, Scheme,
    },
};

const BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:streamGenerateContent";
const API_KEY: &str = "YOUR_API_KEY";

struct Component;

impl Guest for Component {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        executor::run(async move {
            handle_request(request, response_out).await;
        })
    }
}

async fn handle_request(request: IncomingRequest, response_out: ResponseOutparam) {
    assert!(request.authority().is_some());

    match (request.method(), request.path_with_query().as_deref()) {
        (Method::Post, Some("/gemini-proxy")) => {
            // Pipe the request body to Gemini API and stream the response back to the client.

            match gemini_proxy(request).await {
                Ok(response) => {
                    let mut stream = executor::incoming_body(
                        response.consume().expect("response should be consumable"),
                    );

                    let response = OutgoingResponse::new(
                        Fields::from_list(&[
                            ("content-type".to_string(), b"text/event-stream".to_vec()),
                            ("cache-control".to_string(), b"no-cache".to_vec()),
                        ])
                        .unwrap(),
                    );

                    let mut body = executor::outgoing_body(
                        response.body().expect("response should be writable"),
                    );

                    ResponseOutparam::set(response_out, Ok(response));

                    while let Some(chunk) = stream.next().await {
                        match chunk {
                            Ok(data) => {
                                if let Err(_) = body.send(data).await {
                                    break;
                                }
                            }
                            Err(_) => {
                                break;
                            }
                        }
                    }
                }

                Err(_) => {
                    server_error(response_out);
                }
            }
        }

        _ => {
            eprintln!("[COMPONENT] No route matched, returning 405");
            method_not_allowed(response_out);
        }
    }
}

async fn gemini_proxy(incoming_request: IncomingRequest) -> Result<IncomingResponse> {
    // NOTE: Replace `YOUR_API_KEY` with your actual API key or wire it up from env/config.
    // Putting the key in the URL query ensures the request doesn't rely on non-standard headers
    // which some WASI hosts might strip.
    let full_url = format!("{}?alt=sse&key={}", BASE, API_KEY);
    let url: Url = Url::parse(&full_url)?;

    // Build outgoing headers
    let headers = Fields::new();
    headers
        .append(
            &"content-type".to_string(),
            &b"application/json; charset=utf-8".to_vec(),
        )
        .map_err(|_| anyhow!("failed to append content-type header"))?;

    let outgoing_request = OutgoingRequest::new(headers);

    outgoing_request
        .set_method(&Method::Post)
        .map_err(|()| anyhow!("failed to set method"))?;

    // Use the full path + query (CRITICAL for streaming)
    let path_with_query = match url.query() {
        Some(q) => format!("{}?{}", url.path(), q),
        None => url.path().to_string(),
    };

    outgoing_request
        .set_path_with_query(Some(&path_with_query))
        .map_err(|()| anyhow!("failed to set path_with_query"))?;

    outgoing_request
        .set_scheme(Some(&match url.scheme() {
            "http" => Scheme::Http,
            "https" => Scheme::Https,
            scheme => Scheme::Other(scheme.into()),
        }))
        .map_err(|()| anyhow!("failed to set scheme"))?;

    outgoing_request
        .set_authority(Some(&format!(
            "{}{}",
            url.host_str().unwrap_or(""),
            if let Some(port) = url.port() {
                format!(":{port}")
            } else {
                String::new()
            }
        )))
        .map_err(|()| anyhow!("failed to set authority"))?;

    // Read the incoming prompt from the client's request body (fully) FIRST
    let mut incoming_stream = executor::incoming_body(
        incoming_request
            .consume()
            .expect("request should be consumable"),
    );

    let mut prompt_bytes = Vec::new();
    while let Some(chunk) = incoming_stream.next().await {
        prompt_bytes.extend_from_slice(&chunk?);
    }

    let prompt = std::str::from_utf8(&prompt_bytes)
        .map_err(|e| anyhow!("Invalid UTF-8 in prompt: {}", e))?
        .to_string();

    // Safely escape JSON special characters for embedding in a string literal
    let escaped_prompt = prompt
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");

    // Build the Gemini JSON request body. Adjust structure if you want different model args.
    let json_request = format!(
        r#"{{
  "contents": [
    {{
      "parts": [
        {{
          "text": "{}"
        }}
      ]
    }}
  ]
}}"#,
        escaped_prompt
    );

    // NOW get the outgoing body and send immediately
    let mut body = executor::outgoing_body(
        outgoing_request
            .body()
            .expect("request body should be writable"),
    );

    // Send the JSON payload RIGHT NOW (before awaiting response)
    body.send(json_request.into_bytes()).await?;

    // Drop body to signal we're done writing (triggers request completion)
    drop(body);

    // NOW send the outgoing request and await the response
    let response = executor::outgoing_request_send(outgoing_request).await?;

    let status = response.status();

    if !(200..300).contains(&status) {
        bail!("unexpected status: {status}");
    }

    Ok(response)
}

fn server_error(response_out: ResponseOutparam) {
    respond(500, response_out)
}

fn method_not_allowed(response_out: ResponseOutparam) {
    respond(405, response_out)
}

fn respond(status: u16, response_out: ResponseOutparam) {
    let response = OutgoingResponse::new(Fields::new());
    response
        .set_status_code(status)
        .expect("setting status code");

    let body = response.body().expect("response should be writable");

    ResponseOutparam::set(response_out, Ok(response));

    OutgoingBody::finish(body, None).expect("outgoing-body.finish");
}

mod executor {
    use crate::bindings::wasi::{
        http::{
            outgoing_handler,
            types::{
                self, FutureTrailers, IncomingBody, IncomingResponse, InputStream, OutgoingBody,
                OutgoingRequest, OutputStream,
            },
        },
        io::{self, streams::StreamError},
    };
    use anyhow::{anyhow, Error, Result};
    use futures::{future, sink, stream, Sink, Stream};
    use std::{
        cell::RefCell,
        future::Future,
        mem,
        rc::Rc,
        sync::{Arc, Mutex},
        task::{Context, Poll, Wake, Waker},
    };

    const READ_SIZE: u64 = 16 * 1024;

    static WAKERS: Mutex<Vec<(io::poll::Pollable, Waker)>> = Mutex::new(Vec::new());

    pub fn run<T>(future: impl Future<Output = T>) -> T {
        futures::pin_mut!(future);

        struct DummyWaker;

        impl Wake for DummyWaker {
            fn wake(self: Arc<Self>) {}
        }

        let waker = Arc::new(DummyWaker).into();

        loop {
            match future.as_mut().poll(&mut Context::from_waker(&waker)) {
                Poll::Pending => {
                    let mut new_wakers = Vec::new();

                    let wakers = mem::take::<Vec<_>>(&mut WAKERS.lock().unwrap());

                    assert!(!wakers.is_empty());

                    let pollables = wakers
                        .iter()
                        .map(|(pollable, _)| pollable)
                        .collect::<Vec<_>>();

                    let mut ready = vec![false; wakers.len()];

                    for index in io::poll::poll(&pollables) {
                        ready[usize::try_from(index).unwrap()] = true;
                    }

                    for (ready, (pollable, waker)) in ready.into_iter().zip(wakers) {
                        if ready {
                            waker.wake()
                        } else {
                            new_wakers.push((pollable, waker));
                        }
                    }

                    *WAKERS.lock().unwrap() = new_wakers;
                }
                Poll::Ready(result) => break result,
            }
        }
    }

    pub fn outgoing_body(body: OutgoingBody) -> impl Sink<Vec<u8>, Error = Error> {
        struct Outgoing(Option<(OutputStream, OutgoingBody)>);

        impl Drop for Outgoing {
            fn drop(&mut self) {
                if let Some((stream, body)) = self.0.take() {
                    drop(stream);
                    OutgoingBody::finish(body, None).expect("outgoing-body.finish");
                }
            }
        }

        let stream = body.write().expect("response body should be writable");
        let pair = Rc::new(RefCell::new(Outgoing(Some((stream, body)))));

        sink::unfold((), {
            move |(), chunk: Vec<u8>| {
                future::poll_fn({
                    let mut offset = 0;
                    let mut flushing = false;
                    let pair = pair.clone();

                    move |context| {
                        let pair = pair.borrow();
                        let (stream, _) = &pair.0.as_ref().unwrap();

                        loop {
                            match stream.check_write() {
                                Ok(0) => {
                                    WAKERS
                                        .lock()
                                        .unwrap()
                                        .push((stream.subscribe(), context.waker().clone()));

                                    break Poll::Pending;
                                }
                                Ok(count) => {
                                    if offset == chunk.len() {
                                        if flushing {
                                            break Poll::Ready(Ok(()));
                                        } else {
                                            stream.flush().expect("stream should be flushable");
                                            flushing = true;
                                        }
                                    } else {
                                        let count = usize::try_from(count)
                                            .unwrap()
                                            .min(chunk.len() - offset);

                                        match stream.write(&chunk[offset..][..count]) {
                                            Ok(()) => {
                                                offset += count;
                                            }
                                            Err(_) => break Poll::Ready(Err(anyhow!("I/O error"))),
                                        }
                                    }
                                }
                                Err(_) => break Poll::Ready(Err(anyhow!("I/O error"))),
                            }
                        }
                    }
                })
            }
        })
    }

    pub fn outgoing_request_send(
        request: OutgoingRequest,
    ) -> impl Future<Output = Result<IncomingResponse, types::ErrorCode>> {
        future::poll_fn({
            let response = outgoing_handler::handle(request, None);

            move |context| match &response {
                Ok(response) => {
                    if let Some(response) = response.get() {
                        Poll::Ready(response.unwrap())
                    } else {
                        WAKERS
                            .lock()
                            .unwrap()
                            .push((response.subscribe(), context.waker().clone()));
                        Poll::Pending
                    }
                }
                Err(error) => Poll::Ready(Err(error.clone())),
            }
        })
    }

    pub fn incoming_body(body: IncomingBody) -> impl Stream<Item = Result<Vec<u8>>> {
        enum Inner {
            Stream {
                stream: InputStream,
                body: IncomingBody,
            },
            Trailers(FutureTrailers),
            Closed,
        }

        struct Incoming(Inner);

        impl Drop for Incoming {
            fn drop(&mut self) {
                match mem::replace(&mut self.0, Inner::Closed) {
                    Inner::Stream { stream, body } => {
                        drop(stream);
                        IncomingBody::finish(body);
                    }
                    Inner::Trailers(_) | Inner::Closed => {}
                }
            }
        }

        stream::poll_fn({
            let stream = body.stream().expect("response body should be readable");
            let mut incoming = Incoming(Inner::Stream { stream, body });

            move |context| {
                loop {
                    match &incoming.0 {
                        Inner::Stream { stream, .. } => match stream.read(READ_SIZE) {
                            Ok(buffer) => {
                                return if buffer.is_empty() {
                                    WAKERS
                                        .lock()
                                        .unwrap()
                                        .push((stream.subscribe(), context.waker().clone()));
                                    Poll::Pending
                                } else {
                                    Poll::Ready(Some(Ok(buffer)))
                                };
                            }
                            Err(StreamError::Closed) => {
                                let Inner::Stream { stream, body } =
                                    mem::replace(&mut incoming.0, Inner::Closed)
                                else {
                                    unreachable!();
                                };
                                drop(stream);
                                incoming.0 = Inner::Trailers(IncomingBody::finish(body));
                            }
                            Err(StreamError::LastOperationFailed(error)) => {
                                return Poll::Ready(Some(Err(anyhow!(
                                    "{}",
                                    error.to_debug_string()
                                ))));
                            }
                        },

                        Inner::Trailers(trailers) => {
                            match trailers.get() {
                                Some(Ok(trailers)) => {
                                    incoming.0 = Inner::Closed;
                                    match trailers {
                                        Ok(Some(_)) => {
                                            // Currently, we just ignore any trailers.  TODO: Add a test that
                                            // expects trailers and verify they match the expected contents.
                                        }
                                        Ok(None) => {
                                            // No trailers; nothing else to do.
                                        }
                                        Err(error) => {
                                            // Error reading the trailers: pass it on to the application.
                                            return Poll::Ready(Some(Err(anyhow!("{error:?}"))));
                                        }
                                    }
                                }
                                Some(Err(_)) => {
                                    // Should only happen if we try to retrieve the trailers twice, i.e. a bug in
                                    // this code.
                                    unreachable!();
                                }
                                None => {
                                    WAKERS
                                        .lock()
                                        .unwrap()
                                        .push((trailers.subscribe(), context.waker().clone()));
                                    return Poll::Pending;
                                }
                            }
                        }

                        Inner::Closed => {
                            return Poll::Ready(None);
                        }
                    }
                }
            }
        })
    }
}

bindings::export!(Component with_types_in bindings);
