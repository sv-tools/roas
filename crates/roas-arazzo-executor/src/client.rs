//! Ready-made clients built on [`reqwest`], behind the `reqwest`
//! feature.
//!
//! One type over both of reqwest's clients — blocking and async — the
//! way `roas-http-fetcher` does it for the loader, so the choice is a
//! type parameter rather than two APIs.

use crate::http::{
    AsyncHttpClient, ClientError, HttpClient, HttpRequest, HttpResponse, SendFuture, SleepFuture,
};
use std::time::Duration;

/// An HTTP client for the executor.
///
/// `Client<reqwest::blocking::Client>` implements [`HttpClient`] and
/// `Client<reqwest::Client>` implements [`AsyncHttpClient`].
#[derive(Clone, Debug)]
pub struct Client<C> {
    client: C,
}

impl<C> Client<C> {
    /// Use a client the caller has already configured — with its own
    /// timeouts, proxy, TLS or middleware.
    pub fn with(client: C) -> Self {
        Self { client }
    }
}

impl Client<reqwest::blocking::Client> {
    /// A blocking client with reqwest's defaults.
    ///
    /// # Panics
    ///
    /// If reqwest cannot build its default client — the same condition
    /// `reqwest::blocking::Client::new` panics on.
    #[must_use]
    pub fn blocking() -> Self {
        Self::with(reqwest::blocking::Client::new())
    }
}

impl Client<reqwest::Client> {
    /// An async client with reqwest's defaults.
    #[must_use]
    pub fn asynchronous() -> Self {
        Self::with(reqwest::Client::new())
    }
}

impl HttpClient for Client<reqwest::blocking::Client> {
    fn send(&mut self, request: &HttpRequest) -> Result<HttpResponse, ClientError> {
        let method = method(request)?;
        let mut builder = self.client.request(method, &request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }
        if let Some(timeout) = request.timeout {
            builder = builder.timeout(timeout);
        }
        let response = builder.send().map_err(ClientError::new)?;
        let status = response.status().as_u16();
        let headers = headers(response.headers());
        let body = response.bytes().map_err(ClientError::new)?.to_vec();
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

impl AsyncHttpClient for Client<reqwest::Client> {
    fn send<'a>(&'a mut self, request: &'a HttpRequest) -> SendFuture<'a> {
        let client = self.client.clone();
        Box::pin(async move {
            let method = method(request)?;
            let mut builder = client.request(method, &request.url);
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }
            if let Some(body) = &request.body {
                builder = builder.body(body.clone());
            }
            if let Some(timeout) = request.timeout {
                builder = builder.timeout(timeout);
            }
            let response = builder.send().await.map_err(ClientError::new)?;
            let status = response.status().as_u16();
            let headers = headers(response.headers());
            let body = response.bytes().await.map_err(ClientError::new)?.to_vec();
            Ok(HttpResponse {
                status,
                headers,
                body,
            })
        })
    }

    fn sleep(&self, duration: Duration) -> SleepFuture<'_> {
        Box::pin(tokio::time::sleep(duration))
    }
}

fn method(request: &HttpRequest) -> Result<reqwest::Method, ClientError> {
    reqwest::Method::from_bytes(request.method.as_bytes()).map_err(ClientError::new)
}

fn headers(headers: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            (
                name.to_string(),
                value.to_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_method_is_read_from_what_the_step_wrote() {
        let request = HttpRequest {
            method: "PATCH".to_owned(),
            ..HttpRequest::default()
        };
        assert_eq!(method(&request).unwrap(), reqwest::Method::PATCH);
        let bad = HttpRequest {
            method: "not a method".to_owned(),
            ..HttpRequest::default()
        };
        assert!(method(&bad).is_err());
    }

    #[test]
    fn headers_come_across_as_pairs() {
        let mut map = reqwest::header::HeaderMap::new();
        map.insert("x-thing", "value".parse().expect("a header value"));
        assert_eq!(headers(&map), [("x-thing".to_owned(), "value".to_owned())]);
    }

    #[test]
    fn both_clients_can_be_built() {
        let _ = Client::blocking();
        let _ = Client::asynchronous();
        let _ = Client::with(reqwest::Client::new());
    }
}
