use std::collections::HashSet;

/// Normalize text: lowercase, strip punctuation and collapse whitespace
pub fn normalize(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut prev_space = false;

    for c in s.chars() {
        if c.is_alphanumeric() {
            for lc in c.to_lowercase() {
                result.push(lc);
            }
            prev_space = false;
        } else if !prev_space {
            result.push(' ');
            prev_space = true;
        }
    }

    result.trim().to_string()
}

/// Tokenize normalized string into words
fn tokenize(s: &str) -> Vec<&str> {
    s.split_whitespace().collect()
}

/// Calculate character bigram Sørensen-Dice similarity (0.0 to 1.0)
/// Robust against typos, transliterations, and minor spelling differences
pub fn dice_bigram_similarity(s1: &str, s2: &str) -> f64 {
    if s1.is_empty() && s2.is_empty() {
        return 1.0;
    }
    if s1.is_empty() || s2.is_empty() {
        return 0.0;
    }
    if s1 == s2 {
        return 1.0;
    }

    let chars1: Vec<char> = s1.chars().collect();
    let chars2: Vec<char> = s2.chars().collect();

    if chars1.len() < 2 || chars2.len() < 2 {
        return if s1.contains(s2) || s2.contains(s1) { 0.8 } else { 0.0 };
    }

    let mut bigrams1 = HashSet::new();
    for window in chars1.windows(2) {
        bigrams1.insert((window[0], window[1]));
    }

    let mut intersection = 0;
    let total_bigrams2 = chars2.len() - 1;
    let total_bigrams1 = bigrams1.len();

    let mut matched2 = HashSet::new();
    for window in chars2.windows(2) {
        let pair = (window[0], window[1]);
        if bigrams1.contains(&pair) && matched2.insert(pair) {
            intersection += 1;
        }
    }

    (2.0 * intersection as f64) / ((total_bigrams1 + total_bigrams2) as f64)
}

/// Token Set Similarity: checks how many words from query exist in target (and vice versa)
pub fn token_coverage_score(query: &str, target: &str) -> f64 {
    let q_tokens = tokenize(query);
    let t_tokens = tokenize(target);

    if q_tokens.is_empty() {
        return 0.0;
    }

    let t_set: HashSet<&str> = t_tokens.iter().copied().collect();
    let mut matched_tokens = 0;

    for &q in &q_tokens {
        // Direct word match or high partial similarity word
        if t_set.contains(q) {
            matched_tokens += 1;
        } else {
            let mut best_word_sim = 0.0f64;
            for &t in &t_tokens {
                let sim = dice_bigram_similarity(q, t);
                if sim > best_word_sim {
                    best_word_sim = sim;
                }
            }
            if best_word_sim >= 0.70 {
                matched_tokens += 1;
            }
        }
    }

    matched_tokens as f64 / q_tokens.len() as f64
}

/// Calculate comprehensive fuzzy match score between query and a track's metadata (0.0 to 1.0)
pub fn calculate_fuzzy_score(
    query: &str,
    title: &str,
    artist_or_channel: &str,
    extra: Option<&str>,
) -> f64 {
    let norm_q = normalize(query);
    let norm_title = normalize(title);
    let norm_artist = normalize(artist_or_channel);
    let norm_extra = extra.map(normalize).unwrap_or_default();

    if norm_q.is_empty() {
        return 0.0;
    }

    // Combined target text: "title artist album/info"
    let full_target = format!("{} {} {}", norm_title, norm_artist, norm_extra);

    // 1. Exact match / Direct substring match
    if norm_title == norm_q {
        return 1.0;
    }
    if full_target.contains(&norm_q) {
        let title_len = norm_title.len();
        let q_len = norm_q.len();
        let ratio = q_len as f64 / title_len.max(1) as f64;
        return 0.90 + (0.10 * ratio.min(1.0));
    }

    // 2. Token coverage: what percentage of query words exist in target
    let token_score = token_coverage_score(&norm_q, &full_target);

    // 3. String Dice Bigram similarity against title and full target
    let title_dice = dice_bigram_similarity(&norm_q, &norm_title);
    let full_dice = dice_bigram_similarity(&norm_q, &full_target);
    let best_dice = title_dice.max(full_dice);

    // Weighted composite score
    let composite = (token_score * 0.70) + (best_dice * 0.30);

    // If all query tokens matched in target, guarantee at least 0.85
    if token_score >= 0.99 {
        return composite.max(0.85);
    }

    composite.clamp(0.0, 1.0)
}
