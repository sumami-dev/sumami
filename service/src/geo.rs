//! Where the visitor is.
//!
//! The country, the region and the city of a visit — worked out from the IP
//! address. **The IP address itself is never stored**: only the country code,
//! the region and the city end up in the database, and the lookup happens in
//! memory, on the collection path.
//!
//! There are three ways to know where a visitor is, and they are tried in this
//! order:
//!
//! 1. **The header.** A proxy in front of the server knows the country and puts
//!    it in a header (`CF-IPCountry` and friends). It is the most accurate
//!    answer, and it costs nothing — this is what the open-source build uses.
//! 2. **The resolver.** The server's own geolocation database. The private
//!    edition has one: a MaxMind GeoLite2 file, read in memory.
//! 3. **Nothing.** The country stays empty and the report says "unknown".
//!
//! The resolver is a trait, so both editions use the same collection path and
//! the same reports — one edition ships no data files at all.

use std::net::IpAddr;

/// What is known about where a visitor is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Location {
    /// The two-letter country code (`DE`).
    pub country: Option<String>,
    /// The region — a state, a province.
    pub region: Option<String>,
    /// The city.
    pub city: Option<String>,
    /// The latitude, in degrees.
    ///
    /// The city-level database has it; the country-level one does not.
    pub latitude: Option<f64>,
    /// The longitude, in degrees.
    pub longitude: Option<f64>,
}

/// Looks up where an IP address is.
///
/// The lookup is on the collection path, so it has to be fast: an in-memory
/// table, not a query.
pub trait Resolver: Send + Sync {
    /// Where this address is, if it is known.
    fn lookup(&self, ip: IpAddr) -> Option<Location>;
}

/// The resolver of a build that has none: everything stays unknown.
///
/// This is what the open-source build uses unless the site owner puts a
/// country in a header.
pub struct NoResolver;

impl Resolver for NoResolver {
    fn lookup(&self, _ip: IpAddr) -> Option<Location> {
        None
    }
}

/// Read the country out of a header value.
///
/// The value is the country code (`DE`, `de`, `DEU`) — a proxy puts it there.
/// Anything that is not a country code is ignored: a header that says
/// `XX` or `T1` is a Tor exit node, not a place.
pub fn country_from_header(value: &str) -> Option<String> {
    let value = value.trim().to_uppercase();
    if value.len() != 2 || !value.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    // `XX` is the code for "we do not know" — a Tor exit node, a proxy that
    // does not know either. It is not a place.
    if value == "XX" {
        return None;
    }
    Some(value)
}

/// The region and the city from a header value.
///
/// Some proxies send them as `Region/City` (`Berlin/DE`) — the format the
/// header-based country lookup uses elsewhere in the industry.
pub fn region_city_from_header(value: &str) -> (Option<String>, Option<String>) {
    let value = value.trim();
    if value.is_empty() {
        return (None, None);
    }
    match value.split_once('/') {
        Some((region, city)) => (
            non_empty(region),
            non_empty(city),
        ),
        None => (None, non_empty(value)),
    }
}

/// A non-empty string, or nothing.
fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_country_code_is_read_from_the_header() {
        assert_eq!(country_from_header("DE"), Some("DE".to_string()));
        assert_eq!(country_from_header("de"), Some("DE".to_string()));
        assert_eq!(country_from_header(" de "), Some("DE".to_string()));
    }

    #[test]
    fn something_that_is_not_a_country_is_ignored() {
        // A Tor exit node, a list, an empty value.
        assert_eq!(country_from_header("XX"), None);
        assert_eq!(country_from_header("T1"), None);
        assert_eq!(country_from_header(""), None);
        assert_eq!(country_from_header("DEU"), None);
        assert_eq!(country_from_header("1A"), None);
    }

    #[test]
    fn a_region_and_a_city_are_read_from_the_header() {
        assert_eq!(
            region_city_from_header("Berlin/DE"),
            (Some("Berlin".to_string()), Some("DE".to_string()))
        );
        assert_eq!(
            region_city_from_header("Berlin"),
            (None, Some("Berlin".to_string()))
        );
        assert_eq!(region_city_from_header(""), (None, None));
    }
}
