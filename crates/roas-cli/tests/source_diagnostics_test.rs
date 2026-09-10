use serde_json::json;
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn prepared_runs_emit_source_warnings_once_except_when_quiet() {
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};
    for outcome in ["success", "failure", "runtime-error"] {
        for quiet in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let base = format!("api=http://{}", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(10);
                loop {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream.set_nonblocking(false).unwrap();
                            stream
                                .set_read_timeout(Some(Duration::from_secs(5)))
                                .unwrap();
                            let mut reader = BufReader::new(&stream);
                            let mut line = String::new();
                            loop {
                                line.clear();
                                assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                                if line == "\r\n" {
                                    break;
                                }
                            }
                            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
                            break;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                Instant::now() < deadline,
                                "no API request reached the test server"
                            );
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("{error}"),
                    }
                }
            });
            let mut value = json!({
                "arazzo":"1.1.0", "info":{"title":"Diagnostic", "version":"1"},
                "sourceDescriptions":[
                    {"name":"api", "url":"https://example.test/api.json", "type":"openapi"},
                    {"name":"unused", "url":"https://unavailable.test/unused.json", "type":"openapi"}
                ],
                "workflows":[{"workflowId":"w", "steps":[{"stepId":"s", "operationId":"$sourceDescriptions.api.check"}]}]
            });
            match outcome {
                "failure" => {
                    value["workflows"][0]["steps"][0]["successCriteria"] =
                        json!([{"condition":"$statusCode == 201"}])
                }
                "runtime-error" => {
                    value["workflows"][0]["steps"][0]["outputs"] =
                        json!({"missing":"$response.body#/absent"})
                }
                _ => {}
            }
            let fixture = format!(
                "api={}/tests/fixtures/source-graph/api.json",
                env!("CARGO_MANIFEST_DIR")
            );
            let mut command = Command::new(env!("CARGO_BIN_EXE_roas"));
            command.args([
                "arazzo",
                "run",
                "--format",
                "json",
                "--load-all-sources",
                "--source",
                &fixture,
                "--base-url",
                &base,
            ]);
            if quiet {
                command.arg("--quiet");
            }
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(value.to_string().as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            server.join().unwrap();
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert_eq!(output.status.success(), outcome == "success", "{stderr}");
            assert_eq!(
                stderr.matches("#.sourceDescriptions[1].url").count(),
                usize::from(!quiet),
                "{outcome}: {stderr}"
            );
            assert!(!stderr.contains("DocumentId("), "{stderr}");
        }
    }
}

#[test]
fn failing_run_prints_each_located_source_diagnostic_once_with_or_without_quiet() {
    for quiet in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_roas"));
        command.args(["arazzo", "run", "--format", "json"]);
        if quiet {
            command.arg("--quiet");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let value = json!({
            "arazzo":"1.1.0", "info":{"title":"Diagnostic", "version":"1"},
            "sourceDescriptions":[{"name":"api", "url":"https://unavailable.test/openapi.json", "type":"openapi"}],
            "workflows":[{"workflowId":"w", "steps":[{"stepId":"s", "operationId":"$sourceDescriptions.api.check"}]}]
        });
        child
            .stdin
            .take()
            .unwrap()
            .write_all(value.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            stderr.matches("#.sourceDescriptions[0].url").count(),
            1,
            "{stderr}"
        );
        assert!(!stderr.contains("DocumentId("), "{stderr}");
        assert!(stderr.contains("no fetcher registered"), "{stderr}");
        assert!(stderr.contains("--source <name>=<path>"), "{stderr}");
    }
}
