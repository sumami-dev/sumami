//! The traffic a test or a benchmark needs.
//!
//! A [`Hit`] has more than forty fields, and most callers care about a handful
//! of them. Spelling the rest out (`..Default::default()`) is noisy; spelling
//! **all** of them out is worse. The builder names the fields, so a caller
//! writes
//!
//! ```
//! use sumimi_service::testing::HitBuilder;
//!
//! let hit = HitBuilder::new("w1", "s1", "v1")
//!     .url("/one")
//!     .referrer("news.ycombinator.com")
//!     .country("DE")
//!     .build();
//!
//! assert_eq!(hit.url.as_deref(), Some("/one"));
//! assert_eq!(hit.referrer.as_deref(), Some("news.ycombinator.com"));
//! // The fields nobody named keep the defaults they have everywhere else.
//! assert_eq!(hit.title, None);
//! ```
//!
//! and the fields it does not name keep the defaults they have everywhere else.

use chrono::NaiveDateTime;
use uuid::Uuid;

use crate::models::{Hit, HitKind};
use crate::ua::uuid_from_text;

/// The text fields of a hit.
///
/// Each one is set the same way, so they are written once.
macro_rules! text_fields {
    ($($name:ident),* $(,)?) => {
        $(
            /// Set the field.
            ///
            /// An empty text means the field is not there.
            pub fn $name(mut self, value: impl Into<String>) -> Self {
                self.hit.$name = text(value.into());
                self
            }
        )*
    };
}

/// The number fields of a hit.
macro_rules! number_fields {
    ($($name:ident),* $(,)?) => {
        $(
            /// Set the field.
            pub fn $name(mut self, value: impl Into<f64>) -> Self {
                self.hit.$name = Some(value.into());
                self
            }
        )*
    };
}

/// A hit under construction.
///
/// Every setter returns the builder, so the fields a caller cares about are
/// named in a row and the rest are left alone.
#[derive(Debug, Clone)]
pub struct HitBuilder {
    hit: Hit,
}

impl HitBuilder {
    /// A page view for a website, in a session, seen by a visitor, happening now.
    ///
    /// The session and the visitor are named, not generated: the same text
    /// always means the same session, so a test can put two hits in one
    /// session by naming the same session twice.
    pub fn new(website_id: &str, session: &str, visitor: &str) -> Self {
        Self {
            hit: Hit {
                website_id: website_id.to_string(),
                session_id: uuid_from_text(session),
                visitor_id: uuid_from_text(visitor),
                kind: HitKind::Pageview,
                created_at: chrono::Utc::now().naive_utc(),
                ..Hit::default()
            },
        }
    }

    /// When it happened.
    pub fn at(mut self, at: NaiveDateTime) -> Self {
        self.hit.created_at = at;
        self
    }

    /// What kind of hit it is.
    pub fn kind(mut self, kind: HitKind) -> Self {
        self.hit.kind = kind;
        self
    }

    /// How long the website keeps its data.
    pub fn retention_days(mut self, days: i32) -> Self {
        self.hit.retention_days = days;
        self
    }

    /// The session, by id.
    pub fn session_id(mut self, id: Uuid) -> Self {
        self.hit.session_id = id;
        self
    }

    /// The visitor, by id.
    pub fn visitor_id(mut self, id: Uuid) -> Self {
        self.hit.visitor_id = id;
        self
    }

    /// The visit this hit belongs to, by name.
    pub fn visit(mut self, visit: &str) -> Self {
        self.hit.visit_id = Some(uuid_from_text(visit));
        self
    }

    /// The visit this hit belongs to, by id.
    pub fn visit_id(mut self, id: Uuid) -> Self {
        self.hit.visit_id = Some(id);
        self
    }

    text_fields!(
        url,
        url_query,
        title,
        referrer,
        referrer_path,
        referrer_query,
        referrer_name,
        hostname,
        channel,
        browser,
        browser_version,
        os,
        os_version,
        device,
        screen,
        language,
        country,
        region,
        city,
        utm_source,
        utm_medium,
        utm_campaign,
        utm_content,
        utm_term,
        gclid,
        fbclid,
        msclkid,
        ttclid,
        lifatid,
        twclid,
        name,
        tag,
        data,
    );

    number_fields!(latitude, longitude, lcp, inp, cls, fcp, ttfb);

    /// The hit.
    pub fn build(self) -> Hit {
        self.hit
    }
}

/// A text value: an empty text means the field is not there.
///
/// This is the rule the collector applies to the fields it reads off the wire
/// — an empty title is no title — so a caller that has nothing to say says
/// nothing.
fn text(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}
