use std::sync::mpsc;

const OLLAMA_URL: &str = "http://localhost:11434/api/generate";
const MODEL: &str = "qwen2.5:1.5b";

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

/// Non-blocking — spawns a thread, sends result back via mpsc
pub fn query_async(context: String, tx: mpsc::Sender<LlmResponse>) {
    std::thread::spawn(move || {
        let result = query_blocking(&context);
        let _ = tx.send(result);
    });
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
