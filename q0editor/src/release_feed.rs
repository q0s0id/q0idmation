use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use egui::Context;
use serde::Deserialize;

const RELEASES_URL: &str =
    "https://api.github.com/repos/overisneverover-lab/q0idmation/releases?per_page=5";
const RELEASES_ATOM_URL: &str = "https://github.com/overisneverover-lab/q0idmation/releases.atom";
const USER_AGENT: &str = "q0editor-release-feed";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseNote {
    pub title: String,
    pub tag: String,
    pub body: String,
    pub url: String,
    pub published_at: Option<String>,
    pub prerelease: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkdownSpanKind {
    Plain,
    Strong,
    Code,
    Emphasis,
    Link(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownSpan {
    pub text: String,
    pub kind: MarkdownSpanKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkdownBlock {
    Heading { level: u8, spans: Vec<MarkdownSpan> },
    Bullet(Vec<MarkdownSpan>),
    Paragraph(Vec<MarkdownSpan>),
    Spacer,
}

pub fn parse_release_markdown(body: &str) -> Vec<MarkdownBlock> {
    body.lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return MarkdownBlock::Spacer;
            }

            if let Some((level, text)) = heading_line(trimmed) {
                return MarkdownBlock::Heading {
                    level,
                    spans: parse_inline_markdown(text),
                };
            }
            if let Some(text) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
            {
                return MarkdownBlock::Bullet(parse_inline_markdown(text));
            }
            MarkdownBlock::Paragraph(parse_inline_markdown(trimmed))
        })
        .collect()
}

fn heading_line(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&hashes) || line.as_bytes().get(hashes) != Some(&b' ') {
        return None;
    }
    Some((hashes as u8, line[hashes + 1..].trim()))
}

fn parse_inline_markdown(mut text: &str) -> Vec<MarkdownSpan> {
    let mut spans = Vec::new();
    while !text.is_empty() {
        if let Some(rest) = text.strip_prefix("**") {
            if let Some(end) = rest.find("**") {
                push_span(&mut spans, &rest[..end], MarkdownSpanKind::Strong);
                text = &rest[end + 2..];
                continue;
            }
        }
        if let Some(rest) = text.strip_prefix('`') {
            if let Some(end) = rest.find('`') {
                push_span(&mut spans, &rest[..end], MarkdownSpanKind::Code);
                text = &rest[end + 1..];
                continue;
            }
        }
        if let Some(rest) = text.strip_prefix('[') {
            if let Some(label_end) = rest.find("](") {
                let after_label = &rest[label_end + 2..];
                if let Some(url_end) = after_label.find(')') {
                    let label = &rest[..label_end];
                    let url = &after_label[..url_end];
                    if !label.is_empty() && !url.is_empty() {
                        push_span(&mut spans, label, MarkdownSpanKind::Link(url.to_string()));
                        text = &after_label[url_end + 1..];
                        continue;
                    }
                }
            }
        }
        if let Some(rest) = text.strip_prefix('*') {
            if !rest.starts_with('*') {
                if let Some(end) = rest.find('*') {
                    push_span(&mut spans, &rest[..end], MarkdownSpanKind::Emphasis);
                    text = &rest[end + 1..];
                    continue;
                }
            }
        }

        let next = [
            text.find("**"),
            text.find('`'),
            text.find('['),
            text.find('*'),
        ]
        .into_iter()
        .flatten()
        .filter(|index| *index > 0)
        .min()
        .unwrap_or(text.len());
        if next == 0 || next == text.len() {
            push_span(&mut spans, text, MarkdownSpanKind::Plain);
            break;
        }
        push_span(&mut spans, &text[..next], MarkdownSpanKind::Plain);
        text = &text[next..];
    }
    spans
}

fn push_span(spans: &mut Vec<MarkdownSpan>, text: &str, kind: MarkdownSpanKind) {
    if text.is_empty() {
        return;
    }
    spans.push(MarkdownSpan {
        text: text.to_string(),
        kind,
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseFeedStatus {
    Idle,
    Loading,
    Ready(Vec<ReleaseNote>),
    Error(String),
}

pub struct ReleaseFeedState {
    status: ReleaseFeedStatus,
    receiver: Option<Receiver<Result<Vec<ReleaseNote>, String>>>,
}

impl Default for ReleaseFeedState {
    fn default() -> Self {
        Self {
            status: ReleaseFeedStatus::Idle,
            receiver: None,
        }
    }
}

impl ReleaseFeedState {
    pub fn ensure_started(&mut self, ctx: &Context) {
        if matches!(self.status, ReleaseFeedStatus::Idle) {
            self.start(ctx);
        }
    }

    pub fn retry(&mut self, ctx: &Context) {
        self.start(ctx);
    }

    pub fn poll(&mut self) {
        let Some(receiver) = self.receiver.as_ref() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(releases)) => {
                self.status = ReleaseFeedStatus::Ready(releases);
                self.receiver = None;
            }
            Ok(Err(error)) => {
                self.status = ReleaseFeedStatus::Error(error);
                self.receiver = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.status = ReleaseFeedStatus::Error(
                    "release-feed worker stopped before returning a result".to_string(),
                );
                self.receiver = None;
            }
        }
    }

    pub fn status(&self) -> &ReleaseFeedStatus {
        &self.status
    }

    fn start(&mut self, ctx: &Context) {
        let (sender, receiver) = mpsc::channel();
        let repaint = ctx.clone();
        match std::thread::Builder::new()
            .name("q0editor-release-feed".to_string())
            .spawn(move || {
                let result = fetch_releases();
                let _ = sender.send(result);
                repaint.request_repaint();
            }) {
            Ok(_) => {
                self.status = ReleaseFeedStatus::Loading;
                self.receiver = Some(receiver);
            }
            Err(error) => {
                self.status = ReleaseFeedStatus::Error(format!(
                    "could not start release-feed worker: {error}"
                ));
                self.receiver = None;
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    name: Option<String>,
    tag_name: String,
    body: Option<String>,
    html_url: String,
    published_at: Option<String>,
    prerelease: bool,
    draft: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FetchError {
    message: String,
    retryable: bool,
}

impl FetchError {
    fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }

    fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }
}

fn fetch_releases() -> Result<Vec<ReleaseNote>, String> {
    const RETRY_DELAYS: [Duration; 1] = [Duration::from_millis(350)];
    let rest = retry_transient(
        &RETRY_DELAYS,
        || {
            let agent = release_feed_agent();
            fetch_releases_rest_once(&agent)
        },
        thread::sleep,
    );
    rest_then_atom(rest, || {
        retry_transient(
            &RETRY_DELAYS,
            || {
                let agent = release_feed_agent();
                fetch_releases_atom_once(&agent)
            },
            thread::sleep,
        )
    })
}

fn rest_then_atom<T>(
    rest: Result<T, FetchError>,
    atom: impl FnOnce() -> Result<T, FetchError>,
) -> Result<T, String> {
    match rest {
        Ok(value) => Ok(value),
        Err(rest_error) => atom().map_err(|atom_error| {
            format!(
                "GitHub releases unavailable (REST: {}; Atom: {})",
                rest_error.message, atom_error.message
            )
        }),
    }
}

fn release_feed_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(4))
        .timeout_read(Duration::from_secs(8))
        .timeout_write(Duration::from_secs(4))
        .build()
}

fn fetch_releases_rest_once(agent: &ureq::Agent) -> Result<Vec<ReleaseNote>, FetchError> {
    let response = request(agent, RELEASES_URL, true)?;
    let json = response.into_string().map_err(|error| {
        FetchError::retryable(format!("GitHub REST response could not be read: {error}"))
    })?;
    parse_releases_json(&json).map_err(FetchError::fatal)
}

fn fetch_releases_atom_once(agent: &ureq::Agent) -> Result<Vec<ReleaseNote>, FetchError> {
    let response = request(agent, RELEASES_ATOM_URL, false)?;
    let atom = response.into_string().map_err(|error| {
        FetchError::retryable(format!("GitHub Atom response could not be read: {error}"))
    })?;
    parse_releases_atom(&atom).map_err(FetchError::fatal)
}

fn request(agent: &ureq::Agent, url: &str, github_api: bool) -> Result<ureq::Response, FetchError> {
    let mut request = agent.get(url).set("User-Agent", USER_AGENT);
    if github_api {
        request = request
            .set("Accept", "application/vnd.github+json")
            .set("X-GitHub-Api-Version", "2022-11-28");
    } else {
        request = request.set("Accept", "application/atom+xml");
    }

    match request.call() {
        Ok(response) => Ok(response),
        Err(ureq::Error::Status(status, response)) => {
            let rate_limited = status == 403
                && response
                    .header("x-ratelimit-remaining")
                    .is_some_and(|remaining| remaining == "0");
            let message = if rate_limited {
                format!("GitHub request rate-limited with HTTP {status}")
            } else {
                format!("GitHub request failed with HTTP {status}")
            };
            Err(if retryable_http_status(status) {
                FetchError::retryable(message)
            } else {
                FetchError::fatal(message)
            })
        }
        Err(ureq::Error::Transport(error)) => Err(FetchError::retryable(format!(
            "GitHub transport failed: {error}"
        ))),
    }
}

fn retryable_http_status(status: u16) -> bool {
    status == 408 || status == 429 || (500..=599).contains(&status)
}

fn retry_transient<T>(
    delays: &[Duration],
    mut fetch: impl FnMut() -> Result<T, FetchError>,
    mut sleep: impl FnMut(Duration),
) -> Result<T, FetchError> {
    for delay in delays {
        match fetch() {
            Ok(value) => return Ok(value),
            Err(error) if error.retryable => sleep(*delay),
            Err(error) => return Err(error),
        }
    }
    fetch()
}

fn parse_releases_atom(atom: &str) -> Result<Vec<ReleaseNote>, String> {
    let mut releases = Vec::new();
    let mut rest = atom;
    while let Some(entry_start) = rest.find("<entry>") {
        rest = &rest[entry_start + "<entry>".len()..];
        let Some(entry_end) = rest.find("</entry>") else {
            return Err("GitHub Atom release entry was truncated".to_string());
        };
        let entry = &rest[..entry_end];
        rest = &rest[entry_end + "</entry>".len()..];

        let title = atom_text(entry, "title")
            .ok_or_else(|| "GitHub Atom release was missing a title".to_string())?;
        let published_at = atom_text(entry, "updated");
        let url = atom_alternate_href(entry)
            .ok_or_else(|| "GitHub Atom release was missing its link".to_string())?;
        let tag = url
            .split("/tag/")
            .nth(1)
            .filter(|tag| !tag.is_empty())
            .map(decode_xml_entities)
            .ok_or_else(|| "GitHub Atom release link had no tag".to_string())?;
        let html = atom_text(entry, "content")
            .ok_or_else(|| "GitHub Atom release was missing its content".to_string())?;
        let markdown = github_release_html_to_markdown(&html);
        releases.push(ReleaseNote {
            title,
            tag: tag.clone(),
            body: normalize_body(Some(&markdown)),
            url,
            published_at,
            prerelease: tag_looks_prerelease(&tag),
        });
    }

    releases.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    Ok(releases)
}

fn atom_text(entry: &str, tag: &str) -> Option<String> {
    let open_prefix = format!("<{tag}");
    let start = entry.find(&open_prefix)?;
    let open_end = entry[start..].find('>')? + start;
    let close = format!("</{tag}>");
    let end = entry[open_end + 1..].find(&close)? + open_end + 1;
    Some(decode_xml_entities(&entry[open_end + 1..end]))
}

fn atom_alternate_href(entry: &str) -> Option<String> {
    let mut rest = entry;
    while let Some(start) = rest.find("<link") {
        rest = &rest[start..];
        let end = rest.find('>')?;
        let tag = &rest[..=end];
        rest = &rest[end + 1..];
        if attribute(tag, "rel").as_deref() == Some("alternate") {
            return attribute(tag, "href").map(|href| decode_xml_entities(&href));
        }
    }
    None
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let needle = format!("{name}={quote}");
        if let Some(start) = tag.find(&needle) {
            let value = &tag[start + needle.len()..];
            let end = value.find(quote)?;
            return Some(value[..end].to_string());
        }
    }
    None
}

fn tag_looks_prerelease(tag: &str) -> bool {
    tag.trim_start_matches('v').contains('-')
}

fn decode_xml_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';') else {
            out.push_str(rest);
            return out;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            _ if entity.starts_with("#x") => u32::from_str_radix(&entity[2..], 16)
                .ok()
                .and_then(char::from_u32),
            _ if entity.starts_with('#') => entity[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        if let Some(ch) = decoded {
            out.push(ch);
        } else {
            out.push_str(&rest[..=semi]);
        }
        rest = &rest[semi + 1..];
    }
    out.push_str(rest);
    out
}

fn github_release_html_to_markdown(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    let mut links = Vec::<String>::new();
    while !rest.is_empty() {
        let Some(tag_start) = rest.find('<') else {
            push_html_text(&mut out, rest);
            break;
        };
        push_html_text(&mut out, &rest[..tag_start]);
        rest = &rest[tag_start..];
        let Some(tag_end) = rest.find('>') else {
            push_html_text(&mut out, rest);
            break;
        };
        let raw_tag = &rest[1..tag_end];
        rest = &rest[tag_end + 1..];
        let trimmed = raw_tag.trim();
        let closing = trimmed.starts_with('/');
        let tag_name = trimmed
            .trim_start_matches('/')
            .split_ascii_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_ascii_lowercase();

        match (closing, tag_name.as_str()) {
            (false, "h1") => push_block_prefix(&mut out, "# "),
            (false, "h2") => push_block_prefix(&mut out, "## "),
            (false, "h3") => push_block_prefix(&mut out, "### "),
            (true, "h1" | "h2" | "h3" | "p") => push_block_suffix(&mut out),
            (false, "p") => ensure_block_boundary(&mut out),
            (false, "li") => push_block_prefix(&mut out, "- "),
            (true, "li") => ensure_newline(&mut out),
            (false, "strong" | "b") | (true, "strong" | "b") => out.push_str("**"),
            (false, "em" | "i") | (true, "em" | "i") => out.push('*'),
            (false, "code") | (true, "code") => out.push('`'),
            (false, "br") => ensure_newline(&mut out),
            (false, "a") => {
                links.push(attribute(trimmed, "href").unwrap_or_default());
                out.push('[');
            }
            (true, "a") => {
                let href = links.pop().unwrap_or_default();
                out.push_str("](");
                out.push_str(&decode_xml_entities(&href));
                out.push(')');
            }
            _ => {}
        }
    }
    out.trim().to_string()
}

fn push_html_text(out: &mut String, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    out.push_str(&decode_xml_entities(text));
}

fn ensure_newline(out: &mut String) {
    if !out.ends_with('\n') {
        out.push('\n');
    }
}

fn ensure_block_boundary(out: &mut String) {
    if out.is_empty() {
        return;
    }
    ensure_newline(out);
    if !out.ends_with("\n\n") {
        out.push('\n');
    }
}

fn push_block_prefix(out: &mut String, prefix: &str) {
    ensure_block_boundary(out);
    out.push_str(prefix);
}

fn push_block_suffix(out: &mut String) {
    ensure_newline(out);
    if !out.ends_with("\n\n") {
        out.push('\n');
    }
}

fn parse_releases_json(json: &str) -> Result<Vec<ReleaseNote>, String> {
    let releases: Vec<GithubRelease> = serde_json::from_str(json)
        .map_err(|error| format!("GitHub release data was invalid: {error}"))?;
    let mut releases = releases
        .into_iter()
        .filter(|release| !release.draft)
        .map(|release| {
            let title = release
                .name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| release.tag_name.clone());
            ReleaseNote {
                title,
                tag: release.tag_name,
                body: normalize_body(release.body.as_deref()),
                url: release.html_url,
                published_at: release.published_at,
                prerelease: release.prerelease,
            }
        })
        .collect::<Vec<_>>();
    releases.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    Ok(releases)
}

fn normalize_body(body: Option<&str>) -> String {
    let body = body.unwrap_or_default().replace('\r', "");
    let body = body.trim();
    if body.is_empty() {
        return "No release notes were provided for this release.".to_string();
    }

    const MAX_CHARS: usize = 1800;
    let mut chars = body.chars();
    let preview = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}\n\n...")
    } else {
        preview
    }
}

pub fn short_date(value: Option<&str>) -> Option<&str> {
    value.and_then(|value| value.get(..10))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_github_release_shape_and_keeps_notes() {
        let json = r###"[
            {
                "name": "q0idmation v0.1.0-beta.1 - first public beta",
                "tag_name": "v0.1.0-beta.1",
                "body": "## What's Changed\n\n- real release note",
                "html_url": "https://github.com/overisneverover-lab/q0idmation/releases/tag/v0.1.0-beta.1",
                "published_at": "2026-08-06T20:38:29Z",
                "prerelease": true,
                "draft": false
            }
        ]"###;

        let releases = parse_releases_json(json).expect("parse release fixture");
        assert_eq!(releases.len(), 1);
        assert_eq!(
            releases[0].title,
            "q0idmation v0.1.0-beta.1 - first public beta"
        );
        assert_eq!(releases[0].tag, "v0.1.0-beta.1");
        assert!(releases[0].body.contains("real release note"));
        assert!(releases[0].prerelease);
        assert_eq!(
            short_date(releases[0].published_at.as_deref()),
            Some("2026-08-06")
        );
    }

    #[test]
    fn empty_release_body_does_not_turn_a_real_release_into_an_empty_feed() {
        let json = r###"[
            {
                "name": null,
                "tag_name": "v1.2.3",
                "body": "",
                "html_url": "https://github.com/overisneverover-lab/q0idmation/releases/tag/v1.2.3",
                "published_at": null,
                "prerelease": false,
                "draft": false
            }
        ]"###;

        let releases = parse_releases_json(json).expect("parse release fixture");
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].title, "v1.2.3");
        assert_eq!(
            releases[0].body,
            "No release notes were provided for this release."
        );
    }

    #[test]
    fn release_markdown_formats_real_release_notes_without_losing_text() {
        let body = "# q0idmation\n\n## included\n\n- **q0editor** opens `.q1s` and [releases](https://example.invalid).\n\nplain *beta* note";
        let blocks = parse_release_markdown(body);

        assert!(matches!(
            &blocks[0],
            MarkdownBlock::Heading { level: 1, spans }
                if spans == &vec![MarkdownSpan {
                    text: "q0idmation".to_string(),
                    kind: MarkdownSpanKind::Plain,
                }]
        ));
        assert!(matches!(
            &blocks[2],
            MarkdownBlock::Heading { level: 2, .. }
        ));

        let MarkdownBlock::Bullet(spans) = &blocks[4] else {
            panic!("release list item was not parsed as a bullet");
        };
        assert!(spans.iter().any(|span| {
            span.text == "q0editor" && matches!(span.kind, MarkdownSpanKind::Strong)
        }));
        assert!(spans
            .iter()
            .any(|span| { span.text == ".q1s" && matches!(span.kind, MarkdownSpanKind::Code) }));
        assert!(spans.iter().any(|span| {
            span.text == "releases"
                && matches!(
                    &span.kind,
                    MarkdownSpanKind::Link(url) if url == "https://example.invalid"
                )
        }));

        let MarkdownBlock::Paragraph(spans) = &blocks[6] else {
            panic!("plain release note was not parsed as a paragraph");
        };
        assert!(spans.iter().any(|span| {
            span.text == "beta" && matches!(span.kind, MarkdownSpanKind::Emphasis)
        }));
    }

    #[test]
    fn malformed_inline_markdown_degrades_to_visible_plain_text() {
        let blocks = parse_release_markdown("broken **bold and `code");
        let MarkdownBlock::Paragraph(spans) = &blocks[0] else {
            panic!("malformed markdown should remain a paragraph");
        };
        assert_eq!(
            spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>(),
            "broken **bold and `code"
        );
    }

    #[test]
    fn transient_initial_fetch_is_retried_before_the_ui_sees_an_error() {
        let mut attempts = 0;
        let mut sleeps = Vec::new();
        let result = retry_transient(
            &[Duration::from_millis(350)],
            || {
                attempts += 1;
                if attempts == 1 {
                    Err(FetchError::retryable("startup transport hiccup"))
                } else {
                    Ok("release feed")
                }
            },
            |delay| sleeps.push(delay),
        );

        assert_eq!(result.as_deref(), Ok("release feed"));
        assert_eq!(attempts, 2);
        assert_eq!(sleeps, vec![Duration::from_millis(350)]);
    }

    #[test]
    fn fatal_fetch_error_is_not_retried() {
        let mut attempts = 0;
        let result = retry_transient(
            &[Duration::from_millis(350)],
            || {
                attempts += 1;
                Err::<(), _>(FetchError::fatal("bad github data"))
            },
            |_| panic!("fatal fetch must not sleep or retry"),
        );

        let error = result.expect_err("fatal fetch should fail");
        assert_eq!(error.message, "bad github data");
        assert!(!error.retryable);
        assert_eq!(attempts, 1);
    }

    #[test]
    fn only_transient_http_statuses_are_retried() {
        for status in [408, 429, 500, 502, 503, 599] {
            assert!(retryable_http_status(status), "status {status}");
        }
        for status in [400, 401, 403, 404, 422] {
            assert!(!retryable_http_status(status), "status {status}");
        }
    }

    #[test]
    fn rest_failure_uses_atom_before_the_ui_sees_unavailable() {
        let rest = Err::<&str, _>(FetchError::fatal("HTTP 403 rate limit"));
        let result = rest_then_atom(rest, || Ok("atom release"));
        assert_eq!(result.as_deref(), Ok("atom release"));
    }

    #[test]
    fn dual_source_failure_reports_both_causes() {
        let rest = Err::<(), _>(FetchError::fatal("REST 403"));
        let error = rest_then_atom(rest, || Err(FetchError::retryable("Atom timeout")))
            .expect_err("both sources should fail");
        assert!(error.contains("REST 403"));
        assert!(error.contains("Atom timeout"));
    }

    #[test]
    fn github_atom_fallback_preserves_release_metadata_and_formatting() {
        let atom = r###"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry>
    <updated>2026-08-06T20:38:29Z</updated>
    <link rel="alternate" type="text/html" href="https://github.com/q0s0id/q0idmation/releases/tag/v0.1.0-beta.1"/>
    <title>q0idmation v0.1.0-beta.1 - first public beta</title>
    <content type="html">&lt;h2&gt;included&lt;/h2&gt;
&lt;ul&gt;&lt;li&gt;&lt;strong&gt;q0editor&lt;/strong&gt; opens &lt;code&gt;.q1s&lt;/code&gt; &amp;amp; exports.&lt;/li&gt;&lt;/ul&gt;</content>
  </entry>
</feed>"###;

        let releases = parse_releases_atom(atom).expect("parse GitHub Atom fixture");
        assert_eq!(releases.len(), 1);
        let release = &releases[0];
        assert_eq!(release.tag, "v0.1.0-beta.1");
        assert_eq!(
            release.published_at.as_deref(),
            Some("2026-08-06T20:38:29Z")
        );
        assert!(release.prerelease);
        assert_eq!(
            release.url,
            "https://github.com/q0s0id/q0idmation/releases/tag/v0.1.0-beta.1"
        );
        assert!(release.body.contains("## included"));
        assert!(release.body.contains("**q0editor**"));
        assert!(release.body.contains("`.q1s`"));
        assert!(release.body.contains("& exports"));
    }

    #[test]
    fn draft_releases_are_not_exposed_in_the_public_feed() {
        let json = r###"[
            {
                "name": "draft",
                "tag_name": "draft",
                "body": "not public",
                "html_url": "https://github.com/overisneverover-lab/q0idmation/releases/tag/draft",
                "published_at": "2026-08-13T00:00:00Z",
                "prerelease": false,
                "draft": true
            }
        ]"###;

        assert!(parse_releases_json(json)
            .expect("parse draft fixture")
            .is_empty());
    }
}
