//! Provider-neutral observations of usable resources across FPL systems.
//!
//! Discovery proves availability, never authority. These contracts contain no
//! Mycelium transport, gossip, routing, or authorization types so local probes,
//! static configuration, and other providers can emit the same representation.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResourceId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub provider: String,
    pub observer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Strong,
    Derived,
    Weak,
    Ambiguous,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation<T> {
    pub schema_version: u16,
    pub resource_id: ResourceId,
    pub observed_at: u64,
    pub expires_at: u64,
    pub provenance: Provenance,
    pub confidence: Confidence,
    pub value: T,
}

impl<T> Observation<T> {
    pub fn is_fresh_at(&self, now: u64) -> bool {
        self.observed_at <= now && now < self.expires_at
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    pub label: String,
    #[serde(flatten)]
    pub profile: ResourceProfile,
}

impl Resource {
    pub fn kind(&self) -> ResourceKind {
        match self.profile {
            ResourceProfile::Gpu(_) => ResourceKind::Gpu,
            ResourceProfile::Storage(_) => ResourceKind::Storage,
            ResourceProfile::Camera(_) => ResourceKind::Camera,
            ResourceProfile::Audio(_) => ResourceKind::Audio,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Gpu,
    Storage,
    Camera,
    Audio,
}

impl ResourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gpu => "gpu",
            Self::Storage => "storage",
            Self::Camera => "camera",
            Self::Audio => "audio",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "profile", rename_all = "snake_case")]
pub enum ResourceProfile {
    Gpu(GpuProfile),
    Storage(StorageProfile),
    Camera(CameraProfile),
    Audio(AudioProfile),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuProfile {
    pub vendor: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub backends: BTreeSet<GpuBackend>,
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
    pub memory_total_bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuBackend {
    Metal,
    Cuda,
    Rocm,
    Vulkan,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageProfile {
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub capacity_bytes: Option<u64>,
    pub removable: Option<bool>,
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraProfile {
    pub vendor: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioProfile {
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub capture_channels: Option<u16>,
    pub playback_channels: Option<u16>,
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub resource_id: ResourceId,
    pub host: HostRef,
    pub transport: AttachmentTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ResourceId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRef {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttachmentTransport {
    Usb {
        locator: String,
        speed_mbps: Option<u64>,
    },
    Pcie {
        address: String,
        current_speed: Option<String>,
        current_width: Option<String>,
    },
    Nvme {
        locator: String,
    },
    Scsi {
        locator: String,
    },
    Virtio {
        locator: String,
    },
    Integrated {
        locator: String,
    },
    Network {
        endpoints: BTreeSet<String>,
    },
    Unknown {
        locator: String,
        #[serde(default)]
        attributes: BTreeMap<String, String>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceCatalog {
    pub schema_version: u16,
    pub observations: Vec<Observation<Resource>>,
    pub attachments: Vec<Observation<Attachment>>,
}

/// A provider-neutral description of a reachable service. Discovery proves
/// availability only; consumers retain ownership of authorization and
/// protocol negotiation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Service {
    /// Open vocabulary such as `mcp`, `dcp`, `unibus`, `isochrone`, or `adb`.
    pub kind: String,
    /// Names the service answers to. An owner lease carries the labels it asks
    /// for; a published catalog carries the zone-composed names, because the
    /// zone is what turns a label into a name.
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub endpoints: BTreeSet<String>,
    #[serde(default)]
    pub protocols: BTreeSet<String>,
    #[serde(default)]
    pub formats: BTreeSet<String>,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}

/// Attribute keys that describe what a service will accept, as opposed to what
/// it is.
///
/// The bag is open so a driver can publish anything it observes; these three
/// are the ones a consumer must agree on, so they live here rather than being
/// spelled out at each reader. An absent key is unknown, never a default: a
/// consumer that guesses capacity turns a refusal into a timeout.
pub mod attributes {
    /// Comma-separated model ids the service serves, e.g. `ornith-9b`.
    pub const MODELS: &str = "models";
    /// Concurrent requests the service admits. `0` is a drain: the service
    /// refuses everything while staying reachable.
    pub const MAX_IN_FLIGHT: &str = "max_in_flight";
    /// Rate the service meters its tokens to, in tokens per second.
    pub const TOKENS_PER_SEC: &str = "tokens_per_sec";
}

impl Service {
    /// Model ids this service serves, in published order. Empty when it does
    /// not say; a caller matching on this must not read empty as "any model".
    pub fn models(&self) -> Vec<&str> {
        self.attributes
            .get(attributes::MODELS)
            .map(|models| {
                models
                    .split(',')
                    .map(str::trim)
                    .filter(|model| !model.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Concurrent requests this service admits. `Some(0)` is a drain; `None`
    /// is unknown, which is not the same as unlimited and must not be read as
    /// one.
    pub fn max_in_flight(&self) -> Option<u32> {
        self.attributes
            .get(attributes::MAX_IN_FLIGHT)
            .and_then(|value| value.trim().parse().ok())
    }

    /// The token rate this service meters to, when it meters one.
    pub fn tokens_per_sec(&self) -> Option<f64> {
        self.attributes
            .get(attributes::TOKENS_PER_SEC)
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|rate| rate.is_finite() && *rate > 0.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ServiceId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceObservation {
    pub schema_version: u16,
    pub service_id: ServiceId,
    pub observed_at: u64,
    pub expires_at: u64,
    pub provenance: Provenance,
    pub confidence: Confidence,
    pub value: Service,
}

impl ServiceObservation {
    pub fn is_fresh_at(&self, now: u64) -> bool {
        self.observed_at <= now && now < self.expires_at
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceCatalog {
    pub schema_version: u16,
    pub observations: Vec<ServiceObservation>,
}

impl ServiceCatalog {
    pub fn normalize(&mut self) {
        self.observations
            .sort_by(|left, right| left.service_id.cmp(&right.service_id));
    }
}

impl ResourceCatalog {
    pub fn normalize(&mut self) {
        self.observations
            .sort_by(|left, right| left.resource_id.cmp(&right.resource_id));
        self.attachments
            .sort_by(|left, right| left.resource_id.cmp(&right.resource_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_is_explicit_and_half_open() {
        let observation = Observation {
            schema_version: SCHEMA_VERSION,
            resource_id: ResourceId("gpu/test".into()),
            observed_at: 10,
            expires_at: 20,
            provenance: Provenance {
                provider: "static".into(),
                observer: "fixture".into(),
                source_id: None,
            },
            confidence: Confidence::Strong,
            value: Resource {
                label: "test".into(),
                profile: ResourceProfile::Gpu(GpuProfile::default()),
            },
        };
        assert!(!observation.is_fresh_at(9));
        assert!(observation.is_fresh_at(10));
        assert!(!observation.is_fresh_at(20));
    }

    #[test]
    fn resource_kind_is_derived_from_its_typed_profile() {
        let resource = Resource {
            label: "disk".into(),
            profile: ResourceProfile::Storage(StorageProfile::default()),
        };
        assert_eq!(resource.kind(), ResourceKind::Storage);
    }

    fn ai(attributes: &[(&str, &str)]) -> Service {
        Service {
            kind: "ai".into(),
            names: Vec::new(),
            endpoints: BTreeSet::new(),
            protocols: BTreeSet::new(),
            formats: BTreeSet::new(),
            attributes: attributes
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        }
    }

    #[test]
    fn models_split_on_commas_and_ignore_empty_entries() {
        let service = ai(&[(attributes::MODELS, " ornith-9b , ornith-122b,")]);
        assert_eq!(service.models(), vec!["ornith-9b", "ornith-122b"]);
    }

    #[test]
    fn a_service_that_says_nothing_reports_no_models_and_no_limits() {
        let service = ai(&[]);
        assert!(service.models().is_empty());
        assert_eq!(service.max_in_flight(), None);
        assert_eq!(service.tokens_per_sec(), None);
    }

    #[test]
    fn a_zero_in_flight_limit_is_a_drain_not_an_unknown() {
        let service = ai(&[(attributes::MAX_IN_FLIGHT, "0")]);
        assert_eq!(service.max_in_flight(), Some(0));
    }

    #[test]
    fn an_unreadable_limit_is_unknown_rather_than_a_guess() {
        let service = ai(&[
            (attributes::MAX_IN_FLIGHT, "two"),
            (attributes::TOKENS_PER_SEC, "-1"),
        ]);
        assert_eq!(service.max_in_flight(), None);
        assert_eq!(service.tokens_per_sec(), None);
    }

    #[test]
    fn fractional_token_rates_survive_the_round_trip() {
        let service = ai(&[(attributes::TOKENS_PER_SEC, "2.5")]);
        assert_eq!(service.tokens_per_sec(), Some(2.5));
    }
}
