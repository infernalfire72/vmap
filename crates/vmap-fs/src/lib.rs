//! Read-only access to a game's files.
//!
//! A game can be opened from three kinds of source:
//!
//! * a CTRMap project (`.cmproj`): an extracted base ROM plus an overlay
//!   folder holding the files the project has changed,
//! * an extracted ROM folder (`header.bin`, `arm9.bin`, `data/`, `overlay/`),
//! * a `.nds` ROM image.
//!
//! All three are exposed through the same virtual paths that the extracted
//! layout uses, so the rest of the tool never cares which one is open.
//! Nothing in this crate writes to the game or the project.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc as Rc, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use vmap_formats::gen5::{arc_path, Arc, Game};
use vmap_formats::nds::{Narc, OverlayEntry, RomHeader, RomIndex};

/// Bytes of one file, cheap to clone.
pub type Bytes = Rc<Vec<u8>>;

enum Base {
    Dir(PathBuf),
    Rom { map: memmap2::Mmap, index: RomIndex },
}

struct CachedArc {
    data: Bytes,
    /// `None` when the file is not a NARC (some "archives" are flat tables).
    narc: Option<Narc>,
}

/// What kind of source a [`GameFs`] was opened from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceKind {
    CtrmapProject,
    ExtractedFolder,
    RomImage,
}

pub struct GameFs {
    base: Base,
    /// Project overlay root (the folder that contains `data/`).
    overlay: Option<PathBuf>,
    kind: SourceKind,
    /// Path the user opened.
    opened: PathBuf,
    header: RomHeader,
    game: Game,
    overlays: Vec<OverlayEntry>,
    cache: Mutex<HashMap<String, Rc<CachedArc>>>,
}

impl GameFs {
    /// Opens a `.cmproj`, a folder containing one, an extracted ROM folder or
    /// a `.nds` image.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            bail!("{} does not exist", path.display());
        }
        if path.is_dir() {
            if let Some(proj) = find_cmproj(path) {
                return Self::open_project(&proj);
            }
            if path.join("header.bin").is_file() {
                return Self::from_parts(Base::Dir(path.to_path_buf()), None, SourceKind::ExtractedFolder, path);
            }
            bail!(
                "{} is neither a CTRMap project folder nor an extracted ROM (no .cmproj and no header.bin)",
                path.display()
            );
        }
        match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
            Some("cmproj") => Self::open_project(path),
            _ => {
                let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
                // SAFETY: the map is read-only and the tool never writes to the ROM.
                let map = unsafe { memmap2::Mmap::map(&file) }.context("mapping ROM image")?;
                let index = RomIndex::parse(&map).context("reading ROM file tables")?;
                Self::from_parts(Base::Rom { map, index }, None, SourceKind::RomImage, path)
            }
        }
    }

    fn open_project(proj: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(proj).with_context(|| format!("reading {}", proj.display()))?;
        let dir = proj.parent().unwrap_or(Path::new("."));
        let key = |name: &str| -> Option<String> {
            text.lines().find_map(|l| {
                let v = l.strip_prefix(name)?.trim_start().strip_prefix(':')?.trim();
                Some(v.trim_matches('"').to_string())
            })
        };
        let base_str = key("VFSBase").ok_or_else(|| anyhow!("project file has no VFSBase entry"))?;
        let overlay_str = key("VFSOverlay").unwrap_or_else(|| "vfs".into());

        let mut base = PathBuf::from(&base_str);
        if !base.exists() {
            base = dir.join(&base_str);
        }
        if !base.exists() {
            bail!("the project's base game folder was not found: {base_str}");
        }
        let overlay = dir.join(&overlay_str);
        let overlay = overlay.is_dir().then_some(overlay);

        if base.is_dir() {
            Self::from_parts(Base::Dir(base), overlay, SourceKind::CtrmapProject, proj)
        } else {
            let file = std::fs::File::open(&base)?;
            // SAFETY: read-only mapping, see above.
            let map = unsafe { memmap2::Mmap::map(&file) }?;
            let index = RomIndex::parse(&map)?;
            Self::from_parts(Base::Rom { map, index }, overlay, SourceKind::CtrmapProject, proj)
        }
    }

    fn from_parts(base: Base, overlay: Option<PathBuf>, kind: SourceKind, opened: &Path) -> Result<Self> {
        let mut fs = Self {
            base,
            overlay,
            kind,
            opened: opened.to_path_buf(),
            header: RomHeader::parse(&[0u8; RomHeader::SIZE])?,
            game: Game::Black,
            overlays: Vec::new(),
            cache: Mutex::new(HashMap::new()),
        };
        let header_bytes = fs.read("header.bin").ok_or_else(|| anyhow!("the game has no ROM header"))?;
        fs.header = RomHeader::parse(&header_bytes)?;
        fs.game = Game::from_game_code(&fs.header.game_code).ok_or_else(|| {
            anyhow!(
                "game code '{}' is not a Generation V Pokemon game (expected IRB, IRA, IRE or IRD)",
                fs.header.game_code
            )
        })?;
        if let Some(y9) = fs.read("y9.bin") {
            fs.overlays = OverlayEntry::parse_table(&y9).unwrap_or_default();
        }
        Ok(fs)
    }

    pub fn game(&self) -> Game {
        self.game
    }

    pub fn header(&self) -> &RomHeader {
        &self.header
    }

    pub fn kind(&self) -> &SourceKind {
        &self.kind
    }

    pub fn opened_path(&self) -> &Path {
        &self.opened
    }

    pub fn overlay_dir(&self) -> Option<&Path> {
        self.overlay.as_deref()
    }

    /// The extracted ROM folder the game is read from, unless it is read
    /// from a ROM image.
    pub fn base_dir(&self) -> Option<&Path> {
        match &self.base {
            Base::Dir(dir) => Some(dir),
            Base::Rom { .. } => None,
        }
    }

    pub fn overlay_table(&self) -> &[OverlayEntry] {
        &self.overlays
    }

    /// Drops cached archives so the next read sees the files as they are on
    /// disk now.
    pub fn reload(&self) {
        self.cache.lock().unwrap().clear();
    }

    /// Reads a file by virtual path, e.g. `data/a/0/1/3` or `arm9.bin`.
    /// The project overlay wins over the base game.
    pub fn read(&self, path: &str) -> Option<Bytes> {
        if let Some(ov) = &self.overlay {
            let p = ov.join(path);
            if p.is_file() {
                if let Ok(b) = std::fs::read(&p) {
                    return Some(Rc::new(b));
                }
            }
        }
        self.read_base(path)
    }

    fn read_base(&self, path: &str) -> Option<Bytes> {
        match &self.base {
            Base::Dir(dir) => std::fs::read(dir.join(path)).ok().map(Rc::new),
            Base::Rom { map, index } => {
                let h = &index.header;
                let range = if let Some(p) = path.strip_prefix("data/") {
                    index.files.get(&p.to_ascii_lowercase())?.clone()
                } else {
                    match path {
                        "header.bin" => 0..0x4000.min(map.len()),
                        "arm9.bin" => {
                            let s = h.arm9_offset as usize;
                            s..s + h.arm9_size as usize
                        }
                        "y9.bin" => {
                            let s = h.arm9_overlay_offset as usize;
                            s..s + h.arm9_overlay_size as usize
                        }
                        _ => {
                            let n: usize =
                                path.strip_prefix("overlay/overlay_")?.strip_suffix(".bin")?.parse().ok()?;
                            let file_id = index.overlays.get(n)?.file_id as usize;
                            index.file_ranges.get(file_id)?.clone()
                        }
                    }
                };
                map.get(range).map(|b| Rc::new(b.to_vec()))
            }
        }
    }

    /// Decompressed ARM9 overlay code.
    pub fn overlay_code(&self, id: usize) -> Option<Vec<u8>> {
        let raw = self.read(&format!("overlay/overlay_{id:04}.bin"))?;
        let compressed = self.overlays.get(id).map(|o| o.compressed).unwrap_or(false);
        Some(if compressed { vmap_formats::nds::compress::blz_decompress(&raw) } else { raw.to_vec() })
    }

    fn cached(&self, data_path: &str) -> Option<Rc<CachedArc>> {
        if let Some(c) = self.cache.lock().unwrap().get(data_path) {
            return Some(c.clone());
        }
        let data = self.read(&format!("data/{data_path}"))?;
        let narc = if Narc::is_narc(&data) { Narc::parse(&data).ok() } else { None };
        let entry = Rc::new(CachedArc { data, narc });
        self.cache.lock().unwrap().insert(data_path.to_string(), entry.clone());
        Some(entry)
    }

    fn overlay_member_dir(&self, data_path: &str) -> Option<PathBuf> {
        let dir = self.overlay.as_ref()?.join("data").join(data_path);
        dir.is_dir().then_some(dir)
    }

    /// Number of members in an archive, counting members the project added.
    pub fn arc_len_path(&self, data_path: &str) -> usize {
        let base = self.cached(data_path).and_then(|c| c.narc.as_ref().map(|n| n.len())).unwrap_or(0);
        let mut len = base;
        if let Some(dir) = self.overlay_member_dir(data_path) {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    if let Some(n) = e.file_name().to_str().and_then(|s| s.parse::<usize>().ok()) {
                        len = len.max(n + 1);
                    }
                }
            }
        }
        len
    }

    /// Reads one archive member. A project override wins over the base game.
    pub fn arc_file_path(&self, data_path: &str, index: usize) -> Option<Bytes> {
        if let Some(dir) = self.overlay_member_dir(data_path) {
            let p = dir.join(index.to_string());
            if p.is_file() {
                if let Ok(b) = std::fs::read(&p) {
                    return Some(Rc::new(b));
                }
            }
        }
        let c = self.cached(data_path)?;
        let narc = c.narc.as_ref()?;
        narc.get(&c.data, index).map(|b| Rc::new(b.to_vec()))
    }

    /// True if the project replaces this archive member.
    pub fn is_overridden_path(&self, data_path: &str, index: usize) -> bool {
        self.overlay_member_dir(data_path).is_some_and(|d| d.join(index.to_string()).is_file())
    }

    /// Indices of all members the project replaces or adds in an archive.
    pub fn overridden_members(&self, data_path: &str) -> Vec<usize> {
        let mut out = Vec::new();
        if let Some(dir) = self.overlay_member_dir(data_path) {
            if let Ok(rd) = std::fs::read_dir(dir) {
                out.extend(rd.flatten().filter_map(|e| e.file_name().to_str()?.parse::<usize>().ok()));
            }
        }
        out.sort_unstable();
        out
    }

    /// The whole archive file. Use for "archives" that are really flat tables.
    pub fn arc_raw_path(&self, data_path: &str) -> Option<Bytes> {
        self.cached(data_path).map(|c| c.data.clone())
    }

    pub fn is_narc_path(&self, data_path: &str) -> bool {
        self.cached(data_path).is_some_and(|c| c.narc.is_some())
    }

    pub fn arc_len(&self, arc: Arc) -> usize {
        self.arc_len_path(&arc.path(self.game))
    }

    pub fn arc_file(&self, arc: Arc, index: usize) -> Option<Bytes> {
        self.arc_file_path(&arc.path(self.game), index)
    }

    pub fn arc_raw(&self, arc: Arc) -> Option<Bytes> {
        self.arc_raw_path(&arc.path(self.game))
    }

    pub fn is_overridden(&self, arc: Arc, index: usize) -> bool {
        self.is_overridden_path(&arc.path(self.game), index)
    }

    /// Lists the numbered archives (`a/X/Y/Z`) that exist in the game.
    pub fn list_archives(&self) -> Vec<u32> {
        let mut out = Vec::new();
        for id in 0..1000u32 {
            let p = arc_path(id);
            let exists = match &self.base {
                Base::Dir(dir) => dir.join("data").join(&p).is_file(),
                Base::Rom { index, .. } => index.files.contains_key(&p),
            };
            if exists {
                out.push(id);
            }
        }
        out
    }
}

fn find_cmproj(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| {
        p.is_file() && p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("cmproj"))
    })
}
