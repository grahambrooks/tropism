//! The server as an agent sees it: the real binary, spoken to over stdio.
//!
//! The unit tests in `main.rs` cover what each tool computes. This covers what
//! they cannot — that the binary initializes, lists exactly the three tools in
//! design/05-interfaces.md, and answers a call with structured content.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Server {
    fn start() -> Self {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        let mut child = Command::new(env!("CARGO_BIN_EXE_tropism-mcp"))
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("tropism-mcp starts");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut server = Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        };
        let init = server.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "stdio-test", "version": "0" },
            }),
        );
        assert_eq!(init["serverInfo"]["name"], "tropism", "{init:#}");
        assert!(init["capabilities"]["tools"].is_object(), "{init:#}");
        assert!(
            init["instructions"]
                .as_str()
                .is_some_and(|text| text.contains("tropism_check")),
            "{init:#}"
        );
        server.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        server
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line).unwrap();
            assert!(read > 0, "server closed stdout while awaiting `{method}`");
            let message: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout carried a non-JSON line ({e}): {line}"));
            // Notifications and server-initiated requests carry no matching id.
            if message["id"] == id {
                assert!(message.get("error").is_none(), "{message:#}");
                return message["result"].clone();
            }
        }
    }

    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let result = self.request(
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        );
        assert_eq!(result["isError"], false, "{result:#}");
        result["structuredContent"].clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn lists_exactly_the_three_specified_tools() {
    let mut server = Server::start();
    let tools = server.request("tools/list", json!({}));
    let mut names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["tropism_check", "tropism_rules", "tropism_summary"]);
    for tool in tools["tools"].as_array().unwrap() {
        assert_eq!(tool["annotations"]["readOnlyHint"], true, "{tool:#}");
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool:#}");
    }
}

#[test]
fn check_answers_over_the_wire() {
    let mut server = Server::start();
    let result = server.call(
        "tropism_check",
        json!({ "files": ["crates/tropism-mcp/src/main.rs"] }),
    );
    assert_eq!(result["gate"], "pass", "{result:#}");
    assert_eq!(result["checked_files"], 1);
}

#[test]
fn a_bad_path_is_a_tool_error_the_agent_can_read() {
    let mut server = Server::start();
    let result = server.request(
        "tools/call",
        json!({ "name": "tropism_summary", "arguments": { "path": "no/such/dir" } }),
    );
    assert_eq!(result["isError"], true, "{result:#}");
    assert!(
        result["structuredContent"]["error"]
            .as_str()
            .unwrap()
            .contains("not a directory")
    );
}
