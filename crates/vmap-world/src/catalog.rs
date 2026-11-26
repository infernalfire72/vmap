//! Browsing the game's archives as assets: what each file is and which
//! textures a model needs.

use std::collections::HashMap;

use vmap_formats::gen5::{arc_path, bw1_buildings_id, Arc};
use vmap_formats::gf::GfContainer;
use vmap_formats::nds::compress;
use vmap_formats::nitro::{NitroFile, Tex0};
use vmap_fs::Bytes;

use crate::World;

/// What a file is, judged from its first bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FileKind {
    Empty,
    Model,
    Textures,
    JointAnimation,
    TextureSrtAnimation,
    TexturePatternAnimation,
    MaterialAnimation,
    VisibilityAnimation,
    Narc,
    /// Game Freak container with its two-letter signature.
    Container(String),
    /// 2D graphics resource (tiles, palette, cells, ...).
    Graphics(String),
    Sound,
    Compressed,
    Text,
    Unknown,
}

impl FileKind {
    pub fn sniff(data: &[u8]) -> Self {
        if data.is_empty() {
            return Self::Empty;
        }
        if data.len() >= 4 && &data[0..4] == b"NARC" {
            return Self::Narc;
        }
        if let Some(magic) = NitroFile::sniff(data) {
            let g = |s: &str| Self::Graphics(s.to_string());
            return match &magic {
                b"BMD0" => Self::Model,
                b"BTX0" => Self::Textures,
                b"BCA0" => Self::JointAnimation,
                b"BTA0" => Self::TextureSrtAnimation,
                b"BTP0" => Self::TexturePatternAnimation,
                b"BMA0" => Self::MaterialAnimation,
                b"BVA0" => Self::VisibilityAnimation,
                b"RGCN" => g("tiles"),
                b"RLCN" => g("palette"),
                b"RECN" => g("cells"),
                b"RNAN" => g("cell animation"),
                b"RCSN" => g("screen"),
                b"RCMN" => g("multi-cell"),
                b"RAMN" => g("multi-cell animation"),
                b"SDAT" => Self::Sound,
                _ => Self::Unknown,
            };
        }
        if data.len() >= 8 && data[0].is_ascii_uppercase() && data[1].is_ascii_uppercase() {
            if let Ok(c) = GfContainer::parse(data) {
                let sane = !c.is_empty()
                    && c.len() < 4096
                    && c.files.first().is_some_and(|f| f.start >= 4 + (c.len() + 1) * 4)
                    && c.files.last().is_some_and(|f| f.end == data.len() || f.end + 4 >= data.len());
                if sane {
                    return Self::Container(c.magic);
                }
            }
        }
        if compress::looks_lz(data) {
            return Self::Compressed;
        }
        Self::Unknown
    }

    pub fn label(&self) -> String {
        match self {
            Self::Empty => "empty".into(),
            Self::Model => "3D model".into(),
            Self::Textures => "textures".into(),
            Self::JointAnimation => "joint animation".into(),
            Self::TextureSrtAnimation => "texture scroll animation".into(),
            Self::TexturePatternAnimation => "texture swap animation".into(),
            Self::MaterialAnimation => "material animation".into(),
            Self::VisibilityAnimation => "visibility animation".into(),
            Self::Narc => "archive".into(),
            Self::Container(m) => format!("container ({m})"),
            Self::Graphics(what) => format!("2D {what}"),
            Self::Sound => "sound archive".into(),
            Self::Compressed => "compressed data".into(),
            Self::Text => "text".into(),
            Self::Unknown => "data".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArchiveInfo {
    /// Archive number; its path is `a/X/Y/Z`.
    pub id: u32,
    pub path: String,
    /// The archive's role, when this tool knows it.
    pub known: Option<Arc>,
    pub is_narc: bool,
    pub len: usize,
    /// Number of members the open project replaces or adds.
    pub modified: usize,
}

impl ArchiveInfo {
    pub fn title(&self) -> String {
        match self.known {
            Some(a) => format!("{}  {}", self.path, a.label()),
            None => self.path.clone(),
        }
    }
}

impl World {
    /// All numbered archives of the game.
    pub fn archives(&self) -> Vec<ArchiveInfo> {
        let known: HashMap<u32, Arc> = Arc::ALL.iter().map(|&a| (a.id(self.game), a)).collect();
        self.fs
            .list_archives()
            .into_iter()
            .map(|id| {
                let path = arc_path(id);
                let is_narc = self.fs.is_narc_path(&path);
                ArchiveInfo {
                    id,
                    known: known.get(&id).copied(),
                    is_narc,
                    len: if is_narc { self.fs.arc_len_path(&path) } else { 1 },
                    modified: self.fs.overridden_members(&path).len(),
                    path,
                }
            })
            .collect()
    }

    /// One file of an archive (the archive itself if it is a flat file).
    pub fn archive_member(&self, id: u32, index: usize) -> Option<Bytes> {
        let path = arc_path(id);
        if self.fs.is_narc_path(&path) {
            self.fs.arc_file_path(&path, index)
        } else {
            self.fs.arc_raw_path(&path)
        }
    }

    /// Kind of a member, using the archive's role where bytes alone cannot tell.
    pub fn member_kind(&self, id: u32, data: &[u8]) -> FileKind {
        let kind = FileKind::sniff(data);
        if kind == FileKind::Unknown {
            let text = [Arc::SystemText.id(self.game), Arc::ScriptText.id(self.game)];
            if text.contains(&id) {
                return FileKind::Text;
            }
        }
        kind
    }

    /// For every map chunk, an area whose textures it is drawn with.
    pub fn chunk_areas(&self) -> HashMap<u32, usize> {
        let mut out = HashMap::new();
        for m in 0..self.matrix_count {
            let Ok(matrix) = self.matrix(m) else { continue };
            let users = self.zones_of_matrix(m);
            for y in 0..matrix.height {
                for x in 0..matrix.width {
                    let Some(chunk) = matrix.chunk(x, y) else { continue };
                    let zone = matrix.zone(x, y).or(users.first().copied());
                    if let Some(area) = zone.and_then(|z| self.seasonal_area(z, 0)) {
                        out.entry(chunk).or_insert(area);
                    }
                }
            }
        }
        out
    }

    /// Texture sets a model from a given archive member is normally drawn
    /// with, beyond what the model file embeds.
    pub fn textures_for(&self, id: u32, index: usize, chunk_areas: &HashMap<u32, usize>) -> Vec<Tex0> {
        let load = |arc: Arc, i: usize| self.fs.arc_file(arc, i).and_then(|d| Tex0::from_btx(&d).ok());
        let g = self.game;
        let mut out = Vec::new();
        if id == Arc::MapChunks.id(g) {
            if let Some(&area) = chunk_areas.get(&(index as u32)) {
                let a = self.areas[area];
                out.extend(load(Arc::MapTextures, a.textures_id as usize));
                let bundle = if g.is_bw2() { a.buildings_id as usize } else { bw1_buildings_id(area) };
                let tex = if a.is_exterior { Arc::BuildingTexExterior } else { Arc::BuildingTexInterior };
                out.extend(load(tex, bundle));
            }
        } else if id == Arc::BuildingsExterior.id(g) {
            out.extend(load(Arc::BuildingTexExterior, index));
        } else if id == Arc::BuildingsInterior.id(g) {
            out.extend(load(Arc::BuildingTexInterior, index));
        }
        out
    }
}

/// A file inside a container.
#[derive(Debug, Clone)]
pub struct SubFile {
    pub kind: FileKind,
    /// Name of the first model, for model files.
    pub name: Option<String>,
    /// Where the file lies in [`Member::data`].
    pub range: std::ops::Range<usize>,
}

/// An archive member, unpacked and looked into.
#[derive(Debug, Clone)]
pub struct Member {
    /// The file's bytes, decompressed if it was stored compressed.
    pub data: Vec<u8>,
    pub kind: FileKind,
    pub was_compressed: bool,
    /// Size as stored in the archive.
    pub stored_size: usize,
    /// The open project replaces this file.
    pub modified: bool,
    /// The files inside, when this is a container.
    pub sub_files: Vec<SubFile>,
}

impl Member {
    pub fn sub(&self, index: usize) -> Option<&[u8]> {
        self.sub_files.get(index).map(|s| &self.data[s.range.clone()])
    }

    /// The sub-file worth showing first: a model, else textures, else the
    /// first one.
    pub fn default_sub(&self) -> Option<usize> {
        let find = |kind: FileKind| self.sub_files.iter().position(|s| s.kind == kind);
        find(FileKind::Model).or_else(|| find(FileKind::Textures)).or((!self.sub_files.is_empty()).then_some(0))
    }
}

impl World {
    /// Reads an archive member, unpacks it and lists what is inside.
    pub fn member(&self, id: u32, index: usize) -> Option<Member> {
        let raw = self.archive_member(id, index)?;
        let stored_size = raw.len();
        let mut data = raw.to_vec();
        let mut kind = self.member_kind(id, &data);
        let mut was_compressed = false;
        if kind == FileKind::Compressed {
            if let Ok(unpacked) = compress::lz_decompress(&data) {
                data = unpacked;
                kind = self.member_kind(id, &data);
                was_compressed = true;
            }
        }
        let mut sub_files = Vec::new();
        if let FileKind::Container(_) = &kind {
            if let Ok(c) = GfContainer::parse(&data) {
                for range in &c.files {
                    let bytes = &data[range.clone()];
                    let kind = FileKind::sniff(bytes);
                    let name = (kind == FileKind::Model)
                        .then(|| {
                            vmap_formats::nitro::Mdl0::from_bmd(bytes)
                                .ok()
                                .and_then(|m| m.models.first().map(|m| m.name.clone()))
                        })
                        .flatten();
                    sub_files.push(SubFile { kind, name, range: range.clone() });
                }
            }
        }
        Some(Member {
            data,
            kind,
            was_compressed,
            stored_size,
            modified: self.fs.is_overridden_path(&arc_path(id), index),
            sub_files,
        })
    }
}

/// What an animation file contains, in words. Animations play on the model
/// they belong to, so a file on its own can only be described.
pub fn describe_animation(kind: &FileKind, data: &[u8]) -> Vec<String> {
    use vmap_formats::nitro::anim::{self, Channel};
    let length = |frames: u16| format!("{frames} frames ({:.1} s)", frames as f32 / crate::ANIMATION_FPS);
    let count = |n: usize, what: &str| if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") };
    let mut out = Vec::new();
    let result = match kind {
        FileKind::JointAnimation => anim::parse_joint(data).map(|list| {
            for a in list {
                out.push(format!(
                    "Joint animation \"{}\": {}, {}",
                    a.name,
                    length(a.frame_count),
                    count(a.joints.len(), "joint")
                ));
                for j in &a.joints {
                    let mut does = Vec::new();
                    if matches!(&j.translation, Channel::Animated(t) if t.iter().any(|t| !t.is_constant())) {
                        does.push("moves");
                    }
                    if matches!(&j.rotation, Channel::Animated(r) if r.values.len() > 1) {
                        does.push("turns");
                    }
                    if matches!(&j.scale, Channel::Animated(s) if s.iter().any(|s| !s.is_constant())) {
                        does.push("changes size");
                    }
                    let does = if does.is_empty() { "holds a fixed pose".to_string() } else { does.join(", ") };
                    out.push(format!("   joint {}: {does}", j.node));
                }
            }
        }),
        FileKind::TextureSrtAnimation => anim::parse_tex_srt(data).map(|list| {
            for a in list {
                out.push(format!(
                    "Texture scroll animation \"{}\": {}, {}",
                    a.name,
                    length(a.frame_count),
                    count(a.tracks.len(), "material")
                ));
                for t in &a.tracks {
                    let mut does = Vec::new();
                    if !t.trans_s.is_constant() || !t.trans_t.is_constant() {
                        does.push("slides");
                    }
                    if !t.rot_sin.is_constant() || !t.rot_cos.is_constant() {
                        does.push("turns");
                    }
                    if !t.scale_s.is_constant() || !t.scale_t.is_constant() {
                        does.push("stretches");
                    }
                    let does = if does.is_empty() { "stays in place".to_string() } else { does.join(", ") };
                    out.push(format!("   material {}: texture {does}", t.material));
                }
            }
        }),
        _ => anim::parse_tex_pattern(data).map(|list| {
            for a in list {
                out.push(format!(
                    "Texture swap animation \"{}\": {}, {}",
                    a.name,
                    length(a.frame_count),
                    count(a.tracks.len(), "material")
                ));
                for t in &a.tracks {
                    out.push(format!("   material {}: {} steps", t.material, t.keys.len()));
                    for k in &t.keys {
                        out.push(format!("      from frame {:>3}: texture {}, palette {}", k.frame, k.texture, k.palette));
                    }
                }
            }
        }),
    };
    if let Err(e) = result {
        out.push(format!("This animation could not be read: {e}"));
    }
    out
}
