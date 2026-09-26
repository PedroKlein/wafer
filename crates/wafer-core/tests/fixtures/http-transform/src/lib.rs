wit_bindgen::generate!({
    path: "../../../../../wit",
    world: "transform-node",
    generate_all,
});

use exports::wafer::pipeline::transform::{Message, OutputMessage, ProcessError};
use wasi::http::{outgoing_handler, types};

struct HttpTransform;

struct RequestSpec<'a> {
    method: types::Method,
    scheme: Option<types::Scheme>,
    authority: &'a str,
    path_with_query: &'a str,
    body: &'a [u8],
}

fn bad_input(message: impl Into<String>) -> ProcessError {
    ProcessError::BadInput(message.into())
}

fn parse_request(input: &[u8]) -> Result<RequestSpec<'_>, ProcessError> {
    let text = std::str::from_utf8(input).map_err(|_| bad_input("request must be UTF-8"))?;
    let mut fields = text.splitn(5, '\n');
    let method = match fields.next() {
        Some("GET") => types::Method::Get,
        Some("POST") => types::Method::Post,
        Some("CONNECT") => types::Method::Connect,
        _ => return Err(bad_input("unsupported test method")),
    };
    let scheme = match fields.next() {
        Some("http") => Some(types::Scheme::Http),
        Some("https") => Some(types::Scheme::Https),
        Some("none") => None,
        _ => return Err(bad_input("unsupported test scheme")),
    };
    let authority = fields.next().ok_or_else(|| bad_input("missing authority"))?;
    let path_with_query = fields.next().ok_or_else(|| bad_input("missing path"))?;
    let body = fields.next().unwrap_or_default().as_bytes();
    Ok(RequestSpec { method, scheme, authority, path_with_query, body })
}

fn execute_request(spec: RequestSpec<'_>) -> String {
    let headers = match types::Headers::from_list(&[
        ("authorization".to_string(), b"Bearer test-auth-secret".to_vec()),
        ("cookie".to_string(), b"session=test-cookie-secret".to_vec()),
        ("x-test-secret".to_string(), b"test-header-secret".to_vec()),
    ]) {
        Ok(headers) => headers,
        Err(error) => return format!("build-error:{error:?}"),
    };
    let request = types::OutgoingRequest::new(headers);
    if request.set_method(&spec.method).is_err()
        || request.set_scheme(spec.scheme.as_ref()).is_err()
        || request.set_authority(Some(spec.authority)).is_err()
        || request.set_path_with_query(Some(spec.path_with_query)).is_err()
    {
        return "build-error:request-fields".to_string();
    }
    let body = match request.body() {
        Ok(body) => body,
        Err(error) => return format!("build-error:{error:?}"),
    };
    let response = match outgoing_handler::handle(request, None) {
        Ok(response) => response,
        Err(error) => return format!("error:{error:?}"),
    };
    if !spec.body.is_empty() {
        let stream = match body.write() {
            Ok(stream) => stream,
            Err(error) => return format!("body-error:{error:?}"),
        };
        if let Err(error) = stream.blocking_write_and_flush(spec.body) {
            return format!("body-error:{error:?}");
        }
    }
    if let Err(error) = types::OutgoingBody::finish(body, None) {
        return format!("body-error:{error:?}");
    }

    let result = match response.get() {
        Some(result) => result,
        None => {
            response.subscribe().block();
            response.get().expect("response must be ready after poll")
        }
    };
    match result {
        Ok(Ok(response)) => format!("status:{}", response.status()),
        Ok(Err(error)) => format!("error:{error:?}"),
        Err(()) => "error:response-taken".to_string(),
    }
}

impl exports::wafer::pipeline::lifecycle::Guest for HttpTransform {
    fn validate(_config: exports::wafer::pipeline::lifecycle::NodeConfig) -> Option<String> {
        None
    }

    fn init(
        _config: exports::wafer::pipeline::lifecycle::NodeConfig,
    ) -> Result<(), exports::wafer::pipeline::lifecycle::ProcessError> {
        Ok(())
    }

    fn close() {}
}

impl exports::wafer::pipeline::transform::Guest for HttpTransform {
    fn process(input: Message) -> Result<OutputMessage, ProcessError> {
        let payload = input.payload.read_all();
        if payload == b"trap" {
            panic!("requested fixture trap");
        }
        let result = execute_request(parse_request(&payload)?).into_bytes();
        Ok(OutputMessage {
            id: input.id,
            timestamp: input.timestamp,
            source: input.source,
            content_type: "text/plain".to_string(),
            metadata: input.metadata,
            payload: result,
        })
    }
}

export!(HttpTransform);
