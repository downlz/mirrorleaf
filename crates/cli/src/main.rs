//! mirrorleaf-cli: a headless peer for tests and measurements (implementation plan,
//! M1). Each data folder is one device; run two to sync with yourself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use mirrorleaf_core::Replica;
use mirrorleaf_net::{Account, DevTicket, Event, Network, SecretKey, State, SyncNode, describe};
use mirrorleaf_proto::{DeviceEntry, DeviceId};

const KEY_FILE: &str = "device.key";
const DB_FILE: &str = "mirrorleaf.db";
const DEVICE_NAME: &str = "device_name";
const DIAL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Parser)]
#[command(version, about = "Mirrorleaf headless peer (Phase 0)")]
struct Cli {
    /// This device's data folder [default: $MIRRORLEAF_DIR, or ~/.mirrorleaf-dev]
    #[arg(long, global = true)]
    dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create this device's key and database
    Init {
        /// A name other devices will show for this one
        #[arg(long)]
        name: Option<String>,
    },
    /// Developer accounts for Phase 0 (real pairing comes in Phase 1)
    #[command(name = "dev-account", subcommand)]
    DevAccount(DevAccount),
    #[command(subcommand)]
    Note(Note),
    /// Sync with every device on the account that can be reached
    Sync {
        /// Stay connected and keep syncing until Ctrl-C
        #[arg(long)]
        watch: bool,
    },
    /// Show this device, its account and its change log
    Status,
}

#[derive(Subcommand)]
enum DevAccount {
    /// Start a new account with this device as its first member
    New,
    /// Print a ticket another device can join with (it contains the account key)
    Ticket,
    /// Join an account from a ticket, then sync with the inviting device
    Join { ticket: String },
}

#[derive(Subcommand)]
enum Note {
    New {
        title: String,
        /// A first paragraph
        #[arg(long)]
        text: Option<String>,
    },
    List {
        /// Include trashed notes
        #[arg(long)]
        all: bool,
    },
    /// Print a note (ID or a unique prefix of it)
    Cat { id: String },
    Edit {
        id: String,
        /// Add a paragraph at the end
        #[arg(long)]
        append: Option<String>,
        /// Insert text: PARAGRAPH:OFFSET:TEXT (both from 0)
        #[arg(long, value_name = "P:O:TEXT")]
        insert: Option<String>,
        /// Delete text: PARAGRAPH:OFFSET:LENGTH
        #[arg(long, value_name = "P:O:LEN")]
        delete: Option<String>,
        #[arg(long)]
        title: Option<String>,
        /// Move the note to the trash
        #[arg(long)]
        trash: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Iroh's NAT report warns on every start behind some routers; RUST_LOG=info shows it.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,iroh::net_report=error".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let dir = data_dir(cli.dir)?;
    match cli.command {
        Command::Init { name } => init(&dir, name),
        Command::DevAccount(cmd) => dev_account(&dir, cmd).await,
        Command::Note(cmd) => note(&dir, cmd),
        Command::Sync { watch } => sync(&dir, watch).await,
        Command::Status => status(&dir),
    }
}

fn data_dir(given: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(dir) = given.or_else(|| std::env::var_os("MIRRORLEAF_DIR").map(PathBuf::from)) {
        return Ok(dir);
    }
    let home = std::env::var_os("HOME").context("set --dir or MIRRORLEAF_DIR")?;
    Ok(PathBuf::from(home).join(".mirrorleaf-dev"))
}

fn init(dir: &Path, name: Option<String>) -> Result<()> {
    let key_path = dir.join(KEY_FILE);
    if key_path.exists() {
        bail!("{} is already set up", dir.display());
    }
    std::fs::create_dir_all(dir)?;
    let secret = SecretKey::generate();
    write_private(&key_path, &secret.to_bytes())?;
    let device = device_of(&secret);
    let mut replica = Replica::open(dir.join(DB_FILE), device)?;
    let name = name.unwrap_or_else(|| format!("device {}", device.short()));
    replica.set_meta(DEVICE_NAME, name.as_bytes())?;
    println!("Set up {name} ({device}) in {}", dir.display());
    println!("Next: `dev-account new` here, or `dev-account join <ticket>` from another device.");
    Ok(())
}

async fn dev_account(dir: &Path, cmd: DevAccount) -> Result<()> {
    let (secret, mut replica) = open(dir)?;
    let existing = Account::load(&replica)?;
    match cmd {
        DevAccount::New => {
            if existing.is_some() {
                bail!("this device already belongs to an account");
            }
            let account = Account::create(me(&replica, &secret)?);
            account.save(&mut replica)?;
            println!("Created developer account {}", account.id());
        }
        DevAccount::Ticket => {
            let account = existing.context("no account yet: run `dev-account new` first")?;
            let key = account
                .key
                .as_ref()
                .context("this device doesn't hold the account key")?;
            let account_secret = key.to_bytes();
            let devices = account.devices.clone();
            let node =
                SyncNode::bind(State { replica, account }, secret, Network::Internet).await?;
            if tokio::time::timeout(Duration::from_secs(15), node.online())
                .await
                .is_err()
            {
                eprintln!(
                    "warning: no relay connection yet; the ticket holds direct addresses only"
                );
            }
            let ticket = DevTicket {
                account_secret,
                devices,
                inviter: node.addr(),
            };
            println!("{ticket}");
            eprintln!(
                "\nThis ticket contains the account key: share it only with your own devices."
            );
            eprintln!("Keep `sync --watch` running here while the new device joins.");
            node.close().await;
        }
        DevAccount::Join { ticket } => {
            if existing.is_some() {
                bail!("this device already belongs to an account");
            }
            let ticket: DevTicket = ticket.parse()?;
            let account = Account::join(
                ticket.account_key(),
                &ticket.devices,
                me(&replica, &secret)?,
            )?;
            account.save(&mut replica)?;
            println!("Joined account {}", account.id());
            let node =
                SyncNode::bind(State { replica, account }, secret, Network::Internet).await?;
            let started = Instant::now();
            match tokio::time::timeout(DIAL_TIMEOUT, node.sync_once(ticket.inviter)).await {
                Ok(Ok((path, report))) => println!(
                    "Synced with the inviting device via {path}: received {} changes in {:.1} s",
                    report.received_changes,
                    started.elapsed().as_secs_f64()
                ),
                Ok(Err(e)) => println!("Couldn't sync yet ({}); run `sync` later.", describe(&e)),
                Err(_) => println!("The inviting device didn't answer; run `sync` later."),
            }
            node.close().await;
        }
    }
    Ok(())
}

fn note(dir: &Path, cmd: Note) -> Result<()> {
    let (_, mut replica) = open(dir)?;
    match cmd {
        Note::New { title, text } => {
            let id = replica.create_note(&title)?;
            if let Some(text) = text {
                replica.append_paragraph(&id, &text)?;
            }
            println!("{id}");
        }
        Note::List { all } => {
            for note in replica.list_notes()? {
                if note.trashed && !all {
                    continue;
                }
                let trashed = if note.trashed { "  [trash]" } else { "" };
                println!("{}  {}{trashed}", &note.id[..13], note.title);
            }
        }
        Note::Cat { id } => {
            let note = replica.find_note(&id)?;
            println!(
                "# {}{}",
                note.title,
                if note.trashed { "  [trash]" } else { "" }
            );
            for (i, paragraph) in replica.note_paragraphs(&note.id)?.iter().enumerate() {
                println!("{i}: {paragraph}");
            }
        }
        Note::Edit {
            id,
            append,
            insert,
            delete,
            title,
            trash,
        } => {
            let id = replica.find_note(&id)?.id;
            if let Some(text) = append {
                replica.append_paragraph(&id, &text)?;
            }
            if let Some(spec) = insert {
                let (p, o, text) = parse_spec(&spec)?;
                replica.insert_text(&id, p, o, text)?;
            }
            if let Some(spec) = delete {
                let (p, o, len) = parse_spec(&spec)?;
                replica.delete_text(&id, p, o, len.parse().context("length must be a number")?)?;
            }
            if let Some(title) = title {
                replica.set_title(&id, &title)?;
            }
            if trash {
                replica.trash_note(&id)?;
            }
        }
    }
    Ok(())
}

/// Splits "P:O:REST" into two numbers and the rest.
fn parse_spec(spec: &str) -> Result<(u32, u32, &str)> {
    let mut parts = spec.splitn(3, ':');
    let (Some(p), Some(o), Some(rest)) = (parts.next(), parts.next(), parts.next()) else {
        bail!("expected PARAGRAPH:OFFSET:VALUE, got {spec}");
    };
    Ok((
        p.parse().context("paragraph must be a number")?,
        o.parse().context("offset must be a number")?,
        rest,
    ))
}

async fn sync(dir: &Path, watch: bool) -> Result<()> {
    let (secret, replica) = open(dir)?;
    let account =
        Account::load(&replica)?.context("no account yet: run `dev-account new` or `join`")?;
    let node = SyncNode::bind(State { replica, account }, secret, Network::Internet).await?;
    // Names come from the live device list, which grows as devices join.
    let label = |id: DeviceId| {
        let state = node.state().lock().unwrap();
        let name = state
            .account
            .devices
            .list
            .devices
            .iter()
            .find(|d| d.id == id);
        format!(
            "{} ({})",
            id.short(),
            name.map_or("unknown device", |d| d.name.as_str())
        )
    };

    if watch {
        println!("Watching as {}. Ctrl-C to stop.", label(node.device()));
        let (events, mut log) = tokio::sync::mpsc::unbounded_channel();
        let watching = {
            let node = node.clone();
            tokio::spawn(async move { node.watch(events).await })
        };
        // Report each device's failures once, not on every redial.
        let mut last_error: HashMap<Option<DeviceId>, String> = HashMap::new();
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => break,
                event = log.recv() => match event {
                    Some(Event::Connected { peer, path, incoming }) => {
                        last_error.remove(&Some(peer));
                        let how = if incoming { "connected from" } else { "connected to" };
                        println!("{how} {} via {path}", label(peer));
                    }
                    Some(Event::Synced { peer, path, report, took }) => {
                        if report.sent_changes + report.received_changes > 0 {
                            println!(
                                "{} via {path}: sent {}, received {} changes ({} B out, {} B in) in {} ms",
                                label(peer), report.sent_changes, report.received_changes,
                                report.traffic.sent, report.traffic.received, took.as_millis()
                            );
                        }
                    }
                    Some(Event::Failed { peer, error }) => {
                        if last_error.get(&peer) != Some(&error) {
                            let who = peer.map(label).unwrap_or_else(|| "incoming connection".into());
                            println!("{who}: {error}");
                            last_error.insert(peer, error);
                        }
                    }
                    Some(Event::Disconnected { peer }) => println!("{} disconnected", label(peer)),
                    None => break,
                },
            }
        }
        watching.abort();
    } else {
        let me = node.device();
        let peers: Vec<DeviceId> = {
            let state = node.state().lock().unwrap();
            state
                .account
                .devices
                .list
                .devices
                .iter()
                .map(|d| d.id)
                .filter(|d| *d != me)
                .collect()
        };
        if peers.is_empty() {
            println!("No other devices on this account yet.");
        }
        for peer in peers {
            let started = Instant::now();
            let addr = mirrorleaf_net::endpoint_addr(peer)?;
            match tokio::time::timeout(DIAL_TIMEOUT, node.sync_once(addr)).await {
                Ok(Ok((path, report))) => println!(
                    "{} via {path}: sent {}, received {} changes ({} B out, {} B in) in {} ms",
                    label(peer),
                    report.sent_changes,
                    report.received_changes,
                    report.traffic.sent,
                    report.traffic.received,
                    started.elapsed().as_millis()
                ),
                Ok(Err(e)) => println!("{}: {}", label(peer), describe(&e)),
                Err(_) => println!("{}: not reachable", label(peer)),
            }
        }
    }
    node.close().await;
    Ok(())
}

fn status(dir: &Path) -> Result<()> {
    let (secret, replica) = open(dir)?;
    let device = device_of(&secret);
    let name = replica
        .meta(DEVICE_NAME)?
        .map(|n| String::from_utf8_lossy(&n).into_owned())
        .unwrap_or_default();
    println!("Device   {name} ({device})");
    let Some(account) = Account::load(&replica)? else {
        println!("Account  none yet");
        return Ok(());
    };
    println!(
        "Account  {} (device list v{})",
        account.id(),
        account.devices.list.version
    );
    let vv = replica.version_vector()?;
    println!("Devices  (changes held from each)");
    for d in &account.devices.list.devices {
        let me = if d.id == device {
            "  <- this device"
        } else {
            ""
        };
        println!(
            "  {}  {:<16} {:>6}{me}",
            d.id.short(),
            d.name,
            vv.get(&d.id)
        );
    }
    let notes = replica.list_notes()?;
    let trashed = notes.iter().filter(|n| n.trashed).count();
    println!(
        "Notes    {} ({} in trash), {} changes in the log",
        notes.len(),
        trashed,
        replica.change_count()?
    );
    Ok(())
}

fn open(dir: &Path) -> Result<(SecretKey, Replica)> {
    let bytes = std::fs::read(dir.join(KEY_FILE))
        .with_context(|| format!("{} isn't set up: run `init` first", dir.display()))?;
    let secret = SecretKey::from_bytes(
        &bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("{KEY_FILE} is corrupt"))?,
    );
    let replica = Replica::open(dir.join(DB_FILE), device_of(&secret))?;
    Ok((secret, replica))
}

fn me(replica: &Replica, secret: &SecretKey) -> Result<DeviceEntry> {
    let name = replica
        .meta(DEVICE_NAME)?
        .map(|n| String::from_utf8_lossy(&n).into_owned())
        .unwrap_or_default();
    Ok(DeviceEntry {
        id: device_of(secret),
        name,
    })
}

fn device_of(secret: &SecretKey) -> DeviceId {
    DeviceId(*secret.public().as_bytes())
}

/// Writes the device key readable by this user only.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)?;
    Ok(())
}
