//! Install channels.
//!
//! One machine may carry a stable install and a canary install at the same
//! time. What keeps them apart is not the binary but every name the binary
//! writes into the operating system: service labels, plists, sockets and
//! ports. So the channel is a property of the *process environment*
//! (`MYCELIUM_CHANNEL`), and every label is derived from it here instead of
//! being spelled out at each call site.
//!
//! The suffix is appended, not substituted, so a new label cannot accidentally
//! keep a stable name: `dev.fpl.mycelium-frontdoor` becomes
//! `dev.fpl.mycelium-frontdoor-canary`.

use std::fmt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Channel {
    #[default]
    Stable,
    Canary,
}

impl Channel {
    /// The channel this process was started for. Anything that is not the
    /// canary spelling is stable: an unknown value must not silently create a
    /// second set of services that look like the canary's.
    pub fn current() -> Self {
        match std::env::var("MYCELIUM_CHANNEL") {
            Ok(value) if value.eq_ignore_ascii_case("canary") => Self::Canary,
            _ => Self::Stable,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "stable" => Some(Self::Stable),
            "canary" => Some(Self::Canary),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Canary => "canary",
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            Self::Stable => "",
            Self::Canary => "-canary",
        }
    }

    /// The service label for `base` on this channel.
    pub fn label(self, base: &str) -> String {
        format!("{base}{}", self.suffix())
    }

    /// The same for systemd units, which use a different convention: a unit
    /// name cannot repeat a dot suffix without being unambiguous, so the
    /// channel goes in front of the unit suffix (`mycelium-iroh-tun-canary`).
    pub fn unit(self, base: &str) -> String {
        format!("{base}{}", self.suffix())
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canary_labels_never_collide_with_stable_ones() {
        for base in [
            "dev.fpl.mycelium",
            "dev.fpl.mycelium-frontdoor",
            "dev.fpl.mycelium-ingress-https",
            "dev.fpl.mycelium-dnsd",
            "dev.fpl.iroh-tun",
        ] {
            let stable = Channel::Stable.label(base);
            let canary = Channel::Canary.label(base);
            assert_eq!(stable, base);
            assert_ne!(stable, canary);
            assert!(canary.ends_with("-canary"));
        }
    }

    #[test]
    fn an_unknown_or_absent_channel_is_stable() {
        // The environment is per-process, so this test only asserts the parse
        // helper; `current` is exercised by the installer's shell tests.
        assert_eq!(Channel::parse("stable"), Some(Channel::Stable));
        assert_eq!(Channel::parse("canary"), Some(Channel::Canary));
        assert_eq!(Channel::parse("Canary"), None);
        assert_eq!(Channel::parse(""), None);
        assert_eq!(Channel::default(), Channel::Stable);
        assert_eq!(Channel::Stable.suffix(), "");
        assert_eq!(Channel::Canary.suffix(), "-canary");
    }
}
