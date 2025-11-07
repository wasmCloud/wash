// TODO: move to wasmcloud
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;
use tracing::{info, warn};
use wasmtime_wasi::cli::{IsTerminal, StdoutStream};

/// AsyncWrite implementation that forwards all writes to tracing macros
pub struct TracingAsyncWrite {
    component_name: String,
    is_stderr: bool,
}

impl TracingAsyncWrite {
    pub fn new(component_name: String, is_stderr: bool) -> Self {
        Self {
            component_name,
            is_stderr,
        }
    }
}

impl AsyncWrite for TracingAsyncWrite {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        // Convert bytes to string and log immediately
        if let Ok(text) = std::str::from_utf8(buf) {
            // Split by newlines and log each non-empty line
            for line in text.lines() {
                if !line.trim().is_empty() {
                    if self.is_stderr {
                        warn!(ctx = self.component_name, "{}", line);
                    } else {
                        info!(ctx = self.component_name, "{}", line);
                    }
                }
            }

            // Handle the case where buffer doesn't end with newline (partial line)
            if !text.is_empty() && !text.ends_with('\n') {
                // Find the last line (after the last newline)
                if let Some(last_line) = text.lines().last()
                    && !last_line.trim().is_empty()
                {
                    if self.is_stderr {
                        warn!(ctx = self.component_name, "{}", last_line);
                    } else {
                        info!(ctx = self.component_name, "{}", last_line);
                    }
                }
            }
        }

        // Always report successful write of entire buffer
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        // Nothing to flush since we log immediately
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        // Nothing to shutdown
        Poll::Ready(Ok(()))
    }
}

/// Unified stream implementation for both stdout and stderr that forwards to tracing
pub struct TracingStream {
    component_name: String,
    is_stderr: bool,
}

impl TracingStream {
    pub fn stdout(component_name: String) -> Self {
        Self {
            component_name,
            is_stderr: false,
        }
    }

    pub fn stderr(component_name: String) -> Self {
        Self {
            component_name,
            is_stderr: true,
        }
    }
}

impl StdoutStream for TracingStream {
    fn async_stream(&self) -> Box<dyn AsyncWrite + Send + Sync> {
        Box::new(TracingAsyncWrite::new(
            self.component_name.clone(),
            self.is_stderr,
        ))
    }
}

impl IsTerminal for TracingStream {
    fn is_terminal(&self) -> bool {
        false // We're not a TTY since we're forwarding to tracing
    }
}
