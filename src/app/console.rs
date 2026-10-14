//! The `:` line. Words are split on whitespace, with double quotes.
//! `mkdir` / `touch` / `rename` accept a single path segment so a console
//! command cannot write outside the current directory by smuggling a slash.

use crate::fs::SortKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleCommand {
    Cd(String),
    Mkdir(String),
    Touch(String),
    Delete,
    Rename(String),
    Search(String),
    Filter(String),
    ClearFilter,
    Sort(SortKey),
    ToggleReverse,
    ToggleHidden,
    Shell(String),
    Quit,
    TabNew,
    TabClose,
    Help,
}

pub fn parse(line: &str) -> Result<ConsoleCommand, String> {
    let words = split_words(line)?;
    let Some(head) = words.first() else {
        return Err("empty command".to_string());
    };
    let rest = &words[1..];
    match head.as_str() {
        "cd" => one(rest, "cd").map(ConsoleCommand::Cd),
        "mkdir" => one(rest, "mkdir").map(ConsoleCommand::Mkdir),
        "touch" => one(rest, "touch").map(ConsoleCommand::Touch),
        "delete" => none(rest, "delete").map(|()| ConsoleCommand::Delete),
        "rename" => one(rest, "rename").map(ConsoleCommand::Rename),
        "search" => one(rest, "search").map(ConsoleCommand::Search),
        "filter" => parse_filter(rest),
        "sort" => parse_sort(rest),
        "hidden" => none(rest, "hidden").map(|()| ConsoleCommand::ToggleHidden),
        "shell" => {
            if rest.is_empty() {
                Err("shell needs a command".to_string())
            } else {
                Ok(ConsoleCommand::Shell(rest.join(" ")))
            }
        }
        "quit" => none(rest, "quit").map(|()| ConsoleCommand::Quit),
        "tab_new" => none(rest, "tab_new").map(|()| ConsoleCommand::TabNew),
        "tab_close" => none(rest, "tab_close").map(|()| ConsoleCommand::TabClose),
        "help" => none(rest, "help").map(|()| ConsoleCommand::Help),
        other => Err(format!("unknown command {other}")),
    }
}

fn parse_filter(rest: &[String]) -> Result<ConsoleCommand, String> {
    match rest {
        [word] if word == "clear" => Ok(ConsoleCommand::ClearFilter),
        [word] => Ok(ConsoleCommand::Filter(word.clone())),
        _ => Err("filter needs a pattern, or `clear`".to_string()),
    }
}

fn parse_sort(rest: &[String]) -> Result<ConsoleCommand, String> {
    match rest {
        [word] if word == "reverse" => Ok(ConsoleCommand::ToggleReverse),
        [word] => match word.as_str() {
            "name" | "natural" => Ok(ConsoleCommand::Sort(SortKey::Name)),
            "size" => Ok(ConsoleCommand::Sort(SortKey::Size)),
            "mtime" => Ok(ConsoleCommand::Sort(SortKey::Mtime)),
            "extension" | "type" => Ok(ConsoleCommand::Sort(SortKey::Extension)),
            other => Err(format!("unknown sort {other}")),
        },
        _ => Err("sort needs a key".to_string()),
    }
}

fn one(rest: &[String], command: &str) -> Result<String, String> {
    match rest {
        [word] if !word.is_empty() => Ok(word.clone()),
        _ => Err(format!("{command} needs one argument")),
    }
}

fn none(rest: &[String], command: &str) -> Result<(), String> {
    if rest.is_empty() {
        Ok(())
    } else {
        Err(format!("{command} takes no arguments"))
    }
}

pub fn split_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut in_quote = false;
    let mut started = false;
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                in_quote = !in_quote;
                started = true;
            }
            '\\' if in_quote => {
                let Some(next) = chars.next() else {
                    return Err("trailing backslash".to_string());
                };
                current.push(next);
                started = true;
            }
            ch if ch.is_whitespace() && !in_quote => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            ch => {
                current.push(ch);
                started = true;
            }
        }
    }
    if in_quote {
        return Err("unclosed quote".to_string());
    }
    if started {
        words.push(current);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_quotes_and_rejects_an_unclosed_quote() {
        assert_eq!(split_words(r#"mkdir "my file""#).unwrap(), vec!["mkdir", "my file"]);
        assert_eq!(split_words(r#""""#).unwrap(), vec![""]);
        assert!(split_words("mkdir \"oops").is_err());
        assert_eq!(split_words("   cd   /tmp  ").unwrap(), vec!["cd", "/tmp"]);
    }

    #[test]
    fn parses_the_commands_the_console_actually_runs() {
        assert_eq!(parse("cd ..").unwrap(), ConsoleCommand::Cd("..".into()));
        assert_eq!(parse("mkdir sub").unwrap(), ConsoleCommand::Mkdir("sub".into()));
        assert_eq!(parse("touch a.txt").unwrap(), ConsoleCommand::Touch("a.txt".into()));
        assert_eq!(parse("delete").unwrap(), ConsoleCommand::Delete);
        assert_eq!(parse("rename b.txt").unwrap(), ConsoleCommand::Rename("b.txt".into()));
        assert_eq!(parse("search foo").unwrap(), ConsoleCommand::Search("foo".into()));
        assert_eq!(parse("filter zz").unwrap(), ConsoleCommand::Filter("zz".into()));
        assert_eq!(parse("filter clear").unwrap(), ConsoleCommand::ClearFilter);
        assert_eq!(parse("sort size").unwrap(), ConsoleCommand::Sort(SortKey::Size));
        assert_eq!(parse("sort reverse").unwrap(), ConsoleCommand::ToggleReverse);
        assert_eq!(parse("hidden").unwrap(), ConsoleCommand::ToggleHidden);
        assert_eq!(parse("shell echo hi").unwrap(), ConsoleCommand::Shell("echo hi".into()));
        assert_eq!(parse("quit").unwrap(), ConsoleCommand::Quit);
        assert_eq!(parse("tab_new").unwrap(), ConsoleCommand::TabNew);
        assert_eq!(parse("help").unwrap(), ConsoleCommand::Help);
    }

    #[test]
    fn rejects_extra_arguments_unknown_commands_and_empty_input() {
        assert!(parse("").is_err());
        assert!(parse("mkdir a b").is_err());
        assert!(parse("delete now").is_err());
        assert!(parse("nope").unwrap_err().contains("unknown"));
        assert!(parse("sort rainbow").is_err());
        assert!(parse("shell").is_err());
    }
}
