//! Preserve transient HTTP response categories through object_store's retry errors.

use object_store::client::{
    HttpClient, HttpConnector, HttpError, HttpErrorKind, HttpRequest, HttpResponse, HttpService,
    ReqwestConnector,
};
use object_store::ClientOptions;

#[derive(Debug)]
pub(crate) struct Connector;

impl HttpConnector for Connector {
    fn connect(&self, options: &ClientOptions) -> object_store::Result<HttpClient> {
        Ok(HttpClient::new(Service(
            ReqwestConnector::default().connect(options)?,
        )))
    }
}

#[derive(Debug)]
struct Service(HttpClient);

#[async_trait::async_trait]
impl HttpService for Service {
    async fn call(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let read = matches!(request.method().as_str(), "GET" | "HEAD");
        let response = self.0.execute(request).await?;
        // object_store keeps HTTP status errors in a private type. Preserve
        // transient read failures as public HttpErrors before that conversion.
        if read {
            let status = response.status().as_u16();
            let kind = match status {
                408 | 504 => Some(HttpErrorKind::Timeout),
                429 | 500..=599 => Some(HttpErrorKind::Request),
                _ => None,
            };
            if let Some(kind) = kind {
                return Err(HttpError::new(
                    kind,
                    std::io::Error::other(format!("HTTP {status}")),
                ));
            }
        }
        Ok(response)
    }
}
