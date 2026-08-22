//! Item lifetimes: [`Ttl`] for every write verb and [`Freshness`] for
//! `fetch`.
//!
//! `Ttl` is opaque. Constructors never fail; the verb that sends the value
//! validates it, so a `const` TTL can live in a `static` and a bad one is
//! reported where it is used. The protocol rules (0 means "never expires",
//! a value above 30 days is an absolute unix timestamp) are applied at send
//! time and cannot be triggered by accident: a zero TTL is a usage error
//! rather than a silent "keep forever", and a long relative TTL is turned
//! into an absolute timestamp only when the command is encoded.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::error::{Error, Result};

/// Relative TTLs above this many seconds are read by the server as
/// absolute unix timestamps.
const MAX_RELATIVE_SECS: u32 = 30 * 24 * 60 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TtlRepr {
    Never,
    Secs(u32),
    At(SystemTime),
    /// A protocol-layer passthrough: sent verbatim, no rules applied.
    Raw(u32),
}

/// How long a stored item lives. See the [module docs](self).
///
/// ```
/// use std::time::Duration;
/// use memcache::exp::Ttl;
///
/// const SESSION: Ttl = Ttl::secs(1800);
/// let from_duration: Ttl = Duration::from_secs(90).into();
/// let forever = Ttl::NEVER;
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ttl(TtlRepr);

impl Ttl {
    /// The item never expires (it can still be evicted).
    pub const NEVER: Ttl = Ttl(TtlRepr::Never);

    /// Expire `n` seconds from the moment the command is sent. `0` is not
    /// "never": it is rejected by the verb that sends it, use
    /// [`NEVER`](Self::NEVER) instead. Values above 30 days are sent as an
    /// absolute timestamp computed at send time.
    pub const fn secs(n: u32) -> Ttl {
        Ttl(TtlRepr::Secs(n))
    }

    /// Expire at an absolute moment.
    pub fn at(t: SystemTime) -> Ttl {
        Ttl(TtlRepr::At(t))
    }

    /// Turn this TTL into a [`Freshness`] that also asks `fetch` to refresh
    /// the value `d` before it expires. Only `fetch` accepts a `Freshness`,
    /// so the modifier cannot leak into plain writes.
    pub fn refresh_ahead(self, d: Duration) -> Freshness {
        Freshness {
            ttl: self,
            refresh_ahead: Some(d),
        }
    }

    /// A protocol-layer passthrough for callers who already hold a wire
    /// value (for example an absolute timestamp). No rules are applied.
    pub(crate) const fn raw(n: u32) -> Ttl {
        Ttl(TtlRepr::Raw(n))
    }

    /// Encode for the wire, applying the protocol rules now.
    pub(crate) fn wire(self) -> Result<u32> {
        match self.0 {
            TtlRepr::Never => Ok(0),
            TtlRepr::Raw(n) => Ok(n),
            TtlRepr::Secs(0) => Err(Error::Usage(
                "ttl must be at least one second; use Ttl::NEVER for no expiry",
            )),
            TtlRepr::Secs(n) if n <= MAX_RELATIVE_SECS => Ok(n),
            TtlRepr::Secs(n) => unix_secs(SystemTime::now() + Duration::from_secs(u64::from(n))),
            TtlRepr::At(t) => {
                let secs = unix_secs(t)?;
                if secs <= MAX_RELATIVE_SECS {
                    // The server would read it as a relative duration.
                    return Err(Error::Usage(
                        "absolute ttl must be later than 30 days after the unix epoch",
                    ));
                }
                Ok(secs)
            }
        }
    }

    /// Seconds left before expiry, measured from now, for validating a
    /// refresh window against it. `None` for [`NEVER`](Self::NEVER).
    // Consumed by the scenario layer's fetch.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn remaining_secs(self) -> Result<Option<u64>> {
        match self.0 {
            TtlRepr::Never => Ok(None),
            TtlRepr::Secs(n) => Ok(Some(u64::from(n))),
            TtlRepr::Raw(0) => Ok(None),
            TtlRepr::Raw(n) if n <= MAX_RELATIVE_SECS => Ok(Some(u64::from(n))),
            TtlRepr::Raw(n) => Ok(Some(u64::from(n).saturating_sub(unix_now()))),
            TtlRepr::At(t) => Ok(Some(u64::from(unix_secs(t)?).saturating_sub(unix_now()))),
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn unix_secs(t: SystemTime) -> Result<u32> {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::Usage("absolute ttl is before the unix epoch"))?
        .as_secs();
    u32::try_from(secs).map_err(|_| Error::Usage("absolute ttl does not fit the protocol's 32-bit timestamp"))
}

/// Whole seconds, rounding a fractional duration up so that a positive
/// duration never collapses to zero.
fn ceil_secs(d: Duration) -> u32 {
    let secs = d.as_secs() + u64::from(d.subsec_nanos() > 0);
    u32::try_from(secs).unwrap_or(u32::MAX)
}

impl From<Duration> for Ttl {
    /// Rounds up to whole seconds. `Duration::ZERO` is not "never": like
    /// [`Ttl::secs(0)`](Ttl::secs) it is rejected when sent.
    fn from(d: Duration) -> Ttl {
        Ttl::secs(ceil_secs(d))
    }
}

/// The freshness policy of a `fetch`: a [`Ttl`] plus, optionally, how long
/// before expiry one reader is elected to recompute the value
/// ([`Ttl::refresh_ahead`]).
///
/// Both `Ttl` and `Duration` convert into it, so `fetch(key, Ttl::secs(300),
/// ..)` and `fetch(key, Duration::from_secs(300), ..)` work; the refresh
/// window is validated when `fetch` runs, not here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Freshness {
    ttl: Ttl,
    refresh_ahead: Option<Duration>,
}

// Consumed by the scenario layer's fetch.
#[cfg_attr(not(test), allow(dead_code))]
impl Freshness {
    pub(crate) fn ttl(self) -> Ttl {
        self.ttl
    }

    /// Validate and resolve the refresh window to whole seconds for the
    /// protocol's `R` token. `None` when no window was requested.
    pub(crate) fn refresh_before_secs(self) -> Result<Option<u32>> {
        let Some(window) = self.refresh_ahead else {
            return Ok(None);
        };
        if window < Duration::from_secs(1) {
            return Err(Error::Usage("refresh_ahead must be at least one second"));
        }
        let Some(remaining) = self.ttl.remaining_secs()? else {
            return Err(Error::Usage(
                "refresh_ahead needs an expiring ttl, Ttl::NEVER never enters the window",
            ));
        };
        let secs = ceil_secs(window);
        if u64::from(secs) >= remaining {
            return Err(Error::Usage("refresh_ahead must be shorter than the ttl"));
        }
        Ok(Some(secs))
    }
}

impl From<Ttl> for Freshness {
    fn from(ttl: Ttl) -> Freshness {
        Freshness {
            ttl,
            refresh_ahead: None,
        }
    }
}

impl From<Duration> for Freshness {
    fn from(d: Duration) -> Freshness {
        Ttl::from(d).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_rules() {
        assert_eq!(Ttl::NEVER.wire().unwrap(), 0);
        assert_eq!(Ttl::secs(300).wire().unwrap(), 300);
        assert_eq!(Ttl::secs(MAX_RELATIVE_SECS).wire().unwrap(), MAX_RELATIVE_SECS);
        assert_eq!(Ttl::raw(0).wire().unwrap(), 0);
        assert_eq!(Ttl::raw(5_000_000).wire().unwrap(), 5_000_000);
        assert!(matches!(Ttl::secs(0).wire(), Err(Error::Usage(_))));
        assert!(matches!(Ttl::from(Duration::ZERO).wire(), Err(Error::Usage(_))));
    }

    #[test]
    fn long_relative_ttl_becomes_absolute() {
        let now = unix_now();
        let wire = u64::from(Ttl::secs(MAX_RELATIVE_SECS + 1).wire().unwrap());
        assert!(wire > now + u64::from(MAX_RELATIVE_SECS));
        assert!(wire <= now + u64::from(MAX_RELATIVE_SECS) + 2);
    }

    #[test]
    fn absolute_ttl() {
        let t = SystemTime::now() + Duration::from_secs(60);
        let expected = t.duration_since(UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(u64::from(Ttl::at(t).wire().unwrap()), expected);
        assert!(matches!(
            Ttl::at(UNIX_EPOCH + Duration::from_secs(100)).wire(),
            Err(Error::Usage(_))
        ));
        assert!(matches!(
            Ttl::at(UNIX_EPOCH - Duration::from_secs(1)).wire(),
            Err(Error::Usage(_))
        ));
        assert!(matches!(
            Ttl::at(UNIX_EPOCH + Duration::from_secs(u64::from(u32::MAX) + 1)).wire(),
            Err(Error::Usage(_))
        ));
    }

    #[test]
    fn duration_rounds_up() {
        assert_eq!(Ttl::from(Duration::from_millis(1)), Ttl::secs(1));
        assert_eq!(Ttl::from(Duration::from_millis(1500)), Ttl::secs(2));
        assert_eq!(Ttl::from(Duration::from_secs(2)), Ttl::secs(2));
    }

    #[test]
    fn freshness_conversions() {
        let plain: Freshness = Ttl::secs(300).into();
        assert_eq!(plain.ttl(), Ttl::secs(300));
        assert_eq!(plain.refresh_before_secs().unwrap(), None);

        let from_duration: Freshness = Duration::from_secs(300).into();
        assert_eq!(from_duration, plain);

        let windowed = Ttl::secs(300).refresh_ahead(Duration::from_millis(30_500));
        assert_eq!(windowed.ttl(), Ttl::secs(300));
        assert_eq!(windowed.refresh_before_secs().unwrap(), Some(31));
    }

    #[test]
    fn refresh_ahead_validation() {
        let usage = |f: Freshness| matches!(f.refresh_before_secs(), Err(Error::Usage(_)));
        assert!(usage(Ttl::secs(300).refresh_ahead(Duration::ZERO)));
        assert!(usage(Ttl::secs(300).refresh_ahead(Duration::from_millis(999))));
        assert!(usage(Ttl::secs(300).refresh_ahead(Duration::from_secs(300))));
        assert!(usage(Ttl::secs(300).refresh_ahead(Duration::from_secs(301))));
        assert!(usage(Ttl::NEVER.refresh_ahead(Duration::from_secs(10))));
        // The window is compared against the time left on an absolute ttl.
        let at = Ttl::at(SystemTime::now() + Duration::from_secs(3600));
        assert_eq!(
            at.refresh_ahead(Duration::from_secs(60)).refresh_before_secs().unwrap(),
            Some(60)
        );
        assert!(usage(at.refresh_ahead(Duration::from_secs(3600))));
    }
}
