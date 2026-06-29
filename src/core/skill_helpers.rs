fn to_skill_info(document: &SkillDocument, score: Option<i64>) -> SkillInfo {
    SkillInfo {
        name: document.name.clone(),
        description: document.description.clone(),
        location: document.location.clone(),
        directory: document.directory.clone(),
        metadata: document.metadata.clone(),
        score,
    }
}

fn query_terms(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .replace(['_', '-'], " ")
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn score_document(document: &SkillDocument, terms: &[String]) -> i64 {
    let name = document.name.to_lowercase();
    let description = document.description.to_lowercase();
    let content = document.content.to_lowercase();
    let metadata_text = document
        .metadata
        .iter()
        .map(|(key, value)| format!("{key} {value}"))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let mut score = 0;
    for term in terms {
        if name.contains(term) {
            score += 8;
        }
        if description.contains(term) {
            score += 5;
        }
        if metadata_text.contains(term) {
            score += 3;
        }
        if content.contains(term) {
            score += 1;
        }
    }
    score
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let regex = format!("^{}$", glob_to_regex(pattern));
    Regex::new(&regex)
        .map(|regex| regex.is_match(text))
        .unwrap_or(false)
}

fn glob_to_regex(pattern: &str) -> String {
    let mut regex = String::new();
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '[' | ']' | '{' | '}' | '\\' => {
                regex.push('\\');
                regex.push(ch);
            }
            other => regex.push(other),
        }
    }
    regex
}
