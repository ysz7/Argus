//! The MCP handshake and tool listing, without touching the screen.

use argus_mcp::{Config, Policy, Server};
use serde_json::{Value, json};

fn server() -> Server {
    Server::new(Config::default())
}

fn ask(server: &mut Server, message: Value) -> Value {
    server.handle_line(&message.to_string()).expect("an answer")
}

#[test]
fn initialize_negotiates_the_protocol_version() {
    let mut server = server();
    let answer = ask(
        &mut server,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
               "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                          "clientInfo": {"name": "test", "version": "0"}}}),
    );
    assert_eq!(answer["id"], 1);
    assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(answer["result"]["serverInfo"]["name"], "argus");
    assert!(answer["result"]["capabilities"]["tools"].is_object());
    assert!(answer["result"]["instructions"].as_str().unwrap().contains("expect"));

    let unknown = ask(
        &mut server,
        json!({"jsonrpc": "2.0", "id": 2, "method": "initialize",
               "params": {"protocolVersion": "1999-01-01"}}),
    );
    assert_eq!(unknown["result"]["protocolVersion"], argus_mcp::PROTOCOL_VERSIONS[0]);
}

#[test]
fn notifications_get_no_answer() {
    let mut server = server();
    let line = json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string();
    assert!(server.handle_line(&line).is_none());
}

#[test]
fn tools_are_listed_with_schemas() {
    let mut server = server();
    let answer = ask(&mut server, json!({"jsonrpc": "2.0", "id": "a", "method": "tools/list"}));
    let tools = answer["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|tool| tool["name"].as_str().unwrap()).collect();
    for name in
        ["list_apps", "observe", "click", "type_text", "press_keys", "scroll", "drag", "screenshot"]
    {
        assert!(names.contains(&name), "{name} missing from {names:?}");
    }
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        assert!(tool["description"].as_str().is_some_and(|text| !text.is_empty()));
    }
    let click = tools.iter().find(|tool| tool["name"] == "click").unwrap();
    assert!(click["inputSchema"]["properties"]["expect"].is_object());
    assert_eq!(click["annotations"]["readOnlyHint"], false);
}

#[test]
fn errors_follow_json_rpc() {
    let mut server = server();
    assert_eq!(server.handle_line("{not json").unwrap()["error"]["code"], -32700);
    let missing = ask(&mut server, json!({"jsonrpc": "2.0", "id": 3, "method": "resources/list"}));
    assert_eq!(missing["error"]["code"], -32601);
    let unknown = ask(
        &mut server,
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "format_disk"}}),
    );
    assert_eq!(unknown["error"]["code"], -32602);
    assert_eq!(
        ask(&mut server, json!({"jsonrpc": "2.0", "id": 5, "method": "ping"}))["result"],
        json!({})
    );
}

#[test]
fn actions_need_an_observed_application_and_respect_read_only() {
    let call = |server: &mut Server, name: &str, arguments: Value| {
        ask(
            server,
            json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call",
                   "params": {"name": name, "arguments": arguments}}),
        )["result"]
            .clone()
    };
    let mut server = server();
    let answer = call(&mut server, "click", json!({"element_id": "e_1", "expect": "OK"}));
    assert_eq!(answer["isError"], true);
    assert!(answer["content"][0]["text"].as_str().unwrap().contains("observe"), "{answer}");

    let mut read_only = Server::new(Config {
        policy: Policy { read_only: true, ..Policy::default() },
        log_file: None,
    });
    let answer = call(&mut read_only, "press_keys", json!({"keys": "Return"}));
    assert_eq!(answer["isError"], true);
    assert!(answer["content"][0]["text"].as_str().unwrap().contains("read-only"), "{answer}");

    let blocked = call(&mut server, "observe", json!({"app": "Terminal"}));
    assert_eq!(blocked["isError"], true);
    assert!(blocked["content"][0]["text"].as_str().unwrap().contains("blocked"), "{blocked}");
}

#[test]
fn batches_are_answered_together() {
    let mut server = server();
    let batch = json!([
        {"jsonrpc": "2.0", "id": 1, "method": "ping"},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "ping"},
    ]);
    let answer = server.handle_line(&batch.to_string()).unwrap();
    assert_eq!(answer.as_array().unwrap().len(), 2);
}
