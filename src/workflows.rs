//! Workflows: commands with blanks, kept in local TOML files
//! (`~/.config/rterm/workflows/*.toml`) and launched from the palette.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::PathBuf;

use serde::Deserialize;

const EXAMPLES: &str = include_str!("workflows_examples.toml");

#[derive(Debug, Clone, Deserialize)]
pub struct Workflow {
    pub name: String,
    /// The command, with its parameters between double braces: `{{name}}`.
    pub command: String,
    #[serde(default)]
    pub description: String,
    /// Default value of each parameter.
    #[serde(default)]
    arguments: BTreeMap<String, toml::Value>,
}

#[derive(Deserialize)]
struct WorkflowFile {
    #[serde(default)]
    workflow: Vec<Workflow>,
}

pub fn directory() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".config/rterm/workflows"))
}

/// Loads every workflow. On the first call, the directory is created with
/// an examples file. Also returns the files that could not be read.
pub fn load_all() -> (Vec<Workflow>, Vec<String>) {
    let Some(directory) = directory() else {
        return (Vec::new(), Vec::new());
    };
    if !directory.exists() && std::fs::create_dir_all(&directory).is_ok() {
        let _ = std::fs::write(directory.join("examples.toml"), EXAMPLES);
    }

    let mut paths: Vec<PathBuf> = std::fs::read_dir(&directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "toml"))
        .collect();
    paths.sort();

    let (mut workflows, mut errors) = (Vec::new(), Vec::new());
    for path in paths {
        let parsed = std::fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|text| parse(&text).map_err(|error| error.to_string()));
        match parsed {
            Ok(found) => workflows.extend(found),
            Err(error) => errors.push(format!("{} : {error}", path.display())),
        }
    }
    (workflows, errors)
}

fn parse(text: &str) -> Result<Vec<Workflow>, toml::de::Error> {
    Ok(toml::from_str::<WorkflowFile>(text)?.workflow)
}

impl Workflow {
    /// The command with its parameters replaced by their default value (or
    /// their name, failing that), and the location of each in the result.
    pub fn expand(&self) -> (String, Vec<Range<usize>>) {
        let mut text = String::new();
        let mut fields = Vec::new();
        let mut rest = self.command.as_str();

        while let Some(open) = rest.find("{{") {
            let Some(length) = rest[open..].find("}}") else {
                break;
            };
            let name = rest[open + 2..open + length].trim();
            let value = match self.arguments.get(name) {
                Some(toml::Value::String(value)) => value.clone(),
                Some(other) => other.to_string(),
                None => name.to_owned(),
            };
            text.push_str(&rest[..open]);
            fields.push(text.len()..text.len() + value.len());
            text.push_str(&value);
            rest = &rest[open + length + 2..];
        }
        text.push_str(rest);
        (text, fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_examples_are_valid() {
        let workflows = parse(EXAMPLES).unwrap();
        assert!(workflows.len() >= 3);
        assert!(workflows.iter().all(|workflow| !workflow.name.is_empty()));
    }

    #[test]
    fn expands_arguments_and_reports_their_positions() {
        let workflows = parse(
            r#"
            [[workflow]]
            name = "Test"
            command = "git rebase -i HEAD~{{count}} --onto {{ branch }} {{free}}"
            arguments = { count = 3, branch = "main" }
            "#,
        )
        .unwrap();
        let (text, fields) = workflows[0].expand();
        assert_eq!(text, "git rebase -i HEAD~3 --onto main free");
        let values: Vec<&str> = fields.iter().map(|field| &text[field.clone()]).collect();
        assert_eq!(values, vec!["3", "main", "free"]);
    }

    #[test]
    fn command_without_arguments_is_left_alone() {
        let workflows = parse("[[workflow]]\nname = \"x\"\ncommand = \"echo {{ open\"\n").unwrap();
        assert_eq!(workflows[0].expand(), ("echo {{ open".to_owned(), vec![]));
    }
}
