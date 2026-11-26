//! Lettering fonts beyond the computer's own: a few that come with the app
//! (in a `fonts` folder beside it), and the rest of zigzag.diy's catalogue,
//! downloaded the first time they're wanted and kept in ZigZag's settings
//! folder after that.
//!
//! The catalogue (`GET /v1/fonts`) lists each family's files at addresses
//! that never change, each with its SHA-256: a download is checked against
//! it before it's kept. The catalogue itself is kept too, so the font menu
//! still lists the whole of it offline (those not downloaded greyed out).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use zigzag::svg::font_files;
use std::sync::mpsc;

use serde_json::Value;

/// One family in the catalogue.
#[derive(Clone, Debug, PartialEq)]
pub struct CatFont {
    pub family: String,
    pub slug: String,
    /// display, serif, handwriting…
    pub category: String,
    pub license: String,
    /// Made for embroidery (single-line fonts for running stitch, say),
    /// rather than an ordinary font that sews well.
    pub embroidery: bool,
    pub variants: Vec<Variant>,
}

/// One font file of a family.
#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    /// normal or italic.
    pub style: String,
    pub weight: u16,
    /// A variable font's weights, least to most.
    pub weights: Option<(u16, u16)>,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// The catalogue's families, from its JSON (entries it can't use left out).
pub fn parse_catalogue(v: &Value) -> Vec<CatFont> {
    let Some(list) = v["fonts"].as_array() else {
        return Vec::new();
    };
    let text = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_string();
    list.iter()
        .filter_map(|f| {
            let variants: Vec<Variant> = f["variants"]
                .as_array()?
                .iter()
                .filter_map(|v| {
                    let url = v["url"].as_str()?;
                    let sha = v["sha256"].as_str()?;
                    // (Only over https, and only a whole hash.)
                    (url.starts_with("https://") || url.starts_with("http://127.0.0.1"))
                        .then_some(())?;
                    (sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit())).then_some(())?;
                    let w = v["weights"].as_array();
                    Some(Variant {
                        style: v["style"].as_str().unwrap_or("normal").to_string(),
                        weight: v["weight"].as_u64().unwrap_or(400) as u16,
                        weights: w.and_then(|w| {
                            Some((w.first()?.as_u64()? as u16, w.last()?.as_u64()? as u16))
                        }),
                        url: url.to_string(),
                        size: v["size"].as_u64().unwrap_or(0),
                        sha256: sha.to_ascii_lowercase(),
                    })
                })
                .collect();
            let family = text(f, "family");
            let slug = text(f, "slug");
            // (The slug names a folder: nothing that could leave it.)
            let slug_ok = !slug.is_empty()
                && slug
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
            if family.is_empty() || !slug_ok || variants.is_empty() {
                return None;
            }
            let category = text(f, "category");
            Some(CatFont {
                embroidery: f["embroidery"].as_bool().unwrap_or(false)
                    || matches!(category.as_str(), "embroidery" | "single-line"),
                family,
                slug,
                category,
                license: text(f, "license"),
                variants,
            })
        })
        .collect()
}

/// The files of a family lettering needs: regular, bold and their
/// italics. A variable file covers every weight it has; of static files
/// (Lato comes as eighteen) only the 400 and 700 weights are taken, or
/// the nearest there is to each.
pub fn files_to_fetch(f: &CatFont) -> Vec<&Variant> {
    let mut out: Vec<&Variant> = Vec::new();
    for style in ["normal", "italic"] {
        let of: Vec<&Variant> = f.variants.iter().filter(|v| v.style == style).collect();
        for want in [400u16, 700] {
            let covers = |v: &Variant| match v.weights {
                Some((lo, hi)) => lo <= want && want <= hi,
                None => v.weight == want,
            };
            let pick = of.iter().find(|v| covers(v)).or_else(|| {
                of.iter()
                    .min_by_key(|v| (v.weight as i32 - want as i32).unsigned_abs())
            });
            if let Some(v) = pick
                && !out.iter().any(|o| o.url == v.url)
            {
                out.push(v);
            }
        }
    }
    out
}

/// SHA-256 of `data`, as lowercase hex (FIPS 180-4).
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, c) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

/// Where a downloaded file is kept: under the family's slug, named by its
/// hash (a new version of the font is a new file).
pub fn cached_path(dir: &Path, f: &CatFont, v: &Variant) -> PathBuf {
    let ext = match v.url.rsplit('.').next() {
        Some(e) if matches!(e, "ttf" | "otf" | "ttc") => e,
        _ => "ttf",
    };
    dir.join(&f.slug).join(format!("{}.{ext}", &v.sha256[..16]))
}

/// The family's files, downloaded with `get` into `dir` where they aren't
/// there yet, each checked against its hash. The files, or why not.
pub fn fetch_family(
    dir: &Path,
    f: &CatFont,
    get: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for v in files_to_fetch(f) {
        let path = cached_path(dir, f, v);
        if !path.exists() {
            let bytes = get(&v.url)?;
            if sha256_hex(&bytes) != v.sha256 {
                return Err(format!(
                    "the download of {} didn't match the catalogue (it may have been cut short)",
                    f.family
                ));
            }
            let folder = path.parent().ok_or("no folder")?;
            std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
            // (Written beside it and moved into place: a file there is whole.)
            let part = path.with_extension("part");
            std::fs::write(&part, &bytes).map_err(|e| e.to_string())?;
            std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
        }
        out.push(path);
    }
    Ok(out)
}

/// The bundled fonts' list (`fonts/fonts.json`): family and whether it was
/// made for embroidery.
pub fn bundled_list(dir: &Path) -> Vec<(String, bool)> {
    let Ok(text) = std::fs::read_to_string(dir.join("fonts.json")) else {
        return Vec::new();
    };
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    v["fonts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    Some((
                        f["family"].as_str()?.to_string(),
                        f["embroidery"].as_bool().unwrap_or(false),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Where downloads and the catalogue are kept.
fn cache_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("ZIGZAG_FONT_CACHE") {
        return Some(PathBuf::from(d));
    }
    crate::dock::config_file("fonts")
}

/// Every font downloaded before, for lettering from the start (the bundled
/// ones the library finds itself). Called once at launch, before anything
/// is opened (see [`zigzag::svg::use_fonts`]).
pub fn use_kept_fonts() {
    if let Some(d) = cache_dir() {
        zigzag::svg::use_fonts(font_files(&d));
    }
}

/// Bytes at `url`, by curl (as the rest of the app reaches the network).
fn curl_get(url: &str) -> Result<Vec<u8>, String> {
    let mut c = std::process::Command::new("curl");
    crate::no_console(&mut c);
    let out = c
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--proto", "=https,http", "--connect-timeout", "20"])
        .args(["--max-time", "120", "--max-filesize", "40000000"])
        .args(["--user-agent", concat!("ZigZag/", env!("CARGO_PKG_VERSION"))])
        .arg(url)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr)
            .trim()
            .trim_start_matches("curl: ")
            .to_string();
        return Err(if why.is_empty() {
            "no answer".into()
        } else {
            why
        });
    }
    Ok(out.stdout)
}

/// Where a font in the menu stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Have {
    /// On this computer (its own, bundled, or downloaded).
    Here,
    /// In the catalogue, not downloaded yet.
    Download,
    /// Downloading now.
    Getting,
}

/// A font as the menu lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct FontItem {
    pub name: String,
    pub have: Have,
    /// Made for embroidery: marked in the menu.
    pub embroidery: bool,
}

/// What the background work reports.
enum Done {
    Catalogue(Result<Value, String>),
    Family(String, Result<Vec<PathBuf>, String>),
}

/// The catalogue, what's downloading, and what came of it.
pub struct FontStore {
    pub catalogue: Vec<CatFont>,
    /// The catalogue was read (from the server or the kept copy).
    catalogue_read: bool,
    asked_catalogue: bool,
    /// The server answered about the catalogue this launch (or couldn't be
    /// reached): until then a font the kept copy lacks may be a new one.
    catalogue_settled: bool,
    getting: HashSet<String>,
    /// Families that couldn't be downloaded, and why.
    pub failed: HashMap<String, String>,
    /// Families made for embroidery (bundled or in the catalogue).
    embroidery: HashSet<String>,
    tx: mpsc::Sender<Done>,
    rx: mpsc::Receiver<Done>,
    /// Families wanted before the catalogue was read, to look up in it
    /// once it is.
    pub wanted: HashSet<String>,
    /// Per tab (its record), the fonts its document named that weren't
    /// here when it was read: drawn in a stand-in until they are.
    pub stand_in: HashMap<u32, Vec<String>>,
    /// Lettering set in a font that wasn't here yet: measured in a
    /// stand-in, so set again (its size worked out anew) once it's here.
    pub reset: HashSet<(u32, String)>,
    /// What to tell about a font that can't be had.
    pub warning: Option<FontWarning>,
    /// Fonts already warned about (once a launch each).
    warned: HashSet<String>,
    /// Planning waits for fonts on their way until then (not for long: a
    /// slow line shouldn't keep the stitches from showing).
    pub hold_until: Option<std::time::Instant>,
    /// For tests: where files come from instead of the network.
    #[allow(clippy::type_complexity)]
    pub get: Option<std::sync::Arc<dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync>>,
    /// For tests: where downloads are kept instead of the settings folder.
    pub dir: Option<PathBuf>,
}

impl Default for FontStore {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        let embroidery = zigzag::svg::bundled_font_dir()
            .map(|d| bundled_list(&d))
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, e)| *e)
            .map(|(f, _)| f.to_lowercase())
            .collect();
        FontStore {
            catalogue: Vec::new(),
            catalogue_read: false,
            catalogue_settled: false,
            asked_catalogue: false,
            getting: HashSet::new(),
            failed: HashMap::new(),
            embroidery,
            tx,
            rx,
            wanted: HashSet::new(),
            stand_in: HashMap::new(),
            reset: HashSet::new(),
            warning: None,
            warned: HashSet::new(),
            hold_until: None,
            get: None,
            dir: None,
        }
    }
}

/// What came in since last asked.
#[derive(Debug, Default, PartialEq)]
pub struct Arrived {
    /// Families downloaded and added.
    pub fonts: Vec<String>,
    /// Families that couldn't be, with why.
    pub failed: Vec<(String, String)>,
    /// The catalogue came (or was found not to).
    pub catalogue: bool,
}

impl FontStore {
    fn dir(&self) -> Option<PathBuf> {
        self.dir.clone().or_else(cache_dir)
    }

    fn getter(&self) -> std::sync::Arc<dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync> {
        self.get
            .clone()
            .unwrap_or_else(|| std::sync::Arc::new(curl_get))
    }

    /// The catalogue: the kept copy at once, and a fresh one from the
    /// server in the background (once a launch).
    pub fn want_catalogue(&mut self) {
        if !self.catalogue_read
            && let Some(d) = self.dir()
            && let Ok(text) = std::fs::read_to_string(d.join("catalogue.json"))
            && let Ok(v) = serde_json::from_str::<Value>(&text)
        {
            self.set_catalogue(&v);
        }
        if self.asked_catalogue {
            return;
        }
        self.asked_catalogue = true;
        let tx = self.tx.clone();
        let get = self.getter();
        let url = format!("{}/v1/fonts", crate::share::server());
        std::thread::spawn(move || {
            let v = get(&url).and_then(|b| {
                serde_json::from_slice::<Value>(&b).map_err(|_| "unreadable catalogue".into())
            });
            let _ = tx.send(Done::Catalogue(v));
        });
    }

    fn set_catalogue(&mut self, v: &Value) {
        self.catalogue = parse_catalogue(v);
        self.catalogue_read = true;
        for f in &self.catalogue {
            if f.embroidery {
                self.embroidery.insert(f.family.to_lowercase());
            }
        }
    }

    pub fn find(&self, family: &str) -> Option<&CatFont> {
        self.catalogue
            .iter()
            .find(|f| f.family.eq_ignore_ascii_case(family))
    }

    /// Whether what the catalogue lacks is known to be missing: the server
    /// has answered this launch, or couldn't be reached. (The copy kept
    /// from last time may be older than a font a design names.)
    pub fn catalogue_known(&self) -> bool {
        self.catalogue_settled
    }

    /// Download `family` (in the background) unless it's on its way.
    /// False if the catalogue doesn't have it.
    pub fn fetch(&mut self, family: &str) -> bool {
        let Some(f) = self.find(family).cloned() else {
            return false;
        };
        if !self.getting.insert(f.family.clone()) {
            return true;
        }
        self.failed.remove(&f.family);
        let Some(dir) = self.dir() else {
            self.getting.remove(&f.family);
            self.failed
                .insert(f.family.clone(), "there's no settings folder to keep it in".into());
            return true;
        };
        let tx = self.tx.clone();
        let get = self.getter();
        std::thread::spawn(move || {
            let r = fetch_family(&dir, &f, &*get);
            let _ = tx.send(Done::Family(f.family.clone(), r));
        });
        true
    }

    pub fn getting(&self, family: &str) -> bool {
        self.getting.iter().any(|f| f.eq_ignore_ascii_case(family))
    }

    pub fn busy(&self) -> bool {
        !self.getting.is_empty()
    }

    /// Take in what the background work finished: downloaded fonts are
    /// added for lettering, the catalogue is kept.
    pub fn poll(&mut self) -> Arrived {
        let mut out = Arrived::default();
        while let Ok(d) = self.rx.try_recv() {
            match d {
                Done::Catalogue(r) => {
                    out.catalogue = true;
                    self.catalogue_settled = true;
                    match r {
                        Ok(v) if v["fonts"].is_array() => {
                            self.set_catalogue(&v);
                            if let Some(d) = self.dir() {
                                let _ = std::fs::create_dir_all(&d);
                                let _ = std::fs::write(d.join("catalogue.json"), v.to_string());
                            }
                        }
                        // (Offline: the kept copy, if any, stands.)
                        _ => self.catalogue_read = true,
                    }
                }
                Done::Family(family, r) => {
                    self.getting.remove(&family);
                    match r {
                        Ok(files) => {
                            zigzag::svg::add_fonts(&files);
                            out.fonts.push(family);
                        }
                        Err(why) => {
                            self.failed.insert(family.clone(), why.clone());
                            out.failed.push((family, why));
                        }
                    }
                }
            }
        }
        out
    }

    pub fn is_embroidery(&self, family: &str) -> bool {
        self.embroidery.contains(&family.to_lowercase())
    }

    /// The font menu's list: the fonts here, and the catalogue's others
    /// (marked to download), sorted by name.
    pub fn menu(&self, here: &[String]) -> Vec<FontItem> {
        let mut out: Vec<FontItem> = here
            .iter()
            .map(|n| FontItem {
                name: n.clone(),
                have: Have::Here,
                embroidery: self.is_embroidery(n),
            })
            .collect();
        let have: HashSet<String> = here.iter().map(|n| n.to_lowercase()).collect();
        for f in &self.catalogue {
            if !have.contains(&f.family.to_lowercase()) {
                out.push(FontItem {
                    name: f.family.clone(),
                    have: if self.getting(&f.family) {
                        Have::Getting
                    } else {
                        Have::Download
                    },
                    embroidery: f.embroidery || self.is_embroidery(&f.family),
                });
            }
        }
        out.sort_by_key(|i| i.name.to_lowercase());
        out
    }
}

/// A font that can't be had, said in a dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct FontWarning {
    pub family: String,
    pub title: String,
    pub body: String,
    /// A download that failed can be tried again.
    pub retry: bool,
}

/// How long opening a design waits for its fonts before planning with a
/// stand-in (planned again when they come).
const HOLD: std::time::Duration = std::time::Duration::from_secs(8);

impl crate::app::Studio {
    /// The fonts the document in front names that aren't here: downloaded
    /// from the catalogue if it has them, else said so. Called when a
    /// document is opened and when its tab comes to the front.
    pub(crate) fn check_document_fonts(&mut self) {
        let rec = self.session.rec;
        // (A design with no text asks nothing of the fonts: the computer's
        // are still being read in the background as the first one opens.)
        let texts = self.doc.lettering_texts();
        if texts.is_empty() {
            self.font_store.stand_in.remove(&rec);
            return;
        }
        let mut named: Vec<String> = Vec::new();
        for (f, _) in &texts {
            if !named.iter().any(|n| n.eq_ignore_ascii_case(f)) {
                named.push(f.clone());
            }
        }
        let here = self.fonts_here();
        // Text with a character its font hasn't got: said.
        self.warn_missing_characters(&texts);
        let is_here = |f: &str| here.iter().any(|h| h.eq_ignore_ascii_case(f));
        // Fonts that arrived while this tab was at the back.
        if self
            .font_store
            .stand_in
            .get(&rec)
            .is_some_and(|was| was.iter().any(|f| is_here(f)))
        {
            self.reload_document_fonts();
        }
        let missing: Vec<String> = named.into_iter().filter(|f| !is_here(f)).collect();
        if missing.is_empty() {
            self.font_store.stand_in.remove(&rec);
            return;
        }
        self.font_store.stand_in.insert(rec, missing.clone());
        for f in missing {
            self.want_font(&f, true);
        }
    }

    /// Lettering was set in `font` (picked in a menu, or by an agent): if
    /// it isn't here, it's downloaded, and the lettering set again in it
    /// when it comes.
    pub(crate) fn lettering_font_used(&mut self, font: &str) {
        let here = self.fonts_here();
        if here.iter().any(|h| h.eq_ignore_ascii_case(font)) {
            // (Here, but has it every character that was typed?)
            let texts = self.doc.lettering_texts();
            self.warn_missing_characters(&texts);
            return;
        }
        let rec = self.session.rec;
        let list = self.font_store.stand_in.entry(rec).or_default();
        if !list.iter().any(|f| f.eq_ignore_ascii_case(font)) {
            list.push(font.to_string());
        }
        self.font_store.reset.insert((rec, font.to_lowercase()));
        self.want_font(font, false);
    }

    /// Start downloading `font` if it isn't here (picked in the Add Text
    /// window, before there's lettering set in it).
    pub(crate) fn fetch_font_if_missing(&mut self, font: &str) {
        if !self.fonts_here().iter().any(|h| h.eq_ignore_ascii_case(font)) {
            self.font_store.want_catalogue();
            if self.font_store.fetch(font) {
                self.status = format!("Downloading the font {font}…");
            }
        }
    }

    /// Download `font` from the catalogue; or, when it isn't there (or
    /// can't be read), say so. `opening`: for a design just opened, whose
    /// planning waits a moment for it.
    fn want_font(&mut self, font: &str, opening: bool) {
        self.font_store.want_catalogue();
        if self.font_store.fetch(font) {
            if opening {
                self.font_store.hold_until = Some(std::time::Instant::now() + HOLD);
            }
            self.status = format!("Downloading the font {font}…");
        } else if self.font_store.catalogue_known() {
            self.warn_font_missing(font);
        } else {
            // (Looked up once the catalogue comes.)
            self.font_store.wanted.insert(font.to_string());
            if opening {
                self.font_store.hold_until = Some(std::time::Instant::now() + HOLD);
            }
        }
    }

    /// Say so in the status line (once a launch for each font and its
    /// characters) where text has a character its font lacks: it's drawn,
    /// and would sew, as the font's missing-character box (in a single-line
    /// font, usually nothing at all).
    /// `texts`: [`zigzag::document::Document::lettering_texts`].
    fn warn_missing_characters(&mut self, texts: &[(String, String)]) {
        for (font, text) in texts {
            let lacks = zigzag::svg::font_lacks(font, text);
            if lacks.is_empty() {
                continue;
            }
            let chars: String = lacks.iter().collect();
            if !self
                .font_store
                .warned
                .insert(format!("{}\u{0}{chars}", font.to_lowercase()))
            {
                continue;
            }
            let list = lacks
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(" ");
            self.status = format!("{font} has no {list}: it's left blank, or as the font's empty box.");
        }
    }

    fn warn_font_missing(&mut self, font: &str) {
        if !self.font_store.warned.insert(font.to_lowercase()) {
            return;
        }
        self.status = format!("The font {font} isn't on this computer: a stand-in is shown.");
        self.font_store.warning = Some(FontWarning {
            family: font.to_string(),
            title: format!("{font} isn't on this computer"),
            body: format!(
                "This design's lettering is set in {font}, which isn't installed here and \
                 isn't one ZigZag can download. It's shown, and would sew, in a stand-in \
                 font. Install {font} and open the design again to see it as it was made."
            ),
            retry: false,
        });
    }

    /// Whether planning waits for fonts the document in front is getting.
    pub(crate) fn fonts_hold_plan(&self) -> bool {
        self.font_store
            .hold_until
            .is_some_and(|t| std::time::Instant::now() < t)
            && self.font_store.stand_in.contains_key(&self.session.rec)
            && (self.font_store.busy() || !self.font_store.wanted.is_empty())
    }

    /// Read the document in front again now that fonts it names are here,
    /// and set again what was set in them while they weren't (see
    /// [`FontStore::reset`]). Planned again on the next frame.
    fn reload_document_fonts(&mut self) {
        let rec = self.session.rec;
        let here = self.fonts_here();
        let is_here = |f: &str| here.iter().any(|h| h.eq_ignore_ascii_case(f));
        if let Err(e) = self.doc.reload_fonts() {
            self.status = format!("Couldn't redraw the lettering: {e:#}");
            return;
        }
        let resets: Vec<String> = self
            .font_store
            .reset
            .iter()
            .filter(|(r, f)| *r == rec && is_here(f))
            .map(|(_, f)| f.clone())
            .collect();
        let mut changed = false;
        if !resets.is_empty() {
            for i in 0..self.doc.shapes.len() {
                let Some(spec) = self.doc.lettering(i) else {
                    continue;
                };
                if resets.iter().any(|f| f.eq_ignore_ascii_case(&spec.font)) {
                    let before = self.doc.to_svg().ok();
                    if self.doc.set_lettering(i, &spec).is_ok() && self.doc.to_svg().ok() != before {
                        changed = true;
                    }
                }
            }
            self.font_store
                .reset
                .retain(|(r, f)| !(*r == rec && resets.contains(f)));
        }
        if changed {
            self.modified = true;
        }
        let still: Vec<String> = self
            .font_store
            .stand_in
            .get(&rec)
            .map(|v| v.iter().filter(|f| !is_here(f)).cloned().collect())
            .unwrap_or_default();
        if still.is_empty() {
            self.font_store.stand_in.remove(&rec);
        } else {
            self.font_store.stand_in.insert(rec, still);
        }
        self.stable = self.doc.clone();
        self.dirty = true;
    }

    /// Each frame: take in downloaded fonts and the catalogue, and redraw
    /// the lettering that was waiting on them.
    pub(crate) fn tick_fonts(&mut self, ctx: &eframe::egui::Context) {
        let arrived = self.font_store.poll();
        let any = arrived != Arrived::default();
        if arrived.catalogue {
            let wanted: Vec<String> = self.font_store.wanted.drain().collect();
            for f in wanted {
                self.want_font(&f, false);
            }
        }
        let rec = self.session.rec;
        let waiting_here = |s: &Self, f: &str| {
            s.font_store
                .stand_in
                .get(&rec)
                .is_some_and(|v| v.iter().any(|w| w.eq_ignore_ascii_case(f)))
        };
        if arrived.fonts.iter().any(|f| waiting_here(self, f)) {
            self.reload_document_fonts();
            self.status = format!("Downloaded {}.", arrived.fonts.join(", "));
        }
        // (Only of a font the lettering still names: one tried on the way
        // to another is nobody's loss.)
        let named = if arrived.failed.is_empty() {
            Vec::new()
        } else {
            self.doc.lettering_fonts()
        };
        for (f, why) in arrived.failed {
            if waiting_here(self, &f)
                && named.iter().any(|n| n.eq_ignore_ascii_case(&f))
                && self.font_store.warned.insert(f.to_lowercase())
            {
                self.status = format!("Couldn't download the font {f}: a stand-in is shown.");
                self.font_store.warning = Some(FontWarning {
                    family: f.clone(),
                    title: format!("Couldn't download {f}"),
                    body: format!(
                        "This lettering is set in {f}, which ZigZag couldn't download ({why}). \
                         It's shown, and would sew, in a stand-in font until {f} is here."
                    ),
                    retry: true,
                });
            }
        }
        if self.font_store.busy() || !self.font_store.wanted.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        // (What came in shows at once, a dialog included, which egui lays
        // out on its first frame and draws on the next: nothing else may
        // wake the window.)
        if any {
            ctx.request_repaint();
        }
    }

    /// The dialog about a font that can't be had.
    pub(crate) fn font_warning_window(&mut self, ctx: &eframe::egui::Context) {
        use crate::dialog;
        use eframe::egui;
        let Some(w) = self.font_store.warning.clone() else {
            return;
        };
        let (mut close, mut retry) = (false, false);
        dialog::show(ctx, egui::Id::unique("font-warning"), dialog::Place::Top, |ui| {
            dialog::heading(ui, &w.title);
            dialog::body(ui, &w.body);
            dialog::buttons(ui, |ui| {
                if w.retry {
                    retry = dialog::primary(ui, "Try Again", true).clicked();
                    close = dialog::secondary(ui, "OK").clicked();
                } else {
                    close = dialog::primary(ui, "OK", true).clicked();
                }
                close |= ui.input(|i| {
                    i.key_pressed(egui::Key::Escape)
                        || (!w.retry && i.key_pressed(egui::Key::Enter))
                });
            });
        });
        if retry {
            self.font_store.warning = None;
            self.font_store.warned.remove(&w.family.to_lowercase());
            self.want_font(&w.family, false);
        } else if close {
            self.font_store.warning = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sha256_matches_the_standard_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // (Over more than one block.)
        assert_eq!(
            sha256_hex(&[b'a'; 1000]),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }

    fn catalogue() -> Value {
        json!({"fonts": [
            {"family": "Lato", "slug": "lato", "category": "sans-serif", "license": "OFL-1.1",
             "variants": [
                {"style": "normal", "weight": 100, "url": "https://x/lato/1/100.ttf", "size": 1, "sha256": "a".repeat(64)},
                {"style": "normal", "weight": 400, "url": "https://x/lato/1/400.ttf", "size": 1, "sha256": "b".repeat(64)},
                {"style": "normal", "weight": 700, "url": "https://x/lato/1/700.ttf", "size": 1, "sha256": "c".repeat(64)},
                {"style": "normal", "weight": 900, "url": "https://x/lato/1/900.ttf", "size": 1, "sha256": "d".repeat(64)},
                {"style": "italic", "weight": 400, "url": "https://x/lato/1/400i.ttf", "size": 1, "sha256": "e".repeat(64)}
             ]},
            {"family": "Inter", "slug": "inter", "category": "sans-serif", "license": "OFL-1.1",
             "variants": [
                {"style": "normal", "weight": 400, "weights": [100, 900], "url": "https://x/inter/1/normal.ttf", "size": 1, "sha256": "f".repeat(64)}
             ]},
            {"family": "Bad", "slug": "../up", "variants": [
                {"style": "normal", "weight": 400, "url": "https://x/a.ttf", "sha256": "0".repeat(64)}]},
            {"family": "Plain", "slug": "plain", "variants": [
                {"style": "normal", "weight": 400, "url": "http://elsewhere/a.ttf", "sha256": "0".repeat(64)}]},
            {"family": "Run Script", "slug": "run-script", "category": "single-line", "variants": [
                {"style": "normal", "weight": 400, "url": "https://x/r.ttf", "sha256": "1".repeat(64)}]}
        ]})
    }

    #[test]
    fn the_catalogue_is_read_and_unsafe_entries_left_out() {
        let c = parse_catalogue(&catalogue());
        let names: Vec<&str> = c.iter().map(|f| f.family.as_str()).collect();
        // (A slug that would leave the folder, a file not over https.)
        assert_eq!(names, ["Lato", "Inter", "Run Script"]);
        assert!(c[2].embroidery && !c[0].embroidery);
        assert_eq!(c[1].variants[0].weights, Some((100, 900)));
    }

    #[test]
    fn only_the_regular_and_bold_files_are_fetched() {
        let c = parse_catalogue(&catalogue());
        let urls = |f: &CatFont| -> Vec<String> {
            files_to_fetch(f).iter().map(|v| v.url.clone()).collect()
        };
        // Lato's 400 and 700, and its italic (the nearest to bold italic
        // is the same file).
        assert_eq!(
            urls(&c[0]),
            [
                "https://x/lato/1/400.ttf",
                "https://x/lato/1/700.ttf",
                "https://x/lato/1/400i.ttf"
            ]
        );
        // A variable file covers both weights.
        assert_eq!(urls(&c[1]), ["https://x/inter/1/normal.ttf"]);
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zigzag-fonts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_download_is_kept_only_when_its_hash_matches() {
        let dir = scratch("hash");
        let body = b"font bytes".to_vec();
        let font = CatFont {
            family: "Test".into(),
            slug: "test".into(),
            category: String::new(),
            license: String::new(),
            embroidery: false,
            variants: vec![Variant {
                style: "normal".into(),
                weight: 400,
                weights: None,
                url: "https://x/test/1/400.ttf".into(),
                size: 10,
                sha256: sha256_hex(&body),
            }],
        };
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let good = |_: &str| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(body.clone())
        };
        let files = fetch_family(&dir, &font, &good).unwrap();
        assert_eq!(std::fs::read(&files[0]).unwrap(), body);
        // Kept: the second time nothing is downloaded.
        assert_eq!(fetch_family(&dir, &font, &good).unwrap(), files);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        // A file that doesn't match is refused and not kept.
        let _ = std::fs::remove_dir_all(&dir);
        let bad = |_: &str| Ok(b"something else".to_vec());
        assert!(fetch_family(&dir, &font, &bad).is_err());
        assert!(font_files(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A font no computer has: Hershey Sans (bundled) under another
    /// name, the name changed in place in its name table.
    fn renamed_font(to: &str) -> Vec<u8> {
        // (As long as the name it replaces, so nothing moves.)
        assert_eq!(to.len(), "Hershey Sans".len());
        let short = to.replace(' ', "");
        let mut b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/fonts/hershey-sans.ttf"))
            .unwrap();
        let swap = |b: &mut Vec<u8>, from: &[u8], to: &[u8]| {
            let mut n = 0;
            let mut i = 0;
            while i + from.len() <= b.len() {
                if &b[i..i + from.len()] == from {
                    b[i..i + from.len()].copy_from_slice(to);
                    n += 1;
                }
                i += 1;
            }
            n
        };
        let utf16 = |t: &str| -> Vec<u8> { t.encode_utf16().flat_map(|u| u.to_be_bytes()).collect() };
        let mut n = swap(&mut b, b"Hershey Sans", to.as_bytes());
        n += swap(&mut b, b"HersheySans", short.as_bytes());
        n += swap(&mut b, &utf16("Hershey Sans"), &utf16(to));
        n += swap(&mut b, &utf16("HersheySans"), &utf16(&short));
        assert!(n >= 4);
        b
    }

    fn studio_with(dir: &Path, files: HashMap<String, Vec<u8>>) -> (crate::app::Studio, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut s = crate::app::Studio::with_file(None, None);
        s.font_store = FontStore {
            dir: Some(dir.to_path_buf()),
            get: Some({
                let calls = calls.clone();
                std::sync::Arc::new(move |url: &str| {
                    if !url.ends_with("/v1/fonts") {
                        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                    files
                        .iter()
                        .find(|(k, _)| url.ends_with(k.as_str()))
                        .map(|(_, v)| v.clone())
                        .ok_or_else(|| "404".to_string())
                })
            }),
            ..Default::default()
        };
        (s, calls)
    }

    fn wait_for_fonts(s: &mut crate::app::Studio) {
        let ctx = eframe::egui::Context::default();
        for _ in 0..500 {
            s.tick_fonts(&ctx);
            if !s.font_store.busy() && s.font_store.wanted.is_empty() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the fonts never came");
    }

    #[test]
    fn a_design_in_a_font_not_here_gets_it_and_is_drawn_in_it() {
        let dir = scratch("open");
        let font = renamed_font("Hershey Tset");
        let cat = json!({"fonts": [{
            "family": "Hershey Tset", "slug": "hershey-tset", "category": "single-line",
            "license": "Hershey", "variants": [{"style": "normal", "weight": 400,
                "url": "https://x/v1/fonts/hershey-tset/h1/400.ttf",
                "size": font.len(), "sha256": sha256_hex(&font)}]}]})
        .to_string();
        let files = HashMap::from([
            ("/v1/fonts".to_string(), cat.into_bytes()),
            ("/400.ttf".to_string(), font.clone()),
        ]);
        let (mut s, calls) = studio_with(&dir, files);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="40mm" viewBox="0 0 80 40"><text id="hello_fill" x="4" y="28" font-family="Hershey Tset" font-size="21" fill="#000000">Hi</text></svg>"##;
        let doc = zigzag::document::Document::from_svg(svg, "t", None).unwrap();
        // Not here: drawn in a stand-in (an ordinary font's areas).
        assert!(matches!(doc.shapes[0].geometry, zigzag::svg::Geometry::Fill { .. }));
        s.set_document(doc);
        // Planning waits for it a moment.
        assert!(s.fonts_hold_plan());
        wait_for_fonts(&mut s);
        // Downloaded once, checked, kept, and the design drawn in it: a
        // single-line font's lines.
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(matches!(s.doc.shapes[0].geometry, zigzag::svg::Geometry::Stroke { .. }));
        assert!(s.dirty, "planned again");
        assert!(!s.modified, "opening it changed nothing");
        assert!(s.font_store.warning.is_none());
        let kept = font_files(&dir);
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read(&kept[0]).unwrap(), font);
        // The catalogue is kept for offline, and the menu has the font as
        // here now, marked as made for embroidery.
        assert!(dir.join("catalogue.json").is_file());
        let menu = s.fonts();
        let item = menu.iter().find(|i| i.name == "Hershey Tset").unwrap();
        assert_eq!((item.have, item.embroidery), (Have::Here, true));
        // Opened again: nothing more to download.
        let doc = zigzag::document::Document::from_svg(svg, "t", None).unwrap();
        s.set_document(doc);
        wait_for_fonts(&mut s);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_set_in_a_font_still_downloading_is_set_again_once_it_comes() {
        let dir = scratch("pick");
        let font = renamed_font("Hershey Pikk");
        let cat = json!({"fonts": [{
            "family": "Hershey Pikk", "slug": "hershey-pikk", "category": "single-line",
            "variants": [{"style": "normal", "weight": 400,
                "url": "https://x/v1/fonts/hershey-pikk/h1/400.ttf",
                "size": font.len(), "sha256": sha256_hex(&font)}]}]})
        .to_string();
        let files = HashMap::from([
            ("/v1/fonts".to_string(), cat.into_bytes()),
            ("/400.ttf".to_string(), font),
        ]);
        let (mut s, calls) = studio_with(&dir, files);
        let spec = zigzag::document::TextSpec {
            // (A capital alone: the size set is the capitals' height.)
            text: "H".into(),
            font: "Hershey Pikk".into(),
            kind: zigzag::params::Kind::Run,
            ..Default::default()
        };
        let i = s.add_lettering(&spec, (0.0, 0.0)).unwrap();
        let uid = s.doc.shapes[i].uid;
        wait_for_fonts(&mut s);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let i = s.doc.shapes.iter().position(|x| x.uid == uid).unwrap();
        assert!(matches!(s.doc.shapes[i].geometry, zigzag::svg::Geometry::Stroke { .. }));
        // Its size worked out in the font itself: capitals as tall as set.
        let b = zigzag::svg::shapes_bounds(&s.doc.shapes[i..=i]);
        assert!((b.height() / 10.0 - spec.size_mm).abs() < 0.15, "{}", b.height());
        assert!(s.font_store.reset.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn satin_text_picked_into_a_single_line_font_to_download_is_sewn_as_a_run() {
        let dir = scratch("pick-run");
        let font = renamed_font("Hershey Ruun");
        let cat = json!({"fonts": [{
            "family": "Hershey Ruun", "slug": "hershey-ruun", "category": "single-line",
            "embroidery": true,
            "variants": [{"style": "normal", "weight": 400,
                "url": "https://x/v1/fonts/hershey-ruun/h1/400.ttf",
                "size": font.len(), "sha256": sha256_hex(&font)}]}]})
        .to_string();
        let files = HashMap::from([
            ("/v1/fonts".to_string(), cat.into_bytes()),
            ("/400.ttf".to_string(), font),
        ]);
        let (mut s, _) = studio_with(&dir, files);
        let i = s.add_lettering(&zigzag::document::TextSpec::default(), (0.0, 0.0)).unwrap();
        let uid = s.doc.shapes[i].uid;
        assert_eq!(s.doc.shapes[i].params.kind, Some(zigzag::params::Kind::Satin));
        // The menu, once the catalogue is in (it lists the font): picked
        // from it.
        let ctx = eframe::egui::Context::default();
        let mut menu = s.fonts();
        for _ in 0..500 {
            if menu.iter().any(|f| f.name == "Hershey Ruun") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            s.tick_fonts(&ctx);
            menu = s.fonts();
        }
        assert!(menu.iter().any(|f| f.name == "Hershey Ruun" && f.embroidery));
        let mut spec = s.doc.lettering(i).unwrap();
        spec.font = "Hershey Ruun".into();
        crate::lettering::picked_font(&mut spec, &menu);
        s.update_lettering(i, &spec).unwrap();
        wait_for_fonts(&mut s);
        // Drawn in it once it came, and still a run (not the satin it
        // was, read back from the file as it was before the pick).
        let i = s.doc.shapes.iter().position(|x| x.uid == uid).unwrap();
        assert!(s.doc.shapes[i].is_single_line());
        assert_eq!(s.doc.shapes[i].params.kind, Some(zigzag::params::Kind::Run));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_font_newer_than_the_kept_catalogue_is_waited_for_not_warned_about() {
        let dir = scratch("newer");
        let font = renamed_font("Hershey Nuuw");
        // The copy kept from last time doesn't have it; the server does.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("catalogue.json"), r#"{"fonts": []}"#).unwrap();
        let cat = json!({"fonts": [{
            "family": "Hershey Nuuw", "slug": "hershey-nuuw", "category": "single-line",
            "variants": [{"style": "normal", "weight": 400,
                "url": "https://x/v1/fonts/hershey-nuuw/h1/400.ttf",
                "size": font.len(), "sha256": sha256_hex(&font)}]}]})
        .to_string();
        let files = HashMap::from([
            ("/v1/fonts".to_string(), cat.into_bytes()),
            ("/400.ttf".to_string(), font),
        ]);
        let (mut s, calls) = studio_with(&dir, files);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="40mm" viewBox="0 0 80 40"><text id="hello_fill" x="4" y="28" font-family="Hershey Nuuw" font-size="21">Hi</text></svg>"##;
        s.set_document(zigzag::document::Document::from_svg(svg, "t", None).unwrap());
        assert!(s.font_store.warning.is_none(), "warned before the server answered");
        wait_for_fonts(&mut s);
        assert!(s.font_store.warning.is_none());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(matches!(s.doc.shapes[0].geometry, zigzag::svg::Geometry::Stroke { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_font_nobody_has_is_said_so() {
        let dir = scratch("nowhere");
        let files = HashMap::from([("/v1/fonts".to_string(), b"{\"fonts\": []}".to_vec())]);
        let (mut s, _) = studio_with(&dir, files);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="40mm" viewBox="0 0 80 40"><text id="hello_fill" x="4" y="28" font-family="'No Such Font Anywhere', serif" font-size="21">Hi</text></svg>"##;
        s.set_document(zigzag::document::Document::from_svg(svg, "t", None).unwrap());
        wait_for_fonts(&mut s);
        let w = s.font_store.warning.clone().expect("a warning");
        assert_eq!(w.family, "No Such Font Anywhere");
        assert!(!w.retry);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_with_a_character_its_font_lacks_is_said_so_once() {
        let dir = scratch("lacks");
        let files = HashMap::from([("/v1/fonts".to_string(), b"{\"fonts\": []}".to_vec())]);
        let (mut s, _) = studio_with(&dir, files);
        let svg = |text: &str| {
            format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="40mm" viewBox="0 0 80 40"><text id="hello_fill" x="4" y="28" font-family="Hershey Sans" font-size="21">{text}</text></svg>"##)
        };
        // Every character there: nothing to say.
        s.set_document(zigzag::document::Document::from_svg(&svg("Mom's"), "t", None).unwrap());
        assert!(s.font_store.warning.is_none());
        // A curly apostrophe Hershey Sans hasn't got: said in the status
        // line, no dialog, and the letters stay Hershey's lines.
        s.set_document(zigzag::document::Document::from_svg(&svg("Mom\u{2019}s"), "t", None).unwrap());
        assert!(s.font_store.warning.is_none());
        assert_eq!(s.status, "Hershey Sans has no \u{2019}: it's left blank, or as the font's empty box.");
        assert!(s.doc.shapes[0].is_single_line());
        // Once: not again for the same font and character.
        s.status.clear();
        s.set_document(zigzag::document::Document::from_svg(&svg("Dad\u{2019}s"), "t", None).unwrap());
        assert!(s.status.is_empty(), "{}", s.status);
        // A design with no text asks nothing of the fonts.
        let plain = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="40mm" viewBox="0 0 40 40"><rect x="5" y="5" width="30" height="30" fill="#c33"/></svg>"##;
        let doc = zigzag::document::Document::from_svg(plain, "t", None).unwrap();
        assert!(doc.lettering_texts().is_empty());
        s.set_document(doc);
        assert!(s.font_store.warning.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_download_that_fails_is_said_so_and_can_be_tried_again() {
        let dir = scratch("fails");
        let cat = json!({"fonts": [{"family": "Gone Font", "slug": "gone-font", "variants": [
            {"style": "normal", "weight": 400, "url": "https://x/gone.ttf", "sha256": "0".repeat(64)}]}]});
        let files = HashMap::from([("/v1/fonts".to_string(), cat.to_string().into_bytes())]);
        let (mut s, _) = studio_with(&dir, files);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="40mm" viewBox="0 0 80 40"><text id="hello_fill" x="4" y="28" font-family="Gone Font" font-size="21">Hi</text></svg>"##;
        s.set_document(zigzag::document::Document::from_svg(svg, "t", None).unwrap());
        wait_for_fonts(&mut s);
        let w = s.font_store.warning.clone().expect("a warning");
        assert_eq!(w.family, "Gone Font");
        assert!(w.retry);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_menu_lists_the_catalogue_with_what_is_not_here_marked() {
        let mut store = FontStore {
            dir: Some(scratch("menu")),
            ..Default::default()
        };
        store.set_catalogue(&catalogue());
        let menu = store.menu(&["Arial".to_string(), "inter".to_string()]);
        let rows: Vec<(&str, Have, bool)> = menu
            .iter()
            .map(|i| (i.name.as_str(), i.have, i.embroidery))
            .collect();
        assert_eq!(
            rows,
            [
                ("Arial", Have::Here, false),
                // (Here already, by another spelling: listed once.)
                ("inter", Have::Here, false),
                ("Lato", Have::Download, false),
                ("Run Script", Have::Download, true),
            ]
        );
    }
}
