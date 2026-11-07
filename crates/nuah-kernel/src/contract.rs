//! Generated faces start here. Everything in this file reads the core's `describe` answer and
//! nothing else, so it works for any distribution and any plugin set: an OpenAPI document, a typed
//! TypeScript client, a tool list for agents, and the argument parser of a generic CLI.
//! None of it knows what a task is.

use serde_json::{json, Map, Value};

fn ops(describe: &Value) -> Vec<Value> {
    let mut v: Vec<Value> = describe["operations"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter(|(_, o)| o["allowed"] != json!(false))
                .map(|(name, o)| {
                    let mut o = o.clone();
                    o["name"] = json!(name);
                    o
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    v
}

fn object_schema(input: &Value) -> Value {
    if input.is_object() && input.get("type").is_some() {
        input.clone()
    } else {
        json!({"type": "object"})
    }
}

// ---------- OpenAPI ----------

/// An OpenAPI 3.1 document: one path per operation the caller may use.
pub fn openapi(describe: &Value) -> Value {
    let error = json!({"description": "A typed error", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}});
    let mut paths = Map::new();
    for o in &ops(describe) {
        let name = o["name"].as_str().unwrap_or("");
        let command = o["kind"] == "command";
        let mut params = vec![json!({"name": "X-Nuah-Face", "in": "header", "required": true, "schema": {"type": "string"},
            "description": "Which face is calling, for example web, cli or agent."}),
            json!({"name": "X-Nuah-Contract", "in": "header", "required": false, "schema": {"type": "string"},
            "description": "The contract hash this client was built for; a mismatch is an incompatible error."})];
        if command {
            params.push(json!({"name": "Idempotency-Key", "in": "header", "required": false, "schema": {"type": "string"},
                "description": "Repeating a call with the same key and input returns the first answer instead of doing the work twice."}));
        }
        paths.insert(
            format!("/link/call/{name}"),
            json!({"post": {
                "operationId": name, "summary": o["description"], "tags": [o["plugin"]],
                "x-kind": o["kind"], "x-permission": o["permission"], "x-reasons": o["reasons"],
                "parameters": params,
                "requestBody": {"required": true, "content": {"application/json": {"schema": object_schema(&o["input"])}}},
                "responses": {
                    "200": {"description": "Done", "content": {"application/json": {"schema": {"type": "object",
                        "properties": {"ok": {"const": true}, "output": o["output"], "replayed": {"type": "boolean"}}}}}},
                    "400": error, "401": error, "403": error, "404": error, "409": error, "426": error, "500": error}}}),
        );
    }
    paths.insert(
        "/pulse".into(),
        json!({"get": {"operationId": "pulse", "summary": "Server-sent events: everything that changed for the caller, in order. Resume with ?after=<cursor> or Last-Event-ID.",
            "parameters": [{"name": "after", "in": "query", "schema": {"type": "string"}}],
            "responses": {"200": {"description": "text/event-stream", "content": {"text/event-stream": {"schema": {"type": "string"}}}}}}}),
    );
    json!({
        "openapi": "3.1.0",
        "info": {"title": format!("Nuah link: profile {}", describe["profile"].as_str().unwrap_or("")),
                 "version": describe["contract"].as_str().unwrap_or("0")},
        "servers": [{"url": "/"}],
        "security": [{"bearer": []}, {"cookie": []}],
        "paths": paths,
        "components": {
            "securitySchemes": {"bearer": {"type": "http", "scheme": "bearer"}, "cookie": {"type": "apiKey", "in": "cookie", "name": "nuah_session"}},
            "schemas": {"Error": {"type": "object", "required": ["ok", "error"], "properties": {
                "ok": {"const": false},
                "error": {"type": "object", "required": ["category", "key", "message"], "properties": {
                    "category": {"enum": ["invalid", "not_found", "denied", "unauthenticated", "conflict", "incompatible", "internal"]},
                    "reason": {"type": "string", "description": "A plugin-owned code, listed in x-reasons of the operation."},
                    "key": {"type": "string", "description": "Message key a face can translate."},
                    "message": {"type": "string"}, "params": {}}}}}}}
    })
}

// ---------- agent tools ----------

/// Tool definitions in the shape agent protocols use: a name, a description and an input schema.
/// Tool names cannot contain dots, so `tasks.create` becomes `tasks_create`.
pub fn tool_definitions(describe: &Value) -> Value {
    json!(ops(describe)
        .iter()
        .map(|o| {
            let name = o["name"].as_str().unwrap_or("");
            json!({
                "name": name.replace('.', "_"),
                "op": name,
                "description": format!("{} {}", o["description"].as_str().unwrap_or(""), o["permission"].as_str().map(|p| format!(" (needs permission {p})")).unwrap_or_default()),
                "inputSchema": object_schema(&o["input"]),
                "annotations": {"readOnlyHint": o["kind"] == "query"}
            })
        })
        .collect::<Vec<_>>())
}

// ---------- TypeScript ----------

fn ts_type(s: &Value) -> String {
    let Some(o) = s.as_object() else { return "unknown".into() };
    if let Some(options) = o.get("enum").and_then(|e| e.as_array()) {
        return options.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(" | ");
    }
    let one = |t: &str| -> String {
        match t {
            "string" => "string".into(),
            "integer" | "number" => "number".into(),
            "boolean" => "boolean".into(),
            "null" => "null".into(),
            "array" => format!("{}[]", o.get("items").map(|i| paren(ts_type(i))).unwrap_or_else(|| "unknown".into())),
            "object" => match o.get("properties").and_then(|p| p.as_object()) {
                Some(props) => {
                    let required: Vec<&str> = o.get("required").and_then(|r| r.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
                    let fields: Vec<String> = props
                        .iter()
                        .map(|(k, v)| format!("{}{}: {}", json!(k), if required.contains(&k.as_str()) { "" } else { "?" }, ts_type(v)))
                        .collect();
                    let mut members = fields;
                    if o.get("additionalProperties") != Some(&json!(false)) {
                        members.push("[key: string]: unknown".into());
                    }
                    if members.is_empty() { "{}".into() } else { format!("{{ {} }}", members.join("; ")) }
                }
                None => "Record<string, unknown>".into(),
            },
            _ => "unknown".into(),
        }
    };
    match o.get("type") {
        Some(Value::String(t)) => one(t),
        Some(Value::Array(ts)) => ts.iter().filter_map(|t| t.as_str()).map(one).collect::<Vec<_>>().join(" | "),
        _ => "unknown".into(),
    }
}

fn paren(t: String) -> String {
    if t.contains(" | ") { format!("({t})") } else { t }
}

pub(crate) fn camel(name: &str) -> String {
    let mut out = String::new();
    let mut up = false;
    for c in name.chars() {
        if c == '.' || c == '_' || c == '-' {
            up = true;
        } else if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// A typed TypeScript client for the operations the caller may use. Results are `unknown`; the contract
/// carries output schemas for tools to read.
pub fn typescript(describe: &Value) -> String {
    let list = ops(describe);
    let mut s = String::new();
    let clean = |v: &Value| v.as_str().unwrap_or("").chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.').collect::<String>();
    s.push_str(&format!("// Generated from the `{}` profile, contract {}. Do not edit.\n\n", clean(&describe["profile"]), clean(&describe["contract"])));
    s.push_str("export interface Ops {\n");
    for o in list.iter() {
        s.push_str(&format!("  /** {} */\n  {}: {};\n", o["description"].as_str().unwrap_or("").replace("*/", "* /"), json!(o["name"]), ts_type(&object_schema(&o["input"]))));
    }
    s.push_str("}\n\n");
    s.push_str(&format!("export const CONTRACT = {};\n", json!(clean(&describe["contract"]))));
    s.push_str("export const commands = [");
    s.push_str(&list.iter().filter(|o| o["kind"] == "command").map(|o| o["name"].to_string()).collect::<Vec<_>>().join(", "));
    s.push_str("] as const;\n\n");
    s.push_str(
        r#"export class NuahError extends Error {
  constructor(public category: string, message: string, public reason?: string, public key?: string, public params?: unknown) { super(message); }
}

export class NuahClient {
  /** `token` is for bearer faces; a browser leaves it out and relies on the session cookie. */
  constructor(private base: string, private token?: string, private face = "client") {}

  async call<K extends keyof Ops>(op: K, input: Ops[K], idempotencyKey?: string): Promise<unknown> {
    const headers: Record<string, string> = { "Content-Type": "application/json", "X-Nuah-Face": this.face, "X-Nuah-Contract": CONTRACT };
    if (this.token) headers["Authorization"] = `Bearer ${this.token}`;
    if (idempotencyKey) headers["Idempotency-Key"] = idempotencyKey;
    const res = await fetch(`${this.base}/link/call/${String(op)}`, { method: "POST", headers, credentials: "include", body: JSON.stringify(input) });
    const body = await res.json();
    if (!body.ok) throw new NuahError(body.error.category, body.error.message, body.error.reason, body.error.key, body.error.params);
    return body.output;
  }
"#,
    );
    for o in list.iter() {
        let name = o["name"].as_str().unwrap_or("");
        let method = camel(name);
        if ["call", "constructor", "base", "token", "face"].contains(&method.as_str()) {
            continue; // reachable through call("...") only
        }
        s.push_str(&format!("\n  /** {} */\n  {}(input: Ops[{}]) {{ return this.call({}, input); }}\n", o["description"].as_str().unwrap_or("").replace("*/", "* /"), method, json!(name), json!(name)));
    }
    s.push_str("}\n");
    s
}

// ---------- generic CLI ----------

/// Turn command-line words into an operation's input, using the operation's schema to decide
/// types. `key=value` is read as the property's type (a string stays a string, a number becomes
/// a number, `true` a boolean); `key:=<json>` always takes JSON; a lone `{...}` argument is the whole input.
pub fn input_from_args(schema: &Value, args: &[String]) -> Result<Value, String> {
    if let [one] = args {
        if one.trim_start().starts_with('{') {
            return serde_json::from_str(one).map_err(|e| format!("the JSON input is not valid: {e}"));
        }
    }
    let props = schema.get("properties").and_then(|p| p.as_object());
    let mut out = Map::new();
    for a in args {
        let Some(eq) = a.find('=') else {
            return Err(format!("`{a}` is not key=value (use key:=<json> for non-text values)"));
        };
        let (mut k, raw) = (&a[..eq], &a[eq + 1..]);
        if let Some(stripped) = k.strip_suffix(':') {
            k = stripped;
            out.insert(k.to_string(), serde_json::from_str(raw).map_err(|e| format!("{k}: not valid JSON ({e})"))?);
            continue;
        }
        let prop = props.and_then(|p| p.get(k));
        let types: Vec<&str> = match prop.and_then(|p| p.get("type")) {
            Some(Value::String(t)) => vec![t.as_str()],
            Some(Value::Array(a)) => a.iter().filter_map(|t| t.as_str()).collect(),
            // no type: an enum decides (all strings, or JSON values), otherwise text
            _ if prop.and_then(|p| p.get("enum")).and_then(|e| e.as_array()).is_some_and(|e| e.iter().any(|v| !v.is_string())) => vec![],
            _ => vec!["string"],
        };
        let v = if raw == "null" && types.contains(&"null") {
            Value::Null
        } else if types.contains(&"string") {
            Value::String(raw.to_string())
        } else if types.contains(&"integer") {
            json!(raw.parse::<i64>().map_err(|_| format!("{k}: `{raw}` is not a whole number"))?)
        } else if types.contains(&"number") {
            json!(raw.parse::<f64>().ok().filter(|f| f.is_finite()).ok_or_else(|| format!("{k}: `{raw}` is not a number"))?)
        } else if types.contains(&"boolean") {
            match raw {
                "true" | "yes" | "1" => json!(true),
                "false" | "no" | "0" => json!(false),
                _ => return Err(format!("{k}: `{raw}` is not true or false")),
            }
        } else {
            serde_json::from_str(raw).map_err(|e| format!("{k}: expected JSON ({e}); write it as {k}:=<json>"))?
        };
        out.insert(k.to_string(), v);
    }
    Ok(Value::Object(out))
}

/// One line per property of an operation's input, for `help`.
pub fn describe_input(schema: &Value) -> Vec<String> {
    let required: Vec<&str> = schema.get("required").and_then(|r| r.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    schema
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|props| {
            props
                .iter()
                .map(|(k, v)| format!("{k}{}  {}", if required.contains(&k.as_str()) { " (required)" } else { "" }, ts_type(v)))
                .collect()
        })
        .unwrap_or_default()
}


/// Compare what plugins put into Slots (from `describe`) with what a Face says it offers
/// (`slots.json`: `{"face": "web", "slots": {"project.actions": {"context": ["project"], "results": ["none", "download"]}}}`).
/// Returns one sentence per mismatch; an empty list means the Face shows every item it is given
/// ([ADR 0016]).
///
/// [ADR 0016]: https://docs.addaui.com/nuah/adr/0016-slot-items-are-declared-in-manifests-listed-by-describe-and-checked-against-each-face/
pub fn slot_warnings(describe: &Value, declaration: &Value) -> Vec<String> {
    let face = declaration["face"].as_str().unwrap_or("this face");
    let offered = declaration["slots"].as_object().cloned().unwrap_or_default();
    let mut out = vec![];
    for item in describe["slots"].as_array().cloned().unwrap_or_default() {
        let (slot, id) = (item["slot"].as_str().unwrap_or("?"), item["id"].as_str().unwrap_or("?"));
        let Some(decl) = offered.get(slot) else {
            out.push(format!("{id} fills the slot {slot}, which {face} does not offer"));
            continue;
        };
        let context: Vec<&str> = decl["context"].as_array().map(|c| c.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
        let mut refs: Vec<&str> = item["args"].as_object().map(|a| a.values().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
        refs.extend(item["file"].as_str());
        for r in refs {
            // `$project.id`, or a file template such as `$project.name.json`: the name is the first word.
            let mut rest = r;
            while let Some(at) = rest.find('$') {
                let name: String = rest[at + 1..].chars().take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_').collect();
                if !context.contains(&name.as_str()) {
                    out.push(format!("{id} reads ${name}, which the slot {slot} of {face} does not provide"));
                }
                rest = &rest[at + 1..];
            }
        }
        let result = item["result"].as_str().unwrap_or("none");
        let handles: Vec<&str> = decl["results"].as_array().map(|c| c.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
        if !handles.contains(&result) {
            out.push(format!("{id} returns {result}, which {face} cannot handle in the slot {slot}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        json!({"contract": "h1", "profile": "t", "operations": {
            "tasks.create": {"kind": "command", "permission": "tasks.write", "description": "Add a task.", "plugin": "tasks", "allowed": true, "reasons": ["tasks.stale"],
             "input": {"type": "object", "required": ["title"], "additionalProperties": false, "properties": {
                 "title": {"type": "string"}, "priority": {"type": "integer"}, "tags": {"type": "array", "items": {"type": "string"}},
                 "status": {"enum": ["a", "b"]}, "due": {"type": ["string", "null"]}}}},
            "tasks.list": {"kind": "query", "permission": "tasks.read", "description": "List.", "plugin": "tasks", "allowed": true, "input": {"type": "object"}},
            "admin.wipe": {"kind": "command", "permission": "admin", "description": "No.", "plugin": "admin", "allowed": false, "input": {}}}})
    }

    #[test]
    fn openapi_has_one_path_per_allowed_op_and_marks_idempotency() {
        let o = openapi(&sample());
        let paths = o["paths"].as_object().unwrap();
        assert!(paths.contains_key("/link/call/tasks.create") && paths.contains_key("/link/call/tasks.list") && !paths.contains_key("/link/call/admin.wipe"));
        let create = &paths["/link/call/tasks.create"]["post"];
        assert_eq!(create["x-permission"], "tasks.write");
        assert_eq!(create["x-reasons"], json!(["tasks.stale"]));
        assert!(create["parameters"].as_array().unwrap().iter().any(|p| p["name"] == "Idempotency-Key"));
        assert!(!paths["/link/call/tasks.list"]["post"]["parameters"].as_array().unwrap().iter().any(|p| p["name"] == "Idempotency-Key"));
    }

    #[test]
    fn typescript_types_follow_the_schema() {
        let ts = typescript(&sample());
        assert!(ts.contains(r#""title": string"#), "{ts}");
        assert!(ts.contains(r#""priority"?: number"#), "{ts}");
        assert!(ts.contains(r#""tags"?: string[]"#), "{ts}");
        assert!(ts.contains(r#""status"?: "a" | "b""#), "{ts}");
        assert!(ts.contains(r#""due"?: string | null"#), "{ts}");
        assert!(ts.contains("tasksCreate(input: Ops[\"tasks.create\"])"), "{ts}");
        assert!(!ts.contains("adminWipe"));
        assert!(ts.contains(r#"export const commands = ["tasks.create"] as const"#), "{ts}");
        assert!(ts.contains(r#"export const CONTRACT = "h1""#), "{ts}");
    }

    #[test]
    fn open_objects_and_hostile_names_are_valid_typescript() {
        let d = json!({"contract": "a\nb", "profile": "a\nb", "operations": {
            "a.b": {"kind": "query", "permission": "p", "description": "", "plugin": "a", "input": {"type": "object", "properties": {}}},
            "x.call": {"kind": "query", "permission": "p", "description": "", "plugin": "x", "input": {}}}});
        let ts = typescript(&d);
        assert!(ts.contains("[key: string]: unknown }") && !ts.contains("{ ;"), "{ts}");
        assert!(!ts.contains("\nb"), "newline must not reach the comment");
        assert!(ts.contains("xCall(") && !ts.contains("  call(input"));
    }

    #[test]
    fn tools_use_underscored_names_and_flag_read_only() {
        let t = tool_definitions(&sample());
        assert_eq!(t[0]["name"], "tasks_create");
        assert_eq!(t[0]["op"], "tasks.create");
        assert_eq!(t[1]["annotations"]["readOnlyHint"], true);
        assert_eq!(t.as_array().unwrap().len(), 2);
    }

    #[test]
    fn cli_words_become_typed_input() {
        let schema = &sample()["operations"]["tasks.create"]["input"];
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let v = input_from_args(schema, &args(&["title=7", "priority=3", "tags:=[\"a\",\"b\"]", "due=null"])).unwrap();
        assert_eq!(v, json!({"title": "7", "priority": 3, "tags": ["a", "b"], "due": null}));
        assert!(input_from_args(schema, &args(&["priority=high"])).unwrap_err().contains("whole number"));
        assert!(input_from_args(schema, &args(&["oops"])).unwrap_err().contains("key=value"));
        assert_eq!(input_from_args(schema, &args(&[r#"{"title":"x"}"#])).unwrap(), json!({"title": "x"}));
        assert!(describe_input(schema).contains(&"title (required)  string".to_string()));
    }

    #[test]
    fn a_face_is_told_about_items_it_would_not_show() {
        let describe = json!({"slots": [
            {"slot": "project.actions", "id": "export.project", "args": {"project": "$project.id"}, "file": "$project.name.json", "result": "download"},
            {"slot": "task.actions", "id": "x.one", "args": {}, "result": "none"},
            {"slot": "project.actions", "id": "x.two", "args": {"a": "$task.id"}, "result": "none"},
            {"slot": "project.actions", "id": "x.three", "args": {}, "result": "download"},
        ]});
        let face = json!({"face": "web", "slots": {"project.actions": {"context": ["project"], "results": ["none", "download"]}}});
        let w = slot_warnings(&describe, &face);
        assert_eq!(w.len(), 2, "{w:#?}");
        assert!(w.iter().any(|m| m.contains("x.one") && m.contains("does not offer")));
        assert!(w.iter().any(|m| m.contains("x.two") && m.contains("$task")));
        assert!(slot_warnings(&json!({"slots": []}), &face).is_empty());
        let no_download = json!({"face": "web", "slots": {"project.actions": {"context": ["project"], "results": ["none"]}}});
        assert!(slot_warnings(&describe, &no_download).iter().any(|m| m.contains("cannot handle")));
    }
}
