//! The ready-made clients, against a real socket.
//!
//! Everything else here runs on a scripted fake; this is the one test
//! that proves a request actually leaves the process — and that what
//! comes back drives the workflow the same way.

#![cfg(feature = "reqwest")]

use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{Client, Options, Outcome, execute, execute_async};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread::{self, JoinHandle};

/// A server that answers a fixed number of requests, then stops and
/// hands back what it was asked.
struct Server {
    base: String,
    join: JoinHandle<Vec<String>>,
}

impl Server {
    fn answering(count: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let base = format!("http://{}", listener.local_addr().expect("an address"));
        let join = thread::spawn(move || {
            let mut asked = Vec::new();
            for _ in 0..count {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut reader = BufReader::new(stream.try_clone().expect("a clone"));
                let mut request = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    if let Some(value) = line
                        .to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::trim)
                        .and_then(|value| value.parse::<usize>().ok())
                    {
                        length = value;
                    }
                    if line == "\r\n" {
                        break;
                    }
                    request.push_str(&line);
                }
                let mut body = vec![0; length];
                if length > 0 {
                    reader.read_exact(&mut body).expect("the body");
                    request.push_str(&String::from_utf8_lossy(&body));
                }
                asked.push(request);

                let body = br#"{"id":7,"name":"fluffy"}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(body);
                let _ = stream.flush();
            }
            asked
        });
        Self { base, join }
    }

    fn asked(self) -> Vec<String> {
        self.join.join().expect("the server thread")
    }
}

fn openapi() -> Value {
    json!({
        "openapi": "3.0.3",
        "paths": {
            "/pets/{petId}": { "get": { "operationId": "getPetById" } },
            "/pets": { "post": { "operationId": "addPet" } }
        }
    })
}

fn description() -> Description {
    serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "petStore", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": [{
            "workflowId": "buyPet",
            "steps": [
                {
                    "stepId": "addPet",
                    "operationId": "addPet",
                    "requestBody": { "payload": { "name": "fluffy" } },
                    "outputs": { "id": "$response.body#/id" }
                },
                {
                    "stepId": "findPet",
                    "operationId": "getPetById",
                    "parameters": [
                        { "name": "petId", "in": "path", "value": "$steps.addPet.outputs.id" },
                        { "name": "X-Trace", "in": "header", "value": "abc" }
                    ],
                    "successCriteria": [{ "condition": "$statusCode == 200" }]
                }
            ],
            "outputs": { "name": "$steps.addPet.outputs.id" }
        }]
    }))
    .expect("a v1.1 description")
}

#[test]
fn the_blocking_client_really_sends_the_requests() {
    let server = Server::answering(2);
    let options = Options::new()
        .source(
            "petStore",
            "https://api.example.com/openapi.json",
            openapi(),
        )
        .base_url("petStore", &server.base);

    let report = execute(&description(), &options, &mut Client::blocking()).expect("it runs");

    assert_eq!(report.outcome, Outcome::Succeeded);
    assert_eq!(report.outputs["name"], json!(7));

    let asked = server.asked();
    assert_eq!(asked.len(), 2);
    assert!(asked[0].starts_with("POST /pets "), "{}", asked[0]);
    assert!(asked[0].contains(r#"{"name":"fluffy"}"#), "{}", asked[0]);
    assert!(
        asked[0]
            .to_ascii_lowercase()
            .contains("content-type: application/json"),
        "{}",
        asked[0]
    );
    // The second step's path came from the first step's output.
    assert!(asked[1].starts_with("GET /pets/7 "), "{}", asked[1]);
    // reqwest writes header names lowercased, as HTTP/2 does.
    assert!(
        asked[1].to_ascii_lowercase().contains("x-trace: abc"),
        "{}",
        asked[1]
    );
}

#[tokio::test]
async fn the_async_client_really_sends_them_too() {
    let server = Server::answering(2);
    let options = Options::new()
        .source(
            "petStore",
            "https://api.example.com/openapi.json",
            openapi(),
        )
        .base_url("petStore", &server.base);

    let report = execute_async(&description(), &options, &mut Client::asynchronous())
        .await
        .expect("it runs");

    assert_eq!(report.outcome, Outcome::Succeeded);
    let asked = tokio::task::spawn_blocking(move || server.asked())
        .await
        .expect("the server thread");
    assert_eq!(asked.len(), 2);
    assert!(asked[1].starts_with("GET /pets/7 "), "{}", asked[1]);
}
