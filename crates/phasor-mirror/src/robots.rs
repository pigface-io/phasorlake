//! A host's `robots.txt`, read once per run before anything else: a root it
//! disallows this client is an error, and its crawl delay slows the host's
//! pace where it is the longer. A host with none, or one it will not serve
//! (nemweb's sits behind a challenge page), has no rules.

use std::time::Duration;

use futures::StreamExt as _;

use crate::error::MirrorError;
use crate::hosts::Host;
use crate::net::Transport;

/// The product token the rules are read for.
pub const AGENT: &str = "phasor-mirror";

/// The most of a robots.txt that is read.
const MOST_BYTES: usize = 512 * 1024;

/// Reads `host`'s `robots.txt` through `transport` (no pacing, no breaker:
/// one request a host a run) and applies it: a disallowed root refuses,
/// a crawl delay longer than the host's pace becomes its pace.
pub async fn apply(transport: &dyn Transport, host: &mut Host) -> Result<(), MirrorError> {
    let Some(text) = read(transport, &host.base_url).await else {
        return Ok(());
    };
    check(&text, host)
}

/// The body of `{base}/robots.txt` when the host serves one with 200.
async fn read(transport: &dyn Transport, base: &str) -> Option<Vec<u8>> {
    let url = format!("{base}/robots.txt");
    let resp = transport.get(&url).await.ok()?;
    if resp.status != 200 {
        return None;
    }
    let mut body = resp.body;
    let mut out = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.ok()?;
        out.extend_from_slice(&chunk);
        if out.len() > MOST_BYTES {
            return None;
        }
    }
    Some(out)
}

/// Applies the rules in `text` to `host`; text that is no robots.txt is no rules.
pub fn check(text: &[u8], host: &mut Host) -> Result<(), MirrorError> {
    texting_robots::Robot::new(AGENT, text)
        .ok()
        .map_or(Ok(()), |robot| rules(&robot, host))
}

/// Applies `robot`'s rules to `host`.
fn rules(robot: &texting_robots::Robot, host: &mut Host) -> Result<(), MirrorError> {
    for root in &host.roots {
        if !robot.allowed(root.as_str()) {
            return Err(MirrorError::RobotsDisallow {
                host: host.id.clone(),
                root: root.to_string(),
            });
        }
    }
    if let Some(secs) = robot.delay {
        let delay = Duration::from_secs_f32(secs.max(0.0));
        if delay > host.request_interval {
            host.request_interval = delay;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::config::HostConfig;
    use crate::net::transport::{Canned, StaticTransport};

    fn host() -> Host {
        Host::from_config(
            &HostConfig {
                base_url: "https://h".to_owned(),
                roots: vec!["Reports/".to_owned(), "Data/".to_owned()],
                ..HostConfig::nemweb()
            },
            500,
        )
        .unwrap()
    }

    #[test]
    fn a_disallowed_root_refuses_and_a_delay_slows_the_pace() {
        let mut h = host();
        check(
            b"User-agent: *\nDisallow: /private/\nCrawl-delay: 2\n",
            &mut h,
        )
        .unwrap();
        assert_eq!(
            h.request_interval,
            Duration::from_secs(2),
            "2 s is longer than 500 ms"
        );
        let mut h = host();
        check(b"User-agent: phasor-mirror\nCrawl-delay: 0.1\n", &mut h).unwrap();
        assert_eq!(
            h.request_interval,
            Duration::from_millis(500),
            "the pace stays"
        );
        let mut h = host();
        assert!(matches!(
            check(b"User-agent: *\nDisallow: /Data/\n", &mut h).unwrap_err(),
            MirrorError::RobotsDisallow { host, root } if host == "nemweb" && root == "https://h/Data/"
        ));
        let mut h = host();
        check(b"", &mut h).unwrap();
    }

    #[tokio::test]
    async fn only_a_200_body_counts() {
        let t = Arc::new(StaticTransport::default());
        t.add(
            "https://h/robots.txt",
            Canned::ok(b"User-agent: *\nDisallow: /Data/\n"),
        );
        let mut h = host();
        assert!(apply(&t, &mut h).await.is_err());
        let t = Arc::new(StaticTransport::default());
        t.add("https://h/robots.txt", Canned::status(403));
        let mut h = host();
        apply(&t, &mut h).await.unwrap();
        // No route at all: the transport's error is no rules.
        let t = Arc::new(StaticTransport::default());
        let mut h = host();
        apply(&t, &mut h).await.unwrap();
        assert!(read(&t, "https://h").await.is_none());
        // A body past the most read is no rules either.
        let t = Arc::new(StaticTransport::default());
        let big = vec![b'#'; MOST_BYTES + 1];
        t.add("https://h/robots.txt", Canned::ok(&big));
        assert!(read(&t, "https://h").await.is_none());
        // Bytes that are not a robots.txt at all: no rules.
        let mut h = host();
        check(b"\xff\xfe\x00 User-agent", &mut h).unwrap();
        assert_eq!(h.request_interval, Duration::from_millis(500));
    }
}
