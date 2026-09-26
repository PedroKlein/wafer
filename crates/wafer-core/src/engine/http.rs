use std::future::{Future, poll_fn};
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::task::{Poll, ready};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::Body;
use hyper::http::{Method, Request, Response, Uri};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use wafer_types::config::{
    CanonicalHttpDestination, HttpHost, HttpScheme, OutboundHttpDestination, permitted_dns_ip,
};
use wasmtime_wasi_http::{Error, RequestOptions, WasiBody, WasiHttpHooks};

pub(super) struct OutboundHttpHooks {
    node_id: Box<str>,
    destinations: Arc<[CanonicalHttpDestination]>,
}

impl OutboundHttpHooks {
    pub(super) fn new(
        node_id: impl Into<Box<str>>,
        destinations: Arc<[CanonicalHttpDestination]>,
    ) -> Self {
        Self { node_id: node_id.into(), destinations }
    }

    fn authorize(&self, request: &Request<WasiBody>) -> Result<AuthorizedDestination, Error> {
        if request.method() == Method::CONNECT {
            return Err(Error::HttpRequestDenied);
        }
        let uri = request.uri();
        let scheme = match uri.scheme_str() {
            Some("http") => HttpScheme::Http,
            Some("https") => HttpScheme::Https,
            _ => return Err(Error::HttpRequestDenied),
        };
        let authority = uri.authority().ok_or(Error::HttpRequestDenied)?;
        let authority_text = authority.as_str();
        let explicit_port = authority_text.rfind(']').map_or_else(
            || authority_text.contains(':'),
            |index| {
                authority_text
                    .get(index.saturating_add(1)..)
                    .is_some_and(|suffix| suffix.starts_with(':'))
            },
        );
        if explicit_port && authority.port_u16().is_none() {
            return Err(Error::HttpRequestDenied);
        }
        let port = authority.port_u16().unwrap_or_else(|| scheme.default_port());
        let requested = OutboundHttpDestination {
            scheme,
            host: authority.host().to_string(),
            port: Some(port),
        }
        .canonicalize()
        .map_err(|_error| Error::HttpRequestDenied)?;
        let destination = self
            .destinations
            .iter()
            .find(|destination| **destination == requested)
            .cloned()
            .ok_or(Error::HttpRequestDenied)?;
        Ok(AuthorizedDestination { destination })
    }
}

impl WasiHttpHooks for OutboundHttpHooks {
    fn default_scheme(&mut self) -> Option<hyper::http::uri::Scheme> {
        None
    }

    fn send_request(
        &mut self,
        request: Request<WasiBody>,
        options: Option<RequestOptions>,
        response_error: Box<dyn Future<Output = Result<(), Error>> + Send>,
    ) -> Box<
        dyn Future<
                Output = Result<
                    (Response<WasiBody>, Box<dyn Future<Output = Result<(), Error>> + Send>),
                    Error,
                >,
            > + Send,
    > {
        let authorization = self.authorize(&request);
        let node_id = self.node_id.clone();
        Box::new(async move {
            let destination = authorization?;
            tracing::debug!(node = %node_id, destination = %destination, "outbound HTTP request allowed");
            let (response, io) = send_request(request, options, destination).await?;
            drop(response_error);
            Ok((response, io))
        })
    }
}

#[derive(Clone)]
struct AuthorizedDestination {
    destination: CanonicalHttpDestination,
}

impl std::fmt::Display for AuthorizedDestination {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scheme = match self.destination.scheme {
            HttpScheme::Http => "http",
            HttpScheme::Https => "https",
        };
        match &self.destination.host {
            HttpHost::Dns(host) => write!(formatter, "{scheme}://{host}:{}", self.destination.port),
            HttpHost::Ip(std::net::IpAddr::V4(host)) => {
                write!(formatter, "{scheme}://{host}:{}", self.destination.port)
            }
            HttpHost::Ip(std::net::IpAddr::V6(host)) => {
                write!(formatter, "{scheme}://[{host}]:{}", self.destination.port)
            }
        }
    }
}

impl AuthorizedDestination {
    async fn socket_addr(&self) -> Result<std::net::SocketAddr, Error> {
        match &self.destination.host {
            HttpHost::Ip(ip) => Ok(std::net::SocketAddr::new(*ip, self.destination.port)),
            HttpHost::Dns(host) => {
                let addresses = tokio::net::lookup_host((host.as_ref(), self.destination.port))
                    .await
                    .map_err(|_error| Error::DnsError { rcode: None, info_code: None })?;
                let mut found = false;
                for address in addresses {
                    found = true;
                    if permitted_dns_ip(address.ip()) {
                        return Ok(address);
                    }
                }
                if found {
                    Err(Error::DestinationIpProhibited)
                } else {
                    Err(Error::DestinationNotFound)
                }
            }
        }
    }

    fn tls_server_name(&self) -> Result<rustls::pki_types::ServerName<'static>, Error> {
        match &self.destination.host {
            HttpHost::Dns(host) => rustls::pki_types::ServerName::try_from(host.to_string())
                .map_err(|_error| Error::ConfigurationError),
            HttpHost::Ip(ip) => Ok(rustls::pki_types::ServerName::IpAddress((*ip).into())),
        }
    }
}

trait TokioStream: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static {
    fn boxed(self) -> Box<dyn TokioStream>
    where
        Self: Sized,
    {
        Box::new(self)
    }
}

impl<T> TokioStream for T where T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static {}

async fn send_request(
    mut request: Request<WasiBody>,
    options: Option<RequestOptions>,
    destination: AuthorizedDestination,
) -> Result<(Response<WasiBody>, Box<dyn Future<Output = Result<(), Error>> + Send>), Error> {
    let connect_timeout =
        options.and_then(|options| options.connect_timeout).unwrap_or(Duration::from_secs(600));
    let first_byte_timeout =
        options.and_then(|options| options.first_byte_timeout).unwrap_or(Duration::from_secs(600));
    let between_bytes_timeout = options
        .and_then(|options| options.between_bytes_timeout)
        .unwrap_or(Duration::from_secs(600));

    let address = destination.socket_addr().await?;
    let stream = tokio::time::timeout(connect_timeout, TcpStream::connect(address))
        .await
        .map_err(|_elapsed| Error::ConnectionTimeout)?
        .map_err(Error::Connect)?;
    let stream = if destination.destination.scheme == HttpScheme::Https {
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.into() };
        let config =
            rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(destination.tls_server_name()?, stream)
            .await
            .map_err(Error::Tls)?
            .boxed()
    } else {
        stream.boxed()
    };
    let (mut sender, connection) = tokio::time::timeout(
        connect_timeout,
        hyper::client::conn::http1::Builder::new()
            .handshake(wasmtime_wasi_http::io::TokioIo::new(stream)),
    )
    .await
    .map_err(|_elapsed| Error::ConnectionTimeout)??;

    let path = request.uri().path_and_query().map_or("/", |value| value.as_str());
    *request.uri_mut() = Uri::builder()
        .path_and_query(path)
        .build()
        .map_err(|_error| Error::HttpRequestUriInvalid)?;

    let send = async move {
        let response = tokio::time::timeout(first_byte_timeout, sender.send_request(request))
            .await
            .map_err(|_elapsed| Error::ConnectionReadTimeout)?
            .map_err(Error::from)?;
        let mut timeout = tokio::time::interval(between_bytes_timeout);
        timeout.reset();
        Ok(response.map(|incoming| IncomingResponseBody { incoming, timeout }))
    };
    let mut send = pin!(send);
    let mut connection = Some(connection);
    let response = poll_fn(|context| match send.as_mut().poll(context) {
        Poll::Ready(result) => Poll::Ready(result),
        Poll::Pending => {
            let Some(future) = connection.as_mut() else {
                return Poll::Pending;
            };
            match ready!(Pin::new(future).poll(context)) {
                Ok(()) => send.as_mut().poll(context),
                Err(error) => Poll::Ready(Err(Error::from(error))),
            }
        }
    })
    .await?;
    Ok((
        response.map(BodyExt::boxed_unsync),
        Box::new(async move {
            let Some(connection) = connection.take() else {
                return Ok(());
            };
            connection.await.map_err(Error::from)
        }),
    ))
}

struct IncomingResponseBody {
    incoming: hyper::body::Incoming,
    timeout: tokio::time::Interval,
}

impl Body for IncomingResponseBody {
    type Data = Bytes;
    type Error = Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        match Pin::new(&mut self.as_mut().incoming).poll_frame(context) {
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(if error.is_timeout() {
                Error::HttpResponseTimeout
            } else {
                Error::from(error)
            }))),
            Poll::Ready(Some(Ok(frame))) => {
                self.timeout.reset();
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Pending => {
                ready!(self.timeout.poll_tick(context));
                Poll::Ready(Some(Err(Error::ConnectionReadTimeout)))
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.incoming.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.incoming.size_hint()
    }
}
