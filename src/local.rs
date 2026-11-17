//! Model servers on this computer.
//!
//! Pitty does not run models itself. It finds the local servers people already
//! use (Ollama, LM Studio, llama.cpp, vLLM, Jan, GPT4All, Docker Model Runner)
//! through the chat protocol they share, and lists the models each one serves.
//! Every probe goes to this computer only, never through a proxy, and a probe
//! that does not answer quickly is skipped.
use serde_json::Value;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// Default addresses of common local model servers, in display order.
pub const SERVERS: &[(&str, &str)] = &[
    ("Ollama", "http://127.0.0.1:11434/v1"),
    ("LM Studio", "http://127.0.0.1:1234/v1"),
    ("llama.cpp", "http://127.0.0.1:8080/v1"),
    ("vLLM", "http://127.0.0.1:8000/v1"),
    ("Jan", "http://127.0.0.1:1337/v1"),
    ("GPT4All", "http://127.0.0.1:4891/v1"),
    ("Docker Model Runner", "http://127.0.0.1:12434/engines/v1"),
];

const BODY_LIMIT: usize = 1024 * 1024;
const MODEL_LIMIT: usize = 500;

/// A server that answered, and the models it serves (possibly none yet).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Server {
    pub name: String,
    pub base_url: String,
    pub models: Vec<String>,
}

/// Where to look: the defaults, with `OLLAMA_HOST` replacing Ollama's address.
pub fn candidates(ollama_host: Option<&str>) -> Vec<(String, String)> {
    SERVERS
        .iter()
        .map(|(name, url)| {
            let url = match ollama_host.and_then(ollama_base) {
                Some(host) if *name == "Ollama" => host,
                _ => (*url).to_string(),
            };
            ((*name).to_string(), url)
        })
        .collect()
}

/// `OLLAMA_HOST` accepts `host`, `host:port`, or `http://host:port`. A server
/// listening on every interface (0.0.0.0 or [::]) is reached over loopback.
fn ollama_base(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        return None;
    }
    let (scheme, rest) = match value.split_once("://") {
        Some((scheme @ ("http" | "https"), rest)) => (scheme, rest),
        Some(_) => return None,
        None => ("http", value),
    };
    if rest.is_empty()
        || rest.chars().any(|c| {
            matches!(c, '@' | '?' | '#' | '/' | '\\') || c.is_whitespace() || c.is_control()
        })
    {
        return None;
    }
    let rest = if let Some(port) = rest.strip_prefix("0.0.0.0") {
        format!("127.0.0.1{port}")
    } else if let Some(port) = rest.strip_prefix("[::]") {
        format!("[::1]{port}")
    } else {
        rest.to_string()
    };
    let has_port = if rest.starts_with('[') {
        rest.contains("]:")
    } else {
        rest.contains(':')
    };
    let rest = if has_port {
        rest
    } else {
        format!("{rest}:11434")
    };
    Some(format!("{scheme}://{rest}/v1"))
}

/// Model ids from an OpenAI-compatible `GET /models` body. `None` when the
/// body is not that shape, so an unrelated web server on a common port is
/// never mistaken for a model server.
fn parse_models(body: &str) -> Option<Vec<String>> {
    let value: Value = serde_json::from_str(body).ok()?;
    let data = value.get("data")?.as_array()?;
    let mut models: Vec<String> = Vec::new();
    for item in data {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        let id = id.trim();
        if id.is_empty()
            || id.len() > 200
            || id.chars().any(|c| c.is_whitespace() || c.is_control())
            || models.iter().any(|known| known == id)
        {
            continue;
        }
        models.push(id.to_string());
        if models.len() == MODEL_LIMIT {
            break;
        }
    }
    Some(models)
}

async fn probe(client: &reqwest::Client, base: &str) -> Option<Vec<String>> {
    let mut response = client.get(format!("{base}/models")).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len() + chunk.len() > BODY_LIMIT {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_models(&String::from_utf8_lossy(&bytes))
}

/// Ask every candidate at once and keep the ones that answer, in list order.
pub fn detect(cancel: &Arc<AtomicBool>) -> Vec<Server> {
    let ollama = std::env::var("OLLAMA_HOST").ok();
    detect_at(&candidates(ollama.as_deref()), cancel)
}

fn detect_at(candidates: &[(String, String)], cancel: &Arc<AtomicBool>) -> Vec<Server> {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return Vec::new();
    };
    runtime.block_on(async {
        let Ok(client) = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(500))
            .timeout(Duration::from_millis(2500))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
        else {
            return Vec::new();
        };
        let mut probes = tokio::task::JoinSet::new();
        for (index, (name, base)) in candidates.iter().cloned().enumerate() {
            let client = client.clone();
            probes.spawn(async move {
                let models = probe(&client, &base).await;
                (index, name, base, models)
            });
        }
        let mut found = Vec::new();
        loop {
            if cancel.load(Ordering::SeqCst) {
                probes.abort_all();
                return Vec::new();
            }
            tokio::select! {
                next = probes.join_next() => match next {
                    Some(Ok((index, name, base_url, Some(models)))) => {
                        found.push((index, Server { name, base_url, models }));
                    }
                    Some(_) => {}
                    None => break,
                },
                _ = tokio::time::sleep(Duration::from_millis(40)) => {}
            }
        }
        found.sort_by_key(|(index, _)| *index);
        found.into_iter().map(|(_, server)| server).collect()
    })
}

/// Whether a base URL points at this computer.
pub fn is_loopback(base_url: &str) -> bool {
    let Some(rest) = base_url
        .strip_prefix("http://")
        .or_else(|| base_url.strip_prefix("https://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if let Some(bracketed) = authority.strip_prefix('[') {
        return bracketed
            .split(']')
            .next()
            .and_then(|host| host.parse::<std::net::Ipv6Addr>().ok())
            .is_some_and(|ip| ip.is_loopback());
    }
    let host = authority.split(':').next().unwrap_or_default();
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    #[test]
    fn ollama_host_overrides_only_ollama() {
        let defaults = candidates(None);
        assert_eq!(defaults.len(), SERVERS.len());
        assert_eq!(defaults[0].1, "http://127.0.0.1:11434/v1");
        let moved = candidates(Some("0.0.0.0:9999"));
        assert_eq!(moved[0].1, "http://127.0.0.1:9999/v1");
        assert_eq!(moved[1].1, "http://127.0.0.1:1234/v1");
        assert_eq!(
            ollama_base("http://gpu-box.local:11434/").as_deref(),
            Some("http://gpu-box.local:11434/v1")
        );
        assert_eq!(
            ollama_base("studio").as_deref(),
            Some("http://studio:11434/v1")
        );
        assert_eq!(
            ollama_base("[::]").as_deref(),
            Some("http://[::1]:11434/v1")
        );
        assert_eq!(
            ollama_base("[::1]:8888").as_deref(),
            Some("http://[::1]:8888/v1")
        );
        for bad in [
            "",
            "ftp://host",
            "user:pass@host",
            "host/path",
            "host?x=1",
            "a b",
        ] {
            assert_eq!(ollama_base(bad), None, "{bad}");
        }
    }

    #[test]
    fn model_lists_must_have_the_shared_shape() {
        assert_eq!(
            parse_models(
                r#"{"object":"list","data":[{"id":"llama3.2:latest"},{"id":"qwen2.5-coder:7b"},{"id":"llama3.2:latest"},{"id":"bad id"},{"id":""},{"name":"no id"}]}"#
            ),
            Some(vec!["llama3.2:latest".into(), "qwen2.5-coder:7b".into()])
        );
        assert_eq!(parse_models(r#"{"data":[]}"#), Some(vec![]));
        assert_eq!(parse_models("<html>Not a model server</html>"), None);
        assert_eq!(parse_models(r#"{"models":[{"id":"x"}]}"#), None);
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback("http://127.0.0.1:11434/v1"));
        assert!(is_loopback("http://localhost:1234/v1"));
        assert!(is_loopback("http://[::1]:8080/v1"));
        assert!(!is_loopback("https://api.openai.com/v1"));
        assert!(!is_loopback("http://192.168.1.20:11434/v1"));
        assert!(!is_loopback("http://127.example.com/v1"));
    }

    fn serve_once(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0u8; 1024];
                let _ = stream.read(&mut request);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        format!("http://{address}/v1")
    }

    #[test]
    fn detection_keeps_servers_that_answer_in_list_order() {
        let ollama = serve_once(r#"{"data":[{"id":"llama3.2"}]}"#);
        let studio = serve_once(r#"{"data":[]}"#);
        let web_app = serve_once(r#"{"hello":"world"}"#);
        // Nothing listens on a port that was just released.
        let closed = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}/v1", listener.local_addr().unwrap())
        };
        let candidates = vec![
            ("Ollama".to_string(), ollama.clone()),
            ("Closed".to_string(), closed),
            ("Web app".to_string(), web_app),
            ("LM Studio".to_string(), studio.clone()),
        ];
        let found = detect_at(&candidates, &Arc::new(AtomicBool::new(false)));
        assert_eq!(
            found,
            vec![
                Server {
                    name: "Ollama".into(),
                    base_url: ollama,
                    models: vec!["llama3.2".into()],
                },
                Server {
                    name: "LM Studio".into(),
                    base_url: studio,
                    models: vec![],
                },
            ]
        );
        assert!(detect_at(&candidates[..1], &Arc::new(AtomicBool::new(true))).is_empty());
    }
}
