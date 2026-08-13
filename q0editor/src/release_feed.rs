use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use egui::Context;
use serde::Deserialize;

const RELEASES_URL: &str =
    "https://api.github.com/repos/overisneverover-lab/q0idmation/releases?per_page=5";
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

fn fetch_releases() -> Result<Vec<ReleaseNote>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(4))
        .timeout_read(Duration::from_secs(8))
        .timeout_write(Duration::from_secs(4))
        .build();
    let response = agent
        .get(RELEASES_URL)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(|error| format!("GitHub request failed: {error}"))?;
    let json = response
        .into_string()
        .map_err(|error| format!("GitHub response could not be read: {error}"))?;
    parse_releases_json(&json)
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
