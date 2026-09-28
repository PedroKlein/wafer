use std::convert::Infallible;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wafer_core::engine::{Capabilities, WaferEngine, WaferState};
use wafer_core::node::TransformNode;
use wafer_core::node::wasm::WasmTransformNode;
use wafer_core::orchestrator::hotswap::prepare_transform_swap_timed;
use wafer_core::queue::RuntimeEnvelope;
use wafer_core::runner::HotSwapProgress;
use wafer_types::config::{HttpScheme, OutboundHttpDestination};
use wasmtime::Store;
use wasmtime_wasi_http::{Error, WasiBody, WasiHttpView};

#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedWriter;

    fn make_writer(&'a self) -> Self::Writer {
        CapturedWriter(Arc::clone(&self.0))
    }
}

impl Write for CapturedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_poison| io::Error::other("log capture lock poisoned"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl CapturedLogs {
    fn text(&self) -> Result<String> {
        let bytes = self
            .0
            .lock()
            .map_err(|error| anyhow::anyhow!("log capture lock poisoned: {error}"))?
            .clone();
        String::from_utf8(bytes).context("captured logs must be UTF-8")
    }
}

const HTTP_TRANSFORM_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/http-transform/target/wasm32-wasip2/release/wafer_http_transform_fixture.wasm"
);
const TEST_MEMORY_LIMIT: usize = 64 * 1024 * 1024;

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

async fn load_http_transform(capabilities: Capabilities) -> Result<WasmTransformNode> {
    ensure!(
        std::path::Path::new(HTTP_TRANSFORM_WASM).is_file(),
        "mandatory HTTP fixture is missing: {HTTP_TRANSFORM_WASM}"
    );
    let engine = WaferEngine::new()?;
    engine.ensure_epoch_ticker();
    let component = engine.load_component(HTTP_TRANSFORM_WASM)?;
    let pre = Arc::new(engine.pre_instantiate_transform(&component)?);
    let state =
        WaferState::new_with_memory_limit("http-security", capabilities.clone(), TEST_MEMORY_LIMIT);
    let mut store = Store::new(engine.inner(), state);
    store.limiter(|state| state.limits_mut());
    if let Some(fuel) = engine.fuel_limit() {
        store.set_fuel(fuel.get())?;
    }
    if let Some(epoch) = engine.epoch_deadline() {
        store.epoch_deadline_trap();
        store.set_epoch_deadline(epoch.get());
    }
    let bindings = pre.instantiate_async(&mut store).await?;
    let mut node = WasmTransformNode::new(store, bindings, pre, engine.fuel_limit());
    node.configure_runtime(
        capabilities,
        TEST_MEMORY_LIMIT,
        engine.epoch_deadline(),
        "{}".to_string(),
    );
    node.validate_and_init("{}").await?;
    Ok(node)
}

async fn guest_request(
    node: &mut WasmTransformNode,
    method: &str,
    scheme: &str,
    authority: &str,
    path: &str,
    body: &str,
) -> Result<String> {
    let payload = format!("{method}\n{scheme}\n{authority}\n{path}\n{body}");
    let output = node
        .process(RuntimeEnvelope::from_string("http-test", &payload))
        .await
        .map_err(|error| anyhow::anyhow!("HTTP fixture trapped: {error:?}"))?;
    String::from_utf8(output.payload.to_vec()).context("HTTP fixture output must be UTF-8")
}

async fn guest_request_after_swap(
    node: &mut TransformNode,
    method: &str,
    scheme: &str,
    authority: &str,
    path: &str,
) -> Result<String> {
    let payload = format!("{method}\n{scheme}\n{authority}\n{path}\n");
    let output = node
        .process(RuntimeEnvelope::from_string("http-test", &payload))
        .await
        .map_err(|error| anyhow::anyhow!("HTTP fixture trapped: {error:?}"))?;
    String::from_utf8(output.payload.to_vec()).context("HTTP fixture output must be UTF-8")
}

async fn serve_once(listener: TcpListener, response: Vec<u8>) -> Result<()> {
    let _ = serve_and_capture(listener, response, None).await?;
    Ok(())
}

async fn serve_and_capture(
    listener: TcpListener,
    response: Vec<u8>,
    expected_body: Option<&[u8]>,
) -> Result<Vec<u8>> {
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
        let headers_complete = request.windows(4).any(|window| window == b"\r\n\r\n");
        if headers_complete
            && expected_body.is_none_or(|body| request.windows(body.len()).any(|w| w == body))
        {
            break;
        }
    }
    stream.write_all(&response).await.context("write response")?;
    Ok(request)
}

async fn serve_n(listener: TcpListener, response: &[u8], count: usize) -> Result<()> {
    for _ in 0..count {
        let (mut stream, _) = listener.accept().await.context("accept loopback request")?;
        let mut request = Vec::new();
        loop {
            let mut bytes = [0; 1024];
            let read = stream.read(&mut bytes).await.context("read request")?;
            if read == 0 {
                break;
            }
            let chunk = bytes.get(..read).context("read count exceeds buffer")?;
            request.extend_from_slice(chunk);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        stream.write_all(response).await.context("write response")?;
    }
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

#[tokio::test]
#[ignore = "run via eval/scripts/test-http-security.sh after building the fixture"]
async fn real_p2_component_default_denial_and_exact_allow() -> Result<()> {
    let denied_listener = TcpListener::bind("127.0.0.1:0").await?;
    let denied_port = denied_listener.local_addr()?.port();
    let mut denied = load_http_transform(Capabilities::sandbox()).await?;

    let result = guest_request(
        &mut denied,
        "GET",
        "http",
        &format!("127.0.0.1:{denied_port}"),
        "/denied",
        "",
    )
    .await?;
    ensure!(result == "error:ErrorCode::HttpRequestDenied", "unexpected denial result: {result}");
    ensure!(
        tokio::time::timeout(Duration::from_millis(50), denied_listener.accept()).await.is_err(),
        "denied guest request reached the server"
    );
    let empty_listener = TcpListener::bind("127.0.0.1:0").await?;
    let empty_port = empty_listener.local_addr()?.port();
    let mut empty = load_http_transform(Capabilities::default()).await?;
    let result =
        guest_request(&mut empty, "GET", "http", &format!("127.0.0.1:{empty_port}"), "/denied", "")
            .await?;
    ensure!(result == "error:ErrorCode::HttpRequestDenied");
    ensure!(
        tokio::time::timeout(Duration::from_millis(50), empty_listener.accept()).await.is_err(),
        "empty-grant guest request reached the server"
    );
    let allowed_listener = TcpListener::bind("127.0.0.1:0").await?;
    let allowed_port = allowed_listener.local_addr()?.port();
    let server = tokio::spawn(serve_once(
        allowed_listener,
        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    ));
    let mut allowed = load_http_transform(capability(allowed_port)?).await?;
    let result = guest_request(
        &mut allowed,
        "GET",
        "http",
        &format!("127.0.0.1:{allowed_port}"),
        "/allowed",
        "",
    )
    .await?;
    ensure!(result == "status:204", "unexpected allowed result: {result}");
    server.await??;
    Ok(())
}

#[tokio::test]
#[ignore = "run via eval/scripts/test-http-security.sh after building the fixture"]
async fn real_p2_component_rejects_authority_variations() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let other_port = port.checked_add(1).unwrap_or_else(|| port.saturating_sub(1));
    let mut node = load_http_transform(capability(port)?).await?;

    for (id, method, scheme, authority, expected) in [
        ("H04", "GET", "https", format!("127.0.0.1:{port}"), "error:ErrorCode::HttpRequestDenied"),
        ("H05", "GET", "http", format!("127.0.0.2:{port}"), "error:ErrorCode::HttpRequestDenied"),
        (
            "H06",
            "GET",
            "http",
            format!("127.0.0.1:{other_port}"),
            "error:ErrorCode::HttpRequestDenied",
        ),
        ("H07", "GET", "none", format!("127.0.0.1:{port}"), "error:ErrorCode::HttpProtocolError"),
    ] {
        let result = guest_request(&mut node, method, scheme, &authority, "/variation", "").await?;
        ensure!(result == expected, "{id} returned {result}, expected {expected}");
    }
    ensure!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept()).await.is_err(),
        "an authority variation reached the server"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "run via eval/scripts/test-http-security.sh after building the fixture"]
async fn real_p2_component_rejects_dns_loopback_and_connect() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let dns_destination = OutboundHttpDestination {
        scheme: HttpScheme::Http,
        host: "localhost".to_string(),
        port: Some(port),
    }
    .canonicalize()?;
    let mut dns =
        load_http_transform(Capabilities::sandbox().outbound_http(vec![dns_destination])).await?;
    let result =
        guest_request(&mut dns, "GET", "http", &format!("localhost:{port}"), "/dns", "").await?;
    ensure!(result == "error:ErrorCode::DestinationIpProhibited", "H10 returned {result}");
    ensure!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept()).await.is_err(),
        "prohibited DNS result reached the server"
    );
    let mut connect = load_http_transform(capability(port)?).await?;
    let result =
        guest_request(&mut connect, "CONNECT", "http", &format!("127.0.0.1:{port}"), "/", "")
            .await?;
    ensure!(result == "error:ErrorCode::HttpRequestDenied", "H14 returned {result}");
    ensure!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept()).await.is_err(),
        "CONNECT reached the server"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "run via eval/scripts/test-http-security.sh after building the fixture"]
async fn real_p2_component_does_not_expand_redirect_authority() -> Result<()> {
    let target = TcpListener::bind("127.0.0.1:0").await?;
    let target_port = target.local_addr()?.port();
    let allowed = TcpListener::bind("127.0.0.1:0").await?;
    let allowed_port = allowed.local_addr()?.port();
    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{target_port}/expanded\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let server = tokio::spawn(serve_once(allowed, response.into_bytes()));
    let mut node = load_http_transform(capability(allowed_port)?).await?;

    let result = guest_request(
        &mut node,
        "GET",
        "http",
        &format!("127.0.0.1:{allowed_port}"),
        "/redirect",
        "",
    )
    .await?;
    ensure!(result == "status:302", "redirect was not returned to the guest: {result}");
    server.await??;
    ensure!(
        tokio::time::timeout(Duration::from_millis(50), target.accept()).await.is_err(),
        "redirect target was followed automatically"
    );

    let follow_up = guest_request(
        &mut node,
        "GET",
        "http",
        &format!("127.0.0.1:{target_port}"),
        "/expanded",
        "",
    )
    .await?;
    ensure!(
        follow_up == "error:ErrorCode::HttpRequestDenied",
        "redirect follow-up escaped policy: {follow_up}"
    );
    ensure!(
        tokio::time::timeout(Duration::from_millis(50), target.accept()).await.is_err(),
        "denied redirect follow-up reached the target"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "run via eval/scripts/test-http-security.sh after building the fixture"]
async fn real_p2_component_redacts_request_data_from_host_logs() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let server = tokio::spawn(serve_and_capture(
        listener,
        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        Some(b"test-body-secret"),
    ));
    let capture = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(capture.clone())
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .context("install HTTP log capture subscriber")?;
    let mut node = load_http_transform(capability(port)?).await?;

    let result = guest_request(
        &mut node,
        "POST",
        "http",
        &format!("127.0.0.1:{port}"),
        "/submit?query=test-query-secret",
        "test-body-secret",
    )
    .await?;
    ensure!(result == "status:204", "H15 request failed: {result}");
    let request = String::from_utf8(server.await??).context("captured request must be UTF-8")?;
    ensure!(request.contains("test-body-secret"), "server did not receive the request body");
    ensure!(request.contains("Bearer test-auth-secret"), "server did not receive authorization");
    ensure!(request.contains("session=test-cookie-secret"), "server did not receive cookie");
    ensure!(request.contains("test-header-secret"), "server did not receive test header");

    let logs = capture.text()?;
    ensure!(logs.contains("http-security"), "host log omitted node identity: {logs}");
    ensure!(logs.contains("127.0.0.1"), "host log omitted destination identity: {logs}");
    for secret in [
        "test-query-secret",
        "test-body-secret",
        "test-auth-secret",
        "test-cookie-secret",
        "test-header-secret",
    ] {
        ensure!(!logs.contains(secret), "host logs exposed {secret}: {logs}");
    }
    Ok(())
}

#[tokio::test]
#[ignore = "run via eval/scripts/test-http-security.sh after building the fixture"]
async fn real_p2_component_preserves_grant_through_recovery_and_reconfigure() -> Result<()> {
    let allowed = TcpListener::bind("127.0.0.1:0").await?;
    let allowed_port = allowed.local_addr()?.port();
    let denied = TcpListener::bind("127.0.0.1:0").await?;
    let denied_port = denied.local_addr()?.port();
    let response = b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let server = tokio::spawn(serve_n(allowed, response, 2));
    let mut node = load_http_transform(capability(allowed_port)?).await?;

    let trap = node.process(RuntimeEnvelope::from_string("http-test", "trap")).await;
    ensure!(trap.is_err(), "fixture trap did not trap");
    node.recover_from_cached_pre().await?;
    let recovered = guest_request(
        &mut node,
        "GET",
        "http",
        &format!("127.0.0.1:{allowed_port}"),
        "/recovered",
        "",
    )
    .await?;
    ensure!(recovered == "status:204", "recovered Store lost grant: {recovered}");
    let denied_after_recovery =
        guest_request(&mut node, "GET", "http", &format!("127.0.0.1:{denied_port}"), "/denied", "")
            .await?;
    ensure!(denied_after_recovery == "error:ErrorCode::HttpRequestDenied");
    node.try_reconfigure(&format!(
        r#"{{"outbound_http":[{{"scheme":"http","host":"127.0.0.1","port":{denied_port}}}]}}"#
    ))
    .await?;
    let reconfigured = guest_request(
        &mut node,
        "GET",
        "http",
        &format!("127.0.0.1:{allowed_port}"),
        "/reconfigured",
        "",
    )
    .await?;
    ensure!(reconfigured == "status:204", "reconfigure lost original grant: {reconfigured}");
    let denied_after_reconfigure =
        guest_request(&mut node, "GET", "http", &format!("127.0.0.1:{denied_port}"), "/denied", "")
            .await?;
    ensure!(denied_after_reconfigure == "error:ErrorCode::HttpRequestDenied");
    server.await??;
    ensure!(
        tokio::time::timeout(Duration::from_millis(50), denied.accept()).await.is_err(),
        "lifecycle JSON expanded destination authority"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "run via eval/scripts/test-http-security.sh after building the fixture"]
async fn real_p2_component_hot_swap_retains_original_grant() -> Result<()> {
    let allowed = TcpListener::bind("127.0.0.1:0").await?;
    let allowed_port = allowed.local_addr()?.port();
    let denied = TcpListener::bind("127.0.0.1:0").await?;
    let denied_port = denied.local_addr()?.port();
    let server = tokio::spawn(serve_once(
        allowed,
        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    ));
    let capabilities = capability(allowed_port)?;
    let node = load_http_transform(capabilities.clone()).await?;
    let mut node = TransformNode::from(node);
    let engine = Arc::new(WaferEngine::new()?);
    let (progress, _completion) = HotSwapProgress::channel();
    let replacement = prepare_transform_swap_timed(
        &engine,
        &std::fs::read(HTTP_TRANSFORM_WASM)?,
        "http-security",
        capabilities,
        TEST_MEMORY_LIMIT,
        progress,
    )
    .await?;

    replacement.payload.try_apply_transform(&mut node).await?;
    let allowed_result = guest_request_after_swap(
        &mut node,
        "GET",
        "http",
        &format!("127.0.0.1:{allowed_port}"),
        "/after-swap",
    )
    .await?;
    ensure!(allowed_result == "status:204", "hot-swap lost grant: {allowed_result}");
    server.await??;

    let denied_result = guest_request_after_swap(
        &mut node,
        "GET",
        "http",
        &format!("127.0.0.1:{denied_port}"),
        "/expanded",
    )
    .await?;
    ensure!(denied_result == "error:ErrorCode::HttpRequestDenied");
    ensure!(
        tokio::time::timeout(Duration::from_millis(50), denied.accept()).await.is_err(),
        "replacement expanded destination authority"
    );
    Ok(())
}
