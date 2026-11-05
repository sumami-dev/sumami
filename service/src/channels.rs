//! The traffic channel: where a visit came from, in one word.
//!
//! The channel is worked out **once**, when the event is collected, and stored
//! with it: a visit that arrives from a search engine is a search visit, and
//! the report does not have to guess it again from the referrer every time.
//!
//! The buckets are the ones the industry settled on:
//!
//! | channel | what it is |
//! | --- | --- |
//! | direct | no referrer — the visitor typed the address, followed a bookmark, or came from a link in an app |
//! | search | a search engine |
//! | social | a social network |
//! | email | a mail client |
//! | ads | an advertisement |
//! | unknown | a referrer that is none of the above |
//!
//! The list of hosts is a list of **referrers**, not of visits: it says which
//! sites mean "search" and which mean "social". A new one is added when a
//! referrer shows up in the unknown bucket often enough to be worth a name.

/// The channels, and the referrers that mean them.
const CHANNELS: &[(&str, &[&str])] = &[
    (
        "search",
        &[
            "google.com",
            "google.co.uk",
            "google.de",
            "google.fr",
            "google.jp",
            "google.com.au",
            "google.ca",
            "google.co.in",
            "google.com.sg",
            "bing.com",
            "duckduckgo.com",
            "search.brave.com",
            "search.yahoo.com",
            "yahoo.com",
            "yandex.ru",
            "yandex.com",
            "baidu.com",
            "naver.com",
            "seznam.cz",
            "ecosia.org",
            "startpage.com",
            "qwant.com",
            "sogou.com",
            "so.com",
            "ask.com",
            "aol.com",
            "daum.net",
        ],
    ),
    (
        "social",
        &[
            "facebook.com",
            "m.facebook.com",
            "lm.facebook.com",
            "instagram.com",
            "threads.net",
            "twitter.com",
            "x.com",
            "t.co",
            "linkedin.com",
            "lnkd.in",
            "reddit.com",
            "out.reddit.com",
            "pinterest.com",
            "pin.it",
            "tiktok.com",
            "youtube.com",
            "youtu.be",
            "weibo.com",
            "wechat.com",
            "telegram.org",
            "t.me",
            "whatsapp.com",
            "mastodon.social",
            "mastodon.social",
            "bsky.app",
            "vk.com",
            "discord.com",
            "tumblr.com",
            "quora.com",
            "medium.com",
            "news.ycombinator.com",
            "lobste.rs",
            "producthunt.com",
            "slack.com",
        ],
    ),
    (
        "email",
        &[
            "mail.google.com",
            "outlook.live.com",
            "outlook.office.com",
            "outlook.com",
            "mail.yahoo.com",
            "mail.proton.me",
            "mail.ru",
            "mail.qq.com",
            "mail.163.com",
            "fastmail.com",
            "zoho.com",
            "gmx.com",
            "yandex.com",
            "hey.com",
        ],
    ),
    (
        "ads",
        &[
            "googleadservices.com",
            "adservice.google.com",
            "doubleclick.net",
            "googleads.g.doubleclick.net",
            "ads.google.com",
            "adwords.com",
            "ads.linkedin.com",
            "ad.doubleclick.de",
            "ads-twitter.com",
            "ads.facebook.com",
            "adnxs.com",
            "adsrvr.org",
            "criteo.com",
            "taboola.com",
            "outbrain.com",
            "bing.com/aclick",
        ],
    ),
];

/// The channel of a visit.
///
/// The referrer is the **host** of the page that sent the visitor — the
/// tracker sends it that way — but a full URL is accepted too.
pub fn channel(referrer: Option<&str>) -> &'static str {
    let Some(referrer) = referrer.filter(|r| !r.trim().is_empty()) else {
        return "direct";
    };

    let host = host_of(referrer);
    if host.is_empty() {
        return "direct";
    }

    // The **longest** match wins: `mail.google.com` is a mail client, not a
    // search engine, even though it ends with `google.com`.
    let mut best: Option<(usize, &'static str)> = None;
    for (name, hosts) in CHANNELS {
        for known in hosts.iter() {
            if matches(&host, known) {
                let length = known.len();
                if best.map(|(best, _)| length > best).unwrap_or(true) {
                    best = Some((length, name));
                }
            }
        }
    }

    best.map(|(_, name)| name).unwrap_or("unknown")
}

/// The host of a referrer.
///
/// The referrer is normally a host (`news.ycombinator.com`), but a tracker may
/// send the whole URL — the host is taken out of it.
fn host_of(referrer: &str) -> String {
    let referrer = referrer.trim();
    if referrer.is_empty() {
        return String::new();
    }

    // A URL with a scheme.
    let without_scheme = match referrer.find("://") {
        Some(at) => &referrer[at + 3..],
        None => referrer,
    };

    // Everything up to the first slash, question mark or hash.
    let host = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .trim();

    // A userinfo part (`user@host`) is not part of the host.
    match host.rsplit_once('@') {
        Some((_, rest)) => rest.to_lowercase(),
        None => host.to_lowercase(),
    }
}

/// Whether a host is the given domain, or a subdomain of it.
fn matches(host: &str, known: &str) -> bool {
    if host == known {
        return true;
    }
    match host.strip_suffix(known) {
        Some(prefix) => prefix.ends_with('.'),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_referrer_is_a_direct_visit() {
        assert_eq!(channel(None), "direct");
        assert_eq!(channel(Some("")), "direct");
        assert_eq!(channel(Some("   ")), "direct");
    }

    #[test]
    fn a_search_engine_is_a_search() {
        assert_eq!(channel(Some("google.com")), "search");
        assert_eq!(channel(Some("www.google.com")), "search");
        assert_eq!(channel(Some("duckduckgo.com")), "search");
        assert_eq!(channel(Some("news.google.com")), "search");
    }

    #[test]
    fn a_social_network_is_social() {
        assert_eq!(channel(Some("t.co")), "social");
        assert_eq!(channel(Some("news.ycombinator.com")), "social");
        assert_eq!(channel(Some("www.reddit.com")), "social");
    }

    #[test]
    fn a_mail_client_is_email() {
        assert_eq!(channel(Some("mail.google.com")), "email");
        assert_eq!(channel(Some("outlook.live.com")), "email");
    }

    #[test]
    fn an_advertisement_is_ads() {
        assert_eq!(channel(Some("googleadservices.com")), "ads");
    }

    #[test]
    fn something_else_is_unknown() {
        assert_eq!(channel(Some("example.com")), "unknown");
        assert_eq!(channel(Some("notgoogle.com")), "unknown");
    }

    #[test]
    fn a_full_url_is_understood() {
        assert_eq!(channel(Some("https://www.google.com/search?q=x")), "search");
        assert_eq!(channel(Some("https://example.com/page")), "unknown");
    }
}
