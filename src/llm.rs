use std::io::BufRead;
use std::sync::mpsc;

const OLLAMA_URL: &str = "http://localhost:11434/api/generate";
const MODEL: &str = "qwen2.5:1.5b";

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
    pub tip:  String,
    pub joke: String,
}

impl LlmResponse {
    pub fn loading() -> Self {
        Self {
            tip:  "thinking...".into(),
            joke: "thinking...".into(),
        }
    }

    pub fn fallback() -> Self {
        Self {
            tip:  "try: man <command>".into(),
            joke: "why do devs hate nature? too many bugs".into(),
        }
    }
}

fn build_prompt(context: &str) -> String {
    format!(
        "Linux desktop pet. User is: {}. Reply EXACTLY:\nTIP: <linux tip max 8 words>\nJOKE: <programming joke max 10 words>",
        context
    )
}

fn parse_response(raw: &str) -> LlmResponse {
    let mut tip  = String::new();
    let mut joke = String::new();

    for line in raw.lines() {
        let line  = line.trim();
        let upper = line.to_uppercase();
        if upper.starts_with("TIP:") {
            tip  = line[4..].trim().to_string();
        } else if upper.starts_with("JOKE:") {
            joke = line[5..].trim().to_string();
        }
    }

    if tip.is_empty()  { tip  = "try: tldr <command> for quick help".into(); }
    if joke.is_empty() { joke = "sudo make me a sandwich".into(); }

    LlmResponse { tip, joke }
}

/// Ambient state tip — non-blocking, sends a `TipJoke` back via mpsc.
pub fn query_async(context: String, tx: mpsc::Sender<BubbleUpdate>) {
    std::thread::spawn(move || {
        let r = query_blocking(&context);
        let _ = tx.send(BubbleUpdate::TipJoke { tip: r.tip, joke: r.joke });
    });
}

/// Free-form question — non-blocking, streams `Plain` chunks into the bubble.
pub fn query_ask(question: String, tx: mpsc::Sender<BubbleUpdate>) {
    std::thread::spawn(move || {
        if let Err(e) = stream_ask(&question, &tx) {
            log::warn!("ask query failed: {e}");
            let _ = tx.send(BubbleUpdate::Plain(
                "ask failed — is ollama running?".into(),
            ));
        }
    });
}

/// Stream an answer from Ollama token-by-token, pushing the growing text into
/// the bubble (throttled to ~10 updates/sec).
fn stream_ask(question: &str, tx: &mpsc::Sender<BubbleUpdate>) -> Result<(), String> {
    let prompt = format!(
        "You are LinuxPal, a concise Linux desktop assistant. \
         Answer in plain text (no markdown), at most 40 words.\nQuestion: {question}"
    );

    let body = serde_json::json!({
        "model": MODEL,
        "prompt": prompt,
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

fn query_blocking(context: &str) -> LlmResponse {
    let prompt = build_prompt(context);

    let body = serde_json::json!({
        "model": MODEL,
        "prompt": prompt,
        "stream": false,
        "options": {
            "temperature": 0.8,
            "num_predict": 60
        }
    });

    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            log::warn!("failed to build http client: {e}");
            return LlmResponse::fallback();
        }
    };

    match client.post(OLLAMA_URL).json(&body).send() {
        Ok(resp) => match resp.json::<serde_json::Value>() {
            Ok(json) => {
                let raw = json["response"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                log::info!("llm raw response: {raw}");
                parse_response(&raw)
            }
            Err(e) => {
                log::warn!("llm json parse error: {e}");
                LlmResponse::fallback()
            }
        },
        Err(e) => {
            log::warn!("llm request failed (is ollama running?): {e}");
            LlmResponse::fallback()
        }
    }
}
