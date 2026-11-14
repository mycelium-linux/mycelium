use mycelium_core::{CredentialRef, CredentialSet, MyceliumError, Secret};

/// Purpose the daemon leases a geode credential under.
///
/// An operator enrolls the entry with this purpose
/// (`geoded --put geode://<realm>/<account>/<slot> --purpose mycelium.login`).
/// The store denies every other purpose, so holding a reference is not by itself
/// a capability to read the secret.
pub const GEODE_PURPOSE: &str = "mycelium.login";

/// Resolve a non-secret reference into the legacy driver credential shape.
/// This function lives in myceliumd so CLI clients and persisted plans never
/// receive resolved secret material.
pub fn resolve(
    reference: &CredentialRef,
    username: Option<String>,
) -> Result<CredentialSet, MyceliumError> {
    CredentialRef::parse(reference.as_str()).map_err(MyceliumError::Validation)?;
    let mut credentials = CredentialSet {
        username,
        ..CredentialSet::default()
    };
    match reference.scheme() {
        "env" => credentials.password = Some(Secret::Env(reference.location().into())),
        "ssh-key" => credentials.key_path = Some(reference.location().into()),
        "geode" => {
            // Resolution stays lazy: the daemon carries the reference and the
            // driver leases it from the owner-local broker at use time. No
            // backend is selected implicitly, and there is no plaintext or env
            // fallback.
            credentials.password = Some(Secret::Geode {
                reference: reference.clone(),
                purpose: GEODE_PURPOSE.into(),
            });
        }
        provider => {
            return Err(MyceliumError::Validation(format!(
                "unsupported credential provider `{provider}`"
            )))
        }
    }
    Ok(credentials)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_references_without_reading_secret_values() {
        let reference = CredentialRef::parse("env://ROUTER_PASSWORD").unwrap();
        let credentials = resolve(&reference, Some("operator".into())).unwrap();
        assert_eq!(credentials.username(), Some("operator"));
        assert_eq!(
            credentials.password,
            Some(Secret::Env("ROUTER_PASSWORD".into()))
        );
    }

    #[test]
    fn geode_references_stay_lazy_and_name_their_purpose() {
        let reference = CredentialRef::parse("geode://mail/work/imap").unwrap();
        let credentials = resolve(&reference, None).unwrap();
        match &credentials.password {
            Some(Secret::Geode {
                reference: carried,
                purpose,
            }) => {
                assert_eq!(carried, &reference);
                assert_eq!(purpose, GEODE_PURPOSE);
            }
            other => panic!("expected a geode secret, got {other:?}"),
        }
        // The reference is carried, not resolved: no broker was contacted.
        assert!(credentials.username().is_none());
    }
}
