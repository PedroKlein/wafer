use wasip3::http::types::{Fields, Method, Request, RequestOptions, Scheme};
use wasip3::{wit_future, wit_stream};

wasip3::cli::command::export!(Guest);

struct Guest;

impl wasip3::exports::cli::run::Guest for Guest {
    async fn run() -> Result<(), ()> {
        let authority = wasip3::cli::environment::get_environment()
            .into_iter()
            .find_map(|(key, value)| (key == "HTTP_SERVER").then_some(value))
            .ok_or(())?;
        let (_, body) = wit_stream::new();
        let (trailers, trailers_reader) = wit_future::new(|| Ok(None));
        drop(trailers);
        let (request, transmit) =
            Request::new(Fields::new(), Some(body), trailers_reader, Some(RequestOptions::new()));
        request.set_method(&Method::Get).map_err(|_| ())?;
        request.set_scheme(Some(&Scheme::Http)).map_err(|_| ())?;
        request.set_authority(Some(&authority)).map_err(|_| ())?;
        request.set_path_with_query(Some("/smoke")).map_err(|_| ())?;
        let response = wasip3::http::client::send(request).await.map_err(|_| ())?;
        transmit.await.map_err(|_| ())?;
        if response.get_status_code() == 204 { Ok(()) } else { Err(()) }
    }
}
