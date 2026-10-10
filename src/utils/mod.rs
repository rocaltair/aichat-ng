mod abort_signal;
mod clipboard;
mod command;
mod crypto;
mod html_to_md;
mod input;
mod loader;
mod path;
mod render_prompt;
mod request;
mod spinner;
mod variables;

pub use self::abort_signal::*;
pub use self::clipboard::set_text;
pub use self::command::*;
pub use self::crypto::*;
pub use self::html_to_md::*;
pub use self::input::*;
pub use self::loader::*;
pub use self::path::*;
pub use self::render_prompt::render_prompt;
pub use self::request::*;
pub use self::spinner::*;
pub use self::variables::*;

use anyhow::{Context, Result};
use fancy_regex::Regex;
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use is_terminal::IsTerminal;
use std::borrow::Cow;
use std::sync::LazyLock;
use std::{env, path::PathBuf, process};
use unicode_segmentation::UnicodeSegmentation;

pub static CODE_BLOCK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?ms)```\w*(.*)```").unwrap());
pub static THINK_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^\s*<think>.*?</think>(\s*|$)").unwrap());
/// Like [`THINK_TAG_RE`], but captures the reasoning itself (group 1).
static THINK_BLOCK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^\s*<think>(.*?)</think>\s*").unwrap());
pub static IS_STDOUT_TERMINAL: LazyLock<bool> = LazyLock::new(|| std::io::stdout().is_terminal());
pub static NO_COLOR: LazyLock<bool> = LazyLock::new(|| {
    env::var("NO_COLOR")
        .ok()
        .and_then(|v| parse_bool(&v))
        .unwrap_or_default()
        || !*IS_STDOUT_TERMINAL
});

pub fn now() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

pub fn now_timestamp() -> i64 {
    chrono::Local::now().timestamp()
}

pub fn get_env_name(key: &str) -> String {
    format!("AICHAT_{key}").to_ascii_uppercase()
}

pub fn normalize_env_name(value: &str) -> String {
    value.replace('-', "_").to_ascii_uppercase()
}

pub fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

pub fn estimate_token_length(text: &str) -> usize {
    let words: Vec<&str> = text.unicode_words().collect();
    let mut output: f32 = 0.0;
    for word in words {
        if word.is_ascii() {
            output += 1.3;
        } else {
            let count = word.chars().count();
            if count == 1 {
                output += 1.0
            } else {
                output += (count as f32) * 0.5;
            }
        }
    }
    output.ceil() as usize
}

pub fn strip_think_tag(text: &str) -> Cow<'_, str> {
    THINK_TAG_RE.replace_all(text, "")
}

/// Split a leading `<think>...</think>` block off `text`, returning
/// `(content, reasoning)`. `reasoning` is empty when there is no such block.
pub fn split_think_tag(text: &str) -> (&str, &str) {
    match THINK_BLOCK_RE.captures(text).ok().flatten() {
        Some(caps) => {
            let end = caps.get(0).map(|m| m.end()).unwrap_or_default();
            let reasoning = caps.get(1).map(|m| m.as_str().trim()).unwrap_or_default();
            (&text[end..], reasoning)
        }
        None => (text, ""),
    }
}

const THINK_OPEN_TAG: &str = "<think>";
const THINK_CLOSE_TAG: &str = "</think>";

/// Incrementally removes `<think>...</think>` blocks from streamed text.
///
/// Providers emit reasoning between a literal `<think>` / `</think>` pair, and
/// the surrounding chunks can split a tag across calls. A short tail is
/// therefore held back until it can be classified as plain text or as the start
/// of a tag. Call [`ThinkFilter::flush`] once the stream ends.
#[derive(Debug, Default)]
pub struct ThinkFilter {
    in_think: bool,
    skip_leading_ws: bool,
    pending: String,
}

impl ThinkFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the visible part of `text`, dropping any reasoning block.
    pub fn filter(&mut self, text: &str) -> String {
        self.pending.push_str(text);
        let mut output = String::new();
        loop {
            if self.in_think {
                match self.pending.find(THINK_CLOSE_TAG) {
                    Some(idx) => {
                        self.pending.drain(..idx + THINK_CLOSE_TAG.len());
                        self.in_think = false;
                        // Swallow the blank line that separates reasoning from the reply.
                        self.skip_leading_ws = true;
                    }
                    None => {
                        let keep = suffix_prefix_len(&self.pending, THINK_CLOSE_TAG);
                        let cut = self.pending.len() - keep;
                        self.pending.drain(..cut);
                        break;
                    }
                }
            } else {
                if self.skip_leading_ws {
                    let trimmed = self.pending.trim_start();
                    let cut = self.pending.len() - trimmed.len();
                    self.pending.drain(..cut);
                    if self.pending.is_empty() {
                        break;
                    }
                    self.skip_leading_ws = false;
                }
                match self.pending.find(THINK_OPEN_TAG) {
                    Some(idx) => {
                        output.push_str(&self.pending[..idx]);
                        self.pending.drain(..idx + THINK_OPEN_TAG.len());
                        self.in_think = true;
                    }
                    None => {
                        let keep = suffix_prefix_len(&self.pending, THINK_OPEN_TAG);
                        let cut = self.pending.len() - keep;
                        output.push_str(&self.pending[..cut]);
                        self.pending.drain(..cut);
                        break;
                    }
                }
            }
        }
        output
    }

    /// Flush the held-back tail. An unterminated reasoning block is discarded.
    pub fn flush(&mut self) -> String {
        let output = if self.in_think {
            String::new()
        } else {
            std::mem::take(&mut self.pending)
        };
        self.in_think = false;
        self.skip_leading_ws = false;
        output
    }
}

/// Length of the longest suffix of `text` that is a prefix of `tag`, capped at
/// `tag.len() - 1` so a full match is always found by `find`.
fn suffix_prefix_len(text: &str, tag: &str) -> usize {
    let max = (tag.len() - 1).min(text.len());
    (1..=max)
        .rev()
        .find(|&len| text.ends_with(&tag[..len]))
        .unwrap_or(0)
}

pub fn extract_code_block(text: &str) -> &str {
    CODE_BLOCK_RE
        .captures(text)
        .ok()
        .and_then(|v| v?.get(1).map(|v| v.as_str().trim()))
        .unwrap_or(text)
}

pub fn convert_option_string(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

pub fn fuzzy_filter<T, F>(values: Vec<T>, get: F, pattern: &str) -> Vec<T>
where
    F: Fn(&T) -> &str,
{
    let matcher = SkimMatcherV2::default();
    let mut list: Vec<(T, i64)> = values
        .into_iter()
        .filter_map(|v| {
            let score = matcher.fuzzy_match(get(&v), pattern)?;
            Some((v, score))
        })
        .collect();
    list.sort_unstable_by_key(|b| std::cmp::Reverse(b.1));
    list.into_iter().map(|(v, _)| v).collect()
}

pub fn pretty_error(err: &anyhow::Error) -> String {
    let mut output = vec![];
    output.push(format!("Error: {err}"));
    let causes: Vec<_> = err.chain().skip(1).collect();
    let causes_len = causes.len();
    if causes_len > 0 {
        output.push("\nCaused by:".to_string());
        if causes_len == 1 {
            output.push(format!("    {}", indent_text(causes[0], 4).trim()));
        } else {
            for (i, cause) in causes.into_iter().enumerate() {
                output.push(format!("{i:5}: {}", indent_text(cause, 7).trim()));
            }
        }
    }
    output.join("\n")
}

pub fn indent_text<T: ToString>(s: T, size: usize) -> String {
    let indent_str = " ".repeat(size);
    s.to_string()
        .split('\n')
        .map(|line| format!("{indent_str}{line}"))
        .collect::<Vec<String>>()
        .join("\n")
}

pub fn error_text(input: &str) -> String {
    color_text(input, nu_ansi_term::Color::Red)
}

pub fn warning_text(input: &str) -> String {
    color_text(input, nu_ansi_term::Color::Yellow)
}

pub fn color_text(input: &str, color: nu_ansi_term::Color) -> String {
    if *NO_COLOR {
        return input.to_string();
    }
    nu_ansi_term::Style::new()
        .fg(color)
        .paint(input)
        .to_string()
}

pub fn dimmed_text(input: &str) -> String {
    if *NO_COLOR {
        return input.to_string();
    }
    nu_ansi_term::Style::new().dimmed().paint(input).to_string()
}

pub fn multiline_text(input: &str) -> String {
    input
        .split('\n')
        .enumerate()
        .map(|(i, v)| {
            if i == 0 {
                v.to_string()
            } else {
                format!(".. {v}")
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}

pub fn temp_file(prefix: &str, suffix: &str) -> PathBuf {
    env::temp_dir().join(format!(
        "{}-{}{prefix}{}{suffix}",
        env!("CARGO_CRATE_NAME").to_lowercase(),
        process::id(),
        uuid::Uuid::new_v4()
    ))
}

pub fn is_url(path: &str) -> bool {
    path.starts_with("http://") || path.starts_with("https://")
}

pub fn set_proxy(
    mut builder: reqwest::ClientBuilder,
    proxy: &str,
) -> Result<reqwest::ClientBuilder> {
    builder = builder.no_proxy();
    if !proxy.is_empty() && proxy != "-" {
        builder = builder
            .proxy(reqwest::Proxy::all(proxy).with_context(|| format!("Invalid proxy `{proxy}`"))?);
    };
    Ok(builder)
}

pub fn decode_bin<T: serde::de::DeserializeOwned>(data: &[u8]) -> Result<T> {
    let (v, _) = bincode::serde::decode_from_slice(data, bincode::config::legacy())?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_think_filter(chunks: &[&str]) -> String {
        let mut filter = ThinkFilter::new();
        let mut output = String::new();
        for chunk in chunks {
            output.push_str(&filter.filter(chunk));
        }
        output.push_str(&filter.flush());
        output
    }

    #[test]
    fn test_split_think_tag() {
        assert_eq!(
            split_think_tag("<think>\nwhy\n</think>\n\nanswer"),
            ("answer", "why")
        );
        assert_eq!(split_think_tag("answer"), ("answer", ""));
        // The content half must stay equivalent to `strip_think_tag`.
        assert_eq!(
            strip_think_tag("<think>\nwhy\n</think>\n\nanswer"),
            "answer"
        );
    }

    #[test]
    fn test_think_filter_removes_block() {
        assert_eq!(
            run_think_filter(&["<think>\nreasoning\n</think>\n\nanswer"]),
            "answer"
        );
        assert_eq!(run_think_filter(&["plain answer"]), "plain answer");
        assert_eq!(run_think_filter(&["<think>only reasoning</think>"]), "");
        // an unterminated block is dropped
        assert_eq!(run_think_filter(&["<think>\nreasoning"]), "");
        // a longer word that merely starts like the tag is kept
        assert_eq!(run_think_filter(&["<thinker>"]), "<thinker>");
    }

    #[test]
    fn test_think_filter_provider_chunk_sequence() {
        // Mirrors the events emitted by the openai/claude/bedrock streaming handlers.
        assert_eq!(
            run_think_filter(&["<think>\n", "step 1\n", "step 2", "\n</think>\n\n", "Hello"]),
            "Hello"
        );
    }

    #[test]
    fn test_think_filter_split_chunks() {
        let input = "before <think>\nsecret\n</think>\nafter";
        let expected = "before after";
        for i in 1..input.len() {
            if !input.is_char_boundary(i) {
                continue;
            }
            assert_eq!(
                run_think_filter(&[&input[..i], &input[i..]]),
                expected,
                "split at byte {i}"
            );
        }
        // character by character
        let chunks: Vec<String> = input.chars().map(|c| c.to_string()).collect();
        let refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
        assert_eq!(run_think_filter(&refs), expected);
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn test_safe_join_path() {
        assert_eq!(
            safe_join_path("/home/user/dir1", "files/file1"),
            Some(PathBuf::from("/home/user/dir1/files/file1"))
        );
        assert!(safe_join_path("/home/user/dir1", "/files/file1").is_none());
        assert!(safe_join_path("/home/user/dir1", "../file1").is_none());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn test_safe_join_path() {
        assert_eq!(
            safe_join_path("C:\\Users\\user\\dir1", "files/file1"),
            Some(PathBuf::from("C:\\Users\\user\\dir1\\files\\file1"))
        );
        assert!(safe_join_path("C:\\Users\\user\\dir1", "/files/file1").is_none());
        assert!(safe_join_path("C:\\Users\\user\\dir1", "../file1").is_none());
    }
}
