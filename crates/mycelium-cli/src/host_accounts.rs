//! Linux account lifecycle for signed host access.
//!
//! Accounts are created once with the authority-signed UID/GID and are only
//! disabled when their grant goes away. Nothing here deletes an account, a
//! home, or a group. The planner is pure so the dangerous decisions can be
//! tested without touching the host; execution is a small set of fixed
//! `groupadd`/`useradd`/`usermod` calls, never shell interpolation.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AccountId {
    pub uid: u32,
    pub gid: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Observed {
    pub id: AccountId,
    /// False when the account has been moved to a non-login shell.
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) enum Action {
    Create { user: String, id: AccountId },
    Enable { user: String },
    Disable { user: String },
}

/// Decide what to do from signed intent, the previous managed set and local
/// observation. A signed UID/GID mismatch is an error, never a silent
/// renumbering: shared VFS ownership must not move under a live account.
pub(crate) fn plan(
    desired: &BTreeMap<String, Option<AccountId>>,
    previous: &BTreeSet<String>,
    observed: &BTreeMap<String, Observed>,
) -> Result<Vec<Action>, String> {
    let desired_users: BTreeSet<String> = desired.keys().cloned().collect();
    let mut actions = Vec::new();
    for (user, wanted) in desired {
        match (observed.get(user), wanted) {
            (None, Some(id)) => actions.push(Action::Create {
                user: user.clone(),
                id: *id,
            }),
            (None, None) => {
                return Err(format!(
                    "grant for `{user}` has no unix_ids; refusing to create a host-local UID/GID"
                ))
            }
            (Some(current), Some(id)) if current.id != *id => {
                return Err(format!(
                    "account `{user}` is {uid}:{gid} but the signed grant requires {want_uid}:{want_gid}; \
                     refusing to renumber a live account (migrate it explicitly first)",
                    uid = current.id.uid,
                    gid = current.id.gid,
                    want_uid = id.uid,
                    want_gid = id.gid,
                ))
            }
            (Some(_), None) if !previous.contains(user) => {
                return Err(format!(
                    "account `{user}` already exists but the grant carries no unix_ids; refusing to adopt it implicitly"
                ))
            }
            (Some(current), _) if !current.enabled => actions.push(Action::Enable {
                user: user.clone(),
            }),
            _ => {}
        }
    }
    for user in previous.difference(&desired_users) {
        if observed.get(user).is_some_and(|current| current.enabled) {
            actions.push(Action::Disable { user: user.clone() });
        }
    }
    Ok(actions)
}

pub(crate) fn plan_for(
    desired: &BTreeMap<String, Option<AccountId>>,
    previous: &BTreeSet<String>,
) -> Result<Vec<Action>, String> {
    let mut observed = BTreeMap::new();
    for user in desired
        .keys()
        .chain(previous.iter())
        .cloned()
        .collect::<BTreeSet<_>>()
    {
        if let Some(current) = observe(&user)? {
            observed.insert(user, current);
        }
    }
    plan(desired, previous, &observed)
}

/// Observe one account without trusting the daemon or any cached intent.
pub(crate) fn observe(user: &str) -> Result<Option<Observed>, String> {
    let uid = match id_field(user, "-u")? {
        Some(value) => value,
        None => return Ok(None),
    };
    let gid = id_field(user, "-g")?
        .ok_or_else(|| format!("account `{user}` has a UID but no discoverable GID"))?;
    let passwd = capture("getent", &["passwd", user])?
        .ok_or_else(|| format!("account `{user}` has a UID but no passwd entry"))?;
    let shell = passwd
        .trim_end()
        .rsplit(':')
        .next()
        .ok_or_else(|| format!("account `{user}` has an unreadable passwd entry"))?;
    Ok(Some(Observed {
        id: AccountId { uid, gid },
        enabled: !matches!(
            shell,
            "/usr/sbin/nologin" | "/sbin/nologin" | "/bin/false" | "/usr/bin/false"
        ),
    }))
}

fn id_field(user: &str, flag: &str) -> Result<Option<u32>, String> {
    let output = run_output("id", &[flag, user])?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let value = text.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<u32>()
        .map(Some)
        .map_err(|_| format!("account `{user}` returned a non-numeric `id {flag}`"))
}

pub(crate) fn apply(actions: &[Action]) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for action in actions {
        match action {
            Action::Create { user, id } => {
                create(user, *id)?;
                lines.push(format!(
                    "created {user} with uid/gid {uid}:{gid}",
                    uid = id.uid,
                    gid = id.gid
                ));
            }
            Action::Enable { user } => {
                command(
                    "usermod",
                    &["--shell", "/bin/bash", "--expiredate", "", user],
                )?;
                lines.push(format!("re-enabled {user}"));
            }
            Action::Disable { user } => {
                command(
                    "usermod",
                    &["--shell", "/usr/sbin/nologin", "--expiredate", "1", user],
                )?;
                lines.push(format!("disabled {user} (account and home retained)"));
            }
        }
    }
    Ok(lines)
}

fn create(user: &str, id: AccountId) -> Result<(), String> {
    let mut created_group = false;
    match group_gid(user)? {
        Some(gid) if gid != id.gid => {
            return Err(format!(
                "group `{user}` is gid {gid} but the signed grant requires gid {0}",
                id.gid
            ))
        }
        Some(_) => {}
        None => {
            command("groupadd", &["--gid", &id.gid.to_string(), user])?;
            created_group = true;
        }
    }
    let result = (|| {
        command(
            "useradd",
            &[
                "--uid",
                &id.uid.to_string(),
                "--gid",
                &id.gid.to_string(),
                "--create-home",
                "--shell",
                "/bin/bash",
                user,
            ],
        )?;
        command("passwd", &["--lock", user])
    })();
    if result.is_err() && created_group {
        // Never delete a pre-existing group. This one was created for the
        // account that failed to materialize, so removing it is safe.
        let _ = Command::new("groupdel").arg(user).status();
    }
    result
}

fn group_gid(group: &str) -> Result<Option<u32>, String> {
    let Some(line) = capture("getent", &["group", group])? else {
        return Ok(None);
    };
    let gid = line
        .trim_end()
        .split(':')
        .nth(2)
        .ok_or_else(|| format!("group `{group}` has an unreadable entry"))?;
    gid.parse::<u32>()
        .map(Some)
        .map_err(|_| format!("group `{group}` has a non-numeric GID"))
}

fn command(program: &str, args: &[&str]) -> Result<(), String> {
    let output = run_output(program, args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn capture(program: &str, args: &[&str]) -> Result<Option<String>, String> {
    let output = run_output(program, args)?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
}

fn run_output(program: &str, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new(program)
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .args(args)
        .output()
        .map_err(|error| format!("run {program}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(uid: u32) -> AccountId {
        AccountId { uid, gid: uid }
    }

    fn observed(uid: u32, enabled: bool) -> Observed {
        Observed {
            id: id(uid),
            enabled,
        }
    }

    #[test]
    fn new_account_requires_the_signed_ids() {
        let desired = BTreeMap::from([("alice".to_string(), None)]);
        let error = plan(&desired, &BTreeSet::new(), &BTreeMap::new()).unwrap_err();
        assert!(error.contains("no unix_ids"));
    }

    #[test]
    fn new_account_uses_the_signed_ids() {
        let desired = BTreeMap::from([("alice".to_string(), Some(id(4001)))]);
        let actions = plan(&desired, &BTreeSet::new(), &BTreeMap::new()).unwrap();
        assert_eq!(
            actions,
            vec![Action::Create {
                user: "alice".into(),
                id: id(4001)
            }]
        );
    }

    #[test]
    fn a_signed_id_mismatch_never_renumbers_a_live_account() {
        let desired = BTreeMap::from([("alice".to_string(), Some(id(4001)))]);
        let observed = BTreeMap::from([("alice".to_string(), observed(1001, true))]);
        let error = plan(&desired, &BTreeSet::new(), &observed).unwrap_err();
        assert!(error.contains("refusing to renumber"));
    }

    #[test]
    fn an_existing_account_without_signed_ids_is_not_adopted() {
        let desired = BTreeMap::from([("alice".to_string(), None)]);
        let observed = BTreeMap::from([("alice".to_string(), observed(1001, true))]);
        let error = plan(&desired, &BTreeSet::new(), &observed).unwrap_err();
        assert!(error.contains("refusing to adopt"));
    }

    #[test]
    fn revoked_accounts_are_disabled_not_deleted() {
        let previous = BTreeSet::from(["alice".to_string()]);
        let observed = BTreeMap::from([("alice".to_string(), observed(1001, true))]);
        let actions = plan(&BTreeMap::new(), &previous, &observed).unwrap();
        assert_eq!(
            actions,
            vec![Action::Disable {
                user: "alice".into()
            }]
        );
        assert!(actions
            .iter()
            .all(|action| !matches!(action, Action::Create { .. })));
    }

    #[test]
    fn a_returning_grant_re_enables_the_retained_account() {
        let desired = BTreeMap::from([("alice".to_string(), Some(id(1001)))]);
        let previous = BTreeSet::from(["alice".to_string()]);
        let observed = BTreeMap::from([("alice".to_string(), observed(1001, false))]);
        let actions = plan(&desired, &previous, &observed).unwrap();
        assert_eq!(
            actions,
            vec![Action::Enable {
                user: "alice".into()
            }]
        );
    }

    #[test]
    fn already_disabled_accounts_do_not_churn() {
        let previous = BTreeSet::from(["alice".to_string()]);
        let observed = BTreeMap::from([("alice".to_string(), observed(1001, false))]);
        assert!(plan(&BTreeMap::new(), &previous, &observed)
            .unwrap()
            .is_empty());
    }
}
