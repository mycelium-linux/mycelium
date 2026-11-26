//! Install, repair and remove the privileged half of `iroh-tun`.
//!
//! macOS cannot create a `utun` from a normal user, so the tunnel is a
//! root-owned service: the installer writes a small wrapper, a LaunchDaemon
//! (macOS) or systemd unit (Linux), and root-owned copies of the two binaries
//! it runs. Those copies are why a user can upgrade `mycelium` without leaving
//! root executing a binary from their home.
//!
//! Everything here is idempotent and reversible: `install` rewrites the same
//! files, `repair` reuses them, and `uninstall` removes exactly what this
//! module wrote. The wrapper and unit are rendered by pure functions so a
//! dry-run is the same bytes the real install would write.

use std::path::{Path, PathBuf};

pub const ROOT_LIBEXEC: &str = "/usr/local/libexec/mycelium";
pub const PLIST: &str = "/Library/LaunchDaemons/dev.fpl.iroh-tun.plist";
pub const UNIT: &str = "/etc/systemd/system/mycelium-iroh-tun.service";
pub const LABEL: &str = "dev.fpl.iroh-tun";

#[derive(Clone, Debug)]
pub struct Plan {
    pub libexec: PathBuf,
    pub status: PathBuf,
    pub tun: String,
    pub overlay: String,
    pub key: PathBuf,
}

impl Plan {
    pub fn new(user_home: &Path, tun: &str, overlay: &str) -> Self {
        let status = if cfg!(target_os = "macos") {
            PathBuf::from("/var/run/mycelium/iroh-tun-status.json")
        } else {
            PathBuf::from("/run/mycelium/iroh-tun-status.json")
        };
        Self {
            libexec: PathBuf::from(ROOT_LIBEXEC),
            status,
            tun: tun.to_owned(),
            overlay: overlay.to_owned(),
            key: user_home.join(".mycelium/iroh-tun.key"),
        }
    }

    pub fn wrapper(&self) -> PathBuf {
        self.libexec.join("iroh-tun-root")
    }

    pub fn mycelium(&self) -> PathBuf {
        self.libexec.join("mycelium")
    }

    pub fn iroh_tun(&self) -> PathBuf {
        self.libexec.join("iroh-tun")
    }
}

/// The root wrapper: run the tunnel, assign this node's identity address, stay
/// in the foreground so the service manager supervises one process tree.
pub fn wrapper(plan: &Plan) -> String {
    let tun = &plan.tun;
    let overlay = &plan.overlay;
    let key = plan.key.display();
    let status = plan.status.display();
    let mycelium_path = plan.mycelium();
    let iroh_tun_path = plan.iroh_tun();
    let mycelium = mycelium_path.display();
    let iroh_tun = iroh_tun_path.display();
    let address_assign = if cfg!(target_os = "macos") {
        format!(
            "  assigned=0\n  i=0\n  while [ $i -lt 150 ]; do\n    if /sbin/ifconfig {tun} >/dev/null 2>&1 \\\n      && {{ /sbin/ifconfig {tun} inet6 {overlay} >/dev/null 2>&1 \\\n        || /sbin/ifconfig {tun} inet6 {overlay} alias >/dev/null 2>&1; }}; then\n      assigned=1; break\n    fi\n    i=$((i+1)); sleep 0.1\n  done\n"
        )
    } else {
        format!(
            "  ipbin=/usr/sbin/ip; [ -x \"$ipbin\" ] || ipbin=/sbin/ip\n  assigned=0\n  i=0\n  while [ $i -lt 150 ]; do\n    if \"$ipbin\" link show {tun} >/dev/null 2>&1 \\\n      && \"$ipbin\" -6 addr replace {overlay} dev {tun} >/dev/null 2>&1; then\n      assigned=1; break\n    fi\n    i=$((i+1)); sleep 0.1\n  done\n"
        )
    };
    format!(
        "#!/bin/sh\nset -eu\nexport MYCELIUM_NO_AUTOSTART=1\nexport PATH=/usr/bin:/bin:/usr/sbin:/sbin\n'{iroh_tun}' \\\n  --tun '{tun}' --mtu 1280 --refresh-secs 15 \\\n  --secret \"$(cat '{key}')\" \\\n  --status-file '{status}' \\\n  --mycelium '{mycelium}' &\nchild=$!\n{address_assign}\nif [ $assigned -ne 1 ]; then\n  echo \"iroh-tun: could not assign {overlay} to {tun}\" >&2\n  kill $child 2>/dev/null || true\n  exit 1\nfi\nwait $child\n"
    )
}

pub fn plist(plan: &Plan) -> String {
    let wrapper_path = plan.wrapper();
    let wrapper = wrapper_path.display();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{LABEL}</string>\n<key>ProgramArguments</key><array><string>{wrapper}</string></array>\n<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>\n<key>ThrottleInterval</key><integer>10</integer>\n<key>StandardOutPath</key><string>/var/log/mycelium-iroh-tun.log</string>\n<key>StandardErrorPath</key><string>/var/log/mycelium-iroh-tun.log</string>\n</dict></plist>\n"
    )
}

pub fn unit(plan: &Plan) -> String {
    let wrapper_path = plan.wrapper();
    let wrapper = wrapper_path.display();
    format!(
        "[Unit]\nDescription=Mycelium iroh-tun IPv6 overlay\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nExecStart={wrapper}\nRestart=always\nRestartSec=10\nUser=root\nGroup=root\nRuntimeDirectory=mycelium\nRuntimeDirectoryMode=0755\n\n[Install]\nWantedBy=multi-user.target\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Plan {
        Plan::new(Path::new("/Users/founder"), "utun220", "fd7a::1/128")
    }

    #[test]
    fn the_wrapper_runs_the_root_copies_and_the_identity_address() {
        let text = wrapper(&plan());
        assert!(text.contains("/usr/local/libexec/mycelium/iroh-tun"));
        assert!(text.contains("--secret \"$(cat '/Users/founder/.mycelium/iroh-tun.key')\""));
        assert!(text.contains("fd7a::1/128"));
        assert!(text.contains("wait $child"));
        assert!(
            !text.contains("/Users/founder/.local/bin"),
            "root must not run user binaries"
        );
    }

    #[test]
    fn the_plist_is_a_keepalive_daemon() {
        let text = plist(&plan());
        assert!(text.contains("<string>dev.fpl.iroh-tun</string>"));
        assert!(text.contains("<key>KeepAlive</key><true/>"));
        assert!(text.contains("/usr/local/libexec/mycelium/iroh-tun-root"));
    }

    #[test]
    fn the_unit_matches_the_plist_contract() {
        let text = unit(&plan());
        assert!(text.contains("ExecStart=/usr/local/libexec/mycelium/iroh-tun-root"));
        assert!(text.contains("Restart=always"));
    }
}

/// The command surface behind `mycelium iroh-tun install|repair|uninstall`.
pub fn run(action: &str, args: &[String]) -> Result<Vec<String>, String> {
    let mut dry_run = false;
    let mut purge = false;
    let mut tun = String::from("utun220");
    let mut overlay: Option<String> = None;
    let mut from: Option<PathBuf> = None;
    let mut user_home: Option<PathBuf> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--dry-run" => dry_run = true,
            "--purge" => purge = true,
            "--tun" | "--overlay" | "--from" | "--user-home" => {
                let flag = args[index].clone();
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("{flag} needs a value"))?
                    .clone();
                match flag.as_str() {
                    "--tun" => tun = value,
                    "--overlay" => overlay = Some(value),
                    "--from" => from = Some(PathBuf::from(value)),
                    "--user-home" => user_home = Some(PathBuf::from(value)),
                    _ => unreachable!(),
                }
            }
            other => return Err(format!("unknown iroh-tun {action} argument `{other}`")),
        }
        index += 1;
    }
    let home = user_home.unwrap_or_else(default_user_home);
    let overlay = overlay.ok_or_else(|| {
        "iroh-tun install needs --overlay (this node's /128, from `mycelium iroh-tun address`)"
            .to_owned()
    })?;
    let plan = Plan::new(&home, &tun, &overlay);

    match action {
        "install" => install(&plan, from.as_deref(), dry_run),
        "repair" => repair(&plan, from.as_deref(), dry_run),
        "uninstall" => uninstall(&plan, purge, dry_run),
        other => Err(format!("unknown iroh-tun service action `{other}`")),
    }
}

fn default_user_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn install(plan: &Plan, from: Option<&Path>, dry_run: bool) -> Result<Vec<String>, String> {
    let mycelium = from
        .map(|dir| dir.join("mycelium"))
        .filter(|path| path.is_file())
        .or_else(|| std::env::current_exe().ok())
        .ok_or("cannot locate the mycelium binary; pass --from DIR")?;
    let iroh_tun = from
        .map(|dir| dir.join("iroh-tun"))
        .or_else(|| {
            mycelium
                .parent()
                .map(|parent| parent.join("iroh-tun"))
                .filter(|path| path.is_file())
        })
        .filter(|path| path.is_file())
        .ok_or("cannot locate the iroh-tun binary; pass --from DIR with both binaries")?;

    let mut lines = vec![format!(
        "install: {} -> {} (tun {tun}, overlay {overlay})",
        mycelium.display(),
        plan.libexec.display(),
        tun = plan.tun,
        overlay = plan.overlay
    )];
    if dry_run || !is_root() {
        lines.push(format!("--- {}", plan.wrapper().display()));
        lines.push(wrapper(plan));
        lines.push(format!(
            "--- {}",
            if cfg!(target_os = "macos") {
                PLIST
            } else {
                UNIT
            }
        ));
        lines.push(if cfg!(target_os = "macos") {
            plist(plan)
        } else {
            unit(plan)
        });
    }
    if dry_run {
        lines.push("plan only; run the same command with sudo to apply".into());
        return Ok(lines);
    }
    if !is_root() {
        return Err(format!(
            "the tunnel service is root-owned; re-run as: sudo {} iroh-tun install --overlay {} --tun {}{}",
            std::env::current_exe().unwrap_or_default().display(),
            plan.overlay,
            plan.tun,
            from.map(|dir| format!(" --from {}", dir.display())).unwrap_or_default()
        ));
    }

    std::fs::create_dir_all(&plan.libexec)
        .map_err(|error| format!("create {}: {error}", plan.libexec.display()))?;
    if let Some(parent) = plan.status.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    copy_root(&mycelium, &plan.mycelium())?;
    copy_root(&iroh_tun, &plan.iroh_tun())?;
    write_root(&plan.wrapper(), wrapper(plan).as_bytes(), 0o755)?;
    if cfg!(target_os = "macos") {
        write_root(Path::new(PLIST), plist(plan).as_bytes(), 0o644)?;
    } else {
        write_root(Path::new(UNIT), unit(plan).as_bytes(), 0o644)?;
    }

    configure_tunnel(plan, &home_env(plan))?;
    start_service()?;
    lines.push(format!(
        "installed {} and started {LABEL}",
        plan.wrapper().display()
    ));
    lines.push("verify with: mycelium iroh-tun status".into());
    Ok(lines)
}

fn repair(plan: &Plan, from: Option<&Path>, dry_run: bool) -> Result<Vec<String>, String> {
    let unit_present = if cfg!(target_os = "macos") {
        Path::new(PLIST).is_file()
    } else {
        Path::new(UNIT).is_file()
    };
    if !plan.wrapper().is_file() || !unit_present {
        return install(plan, from, dry_run);
    }
    if dry_run || !is_root() {
        return Ok(vec![
            format!(
                "repair: restart {LABEL}{}",
                if dry_run {
                    " (plan only)"
                } else {
                    " (needs sudo)"
                }
            ),
            "run: sudo mycelium iroh-tun repair".into(),
        ]);
    }
    configure_tunnel(plan, &home_env(plan))?;
    start_service()?;
    Ok(vec![format!("repaired and restarted {LABEL}")])
}

fn uninstall(plan: &Plan, purge: bool, dry_run: bool) -> Result<Vec<String>, String> {
    let mut lines = vec![format!(
        "uninstall: stop {LABEL} and remove root-owned files"
    )];
    if dry_run || !is_root() {
        lines.push("plan only; run the same command with sudo to apply".into());
        return Ok(lines);
    }
    if cfg!(target_os = "macos") {
        let _ = command("launchctl", &["bootout", &format!("system/{LABEL}")]);
        let _ = std::fs::remove_file(PLIST);
    } else {
        let _ = command("systemctl", &["disable", "--now", "mycelium-iroh-tun"]);
        let _ = std::fs::remove_file(UNIT);
        let _ = command("systemctl", &["daemon-reload"]);
    }
    for path in [plan.wrapper(), plan.mycelium(), plan.iroh_tun()] {
        let _ = std::fs::remove_file(path);
    }
    let _ = std::fs::remove_file(&plan.status);
    lines.push(format!("removed {}", plan.libexec.display()));
    if purge {
        let _ = command(
            plan.mycelium().to_str().unwrap_or("mycelium"),
            &["iroh-tun", "config", "--enabled", "false", "--write"],
        );
        lines.push("tunnel config marked disabled".into());
    }
    Ok(lines)
}

fn home_env(plan: &Plan) -> PathBuf {
    // The config lives in the user's home even though root writes it.
    plan.key
        .parent()
        .and_then(|home| home.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(default_user_home)
}

fn configure_tunnel(plan: &Plan, user_home: &Path) -> Result<(), String> {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    let owned = std::fs::metadata(user_home)
        .map(|meta| (meta.uid(), meta.gid()))
        .ok();
    let status = command_with_env(
        plan.mycelium().to_str().unwrap_or("mycelium"),
        &[
            "iroh-tun",
            "config",
            "--tun",
            &plan.tun,
            "--overlay",
            &plan.overlay,
            "--mtu",
            "1280",
            "--external",
            "true",
            "--enabled",
            "true",
            "--status",
            plan.status.to_str().unwrap_or_default(),
            "--write",
        ],
        &[(
            "MYCELIUM_HOME",
            format!("{}/.mycelium", user_home.display()),
        )],
    )?;
    let path = user_home.join(".mycelium/iroh-tun.json");
    if let (Some((uid, gid)), Ok(_)) = (owned, std::fs::metadata(user_home)) {
        let _ = command(
            "chown",
            &[&format!("{uid}:{gid}"), path.to_str().unwrap_or_default()],
        );
    }
    if !status {
        return Err("the installed mycelium rejected the external iroh-tun config".into());
    }
    Ok(())
}

fn start_service() -> Result<(), String> {
    if cfg!(target_os = "macos") {
        let _ = command("launchctl", &["bootout", &format!("system/{LABEL}")]);
        if !command("launchctl", &["bootstrap", "system", PLIST]) {
            return Err(format!("launchctl bootstrap system {PLIST} failed"));
        }
        let _ = command(
            "launchctl",
            &["kickstart", "-k", &format!("system/{LABEL}")],
        );
    } else {
        let _ = command("systemctl", &["daemon-reload"]);
        if !command("systemctl", &["enable", "--now", "mycelium-iroh-tun"]) {
            return Err("systemctl enable --now mycelium-iroh-tun failed".into());
        }
    }
    Ok(())
}

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc_geteuid() == 0 }
}

#[cfg(unix)]
fn libc_geteuid() -> u32 {
    extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: geteuid is always safe to call.
    unsafe { geteuid() }
}

#[cfg(not(unix))]
fn libc_geteuid() -> u32 {
    1
}

fn copy_root(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::copy(from, to)
        .map_err(|error| format!("copy {} -> {}: {error}", from.display(), to.display()))?;
    set_mode(to, 0o755)
}

fn write_root(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|error| format!("write {}: {error}", path.display()))?;
    set_mode(path, mode)
}

fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|error| format!("chmod {}: {error}", path.display()))
}

fn command(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn command_with_env(program: &str, args: &[&str], env: &[(&str, String)]) -> Result<bool, String> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.status()
        .map(|status| status.success())
        .map_err(|error| format!("run {program}: {error}"))
}

#[cfg(test)]
mod install_tests {
    use super::*;

    #[test]
    fn a_dry_run_renders_without_touching_the_host() {
        let dir = std::env::temp_dir().join(format!("iroh-tun-service-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["mycelium", "iroh-tun"] {
            std::fs::write(dir.join(name), b"#!/bin/sh\n").unwrap();
        }
        let args = vec![
            "--dry-run".to_owned(),
            "--overlay".to_owned(),
            "fd7a::1/128".to_owned(),
            "--from".to_owned(),
            dir.display().to_string(),
        ];
        let lines = run("install", &args).expect("renders a plan").join("\n");
        assert!(lines.contains("dev.fpl.iroh-tun"), "{lines}");
        assert!(lines.contains("plan only"), "{lines}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn install_refuses_to_run_without_an_overlay() {
        let error = run("install", &["--dry-run".to_owned()]).unwrap_err();
        assert!(error.contains("--overlay"), "{error}");
    }

    #[test]
    fn writing_the_same_files_twice_is_idempotent() {
        let root = std::env::temp_dir().join(format!("iroh-tun-write-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let plan = Plan {
            libexec: root.join("libexec"),
            status: root.join("run/status.json"),
            tun: "utun220".into(),
            overlay: "fd7a::1/128".into(),
            key: root.join("home/.mycelium/iroh-tun.key"),
        };
        std::fs::create_dir_all(&plan.libexec).unwrap();
        write_root(&plan.wrapper(), wrapper(&plan).as_bytes(), 0o755).unwrap();
        let first = std::fs::read(plan.wrapper()).unwrap();
        write_root(&plan.wrapper(), wrapper(&plan).as_bytes(), 0o755).unwrap();
        assert_eq!(std::fs::read(plan.wrapper()).unwrap(), first);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(plan.wrapper())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_real_install_without_root_prints_the_sudo_command() {
        let dir = std::env::temp_dir().join(format!("iroh-tun-refuse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["mycelium", "iroh-tun"] {
            std::fs::write(dir.join(name), b"#!/bin/sh\n").unwrap();
        }
        let args = vec![
            "--overlay".to_owned(),
            "fd7a::1/128".to_owned(),
            "--from".to_owned(),
            dir.display().to_string(),
        ];
        // Tests do not run as root in CI; this asserts the failure is an
        // instruction rather than a partial write.
        if !is_root() {
            let error = run("install", &args).unwrap_err();
            assert!(error.contains("sudo"), "{error}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
