use std::io::BufRead;
use std::sync::mpsc;
use std::sync::OnceLock;

// Two-backend strategy:
//   * Ambient tips/jokes fire constantly (every roam / state change), so they
//     run on a free, unlimited LOCAL model via Ollama. Quality matters little.
//   * `ask` is rare and wants a good answer, so it hits GEMINI first and only
//     falls back to the local model if Gemini is unreachable / rate-limited.
const OLLAMA_URL: &str = "http://localhost:11434/api/generate";
const GEMINI_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const DEFAULT_LOCAL_MODEL: &str = "qwen2.5:1.5b";
const DEFAULT_GEMINI_MODEL: &str = "gemini-flash-latest";

static LOCAL_MODEL: OnceLock<String> = OnceLock::new();
static GEMINI_MODEL: OnceLock<String> = OnceLock::new();
static API_KEY: OnceLock<String> = OnceLock::new();

/// Set the local Ollama model (tips/jokes + `ask` fallback). Call once at startup.
pub fn set_model(name: String) {
    let _ = LOCAL_MODEL.set(name);
}

/// Set the Gemini model used for `ask`. Call once at startup.
pub fn set_gemini_model(name: String) {
    let _ = GEMINI_MODEL.set(name);
}

/// Set the Gemini API key from config (the `GEMINI_API_KEY` env var wins over
/// this). Call once at startup.
pub fn set_api_key(key: String) {
    let _ = API_KEY.set(key);
}

fn local_model() -> &'static str {
    LOCAL_MODEL.get().map(String::as_str).unwrap_or(DEFAULT_LOCAL_MODEL)
}

fn gemini_model() -> &'static str {
    GEMINI_MODEL.get().map(String::as_str).unwrap_or(DEFAULT_GEMINI_MODEL)
}

/// Resolve the API key: env var wins, config value is the fallback. `None` if
/// neither is set (callers then drop to local / show an error).
fn api_key() -> Option<String> {
    if let Ok(k) = std::env::var("GEMINI_API_KEY") {
        if !k.trim().is_empty() {
            return Some(k);
        }
    }
    API_KEY.get().filter(|k| !k.trim().is_empty()).cloned()
}

/// Build the Gemini generateContent endpoint. The key travels in the
/// `X-goog-api-key` header, so it never lands in URLs / logs.
fn gemini_endpoint() -> String {
    format!("{GEMINI_BASE}/{}:streamGenerateContent?alt=sse", gemini_model())
}

/// Pull the answer text out of a `candidates[0].content`, skipping any
/// `thought` parts that thinking models (e.g. gemini-3.5-flash) emit.
fn extract_text(content: &serde_json::Value) -> String {
    let mut out = String::new();
    if let Some(parts) = content["parts"].as_array() {
        for p in parts {
            if p["thought"].as_bool().unwrap_or(false) {
                continue;
            }
            if let Some(t) = p["text"].as_str() {
                out.push_str(t);
            }
        }
    }
    out
}

/// A message destined for the bubble, produced asynchronously by the LLM.
///
/// One persistent channel carries both flavours so the main loop has a single
/// place to drain (the seed of the roadmap's "event bus").
#[derive(Debug, Clone)]
pub enum BubbleUpdate {
    /// Ambient two-line state tip + joke.
    TipJoke { tip: String, joke: String },
    /// Free-form text (a streamed answer, or a status line). Replaces the bubble
    /// body as it grows, so answers fill in live.
    Plain(String),
}

#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub tip: String,
    pub joke: String,
}

fn build_prompt(context: &str) -> String {
    format!(
        "Linux desktop pet. User is: {}. Reply EXACTLY:\nTIP: <linux tip max 8 words>\nJOKE: <programming joke max 10 words>",
        context
    )
}

fn ask_prompt(question: &str) -> String {
    format!(
        "You are LinuxPal, a concise Linux desktop assistant. \
         Answer in plain text (no markdown), at most 40 words.\nQuestion: {question}"
    )
}

fn parse_response(raw: &str) -> LlmResponse {
    let mut tip = String::new();
    let mut joke = String::new();

    for line in raw.lines() {
        let line = line.trim();
        let upper = line.to_uppercase();
        if upper.starts_with("TIP:") {
            tip = line[4..].trim().to_string();
        } else if upper.starts_with("JOKE:") {
            joke = line[5..].trim().to_string();
        }
    }

    if tip.is_empty() {
        tip = "try: tldr <command> for quick help".into();
    }
    if joke.is_empty() {
        joke = "sudo make me a sandwich".into();
    }

    LlmResponse { tip, joke }
}

/// Ambient state tip — non-blocking. Sends a `TipJoke` from the local model; if
/// Ollama is unreachable, falls back to the curated offline bank for `state`.
pub fn query_async(context: String, state: crate::sprites::State, tx: mpsc::Sender<BubbleUpdate>) {
    std::thread::spawn(move || {
        let update = match ollama_tip(&context) {
            Some(r) => BubbleUpdate::TipJoke {
                tip: r.tip,
                joke: r.joke,
            },
            None => BubbleUpdate::TipJoke {
                tip: crate::sprites::offline_tip(&state).to_string(),
                joke: crate::sprites::offline_joke().to_string(),
            },
        };
        let _ = tx.send(update);
    });
}

/// Free-form question — non-blocking. Streams `Plain` chunks into the bubble.
/// Gemini first; on failure, fall back to the local model so `ask` keeps
/// working when the Gemini quota is spent or the network is down.
pub fn query_ask(question: String, tx: mpsc::Sender<BubbleUpdate>) {
    std::thread::spawn(move || {
        let gemini_err = match gemini_stream(&question, &tx) {
            Ok(()) => return,
            Err(e) => {
                log::warn!("gemini ask failed, trying local model: {e}");
                e
            }
        };

        match ollama_stream(&question, &tx) {
            Ok(()) => {}
            Err(local_err) => {
                log::warn!("local ask fallback failed: {local_err}");
                let msg = if gemini_err.contains("429")
                    || gemini_err.contains("RESOURCE_EXHAUSTED")
                {
                    "rate limited & local model down — try later"
                } else {
                    "ask failed — Gemini error and no local model"
                };
                let _ = tx.send(BubbleUpdate::Plain(msg.into()));
            }
        }
    });
}

/// Stream an answer from Gemini token-by-token. `Err` on no key / non-2xx
/// (429 rate limit, 400 bad key) / network so the caller can fall back.
fn gemini_stream(question: &str, tx: &mpsc::Sender<BubbleUpdate>) -> Result<(), String> {
    let key =
        api_key().ok_or("no Gemini API key (set GEMINI_API_KEY or gemini_api_key in config)")?;

    let body = serde_json::json!({
        "contents": [{ "parts": [{ "text": ask_prompt(question) }] }],
        "generationConfig": {
            "temperature": 0.6,
            "maxOutputTokens": 200,
            // Disable model "thinking": flash models otherwise spend the whole
            // token budget on hidden reasoning and return no answer text.
            "thinkingConfig": { "thinkingBudget": 0 }
        }
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .post(gemini_endpoint())
        .header("X-goog-api-key", &key)
        .json(&body)
        .send()
        .map_err(|e| e.to_string())?;

    // A non-2xx (429 rate limit, 400 bad key, …) returns an error JSON, not an
    // SSE stream — surface it so the caller can fall back to the local model.
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("http {status}: {}", body.replace('\n', " ")));
    }

    let reader = std::io::BufReader::new(resp);
    let mut acc = String::new();
    let mut last_push = std::time::Instant::now();

    // With `alt=sse`, Gemini streams Server-Sent Events: each event is a
    // `data: {json}` line carrying a GenerateContentResponse chunk. The stream
    // just ends (no explicit done flag).
    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        let line = line.trim();
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => continue,
        };
        acc.push_str(&extract_text(&v["candidates"][0]["content"]));
        if last_push.elapsed() >= std::time::Duration::from_millis(100) && !acc.trim().is_empty() {
            let _ = tx.send(BubbleUpdate::Plain(acc.trim().to_string()));
            last_push = std::time::Instant::now();
        }
    }

    // Empty stream (e.g. all budget eaten) is treated as a failure so `ask`
    // falls back rather than showing a blank bubble.
    if acc.trim().is_empty() {
        return Err("gemini returned no text".into());
    }
    let _ = tx.send(BubbleUpdate::Plain(acc.trim().to_string()));
    Ok(())
}

/// Stream an answer from the local Ollama model token-by-token. Used as the
/// `ask` fallback when Gemini is unavailable.
fn ollama_stream(question: &str, tx: &mpsc::Sender<BubbleUpdate>) -> Result<(), String> {
    let body = serde_json::json!({
        "model": local_model(),
        "prompt": ask_prompt(question),
        "stream": true,
        "options": {
            "temperature": 0.6,
            "num_predict": 200
        }
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .post(OLLAMA_URL)
        .json(&body)
        .send()
        .map_err(|e| e.to_string())?;

    let reader = std::io::BufReader::new(resp);
    let mut acc = String::new();
    let mut last_push = std::time::Instant::now();

    // Ollama streams newline-delimited JSON objects, each with a `response`
    // fragment and a `done` flag on the last.
    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(chunk) = v["response"].as_str() {
            acc.push_str(chunk);
        }
        if last_push.elapsed() >= std::time::Duration::from_millis(100) && !acc.trim().is_empty() {
            let _ = tx.send(BubbleUpdate::Plain(acc.trim().to_string()));
            last_push = std::time::Instant::now();
        }
        if v["done"].as_bool().unwrap_or(false) {
            break;
        }
    }

    let final_text = if acc.trim().is_empty() {
        "no answer".to_string()
    } else {
        acc.trim().to_string()
    };
    let _ = tx.send(BubbleUpdate::Plain(final_text));
    Ok(())
}

/// Query the local Ollama model for a tip/joke. `None` on any failure (build /
/// network / parse) so the caller can drop to the offline bank.
fn ollama_tip(context: &str) -> Option<LlmResponse> {
    let prompt = build_prompt(context);

    let body = serde_json::json!({
        "model": local_model(),
        "prompt": prompt,
        "stream": false,
        "options": {
            "temperature": 0.8,
            "num_predict": 60
        }
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| log::warn!("failed to build http client: {e}"))
        .ok()?;

    let resp = client
        .post(OLLAMA_URL)
        .json(&body)
        .send()
        .map_err(|e| log::warn!("tip request failed (is ollama running?): {e}"))
        .ok()?;

    let json = resp
        .json::<serde_json::Value>()
        .map_err(|e| log::warn!("tip json parse error: {e}"))
        .ok()?;

    let raw = json["response"].as_str().unwrap_or("").to_string();
    log::info!("local tip raw response: {}", raw.trim());
    Some(parse_response(&raw))
}
