//! mirrorleaf-cli: a headless peer for tests and measurements (implementation plan,
//! M1). Each data folder is one device; run two to sync with yourself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use mirrorleaf_core::{BlobHash, BlobKind, Block, PhotoRef, Replica, photos};
use mirrorleaf_net::ledger::{DeviceProfile, LedgerClient};
use mirrorleaf_net::{
    Account, BlobStorage, DevTicket, Event, Network, SecretKey, State, SyncNode, describe,
};
use mirrorleaf_proto::ledger::{Platform, StatusResponse, WakeResponse};
use mirrorleaf_proto::{DeviceEntry, DeviceId};

const KEY_FILE: &str = "device.key";
const DB_FILE: &str = "mirrorleaf.db";
/// The blob store's folder inside the data folder.
const BLOB_DIR: &str = "blobs";
/// Where the ledger settings are kept in the database's `meta` table.
const LEDGER_URL: &str = "ledger_url";
const LEDGER_PLATFORM: &str = "ledger_platform";
const LEDGER_PUSH_TOKEN: &str = "ledger_push_token";
const LEDGER_NOTIFICATIONS: &str = "ledger_notifications";
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
    #[command(subcommand)]
    Photo(Photo),
    /// The sync-status ledger (M3)
    #[command(subcommand)]
    Ledger(LedgerCmd),
    /// Sync with every device on the account that can be reached
    Sync {
        /// Stay connected and keep syncing until Ctrl-C
        #[arg(long)]
        watch: bool,
        /// Save every photo this device holds into this folder, and each new one
        /// as it arrives (thumbnails as <photo>-thumbnail.jpg)
        #[arg(long, value_name = "FOLDER")]
        export: Option<PathBuf>,
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

#[derive(Subcommand)]
enum LedgerCmd {
    /// Register this device with a ledger; `sync` then reports to it
    Register {
        /// The ledger's address, for example http://192.168.1.20:8080
        url: String,
        /// Report as this platform instead of this machine's. `android` lets a
        /// CLI device stand in for a phone in Phase 0 tests.
        #[arg(long)]
        platform: Option<Platform>,
        /// A push token to report. A made-up one makes the ledger decide (and,
        /// without FCM, log) pushes for this device.
        #[arg(long)]
        push_token: Option<String>,
        /// Report that this device may show notifications
        #[arg(long)]
        notifications: bool,
    },
    /// Report once and show what the ledger knows about each device
    Status,
    /// Ask the ledger to wake the device holding what this one lacks, as
    /// opening the app while behind does
    Wake,
    /// Remove this device from the ledger and stop reporting to it
    Leave,
}

#[derive(Subcommand)]
enum Photo {
    /// Import photos into a note: resized to 2048 px and at most 600 KB, with
    /// their EXIF kept, plus a 320 px thumbnail
    Add {
        /// The note (ID or a unique prefix of it)
        note: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// List the photos in the notes and what this device holds of each
    List,
    /// Ask for a photo's full image; `sync --watch` fetches it from a device that has it
    Fetch {
        /// The photo (its hash or a unique prefix of it)
        photo: String,
        /// Seconds to wait for it to arrive
        #[arg(long, default_value_t = 60)]
        wait: u64,
    },
    /// Save a photo to a file (while `sync --watch` runs, use its --export instead)
    Export {
        photo: String,
        path: PathBuf,
        /// Save the thumbnail instead of the full image
        #[arg(long)]
        thumbnail: bool,
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
        Command::Photo(cmd) => photo(&dir, cmd).await,
        Command::Ledger(cmd) => ledger(&dir, cmd).await,
        Command::Sync { watch, export } => sync(&dir, watch, export).await,
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
            // No photos move here, so the store stays free for `sync --watch`.
            let node = SyncNode::bind(
                State { replica, account },
                secret,
                Network::Internet,
                BlobStorage::Memory,
            )
            .await?;
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
            // Photos follow with the first `sync`.
            let node = SyncNode::bind(
                State { replica, account },
                secret,
                Network::Internet,
                BlobStorage::Memory,
            )
            .await?;
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
            for (i, block) in replica.note_blocks(&note.id)?.into_iter().enumerate() {
                match block {
                    Block::Paragraph(text) => println!("{i}: {text}"),
                    Block::Photo {
                        photo,
                        width,
                        height,
                    } => {
                        let held = if replica.holds_blob(photo.full)? {
                            "full image here"
                        } else if replica.holds_blob(photo.thumbnail)? {
                            "thumbnail only"
                        } else {
                            "not here yet"
                        };
                        println!(
                            "{i}: [photo {} {width}×{height}, {held}]",
                            photo.full.short()
                        );
                    }
                    Block::Other(kind) => println!("{i}: [{kind}]"),
                }
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

async fn photo(dir: &Path, cmd: Photo) -> Result<()> {
    let (_, mut replica) = open(dir)?;
    match cmd {
        Photo::Add { note, files } => {
            let id = replica.find_note(&note)?.id;
            let decoder = photos::system_decoder();
            for file in files {
                let started = Instant::now();
                let bytes = std::fs::read(&file)
                    .with_context(|| format!("can't read {}", file.display()))?;
                let p = replica
                    .import_photo(&id, &bytes, decoder.as_deref())
                    .with_context(|| file.display().to_string())?;
                let exif = match p.exif_bytes {
                    0 => "no EXIF".to_owned(),
                    n => format!("EXIF {} kept", size(n as u64)),
                };
                println!(
                    "{}  {} ({}) -> {}×{}, {} at quality {}, {exif}; thumbnail {} ({} ms)",
                    p.photo.full.short(),
                    file.file_name().unwrap_or_default().to_string_lossy(),
                    size(bytes.len() as u64),
                    p.width,
                    p.height,
                    size(p.full.len() as u64),
                    p.quality,
                    size(p.thumbnail.len() as u64),
                    started.elapsed().as_millis()
                );
            }
            println!("`sync --watch` on this device sends the thumbnails to the other devices.");
        }
        Photo::List => {
            let titles: HashMap<String, String> = replica
                .list_notes()?
                .into_iter()
                .map(|n| (n.id, n.title))
                .collect();
            for p in replica.photos()? {
                let full = match (p.has_full, p.wanted) {
                    (true, _) => "here",
                    (false, true) => "asked for",
                    (false, false) => "not here",
                };
                println!(
                    "{}  thumbnail {:<8}  full image {:<9}  in \"{}\"",
                    p.photo.full.short(),
                    if p.has_thumbnail { "here" } else { "not here" },
                    full,
                    titles.get(&p.note).map_or("?", String::as_str)
                );
            }
        }
        Photo::Fetch { photo, wait } => {
            let p = replica.find_photo(&photo)?;
            if p.has_full {
                println!("The full image is already here.");
                return Ok(());
            }
            replica.want_blob(p.photo.full)?;
            println!(
                "Asked for the full image of {}. `sync --watch` on this device fetches it.",
                p.photo.full.short()
            );
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(wait) {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if replica.holds_blob(p.photo.full)? {
                    println!("Arrived after {:.1} s.", started.elapsed().as_secs_f64());
                    return Ok(());
                }
            }
            println!(
                "Not here yet: it stays asked for, and arrives once a device that has it is online."
            );
        }
        Photo::Export {
            photo,
            path,
            thumbnail,
        } => {
            let p = replica.find_photo(&photo)?.photo;
            let hash = if thumbnail { p.thumbnail } else { p.full };
            let bytes = match replica.staged_blob(hash)? {
                Some(bytes) => Some(bytes),
                None => mirrorleaf_net::read_blob(&BlobStorage::Disk(dir.join(BLOB_DIR)), hash)
                    .await
                    .context("while `sync --watch` runs, use its --export option instead")?,
            };
            let Some(bytes) = bytes else {
                bail!("this device doesn't hold that image yet");
            };
            std::fs::write(&path, &bytes)?;
            println!("Saved {} ({}).", path.display(), size(bytes.len() as u64));
        }
    }
    Ok(())
}

/// A byte count for people: 980 B, 21.3 KB, 2.9 MB (1 KB = 1000 bytes, as in
/// the 600 KB photo target).
fn size(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{:.1} KB", bytes as f64 / 1e3),
        _ => format!("{:.1} MB", bytes as f64 / 1e6),
    }
}

/// The photo a blob belongs to, and which of its images the blob is.
fn photo_of(replica: &Replica, hash: BlobHash) -> Result<Option<(PhotoRef, BlobKind)>> {
    Ok(replica.photos()?.into_iter().find_map(|p| {
        if p.photo.full == hash {
            Some((p.photo, BlobKind::Full))
        } else if p.photo.thumbnail == hash {
            Some((p.photo, BlobKind::Thumbnail))
        } else {
            None
        }
    }))
}

/// How the CLI names a blob: by its photo, which is named by its full image.
fn blob_label(node: &SyncNode, hash: BlobHash) -> String {
    let state = node.state().lock().unwrap();
    match photo_of(&state.replica, hash) {
        Ok(Some((photo, kind))) => format!("{kind} of photo {}", photo.full.short()),
        _ => format!("blob {}", hash.short()),
    }
}

/// Saves the blob `hash` into `folder`, named after its photo, if it belongs to
/// one and this device holds it.
async fn export_blob(node: &SyncNode, folder: &Path, hash: BlobHash) -> Result<()> {
    let found = photo_of(&node.state().lock().unwrap().replica, hash)?;
    let Some((photo, kind)) = found else {
        return Ok(());
    };
    let Some(bytes) = node.blob(hash).await? else {
        return Ok(());
    };
    let name = match kind {
        BlobKind::Full => format!("{}.jpg", photo.full.short()),
        BlobKind::Thumbnail => format!("{}-thumbnail.jpg", photo.full.short()),
    };
    std::fs::write(folder.join(name), bytes)?;
    Ok(())
}

async fn ledger(dir: &Path, cmd: LedgerCmd) -> Result<()> {
    let (secret, mut replica) = open(dir)?;
    let account =
        Account::load(&replica)?.context("no account yet: run `dev-account new` or `join`")?;
    let vv = replica.version_vector()?;
    let names = device_names(&account);
    match cmd {
        LedgerCmd::Register {
            url,
            platform,
            push_token,
            notifications,
        } => {
            let profile = DeviceProfile {
                platform: platform.unwrap_or_else(Platform::this),
                push_token,
                notifications,
            };
            let client = LedgerClient::new(&url, &secret, &account, profile.clone())?;
            client.register().await?;
            replica.set_meta(LEDGER_URL, url.as_bytes())?;
            replica.set_meta(LEDGER_PLATFORM, profile.platform.to_string().as_bytes())?;
            match &profile.push_token {
                Some(token) => replica.set_meta(LEDGER_PUSH_TOKEN, token.as_bytes())?,
                None => replica.set_meta(LEDGER_PUSH_TOKEN, b"")?,
            }
            replica.set_meta(LEDGER_NOTIFICATIONS, &[u8::from(notifications)])?;
            println!("Registered with {url} as {}.", profile.platform);
            let response = client.report(&vv, None).await?;
            print_devices(&response, &names, device_of(&secret));
            println!("`sync` and `sync --watch` now report to this ledger.");
        }
        LedgerCmd::Status => {
            let client = ledger_client(&replica, &secret, &account)?
                .context("no ledger yet: run `ledger register <url>`")?;
            // Leaves this device's online state alone, so it is safe while
            // `sync --watch` runs.
            let response = client.report(&vv, None).await?;
            print_devices(&response, &names, device_of(&secret));
        }
        LedgerCmd::Wake => {
            let client = ledger_client(&replica, &secret, &account)?
                .context("no ledger yet: run `ledger register <url>`")?;
            client.report(&vv, None).await?;
            let name = |d: &DeviceId| names.get(d).cloned().unwrap_or_else(|| d.short());
            match client.wake().await? {
                WakeResponse::UpToDate => println!("Nothing to fetch: this device is up to date."),
                WakeResponse::HolderOnline { device } => {
                    println!(
                        "{} holds the changes and is online: `sync` reaches it directly.",
                        name(&device)
                    )
                }
                WakeResponse::Woken {
                    device,
                    changes,
                    high_priority,
                } => println!(
                    "Woke {} ({} priority) to deliver {changes} changes.",
                    name(&device),
                    if high_priority { "high" } else { "normal" }
                ),
                WakeResponse::AlreadyWaking { device } => {
                    println!(
                        "{} was already woken in the last two minutes.",
                        name(&device)
                    )
                }
                WakeResponse::NoWakeableHolder => println!(
                    "The devices holding the changes are offline and can't be woken (only Android devices can)."
                ),
            }
        }
        LedgerCmd::Leave => {
            let client = ledger_client(&replica, &secret, &account)?
                .context("this device isn't using a ledger")?;
            client.leave().await?;
            replica.set_meta(LEDGER_URL, b"")?;
            println!("Removed this device from {}.", client.url());
        }
    }
    Ok(())
}

/// The ledger this device reports to, if one is set.
fn ledger_client(
    replica: &Replica,
    secret: &SecretKey,
    account: &Account,
) -> Result<Option<LedgerClient>> {
    let text = |key| -> Result<String> {
        Ok(replica
            .meta(key)?
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_default())
    };
    let url = text(LEDGER_URL)?;
    if url.is_empty() {
        return Ok(None);
    }
    let token = text(LEDGER_PUSH_TOKEN)?;
    let profile = DeviceProfile {
        platform: text(LEDGER_PLATFORM)?
            .parse()
            .unwrap_or_else(|_| Platform::this()),
        push_token: (!token.is_empty()).then_some(token),
        notifications: replica.meta(LEDGER_NOTIFICATIONS)?.as_deref() == Some(&[1]),
    };
    Ok(Some(LedgerClient::new(&url, secret, account, profile)?))
}

fn device_names(account: &Account) -> HashMap<DeviceId, String> {
    account
        .devices
        .list
        .devices
        .iter()
        .map(|d| (d.id, d.name.clone()))
        .collect()
}

/// One line per device of the account, as the ledger sees it.
fn print_devices(response: &StatusResponse, names: &HashMap<DeviceId, String>, me: DeviceId) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    for d in &response.devices {
        let name = names
            .get(&d.device)
            .map_or("unknown device", String::as_str);
        let seen = if d.device == me {
            "this device".to_owned()
        } else if d.online {
            String::new()
        } else {
            format!("last seen {} min ago", ((now - d.last_seen) / 60).max(0))
        };
        println!(
            "  {}  {:<16} {:<8} {:<8} behind {:<6} {seen}",
            d.device.short(),
            name,
            d.platform.to_string(),
            if d.online { "online" } else { "offline" },
            d.behind
        );
    }
}

/// The ledger's view in one line, for `sync --watch`.
fn ledger_summary(response: &StatusResponse, names: &HashMap<DeviceId, String>) -> String {
    response
        .devices
        .iter()
        .map(|d| {
            let name = names
                .get(&d.device)
                .map_or_else(|| d.device.short(), Clone::clone);
            let online = if d.online { "online" } else { "offline" };
            format!("{name} {online}, behind {}", d.behind)
        })
        .collect::<Vec<_>>()
        .join(" · ")
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

async fn sync(dir: &Path, watch: bool, export: Option<PathBuf>) -> Result<()> {
    let (secret, replica) = open(dir)?;
    let account =
        Account::load(&replica)?.context("no account yet: run `dev-account new` or `join`")?;
    let ledger = ledger_client(&replica, &secret, &account)?;
    let names = device_names(&account);
    let node = SyncNode::bind(
        State { replica, account },
        secret,
        Network::Internet,
        BlobStorage::Disk(dir.join(BLOB_DIR)),
    )
    .await?;
    if let Some(ledger) = ledger {
        println!("Reporting to the ledger at {}.", ledger.url());
        node.use_ledger(ledger);
    }
    for (hash, len) in node.store_staged().await? {
        println!(
            "stored {} added here ({})",
            blob_label(&node, hash),
            size(len)
        );
    }
    if let Some(folder) = &export {
        std::fs::create_dir_all(folder)?;
        let photos = node.state().lock().unwrap().replica.photos()?;
        for p in photos {
            export_blob(&node, folder, p.photo.thumbnail).await?;
            export_blob(&node, folder, p.photo.full).await?;
        }
    }
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
        let mut last_ledger = String::new();
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
                    Some(Event::Fetched { peer, path, report, .. }) => {
                        println!(
                            "{} via {path}: {}, {} ({} read) in {} ms",
                            label(peer), blob_label(&node, report.hash), size(report.len),
                            size(report.bytes_read), report.took.as_millis()
                        );
                        if let Some(folder) = &export {
                            export_blob(&node, folder, report.hash).await?;
                        }
                    }
                    Some(Event::FetchFailed { hash, error, .. }) => {
                        println!(
                            "{}: no connected device could send it yet ({error})",
                            blob_label(&node, hash)
                        );
                    }
                    Some(Event::LedgerReported { response }) => {
                        // Only when the picture changes, not on every heartbeat.
                        let summary = ledger_summary(&response, &names);
                        if summary != last_ledger {
                            println!("ledger: {summary}");
                            last_ledger = summary;
                        }
                    }
                    Some(Event::LedgerFailed { error }) => {
                        if last_ledger != error {
                            println!("ledger: {error}");
                            last_ledger = error;
                        }
                    }
                    Some(Event::Stored { hash, len }) => {
                        println!("stored {} added here ({})", blob_label(&node, hash), size(len));
                        if let Some(folder) = &export {
                            export_blob(&node, folder, hash).await?;
                        }
                    }
                    None => break,
                },
            }
        }
        watching.abort();
        if node.ledger().is_some() {
            match node.go_offline().await {
                Ok(()) => println!("ledger: reported going offline"),
                Err(e) => println!("ledger: couldn't report going offline: {}", describe(&e)),
            }
        }
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
                Ok(Err(e)) => {
                    println!("{}: {}", label(peer), describe(&e));
                    continue;
                }
                Err(_) => {
                    println!("{}: not reachable", label(peer));
                    continue;
                }
            }
            match node.fetch_missing(peer).await {
                Ok(fetch) => {
                    let path = fetch.path.map(|p| p.to_string()).unwrap_or_default();
                    for (_, report) in fetch.fetched {
                        println!(
                            "{} via {path}: {}, {} ({} read) in {} ms",
                            label(peer),
                            blob_label(&node, report.hash),
                            size(report.len),
                            size(report.bytes_read),
                            report.took.as_millis()
                        );
                        if let Some(folder) = &export {
                            export_blob(&node, folder, report.hash).await?;
                        }
                    }
                    for failure in fetch.failed {
                        println!(
                            "{}: {}: {}",
                            label(peer),
                            blob_label(&node, failure.hash),
                            failure.error
                        );
                    }
                }
                Err(e) => println!("{}: photos: {}", label(peer), describe(&e)),
            }
        }
        // A one-off sync doesn't stay online: it reports what it now holds as it leaves.
        if let Some(ledger) = node.ledger() {
            let vv = node.state().lock().unwrap().replica.version_vector()?;
            match ledger.report(&vv, Some(0)).await {
                Ok(response) => println!("ledger: {}", ledger_summary(&response, &names)),
                Err(e) => println!("ledger: {}", describe(&e)),
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
    match ledger_client(&replica, &secret, &account)? {
        Some(ledger) => println!(
            "Ledger   {} (as {}{})",
            ledger.url(),
            ledger.profile().platform,
            if ledger.profile().push_token.is_some() {
                ", with a push token"
            } else {
                ""
            }
        ),
        None => println!("Ledger   none (`ledger register <url>` to use one)"),
    }
    let photos = replica.photos()?;
    println!(
        "Photos   {} ({} thumbnails and {} full images here)",
        photos.len(),
        photos.iter().filter(|p| p.has_thumbnail).count(),
        photos.iter().filter(|p| p.has_full).count()
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
