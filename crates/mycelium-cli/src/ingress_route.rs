//! Ingress routing: which node should serve a public hostname.
//!
//! Public traffic reaches the fleet through a Cloudflare anycast edge, but the
//! edge only knows which *connector* is nearby. The connector still has to pick
//! an origin, and the right origin is the nearest one that advertised the
//! service. That answer already exists in the service catalogue; this module
//! turns it into a choice instead of leaving every ingress to guess.
//!
//! Two rules decide the order:
//!
//! - **Same site first.** A connector in `fpl` should serve a service in `fpl`
//!   rather than cross a WAN link, even when the remote node is fresher.
//! - **A transport the fleet already trusts next.** An `iroh://` endpoint or an
//!   ALPN the mesh knows beats an address that needs a route nobody verified.
//!   This is the seam where "nearest via iroh" becomes true: the choice names a
//!   node and an ALPN, not just an HTTP URL.
//!
//! Nothing here authorizes anything. A chosen candidate is a *reachable* node,
//! not a granted one; the serving host still checks the caller's lease.

use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Candidate {
    /// The name that matched, e.g. `scribe.myc` or a node-qualified name.
    pub name: String,
    pub node_id: Option<String>,
    pub site: Option<String>,
    pub endpoints: Vec<String>,
    /// ALPNs the service says it serves, in the same words the listener uses.
    pub alpns: Vec<String>,
    pub age_secs: u64,
    /// Higher is better. Filled by [`rank`].
    pub score: i64,
}

/// Every advertised observation that answers `host`.
///
/// A hostname is matched by its first label against a service name's first
/// label, so `elmers.fpl.dev` matches `elmers.myc`; an exact match also counts.
/// Matching is deliberately not a wildcard: a wrong route is worse than a 404.
pub fn candidates(catalog: &serde_json::Value, host: &str, now: u64) -> Vec<Candidate> {
    let wanted = host.split('.').next().unwrap_or(host);
    let mut found = Vec::new();
    let Some(observations) = catalog.get("observations").and_then(|v| v.as_array()) else {
        return found;
    };
    for observation in observations {
        let Some(value) = observation.get("value") else {
            continue;
        };
        let names: Vec<&str> = value
            .get("names")
            .and_then(|v| v.as_array())
            .map(|names| names.iter().filter_map(|n| n.as_str()).collect())
            .unwrap_or_default();
        let Some(name) = names
            .iter()
            .find(|name| **name == host || name.split('.').next() == Some(wanted))
        else {
            continue;
        };
        let attributes = value.get("attributes").cloned().unwrap_or_default();
        let alpns = attributes
            .get("alpns")
            .and_then(|v| v.as_str())
            .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
            .or_else(|| {
                value.get("alpns").and_then(|v| v.as_array()).map(|a| {
                    a.iter()
                        .filter_map(|n| n.as_str().map(str::to_owned))
                        .collect()
                })
            })
            .unwrap_or_default();
        let observed_at = observation
            .get("observed_at")
            .and_then(|v| v.as_u64())
            .unwrap_or(now);
        found.push(Candidate {
            name: (*name).to_owned(),
            node_id: attributes
                .get("node_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            site: attributes
                .get("site")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            endpoints: value
                .get("endpoints")
                .and_then(|v| v.as_array())
                .map(|e| {
                    e.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            alpns,
            age_secs: now.saturating_sub(observed_at),
            score: 0,
        });
    }
    found
}

/// Rank in place and return the best candidate.
///
/// Same site wins, then a mesh transport, then freshness. A tie is broken by
/// name so the answer is deterministic: two connectors asking the same question
/// must not disagree.
pub fn rank(candidates: &mut [Candidate], local_site: Option<&str>) {
    let local_site = local_site.map(str::to_owned);
    for candidate in candidates.iter_mut() {
        let same_site = match (&candidate.site, &local_site) {
            (Some(site), Some(local)) => site == local,
            _ => false,
        };
        // An iroh endpoint is the strongest signal: it is reachable without a
        // guessed address, and the mesh owns the path. A Soil ALPN is weaker —
        // the contract is mesh-aware, the address may not be — so it only
        // breaks ties between otherwise equal candidates.
        let iroh = candidate.endpoints.iter().any(|e| e.starts_with("iroh://"));
        let soil = candidate.alpns.iter().any(|a| a.starts_with("soil/"));
        candidate.score = i64::from(same_site) * 100 + i64::from(iroh) * 10 + i64::from(soil) * 5
            - candidate.age_secs.min(60) as i64;
    }
}

pub fn best(mut candidates: Vec<Candidate>, local_site: Option<&str>) -> Option<Candidate> {
    rank(&mut candidates, local_site);
    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.node_id.cmp(&b.node_id))
    });
    candidates.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(
        name: &str,
        node: &str,
        site: &str,
        endpoint: &str,
        alpn: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "observed_at": 1_000,
            "value": {
                "kind": name,
                "names": [format!("{name}.myc")],
                "endpoints": [endpoint],
                "attributes": {
                    "node_id": node,
                    "site": site,
                    "alpns": serde_json::to_string(&[alpn]).unwrap(),
                }
            }
        })
    }

    fn catalog(items: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({"observations": items})
    }

    #[test]
    fn matches_a_hostname_by_its_first_label() {
        let catalog = catalog(vec![observation(
            "scribe",
            "node-batty",
            "fpl",
            "http://192.168.1.40:8090",
            "soil/http/1",
        )]);
        let found = candidates(&catalog, "scribe.fpl.dev", 1_000);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "scribe.myc");
    }

    #[test]
    fn a_missing_service_is_not_a_wildcard_the_other_way() {
        let catalog = catalog(vec![observation(
            "scribe",
            "node-batty",
            "fpl",
            "http://192.168.1.40:8090",
            "soil/http/1",
        )]);
        assert!(candidates(&catalog, "other.fpl.dev", 1_000).is_empty());
        // `scribe-extra` must not match `scribe`.
        assert!(candidates(&catalog, "scribe-extra.fpl.dev", 1_000).is_empty());
    }

    #[test]
    fn same_site_beats_a_fresher_remote_node() {
        let catalog = catalog(vec![
            observation(
                "scribe",
                "remote",
                "wagar-house",
                "iroh://abc",
                "soil/streams/1",
            ),
            observation(
                "scribe",
                "local",
                "fpl",
                "http://192.168.1.40:8090",
                "soil/http/1",
            ),
        ]);
        let chosen =
            best(candidates(&catalog, "scribe.fpl.dev", 1_000), Some("fpl")).expect("a route");
        assert_eq!(
            chosen.node_id.as_deref(),
            Some("local"),
            "same-site origin wins"
        );
    }

    #[test]
    fn a_mesh_transport_beats_a_bare_http_endpoint() {
        let catalog = catalog(vec![
            observation(
                "scribe",
                "http-only",
                "fpl",
                "http://192.168.1.40:8090",
                "soil/http/1",
            ),
            observation("scribe", "meshed", "fpl", "iroh://abc", "soil/streams/1"),
        ]);
        let chosen =
            best(candidates(&catalog, "scribe.fpl.dev", 1_000), Some("fpl")).expect("a route");
        assert_eq!(chosen.node_id.as_deref(), Some("meshed"));
    }

    #[test]
    fn ties_are_deterministic() {
        let make = || {
            catalog(vec![
                observation(
                    "scribe",
                    "b-node",
                    "fpl",
                    "http://192.168.1.40:8090",
                    "soil/http/1",
                ),
                observation(
                    "scribe",
                    "a-node",
                    "fpl",
                    "http://192.168.1.40:8090",
                    "soil/http/1",
                ),
            ])
        };
        let first = best(candidates(&make(), "scribe.fpl.dev", 1_000), Some("fpl")).unwrap();
        let second = best(candidates(&make(), "scribe.fpl.dev", 1_000), Some("fpl")).unwrap();
        assert_eq!(first.node_id, second.node_id);
    }
}
