//! The recovery kit (E2, assessment §8.4): a 128-bit secret, shown once as 12
//! words and a QR code, from which the account key is derived (ADR-010). A kit
//! is made when an account starts and each time the account key is replaced;
//! replacing the key is what retires the old kit, because the ledger then
//! refuses the old key.
//!
//! The kit is saved as a printable PDF and as plain text. Devices never keep
//! the secret: only while the person is saving the kit.

use std::fmt;
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use data_encoding::BASE32_NOPAD;
use mirrorleaf_proto::{AccountId, AccountKey};
use pdf_writer::{Content, Name, Pdf, Rect, Ref, Str};
use serde::{Deserialize, Serialize};

/// The text a kit's QR code holds starts with this (upper case and base32, so
/// the QR code uses its compact alphanumeric mode).
const QR_PREFIX: &str = "MLR1";

/// The recovery secret. Its `Debug` form hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct RecoverySecret([u8; 16]);

impl fmt::Debug for RecoverySecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoverySecret(…)")
    }
}

impl RecoverySecret {
    pub fn generate() -> Self {
        Self(rand::random())
    }

    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub fn to_bytes(&self) -> [u8; 16] {
        self.0
    }

    /// The account key this secret stands for.
    pub fn account_key(&self) -> AccountKey {
        AccountKey::from_recovery(&self.0)
    }

    /// The 12 words: BIP39's English list, the last word carrying a checksum.
    pub fn words(&self) -> Vec<&'static str> {
        bip39::Mnemonic::from_entropy(&self.0)
            .expect("16 bytes is a valid BIP39 entropy length")
            .words()
            .collect()
    }

    /// Reads the 12 words as a person types them: any case, any separators,
    /// numbering ("1. hello 2. world"), and words cut to their first four
    /// letters (BIP39 words differ within four).
    pub fn parse_words(text: &str) -> Result<Self> {
        let list = bip39::Language::English.word_list();
        let mut words = Vec::new();
        for part in text
            .split(|c: char| !c.is_ascii_alphabetic())
            .filter(|p| !p.is_empty())
        {
            let part = part.to_lowercase();
            let found = if list.contains(&part.as_str()) {
                Some(part.clone())
            } else if part.len() >= 4 {
                let mut matches = list.iter().filter(|w| w.starts_with(&part));
                match (matches.next(), matches.next()) {
                    (Some(w), None) => Some((*w).to_owned()),
                    _ => None,
                }
            } else {
                None
            };
            words.push(found.with_context(|| format!("“{part}” isn't one of the recovery words"))?);
        }
        if words.len() != 12 {
            bail!("a recovery kit has 12 words; this has {}", words.len());
        }
        let mnemonic = bip39::Mnemonic::parse_in_normalized(
            bip39::Language::English,
            &words.join(" "),
        )
        .map_err(|_| {
            anyhow::anyhow!(
                "these words don't belong together: check each one against your recovery kit"
            )
        })?;
        let (entropy, len) = mnemonic.to_entropy_array();
        let bytes: [u8; 16] = entropy[..len]
            .try_into()
            .map_err(|_| anyhow::anyhow!("a recovery kit has 12 words"))?;
        Ok(Self(bytes))
    }
}

/// What a kit's QR code holds: the secret, and the account's ledger, so a
/// recovering device knows where to find the account's other devices.
#[derive(Serialize, Deserialize)]
struct QrBody {
    secret: [u8; 16],
    ledger: Option<String>,
}

/// A recovery code as a person enters it: the kit's QR code or its words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryCode {
    pub secret: RecoverySecret,
    /// From the QR code; typed words don't carry it.
    pub ledger: Option<String>,
}

impl FromStr for RecoveryCode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        if let Some(body) = s
            .get(..QR_PREFIX.len())
            .filter(|p| p.eq_ignore_ascii_case(QR_PREFIX))
            .map(|_| &s[QR_PREFIX.len()..])
        {
            let bytes = BASE32_NOPAD
                .decode(body.trim().to_uppercase().as_bytes())
                .context("the kit's QR code is damaged")?;
            let body: QrBody =
                postcard::from_bytes(&bytes).context("the kit's QR code is damaged")?;
            return Ok(Self {
                secret: RecoverySecret(body.secret),
                ledger: body.ledger,
            });
        }
        if s.starts_with("MLP1") || s.starts_with("mlp1") {
            bail!(
                "that's a code for adding a device; to recover, use the code on your recovery kit"
            );
        }
        Ok(Self {
            secret: RecoverySecret::parse_words(s)?,
            ledger: None,
        })
    }
}

/// A recovery kit, ready to save or print.
#[derive(Clone, Debug)]
pub struct Kit {
    pub secret: RecoverySecret,
    pub account: AccountId,
    /// Unix seconds.
    pub made_at: u64,
    pub ledger: Option<String>,
}

impl Kit {
    pub fn new(secret: RecoverySecret, made_at: u64, ledger: Option<String>) -> Self {
        Self {
            account: secret.account_key().id(),
            secret,
            made_at,
            ledger,
        }
    }

    /// The text the kit's QR code holds.
    pub fn qr_text(&self) -> String {
        let body = QrBody {
            secret: self.secret.0,
            ledger: self.ledger.clone(),
        };
        let bytes = postcard::to_stdvec(&body).expect("the QR body always serialises");
        format!("{QR_PREFIX}{}", BASE32_NOPAD.encode(&bytes))
    }

    /// The account's short form, as Settings shows it ("Account 48a02495").
    pub fn fingerprint(&self) -> String {
        self.account.to_string()[..8].to_owned()
    }

    /// The day the kit was made, in UTC ("9 November 2026").
    pub fn date(&self) -> String {
        long_date(self.made_at)
    }

    pub fn file_name(&self, extension: &str) -> String {
        format!("Mirrorleaf recovery kit {}.{extension}", self.fingerprint())
    }

    /// The kit as plain text.
    pub fn text(&self) -> String {
        let words = self.secret.words();
        let mut out = String::new();
        out.push_str("MIRRORLEAF RECOVERY KIT\n\n");
        out.push_str(&format!("Account   {}\n", self.fingerprint()));
        out.push_str(&format!("Made      {}\n\n", self.date()));
        out.push_str("Your 12 recovery words\n\n");
        for row in 0..4 {
            let line: Vec<String> = (0..3)
                .map(|col| {
                    let n = col * 4 + row;
                    format!("{:>2}. {:<12}", n + 1, words[n])
                })
                .collect();
            out.push_str(line.join("  ").trim_end());
            out.push('\n');
        }
        out.push_str("\nOr, instead of the words, the code from the PDF's QR code:\n");
        out.push_str(&self.qr_text());
        out.push_str("\n\nHOW TO RECOVER YOUR ACCOUNT\n\n");
        for (i, step) in STEPS.iter().enumerate() {
            out.push_str(&format!("{}. {step}\n", i + 1));
        }
        out.push_str("\nKEEP THIS KIT SAFE\n\n");
        for point in SAFETY {
            out.push_str(&format!("- {point}\n"));
        }
        out
    }

    /// The kit as a one-page A4 PDF: the words, the QR code, how to recover
    /// and how to keep it safe. Standard fonts only, so nothing is embedded.
    pub fn pdf(&self) -> Result<Vec<u8>> {
        let (catalog, tree, page, content_id, regular, bold, mono) = (
            Ref::new(1),
            Ref::new(2),
            Ref::new(3),
            Ref::new(4),
            Ref::new(5),
            Ref::new(6),
            Ref::new(7),
        );
        let mut pdf = Pdf::new();
        pdf.catalog(catalog).pages(tree);
        pdf.pages(tree).kids([page]).count(1);
        {
            let mut p = pdf.page(page);
            p.parent(tree)
                .media_box(Rect::new(0.0, 0.0, PAGE_W, PAGE_H))
                .contents(content_id);
            let mut resources = p.resources();
            let mut fonts = resources.fonts();
            fonts.pair(Name(b"R"), regular);
            fonts.pair(Name(b"B"), bold);
            fonts.pair(Name(b"M"), mono);
        }
        for (id, name) in [
            (regular, &b"Helvetica"[..]),
            (bold, b"Helvetica-Bold"),
            (mono, b"Courier"),
        ] {
            pdf.type1_font(id)
                .base_font(Name(name))
                .encoding_predefined(Name(b"WinAnsiEncoding"));
        }

        let mut c = Content::new();
        let ink = |c: &mut Content| {
            c.set_fill_rgb(0.11, 0.13, 0.12);
        };
        let grey = |c: &mut Content| {
            c.set_fill_rgb(0.42, 0.45, 0.43);
        };
        let left = MARGIN;

        ink(&mut c);
        text(&mut c, b"B", 22.0, left, 770.0, "Mirrorleaf recovery kit");
        grey(&mut c);
        text(
            &mut c,
            b"R",
            11.0,
            left,
            748.0,
            &format!("Account {}  ·  Made {}", self.fingerprint(), self.date()),
        );
        c.rect(left, 732.0, PAGE_W - 2.0 * MARGIN, 0.6);
        c.fill_nonzero();

        // The words, numbered down each of three columns.
        ink(&mut c);
        text(&mut c, b"B", 12.0, left, 704.0, "Your 12 recovery words");
        let words = self.secret.words();
        for (n, word) in words.iter().enumerate() {
            let (col, row) = (n / 4, n % 4);
            let x = left + col as f32 * 112.0;
            let y = 676.0 - row as f32 * 28.0;
            grey(&mut c);
            text(&mut c, b"R", 10.0, x, y, &format!("{:>2}", n + 1));
            ink(&mut c);
            text(&mut c, b"B", 15.0, x + 18.0, y, word);
        }

        // The QR code, to the right of the words.
        let qr = qrcode::QrCode::with_error_correction_level(
            self.qr_text().as_bytes(),
            qrcode::EcLevel::M,
        )?;
        let modules = qr.width();
        let size = 140.0;
        let module = size / (modules as f32 + 8.0);
        let (qx, qtop) = (PAGE_W - MARGIN - size, 712.0);
        ink(&mut c);
        for y in 0..modules {
            let mut x = 0;
            while x < modules {
                if qr[(x, y)] == qrcode::Color::Dark {
                    let start = x;
                    while x < modules && qr[(x, y)] == qrcode::Color::Dark {
                        x += 1;
                    }
                    c.rect(
                        qx + (start as f32 + 4.0) * module,
                        qtop - (y as f32 + 5.0) * module,
                        (x - start) as f32 * module,
                        module,
                    );
                } else {
                    x += 1;
                }
            }
        }
        c.fill_nonzero();
        grey(&mut c);
        text(
            &mut c,
            b"R",
            9.0,
            qx + 4.0 * module,
            qtop - size - 10.0,
            "Scan to recover",
        );

        // How to recover.
        let mut y = 520.0;
        ink(&mut c);
        text(&mut c, b"B", 13.0, left, y, "How to recover your account");
        y -= 24.0;
        for (i, step) in STEPS.iter().enumerate() {
            let lines = wrap(step, 86);
            text(&mut c, b"B", 10.5, left, y, &format!("{}.", i + 1));
            for line in lines {
                text(&mut c, b"R", 10.5, left + 16.0, y, &line);
                y -= 15.0;
            }
            y -= 5.0;
        }

        // Keeping it safe.
        y -= 14.0;
        text(&mut c, b"B", 13.0, left, y, "Keep this kit safe");
        y -= 24.0;
        for point in SAFETY {
            text(&mut c, b"R", 10.5, left, y, "–");
            for line in wrap(point, 86) {
                text(&mut c, b"R", 10.5, left + 16.0, y, &line);
                y -= 15.0;
            }
            y -= 5.0;
        }

        // The QR code's text, for typing where a camera isn't available.
        grey(&mut c);
        text(
            &mut c,
            b"R",
            9.0,
            left,
            86.0,
            "No camera? Type the 12 words, or paste this code:",
        );
        ink(&mut c);
        for (i, line) in self
            .qr_text()
            .as_bytes()
            .chunks(72)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .enumerate()
        {
            text(&mut c, b"M", 8.5, left, 72.0 - i as f32 * 11.0, &line);
        }

        pdf.stream(content_id, &c.finish());
        Ok(pdf.finish())
    }
}

const PAGE_W: f32 = 595.0;
const PAGE_H: f32 = 842.0;
const MARGIN: f32 = 56.0;

const STEPS: [&str; 4] = [
    "Install Mirrorleaf on a phone or computer.",
    "Choose “Recover my account”.",
    "Scan the QR code on this kit, or type the 12 words.",
    "Your notes come back from any of your devices that still has them. Then remove the devices you lost, and make a new recovery kit: this one stops working once it has been used.",
];

const SAFETY: [&str; 3] = [
    "Anyone with these words can open your account. Keep the kit offline: printed, in a password manager or on a USB drive.",
    "Mirrorleaf never sees these words and can't reset them. If you lose every device and this kit, nobody can recover your notes.",
    "Making a new kit on any of your devices retires this one.",
];

/// One line of text at (x, y).
fn text(c: &mut Content, font: &[u8], size: f32, x: f32, y: f32, line: &str) {
    c.begin_text();
    c.set_font(Name(font), size);
    c.next_line(x, y);
    c.show(Str(&win_ansi(line)));
    c.end_text();
}

/// Text in the standard fonts' WinAnsi encoding. The kit's own text uses only
/// ASCII and a few punctuation marks; anything else becomes "?".
fn win_ansi(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| match c {
            ' '..='~' => c as u8,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '–' => 0x96,
            '·' => 0xB7,
            _ => b'?',
        })
        .collect()
}

/// Breaks `text` into lines of at most about `width` characters.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// "9 November 2026" for a Unix time, in UTC.
pub fn long_date(unix: u64) -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = (unix / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{day} {} {year}", MONTHS[(month - 1) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_round_trip_however_they_are_typed() {
        let secret = RecoverySecret::from_bytes([7; 16]);
        let words = secret.words();
        assert_eq!(words.len(), 12);
        assert_eq!(
            RecoverySecret::parse_words(&words.join(" ")).unwrap(),
            secret
        );
        // Numbered, upper case, cut to four letters, one per line.
        let typed: Vec<String> = words
            .iter()
            .enumerate()
            .map(|(i, w)| format!("{}. {}", i + 1, w[..w.len().min(4)].to_uppercase()))
            .collect();
        assert_eq!(
            RecoverySecret::parse_words(&typed.join("\n")).unwrap(),
            secret
        );
        // Two words swapped break the checksum (almost always; not for these).
        let mut swapped = words.clone();
        swapped.swap(0, 11);
        if swapped != words {
            assert!(RecoverySecret::parse_words(&swapped.join(" ")).is_err());
        }
        assert!(RecoverySecret::parse_words(&words[..11].join(" ")).is_err());
        assert!(RecoverySecret::parse_words("hello world").is_err());
    }

    #[test]
    fn the_qr_code_carries_the_secret_and_the_ledger() {
        let kit = Kit::new(
            RecoverySecret::generate(),
            1_794_200_000,
            Some("http://192.168.1.13:8080".into()),
        );
        let code: RecoveryCode = kit.qr_text().parse().unwrap();
        assert_eq!(code.secret, kit.secret);
        assert_eq!(code.ledger.as_deref(), Some("http://192.168.1.13:8080"));
        let typed: RecoveryCode = kit.secret.words().join(" ").parse().unwrap();
        assert_eq!(typed.secret, kit.secret);
        assert_eq!(typed.ledger, None);
        assert!("MLP1ABCDEF".parse::<RecoveryCode>().is_err());
        assert_eq!(kit.account, kit.secret.account_key().id());
    }

    #[test]
    fn the_kit_reads_as_text_and_as_a_pdf() {
        let kit = Kit::new(RecoverySecret::from_bytes([3; 16]), 1_794_200_000, None);
        assert_eq!(kit.date(), "9 November 2026");
        let text = kit.text();
        for word in kit.secret.words() {
            assert!(text.contains(word), "{word} missing");
        }
        assert!(text.contains(&kit.fingerprint()));
        let pdf = kit.pdf().unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(pdf.len() < 20_000, "{} bytes", pdf.len());
        assert_eq!(
            kit.file_name("pdf"),
            format!("Mirrorleaf recovery kit {}.pdf", kit.fingerprint())
        );
    }

    #[test]
    fn dates_are_written_out() {
        assert_eq!(long_date(0), "1 January 1970");
        assert_eq!(long_date(1_794_182_400), "9 November 2026");
        assert_eq!(long_date(951_782_400), "29 February 2000");
    }
}
