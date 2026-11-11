//! Search in the command palette.

/// Lowercase without accents, so that "offnen" finds "Öffnen".
pub fn fold(text: &str) -> String {
    let mut folded = String::with_capacity(text.len());
    for ch in text.chars().flat_map(char::to_lowercase) {
        match ch {
            'à' | 'â' | 'ä' => folded.push('a'),
            'é' | 'è' | 'ê' | 'ë' => folded.push('e'),
            'î' | 'ï' => folded.push('i'),
            'ô' | 'ö' => folded.push('o'),
            'ù' | 'û' | 'ü' => folded.push('u'),
            'ç' => folded.push('c'),
            'œ' => folded.push_str("oe"),
            other => folded.push(other),
        }
    }
    folded
}

/// Relevance of `label` for `query` (smaller is better), or `None` if the
/// letters of the query do not appear in it in order.
fn score(query: &str, label: &str) -> Option<usize> {
    if let Some(position) = label.find(query) {
        // An exact match wins, all the more so when it is early in the name.
        return Some(position);
    }
    let mut label_chars = label.chars().enumerate();
    let mut first = None;
    let mut last = 0;
    for wanted in query.chars().filter(|ch| !ch.is_whitespace()) {
        let (index, _) = label_chars.find(|(_, ch)| *ch == wanted)?;
        first.get_or_insert(index);
        last = index;
    }
    // Scattered letters: ranked after, tightest first.
    Some(1000 + last - first.unwrap_or(0))
}

/// Indices of the names matching the query, from most to least relevant.
pub fn matches<'a>(query: &str, labels: impl Iterator<Item = &'a str>) -> Vec<usize> {
    let query = fold(query.trim());
    let mut scored: Vec<(usize, usize)> = labels
        .enumerate()
        .filter_map(|(index, label)| Some((score(&query, &fold(label))?, index)))
        .collect();
    scored.sort();
    scored.into_iter().map(|(_, index)| index).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LABELS: [&str; 4] = ["New tab", "Öffnen später", "Split right", "Close pane"];

    fn search(query: &str) -> Vec<&'static str> {
        matches(query, LABELS.into_iter()).into_iter().map(|index| LABELS[index]).collect()
    }

    #[test]
    fn empty_query_keeps_everything_in_order() {
        assert_eq!(search(""), LABELS.to_vec());
    }

    #[test]
    fn accents_and_case_are_ignored() {
        assert_eq!(search("OFFNEN"), vec!["Öffnen später"]);
        assert_eq!(search("split right"), vec!["Split right"]);
    }

    #[test]
    fn exact_text_ranks_before_scattered_letters() {
        // "pa" appears as is in "pane", and nowhere else.
        assert_eq!(search("pa")[0], "Close pane");
        assert_eq!(search("nwt"), vec!["New tab"]);
        assert!(search("xyz").is_empty());
    }
}
