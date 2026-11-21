# Authority, ownership, and sharing

Status: **design.** This note defines the model. It does not claim that tags,
Desktop membership, or resource shares are implemented yet.

The current implementation has one offline-verifiable authority graph with
`access.publish`, `release.publish`, and `package.promote` capabilities. See
[authority.md](authority.md) for the runbook that exists today. This note is the
larger model those capabilities fit into.

## The three questions

Sharing gets messy when one system answers all three questions at once:

| Question | Answer | Must not be inferred from |
|---|---|---|
| **Who is this?** | Device key, OIDC subject, peer identity | a hostname, a network, a shared secret |
| **What may they do?** | Signed authority credentials | identity alone, discovery, enrollment |
| **Who owns this?** | The resource owner's signed grant | the authority root, the transport, the store |

Mycelium already keeps identity and authority separate for SSH and package
publication. The same separation has to hold for tags, Desktops, VFS books, and
Spore packages.

## Key hierarchy

```
org root (offline, threshold)
  ├─ fleet key        capability: fleet.issue
  ├─ store key        capability: store.publish
  ├─ access key       capability: access.publish
  └─ desktop key      capability: desktop.issue
        │
        ├─ device tags        signed by fleet/desktop key
        ├─ store checkpoints  signed by store key
        ├─ SSH/OIDC grants    signed by access key
        └─ Desktop grants     signed by desktop key

publisher key (delegated by store key) -> .spore entry signatures
device key (peer identity)             -> transport identity, not authority
owner principal                        -> resource grants, local consent
```

The **org root** is not a server. It is a public key pinned in host
configuration, with the private key held offline and split across an operator
threshold. Hosts verify signed records without calling an FPL service, an IdP,
or another peer.

The delegated keys are online and replaceable. Compromising the fleet key does
not let an attacker publish packages or read user data; it only lets them issue
fleet-scoped credentials until revocation converges. The org root remains
offline.

### Why not one key

One key for everything makes every online service a potential root compromise.
It also makes rotation a fleet-wide outage. Scoped delegation means a leaked
store key is fixed by rotating the store key, without reissuing SSH access or
device tags.

### Why not a central FPL authority service

A central authority in the request path becomes a single point of failure and a
central control plane. It also makes sharing depend on a WAN link. The model
here is:

- authority records are signed;
- hosts pin public roots;
- revocation records gossiped like other Mycelium observations;
- decisions are reproducible offline at a supplied timestamp.

An IdP still answers "who is this person?" for OIDC login. It does not decide
what the person may do with a Spore, a Desktop, or an FPL resource.

## Credentials

Every credential is a signed record with an issuer, subject, scope, validity and
a revocation path. The exact envelope can reuse the existing
`AuthorityStatement`/`AuthorityRecord` shape or a sibling record with the same
signature domain. The point is that hosts evaluate records, not live API calls.

| Credential | Issuer | Subject | Grants | Notes |
|---|---|---|---|---|
| Org membership | org root or delegated membership key | principal | org roles, `membership:fpl` | Human or service principal. Expiry bounded. |
| Device tag | fleet key | device node id | `tag:fpl.workload`, `tag:fpl.critical`, `tag:personal` | A tag is a claim, never a hostname. Revocable per device. |
| Device owner | owner principal or org | device | owner consent, local Desktop enablement | A device can be owned by a person or by an org. |
| Desktop grant | desktop key or Desktop owner | principal/team | `viewer`, `commenter`, `editor`, `operator` | Separate from workload placement. |
| Resource share | resource owner | principal/team | `viewer`, `commenter`, `editor` | Covers a VFS book, records book, Spore, or site. |
| Store delegation | store key | publisher key | `package.publish` in scope | Existing store delegation/revocation. |
| Package signature | publisher key | `.spore` entry/package | package provenance | Bound to the package digest and CID. |

### Tags are credentials

`MYCELIUM_NODE_FACTS` is local environment input. It is useful as a placement
hint and must never satisfy an ACL. An ACL-bearing tag such as `tag:fpl` is a
credential signed by the fleet authority and checked against the pinned roots.

This distinction matters more than the tag names. A self-asserted `tag.fpl` is
just a host saying it is important; a signed `tag:fpl.workload` is the org
saying this device may host org workloads.

### Access is not workload

A Desktop can be enabled on a device in two independent ways:

```text
access     the principal can open the Desktop, read its files, use its UI
workload   this device may run the Desktop's service/system packages
```

A personal laptop can hold `membership:fpl` and enable the FPL Desktop for
access, while never satisfying `tag:fpl.workload`. The laptop can open FPL
files and UIs; the placement reconciler refuses FPL service packages there.

The names can be represented as tags, modes, or both, but the two decisions must
not be collapsed. "Can see FPL" is not "is an FPL server."

## Ownership

A device has an **owner principal**. The owner is not necessarily the org that
issued a tag.

- `owner:person:avery` — a personal laptop or phone. The owner consents to
  Desktops enabled locally.
- `owner:org:fpl` — an FPL-controlled server. The org owns the hardware and can
  require workload tags.
- A shared device can have an owner and multiple authorized principals, but the
  owner remains the one who consents to local workload placement.

An org tag does not override owner consent on personal hardware. Conversely, a
personal owner cannot grant themselves an org workload tag. Both sides are
needed for critical placement:

```text
tag:fpl.workload   (org says the device is eligible)
owner:org:fpl      (or explicit owner consent for a personal device)
power.mains
link.wired
class.server
```

### Placement facts

Placement consumes derived facts, never raw secrets:

```text
class.desktop | class.laptop | class.server | class.phone
owner.person:<id> | owner.org:<id>
tag:fpl.access | tag:fpl.workload | tag:fpl.critical
platform.darwin | platform.linux
power.mains | power.battery
link.wired | link.wireless
durability.always-on
```

A Desktop's package set carries selectors over these facts:

```json
{
  "spore": "dev.fpl.ops-dashboard",
  "mode": "workload",
  "placement": {
    "all": ["tag:fpl.workload", "owner:org:fpl"],
    "none": ["power.battery"]
  }
}
```

`mode: access` packages may install on a personal device when the Desktop is
enabled and the platform can run the surface. `mode: workload` packages require
the full placement selector. `mode: system`/critical packages also require the
durability facts.

## Sharing

A share is a signed grant from the resource owner to a principal or team:

```text
resource   vfs:desktop/fpl/projects/operator
            records:desktop/fpl/crm
            spore:dev.fungos.crm
            site:fpl
principal  person:avery | service:ci | team:fpl-eng
role       viewer | commenter | editor | operator
projection default | full | contact-card
not-after  2026-12-01T00:00:00Z
```

Properties:

- The resource owner or an authorized delegate signs it.
- The host verifies against pinned roots and the resource's owner chain.
- Revocation wins over message arrival order.
- Teams are principals with membership credentials; a team is not a second ACL.
- No per-request central check. A host that has the signed grant and a fresh
  revocation view can answer offline.

Sharing a Desktop is not the same as sharing every thread in it. A thread share
is narrower than Desktop membership; Desktop membership does not automatically
grant each project or thread. That is the same containment rule the VFS and
records layers already need.

## Store and package authority

The store is a Spore. It reads a signed index and presents a catalog. It is not
the authority root.

- A `.spore` package is a tar with a `spore.lock` digest.
- The store entry names the package by `ipfs://CID` and by the lock digest.
- The publisher signs the entry/package; the store key delegates publisher keys
  and signs the checkpoint.
- VFS fetches the CID and verifies the bytes against the digest.
- Mycelium placement decides whether this device should install it and which
  mode applies.

The package identity is the digest. The CID is transport. The publisher
signature is provenance. None of those are a placement decision.

## Rotation, revocation, recovery

- **Org root:** offline, threshold (for example 3-of-5). Rotating it is a
  deliberate ceremony; hosts accept an overlap window with old and new roots.
- **Delegated keys:** rotate on a schedule and on suspicion. A new delegation
  supersedes the old; hosts use the newest valid record. A revoked delegation is
  dead even if a later record arrives first.
- **Device tags:** short-lived relative to the device lifetime; renewed by the
  fleet key. A lost device is revoked by node id, not by hostname.
- **Resource shares:** default to an expiry. Permanent shares are explicit.
- **Break-glass:** an offline root share can issue a short-lived emergency
  delegation. It is auditable and expires; it is not a permanent bypass.
- **Recovery:** losing an online key is routine. Losing the org root threshold is
  a disaster; the shares and recovery procedure are part of the authority
  design, not an afterthought.

## Migration from the current graph

Today's resolver is one delegation deep and authorizes `access.publish`,
`release.publish`, and `package.promote`. The larger model needs two verification
steps that are still one delegation deep:

```text
root -> delegated key          (delegation record)
delegated key -> credential    (tag, Desktop grant, store entry)
```

The resolver must verify the delegation, then verify the credential signed by
that delegated subject. It must not accept a credential merely because the
issuer is enrolled or reachable.

Existing `MYCELIUM_ACCESS_KEYS` and `MYCELIUM_RELEASE_KEYS` remain migration
roots. New roots are added as public keys; private keys never enter gossip or a
container image.

## Open decisions

1. **Tag namespace.** `tag:fpl` (org) versus `tag:fpl.workload` versus a
   structured `placement` record. Recommendation: keep org membership and
   workload eligibility separate names, not one overloaded tag.
2. **Team representation.** A team as a principal with membership credentials is
   simpler than a separate group database. Recommendation: principal, not a new
   object type.
3. **Owner consent format.** A local owner signature or a device-local config
   record. Recommendation: signed owner grant, with local config only as a cache.
4. **Root overlap.** How long old and new org roots are both accepted during
   rotation. Recommendation: one explicit overlap window, then old root is
   rejected.
5. **Critical placement.** Whether `tag:fpl.critical` is a separate tag or a
   selector over `tag:fpl.workload` plus durability facts. Recommendation:
   selector, so there is one workload grant and explicit durability facts.
