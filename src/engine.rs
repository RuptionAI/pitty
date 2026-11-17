use serde_json::{Value, json};
use std::sync::OnceLock;
use wasmi::{Config, Engine, Linker, Module, Store, StoreLimitsBuilder};
static ENGINE: OnceLock<Result<(Engine, Module), String>> = OnceLock::new();
fn execute(request: Value) -> Result<Value, String> {
    let (engine, module) = ENGINE
        .get_or_init(|| {
            let mut config = Config::default();
            config.consume_fuel(true);
            let engine = Engine::new(&config);
            let module = Module::new(
                &engine,
                include_bytes!("../assets/pitty_core.wasm").as_slice(),
            )
            .map_err(|e| e.to_string())?;
            Ok((engine, module))
        })
        .as_ref()
        .map_err(Clone::clone)?;
    let input = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
    if input.len() > 4_194_304 {
        return Err("Conversation is too large. Start a new chat.".into());
    }
    let mut store = Store::new(
        engine,
        StoreLimitsBuilder::new()
            .memory_size(64 * 1024 * 1024)
            .build(),
    );
    store.limiter(|limits| limits);
    store.set_fuel(500_000_000).map_err(|e| e.to_string())?;
    let instance = Linker::new(engine)
        .instantiate_and_start(&mut store, module)
        .map_err(|e| e.to_string())?;
    let memory = instance
        .get_memory(&store, "memory")
        .ok_or("Engine memory unavailable")?;
    let allocate = instance
        .get_typed_func::<i32, i32>(&store, "pitty_alloc")
        .map_err(|e| e.to_string())?;
    let run = instance
        .get_typed_func::<(i32, i32), i64>(&store, "pitty_run")
        .map_err(|e| e.to_string())?;
    let ptr = allocate
        .call(&mut store, input.len() as i32)
        .map_err(|e| e.to_string())?;
    memory
        .write(&mut store, ptr as usize, &input)
        .map_err(|e| e.to_string())?;
    let packed = run
        .call(&mut store, (ptr, input.len() as i32))
        .map_err(|e| e.to_string())? as u64;
    let len = (packed & 0xffffffff) as usize;
    if len > 4_194_304 {
        return Err("Engine output exceeds its limit.".into());
    }
    let mut output = vec![0; len];
    memory
        .read(&store, (packed >> 32) as usize, &mut output)
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&output).map_err(|e| e.to_string())
}
pub fn parse(text: &str) -> Result<Value, String> {
    execute(json!({"op":"parse","text":text}))
}
pub fn validate(chat: &pitty_core::ChatRequest) -> Result<(), String> {
    // Attachment bytes are native data; only conversation logic enters Wasm.
    let messages: Vec<_> = chat
        .messages
        .iter()
        .map(|m| json!({"role":m.role,"content":m.content}))
        .collect();
    let value = execute(json!({"op":"validate","chat":{"messages":messages}}))?;
    if value["ok"] == true {
        Ok(())
    } else {
        Err(value["error"]
            .as_str()
            .unwrap_or("Conversation validation failed")
            .into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wasm_commands_and_literal_slashes() {
        assert_eq!(parse("/tokens 4096").unwrap()["name"], "tokens");
        for command in [
            "tools",
            "run",
            "exec",
            "actions",
            "audit",
            "interface",
            "peer",
        ] {
            assert_eq!(parse(&format!("/{command}")).unwrap()["name"], command);
        }
        assert_eq!(parse("/unknown").unwrap()["kind"], "error");
        assert_eq!(parse("//etc/example").unwrap()["text"], "/etc/example");
        assert_eq!(parse("héllo 👋").unwrap()["text"], "héllo 👋");
    }
    #[test]
    fn wasm_validates_conversation() {
        assert!(validate(&pitty_core::ChatRequest { messages: vec![] }).is_err());
        assert!(
            validate(&pitty_core::ChatRequest {
                messages: vec![pitty_core::Message::user("Hi".into())]
            })
            .is_ok()
        );
    }
}
