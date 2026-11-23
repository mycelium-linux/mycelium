# Spore install consent v0

Status: draft. Install consent is the OAuth-like boundary between what a Spore
requests and what the operator grants.

## Principle

A Spore manifest may declare requests. It may not grant itself authority.

```text
requests   declared by the Spore
consent    shown to the operator
grants     approved by the operator, recorded with the install
```

## Manifest requests

```soil
(spore
  (id "dev.deadwax")
  (version "0.1.0")

  (requests
    (streams
      (accept "http"))
    (http
      (allow "http://127.0.0.1:6601")
      (allow "https://api.spotify.com"))
    (credentials
      "spotify")))
```

## Consent page

The installer renders a bounded page before install:

```text
Package       dev.deadwax 0.1.0
Publisher     ...
Digest        sha256:...
Source        ...
License       ...

This Spore asks for:

  Accept HTTP connections
    host-owned listener, no raw socket

  Call these origins
    http://127.0.0.1:6601
    https://api.spotify.com

  Use credential handle
    spotify
    secret stays in the host

[ ] Allow these grants
[Install] [Cancel]
```

Rules:

```text
show every requested interface
show every allowlisted origin
show every credential handle
show package digest and publisher
show whether eval is allowed
no hidden grants
no defaults that widen authority
```

## Approved grants

The installer records:

```soil
(grants
  (streams (accept "http"))
  (http
    (allow "http://127.0.0.1:6601")
    (allow "https://api.spotify.com"))
  (credentials "spotify"))
```

The grant set is sealed with the install generation.

## Runtime enforcement

```text
Loam links only approved grants
Loam enforces allowlists and credential handles
Soil never sees tokens or transport authority
Tweaks cannot widen requests or grants
```

## First target

Deadwax:

```text
soil:streams accept http
soil:http allow local deadwax + api.spotify.com
soil:creds handle spotify
```

Radioman Pi follows the same page with radio-specific requests.

## Not in v0

```text
OAuth provider flows
credential minting
per-request prompts
remote store consent
long-lived delegated scopes
```
