//! Syntax highlighting of the command line in the input editor.
//!
//! Since zsh's prompt is hidden, its highlighting plugins no longer show:
//! RTerm therefore colors what is typed itself, in the same spirit (known
//! or unknown command, strings, options, variables, existing paths).

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Plain,
    /// A command the shell knows how to run.
    Command,
    /// A word in command position that the shell does not know.
    UnknownCommand,
    Option,
    String,
    Variable,
    /// `|`, `&&`, `;`, redirections…
    Operator,
    Comment,
    /// An argument naming an existing file or directory.
    Path,
}

/// Words after which the next word is still a command.
const COMMAND_PREFIXES: [&str; 8] =
    ["sudo", "env", "time", "exec", "nohup", "command", "builtin", "noglob"];

const OPERATORS: &str = "|&;()<>";

/// What the highlighter needs to ask the rest of the application.
pub struct Lookup<'a> {
    /// `Some(true)` if the word is a command, `Some(false)` if it is not,
    /// `None` if we cannot tell (shell not ready yet).
    pub is_command: &'a dyn Fn(&str) -> Option<bool>,
    pub path_exists: &'a dyn Fn(&str) -> bool,
}

/// The style of each byte of `text`.
pub fn highlight(text: &str, lookup: &Lookup) -> Vec<Style> {
    let mut styles = vec![Style::Plain; text.len()];
    let mut expect_command = true;
    let mut redirect_target = false;
    let mut index = 0;

    while let Some(ch) = text[index..].chars().next() {
        if ch == '\n' {
            expect_command = true;
            index += 1;
        } else if ch.is_whitespace() {
            index += ch.len_utf8();
        } else if ch == '#' {
            let end = text[index..].find('\n').map_or(text.len(), |offset| index + offset);
            styles[index..end].fill(Style::Comment);
            index = end;
        } else if OPERATORS.contains(ch) {
            let length = text[index..].chars().take_while(|ch| OPERATORS.contains(*ch)).count();
            let operator = &text[index..index + length];
            styles[index..index + length].fill(Style::Operator);
            // After a redirection comes a file; after the rest, a command.
            if operator.starts_with(['<', '>']) || operator == "&>" {
                redirect_target = true;
            } else {
                expect_command = true;
            }
            index += length;
        } else {
            let word = scan_word(text, index);
            let raw = &text[index..word.end];
            let base = if expect_command {
                if is_assignment(raw) {
                    Style::Plain
                } else {
                    expect_command = COMMAND_PREFIXES.contains(&word.value.as_str());
                    match (lookup.is_command)(&word.value) {
                        Some(true) if word.simple => Style::Command,
                        Some(false) if word.simple => Style::UnknownCommand,
                        _ => Style::Plain,
                    }
                }
            } else if std::mem::take(&mut redirect_target) {
                path_style(&word.value, lookup)
            } else if raw.starts_with('-') {
                Style::Option
            } else {
                path_style(&word.value, lookup)
            };
            styles[index..word.end].fill(base);
            for (range, style) in word.segments {
                styles[range].fill(style);
            }
            index = word.end;
        }
    }
    styles
}

fn path_style(value: &str, lookup: &Lookup) -> Style {
    if !value.is_empty() && (lookup.path_exists)(value) { Style::Path } else { Style::Plain }
}

/// Splits a range of the text into runs of bytes with the same style.
pub fn runs(styles: &[Style], range: Range<usize>) -> Vec<(Range<usize>, Style)> {
    let mut runs: Vec<(Range<usize>, Style)> = Vec::new();
    for index in range {
        match runs.last_mut() {
            Some((run, style)) if *style == styles[index] => run.end = index + 1,
            _ => runs.push((index..index + 1, styles[index])),
        }
    }
    runs
}

struct Word {
    end: usize,
    /// The word without quotes or backslashes.
    value: String,
    /// False if the word contains quotes or variables: we cannot know which
    /// command it names then.
    simple: bool,
    /// Strings and variables inside the word.
    segments: Vec<(Range<usize>, Style)>,
}

fn scan_word(text: &str, start: usize) -> Word {
    let mut word = Word { end: start, value: String::new(), simple: true, segments: Vec::new() };
    let mut index = start;

    while let Some(ch) = text[index..].chars().next() {
        if ch.is_whitespace() || OPERATORS.contains(ch) {
            break;
        }
        match ch {
            '\\' => {
                index += 1;
                if let Some(escaped) = text[index..].chars().next() {
                    word.value.push(escaped);
                    index += escaped.len_utf8();
                }
            }
            '\'' | '`' => {
                let end = text[index + 1..].find(ch).map_or(text.len(), |offset| index + offset + 2);
                word.value.push_str(text[index + 1..end].trim_end_matches(ch));
                word.segments.push((index..end, Style::String));
                word.simple = false;
                index = end;
            }
            '"' => {
                let open = index;
                index += 1;
                let mut variables = Vec::new();
                while let Some(inner) = text[index..].chars().next() {
                    match inner {
                        '"' => break,
                        '\\' => {
                            index += 1;
                            let escaped = text[index..].chars().next().map_or(0, char::len_utf8);
                            word.value.push_str(&text[index..index + escaped]);
                            index += escaped;
                        }
                        '$' => {
                            let end = variable_end(text, index);
                            variables.push((index..end, Style::Variable));
                            index = end;
                        }
                        _ => {
                            word.value.push(inner);
                            index += inner.len_utf8();
                        }
                    }
                }
                index = (index + 1).min(text.len());
                word.segments.push((open..index, Style::String));
                word.segments.extend(variables);
                word.simple = false;
            }
            '$' => {
                let end = variable_end(text, index);
                word.segments.push((index..end, Style::Variable));
                word.simple = false;
                index = end;
                // `$(`: the parenthesis opens a command, handled separately.
                if text[index..].starts_with('(') {
                    break;
                }
            }
            _ => {
                word.value.push(ch);
                index += ch.len_utf8();
            }
        }
    }
    word.end = index.max(start + 1).min(text.len());
    word
}

/// The end of a variable starting at the `$` located at `start`.
fn variable_end(text: &str, start: usize) -> usize {
    let rest = &text[start + 1..];
    if rest.starts_with('{') {
        return rest.find('}').map_or(text.len(), |offset| start + offset + 2);
    }
    let name: usize = rest
        .chars()
        .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
        .map(char::len_utf8)
        .sum();
    if name > 0 {
        return start + 1 + name;
    }
    // `$?`, `$!`, `$$`, `$#`, `$@`, `$*`.
    match rest.chars().next() {
        Some(special) if "?!$#@*".contains(special) => start + 2,
        _ => start + 1,
    }
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use Style::*;

    /// Highlights `text` and returns each piece with its style.
    fn pieces(text: &str) -> Vec<(&str, Style)> {
        let is_command = |word: &str| Some(matches!(word, "git" | "ls" | "echo" | "sudo" | "grep" | "cat"));
        let path_exists = |path: &str| matches!(path, "src/main.rs" | "My documents" | "out.txt");
        let styles = highlight(text, &Lookup { is_command: &is_command, path_exists: &path_exists });
        runs(&styles, 0..text.len())
            .into_iter()
            .map(|(range, style)| (text[range].trim(), style))
            .filter(|(piece, _)| !piece.is_empty())
            .collect()
    }

    #[test]
    fn commands_options_and_paths() {
        assert_eq!(
            pieces("git commit -m src/main.rs"),
            vec![("git", Command), ("commit", Plain), ("-m", Option), ("src/main.rs", Path)]
        );
        assert_eq!(pieces("gti status"), vec![("gti", UnknownCommand), ("status", Plain)]);
    }

    #[test]
    fn pipelines_restart_the_command_position() {
        assert_eq!(
            pieces("cat x | grep y && nope; ls"),
            vec![
                ("cat", Command),
                ("x", Plain),
                ("|", Operator),
                ("grep", Command),
                ("y", Plain),
                ("&&", Operator),
                ("nope", UnknownCommand),
                (";", Operator),
                ("ls", Command),
            ]
        );
        assert_eq!(pieces("FOO=1 sudo ls"), vec![("FOO=1", Plain), ("sudo", Command), ("ls", Command)]);
    }

    #[test]
    fn strings_variables_and_comments() {
        assert_eq!(
            pieces("echo \"hello $USER !\" '$HOME' $? # end"),
            vec![
                ("echo", Command),
                ("\"hello", String),
                ("$USER", Variable),
                ("!\"", String),
                ("'$HOME'", String),
                ("$?", Variable),
                ("# end", Comment),
            ]
        );
        // Unterminated string: colored to the end, without error.
        assert_eq!(pieces("echo 'über"), vec![("echo", Command), ("'über", String)]);
    }

    #[test]
    fn escaped_spaces_redirections_and_substitutions() {
        assert_eq!(
            pieces("ls My\\ documents > out.txt"),
            vec![("ls", Command), ("My\\ documents", Path), (">", Operator), ("out.txt", Path)]
        );
        assert_eq!(
            pieces("echo $(ls)"),
            vec![("echo", Command), ("$", Variable), ("(", Operator), ("ls", Command), (")", Operator)]
        );
    }

    #[test]
    fn unknown_shell_state_never_flags_errors() {
        let styles = highlight("gti", &Lookup { is_command: &|_| None, path_exists: &|_| false });
        assert_eq!(styles, vec![Plain; 3]);
    }
}
