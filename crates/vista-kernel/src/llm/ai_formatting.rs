use std::env;

pub fn has_repetition_loop(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 4 {
        return false;
    }

    let mut consecutive_count = 1;
    for i in 1..words.len() {
        if words[i].eq_ignore_ascii_case(words[i - 1]) {
            consecutive_count += 1;
            if consecutive_count >= 3 {
                return true;
            }
        } else {
            consecutive_count = 1;
        }
    }

    for pattern_len in 1..=3 {
        if words.len() < pattern_len * 3 {
            continue;
        }

        for i in 0..=words.len() - pattern_len {
            let pattern = &words[i..i + pattern_len];
            let mut repeat_count = 1;
            let mut j = i + pattern_len;

            while j + pattern_len <= words.len() {
                let next = &words[j..j + pattern_len];
                if pattern
                    .iter()
                    .zip(next.iter())
                    .all(|(a, b)| a.eq_ignore_ascii_case(b))
                {
                    repeat_count += 1;
                    j += pattern_len;
                } else {
                    break;
                }
            }

            if repeat_count >= 3 {
                return true;
            }
        }
    }

    false
}

pub fn remove_simple_repetitions(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return text.to_string();
    }

    let normalize_word = |word: &str| -> String {
        word.trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase()
    };

    let mut result = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let mut best_pattern_len = 1;
        let mut best_repeat_count = 1;

        for pattern_len in (1..=3).rev() {
            if i + pattern_len > words.len() {
                continue;
            }

            let pattern: Vec<String> = words[i..i + pattern_len]
                .iter()
                .map(|word| normalize_word(word))
                .collect();
            let mut repeat_count = 1;
            let mut j = i + pattern_len;

            while j + pattern_len <= words.len() {
                let next: Vec<String> = words[j..j + pattern_len]
                    .iter()
                    .map(|word| normalize_word(word))
                    .collect();
                if pattern == next {
                    repeat_count += 1;
                    j += pattern_len;
                } else {
                    break;
                }
            }

            if repeat_count >= 2
                && (pattern_len > best_pattern_len || repeat_count > best_repeat_count)
            {
                best_pattern_len = pattern_len;
                best_repeat_count = repeat_count;
            }
        }

        result.extend(
            words[i..i + best_pattern_len]
                .iter()
                .map(|word| word.trim_end_matches(',').to_string()),
        );
        i += best_pattern_len * best_repeat_count;
    }

    result.join(" ")
}

pub fn is_formatting_available() -> bool {
    for key in [
        "LLM_FORMATTING_ENDPOINT",
        "LLM_FORMATTING_MODEL",
        "LLM_FORMATTING_API_KEY",
    ] {
        if env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .is_none()
        {
            return false;
        }
    }
    true
}

pub async fn format_text(text: &str, language: Option<&str>, assistive: bool) -> String {
    let fallback = || -> String {
        let mut cleaned = text.trim().to_string();
        if has_repetition_loop(&cleaned) {
            cleaned = remove_simple_repetitions(&cleaned);
        }
        cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
    };

    if !is_formatting_available() {
        return fallback();
    }

    let endpoint = std::env::var("LLM_FORMATTING_ENDPOINT")
        .or_else(|_| std::env::var("LLM_ENDPOINT"))
        .unwrap_or_default();
    let model = std::env::var("LLM_FORMATTING_MODEL")
        .or_else(|_| std::env::var("LLM_MODEL"))
        .unwrap_or_else(|_| "programmer".to_string());
    let api_key = std::env::var("LLM_FORMATTING_API_KEY")
        .or_else(|_| std::env::var("LLM_ASSISTIVE_API_KEY"))
        .or_else(|_| std::env::var("LLM_API_KEY"))
        .unwrap_or_default();

    let client = reqwest::Client::new();
    let system_prompt = if assistive {
        "Jesteś zaawansowanym asystentem."
    } else {
        "Jesteś precyzyjnym korektorem transkrypcji ASR. Twoim zadaniem jest poprawienie błędów gramatycznych i fleksyjnych (szczególnie w języku polskim) z transkrypcji Whisper. \
        Zachowaj oryginalną długość tekstu oraz styl wypowiedzi. Popraw końcówki (np. 'korzystam z Rust' -> 'korzystam z Rusta'). \
        Usuń wszelkie oczywiste halucynacje dźwiękowe i bezsensowne zlepki słów (np. 'temu biuz dupy'), zastępując je ewentualnie przez '[(niewyraźnie)]', tak jak zrobiłby to zawodowy transkrybent. \
        Zachowaj absolutnie wszystkie intencjonalne słowa techniczne (np. loctree, Toolchain) bez zmian. Zwróć tylko sformatowany tekst."
    };

    let body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": text}
        ],
        "temperature": 0.0
    });

    let mut request = client.post(&endpoint).json(&body);
    if !api_key.is_empty() {
        request = request.header("Authorization", format!("Bearer {}", api_key));
    }

    match request.send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(content) = json["choices"][0]["message"]["content"].as_str() {
                    return content.to_string();
                }
            }
        }
        Ok(resp) => {
            tracing::warn!("LLM formatting failed with status: {}", resp.status());
        }
        Err(e) => {
            tracing::warn!("LLM formatting request failed: {}", e);
        }
    }

    fallback()
}
