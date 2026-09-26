use std::convert::Infallible;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wafer_core::engine::{Capabilities, WaferState};
use wafer_types::config::{HttpScheme, OutboundHttpDestination};
use wasmtime_wasi_http::{Error, WasiBody, WasiHttpView};

fn capability(port: u16) -> Result<Capabilities> {
    let destination = OutboundHttpDestination {
        scheme: HttpScheme::Http,
        host: "127.0.0.1".to_string(),
        port: Some(port),
    }
    .canonicalize()
    .context("loopback literal grant must be valid")?;
    Ok(Capabilities::sandbox().outbound_http(vec![destination]))
}

fn request(uri: String) -> Result<hyper::http::Request<WasiBody>> {
    request_with_method(hyper::http::Method::GET, uri)
}

fn request_with_method(
    method: hyper::http::Method,
    uri: String,
) -> Result<hyper::http::Request<WasiBody>> {
    hyper::http::Request::builder()
        .method(method)
        .uri(uri)
        .body(Empty::<Bytes>::new().map_err(|error: Infallible| match error {}).boxed_unsync())
        .context("test request URI must be valid")
}

async fn send(
    state: &mut WaferState,
    request: hyper::http::Request<WasiBody>,
) -> Result<hyper::http::Response<WasiBody>, Error> {
    let future = state.http().hooks.send_request(request, None, Box::new(async { Ok(()) }));
    let (response, connection) = std::pin::Pin::from(future).await?;
    std::pin::Pin::from(connection).await?;
    Ok(response)
}

async fn serve_once(listener: TcpListener, response: Vec<u8>) -> Result<()> {
    let (mut stream, _) = listener.accept().await.context("accept loopback request")?;
    let mut request = Vec::new();
    loop {
        let mut bytes = [0; 1024];
        let count = stream.read(&mut bytes).await.context("read request")?;
        if count == 0 {
            break;
        }
        let chunk = bytes.get(..count).context("read count exceeds buffer")?;
        request.extend_from_slice(chunk);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    stream.write_all(&response).await.context("write response")?;
    Ok(())
}

#[tokio::test]
async fn omitted_outbound_http_denies_before_loopback_connect() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let mut state = WaferState::new("denied", Capabilities::sandbox());

    let result = send(&mut state, request(format!("http://127.0.0.1:{port}/denied"))?).await;

    ensure!(matches!(result, Err(Error::HttpRequestDenied)));
    ensure!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept()).await.is_err(),
        "denied request reached the server"
    );
    Ok(())
}

#[tokio::test]
async fn exact_loopback_destination_is_allowed() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let server = tokio::spawn(serve_once(
        listener,
        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    ));
    let mut state = WaferState::new("allowed", capability(port)?);

    let response = send(&mut state, request(format!("http://127.0.0.1:{port}/allowed"))?).await?;

    ensure!(response.status() == hyper::http::StatusCode::NO_CONTENT);
    server.await??;
    Ok(())
}

#[tokio::test]
async fn mismatched_port_is_denied_before_connect() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let different_port = port.checked_add(1).unwrap_or_else(|| port.saturating_sub(1));
    let mut state = WaferState::new("wrong-port", capability(different_port)?);

    let result = send(&mut state, request(format!("http://127.0.0.1:{port}/denied"))?).await;

    ensure!(matches!(result, Err(Error::HttpRequestDenied)));
    ensure!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept()).await.is_err(),
        "denied request reached the server"
    );
    Ok(())
}

#[tokio::test]
async fn scheme_and_host_mismatches_are_denied() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let mut state = WaferState::new("mismatch", capability(port)?);

    for uri in [
        format!("https://127.0.0.1:{port}/wrong-scheme"),
        format!("http://127.0.0.2:{port}/wrong-host"),
    ] {
        let result = send(&mut state, request(uri)?).await;
        ensure!(matches!(result, Err(Error::HttpRequestDenied)));
    }
    ensure!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept()).await.is_err(),
        "mismatched request reached the server"
    );
    Ok(())
}

#[tokio::test]
async fn dns_name_resolving_only_to_loopback_is_prohibited() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let destination = OutboundHttpDestination {
        scheme: HttpScheme::Http,
        host: "localhost".to_string(),
        port: Some(port),
    }
    .canonicalize()?;
    let mut state =
        WaferState::new("dns-loopback", Capabilities::sandbox().outbound_http(vec![destination]));

    let result = send(&mut state, request(format!("http://localhost:{port}/denied"))?).await;

    ensure!(matches!(result, Err(Error::DestinationIpProhibited)));
    ensure!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept()).await.is_err(),
        "prohibited DNS result reached the server"
    );
    Ok(())
}

#[tokio::test]
async fn connect_method_is_denied_before_loopback_connect() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let mut state = WaferState::new("connect", capability(port)?);

    let result = send(
        &mut state,
        request_with_method(hyper::http::Method::CONNECT, format!("http://127.0.0.1:{port}/"))?,
    )
    .await;

    ensure!(matches!(result, Err(Error::HttpRequestDenied)));
    ensure!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept()).await.is_err(),
        "CONNECT reached the server"
    );
    Ok(())
}

#[tokio::test]
async fn redirect_is_returned_without_contacting_location() -> Result<()> {
    let redirect_target = TcpListener::bind("127.0.0.1:0").await?;
    let redirect_port = redirect_target.local_addr()?.port();
    let allowed = TcpListener::bind("127.0.0.1:0").await?;
    let allowed_port = allowed.local_addr()?.port();
    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{redirect_port}/expanded\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let server = tokio::spawn(serve_once(allowed, response.into_bytes()));
    let mut state = WaferState::new("redirect", capability(allowed_port)?);

    let result =
        send(&mut state, request(format!("http://127.0.0.1:{allowed_port}/redirect"))?).await?;

    ensure!(result.status() == hyper::http::StatusCode::FOUND);
    server.await??;
    ensure!(
        tokio::time::timeout(Duration::from_millis(50), redirect_target.accept()).await.is_err(),
        "redirect target was contacted"
    );
    Ok(())
}
