use anyhow::Result;

/// Find an available port by binding to a random port (0) and returning the assigned port.
pub async fn find_available_port() -> Result<u16> {
    use tokio::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    Ok(addr.port())
}
