//! `mycelium-lease`: publish a service lease from a shell, a unit or a hook.
//!
//! Non-Rust applications should not have to reimplement the catalog format to
//! be visible, and an operator should not have to hand-edit a file that the
//! daemon expires. This is the smallest surface that makes both true.

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

use mycelium_service_lease::{renewal_interval, Lease, Publisher};

const USAGE: &str = "usage:\n  \
  mycelium-lease publish --node-id ID --kind KIND --endpoint URI [--endpoint URI]...\n    \
                       [--attribute KEY=VALUE]... [--ttl SECS] [--every SECS]\n    \
                       [--home DIR] [--path FILE] [--once]\n  \
  mycelium-lease withdraw --node-id ID --kind KIND [--home DIR] [--path FILE]\n";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mycelium-lease: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let Some(command) = args.first() else {
        return Err(USAGE.trim_end().to_owned());
    };
    let options = Options::parse(&args[1..])?;
    let publisher = match &options.path {
        Some(path) => Publisher::at(path, options.node_id.clone()),
        None => Publisher::new(options.home(), options.node_id.clone()),
    };

    match command.as_str() {
        "publish" => {
            let every = renewal_interval(options.every, options.ttl)
                .map_err(|error| error.to_string())?;
            let lease = options.lease()?;
            loop {
                publisher
                    .publish(&lease)
                    .map_err(|error| error.to_string())?;
                println!(
                    "published {} for {}, renewing every {every}s into {}",
                    lease.kind(),
                    lease.ttl_secs(),
                    publisher.path().display()
                );
                if options.once {
                    return Ok(());
                }
                sleep(Duration::from_secs(every));
            }
        }
        "withdraw" => {
            let Some(kind) = &options.kind else {
                return Err("withdraw needs --kind".into());
            };
            let removed = publisher
                .withdraw(kind)
                .map_err(|error| error.to_string())?;
            println!(
                "{} {kind} from {}",
                if removed { "withdrew" } else { "nothing to withdraw for" },
                publisher.path().display()
            );
            Ok(())
        }
        "list" => {
            // What an observer can see right now: every fresh name, and what
            // it can be asked for. This is the list a scraper is generated
            // from, so a service that wants scraping says so in its lease.
            let seen = mycelium_service_lease::observe(publisher.path())
                .map_err(|error| error.to_string())?;
            for one in seen {
                if let Some((key, value)) = &options.tag {
                    let held = one.attributes.get(key);
                    let keep = match value {
                        Some(value) => held == Some(value),
                        None => held.is_some(),
                    };
                    if !keep {
                        continue;
                    }
                }
                println!(
                    "{} {} healthz={} readyz={} capabilities={}",
                    one.name,
                    one.endpoints.iter().cloned().collect::<Vec<_>>().join(","),
                    one.healthz().unwrap_or("-"),
                    one.readyz().unwrap_or("-"),
                    one.attributes
                        .get(mycelium_service_lease::CAPABILITIES)
                        .map(String::as_str)
                        .unwrap_or("-")
                );
            }
            Ok(())
        }
        other => Err(format!("unknown command {other:?}\n{USAGE}")),
    }
}

struct Options {
    node_id: String,
    kind: Option<String>,
    endpoints: Vec<String>,
    attributes: BTreeMap<String, String>,
    ttl: u64,
    every: Option<u64>,
    home: Option<std::path::PathBuf>,
    path: Option<std::path::PathBuf>,
    once: bool,
    tag: Option<(String, Option<String>)>,
}

impl Options {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Self {
            node_id: String::new(),
            kind: None,
            endpoints: Vec::new(),
            attributes: BTreeMap::new(),
            ttl: mycelium_service_lease::DEFAULT_TTL_SECS,
            every: None,
            home: None,
            path: None,
            once: false,
            tag: None,
        };
        let mut index = 0;
        while index < args.len() {
            let value = |index: usize| {
                args.get(index + 1)
                    .cloned()
                    .ok_or_else(|| format!("{} needs a value", args[index]))
            };
            match args[index].as_str() {
                "--node-id" => {
                    options.node_id = value(index)?;
                    index += 2;
                }
                "--kind" => {
                    options.kind = Some(value(index)?);
                    index += 2;
                }
                "--endpoint" => {
                    options.endpoints.push(value(index)?);
                    index += 2;
                }
                "--attribute" => {
                    let pair = value(index)?;
                    let (key, value) = pair
                        .split_once('=')
                        .ok_or_else(|| format!("--attribute needs KEY=VALUE, not {pair:?}"))?;
                    options.attributes.insert(key.to_owned(), value.to_owned());
                    index += 2;
                }
                "--ttl" => {
                    options.ttl = value(index)?.parse().map_err(|_| "--ttl is seconds")?;
                    index += 2;
                }
                "--every" => {
                    options.every = Some(value(index)?.parse().map_err(|_| "--every is seconds")?);
                    index += 2;
                }
                "--home" => {
                    options.home = Some(value(index)?.into());
                    index += 2;
                }
                "--path" => {
                    options.path = Some(value(index)?.into());
                    index += 2;
                }
                "--once" => {
                    options.once = true;
                    index += 1;
                }
                "--tag" => {
                    let tag = value(index)?;
                    options.tag = Some(match tag.split_once('=') {
                        Some((key, value)) => (key.to_owned(), Some(value.to_owned())),
                        None => (tag, None),
                    });
                    index += 2;
                }
                other => return Err(format!("unknown option {other:?}\n{USAGE}")),
            }
        }
        if options.node_id.is_empty() {
            return Err("--node-id is the identity myceliumd already knows this node by; \
                 it is the only thing a lease cannot guess"
                .into());
        }
        Ok(options)
    }

    fn home(&self) -> std::path::PathBuf {
        self.home.clone().unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            std::path::PathBuf::from(home).join(".mycelium")
        })
    }

    fn lease(&self) -> Result<Lease, String> {
        let Some(kind) = &self.kind else {
            return Err("publish needs --kind".into());
        };
        if self.endpoints.is_empty() {
            return Err("publish needs at least one --endpoint".into());
        }
        let mut lease = Lease::new(kind.clone()).ttl(self.ttl);
        for endpoint in &self.endpoints {
            lease = lease.endpoint(endpoint.clone());
        }
        for (key, value) in &self.attributes {
            lease = lease.attribute(key.clone(), value.clone());
        }
        Ok(lease)
    }
}
