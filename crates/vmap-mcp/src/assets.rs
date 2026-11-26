//! Tools for the game's archives and what is in them: lists, descriptions,
//! pictures of models and of textures.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use vmap_formats::gen5::TextFile;
use vmap_formats::nitro::{Mdl0, NitroFile, Tex0};
use vmap_render::{OrbitCamera, RenderOptions};
use vmap_world::catalog::describe_animation;
use vmap_world::{FileKind, Member, World};

use crate::params::{FileParams, ListArchivesParams, ListFilesParams, RenderModelParams, TexturesParams};
use crate::reply::{parse_color, save_png, to_text, vec3, Picture, Reply, SentImage};
use crate::state::{OpenGame, State};

const SIZE_RANGE: std::ops::RangeInclusive<u32> = 64..=2048;
/// Longest side of a texture sheet, in pixels.
const SHEET_LIMIT: u32 = 2048;

fn member(world: &World, archive: u32, file: usize) -> Result<Member> {
    world.member(archive, file).ok_or_else(|| {
        match world.archives().iter().find(|a| a.id == archive) {
            Some(a) => anyhow!("archive {archive} ({}) has files 0 to {}; there is no file {file}", a.path, a.len.saturating_sub(1)),
            None => anyhow!("there is no archive {archive}; list_archives shows which exist"),
        }
    })
}

/// The files inside a container. Runs of nameless files of one kind are
/// given as one entry: a bundle starts with dozens of small tables.
fn sub_files(member: &Member) -> Vec<Value> {
    let mut out = Vec::new();
    let files = &member.sub_files;
    let mut i = 0;
    while i < files.len() {
        let s = &files[i];
        let mut end = i + 1;
        if s.name.is_none() {
            while end < files.len() && files[end].name.is_none() && files[end].kind == s.kind {
                end += 1;
            }
        }
        if end - i >= 3 {
            let bytes: usize = files[i..end].iter().map(|f| f.range.len()).sum();
            out.push(json!({ "sub_files": format!("{i} to {}", end - 1), "kind": s.kind.label(), "bytes_together": bytes }));
            i = end;
            continue;
        }
        let mut v = json!({ "sub_file": i, "kind": s.kind.label(), "bytes": s.range.len() });
        if let Some(name) = &s.name {
            v["name"] = json!(name);
        }
        out.push(v);
        i += 1;
    }
    out
}

// --------------------------------------------------------------------- lists

pub fn list_archives(state: &mut State, p: ListArchivesParams) -> Result<Reply> {
    let world = state.world()?;
    let needle = p.search.as_deref().map(|s| s.trim().to_lowercase());
    let archives: Vec<Value> = world
        .archives()
        .iter()
        .filter(|a| needle.as_ref().is_none_or(|n| a.title().to_lowercase().contains(n)))
        .map(|a| {
            let mut v = json!({ "archive": a.id, "path": a.path, "files": a.len });
            if let Some(role) = a.known {
                v["holds"] = json!(role.label());
            }
            if a.modified > 0 {
                v["changed_by_project"] = json!(a.modified);
            }
            if !a.is_narc {
                v["single_file"] = json!(true);
            }
            v
        })
        .collect();
    Ok(Reply::json(&json!({ "count": archives.len(), "archives": archives })))
}

pub fn list_files(state: &mut State, p: ListFilesParams) -> Result<Reply> {
    let world = state.world()?;
    let archives = world.archives();
    let archive = archives
        .iter()
        .find(|a| a.id == p.archive)
        .ok_or_else(|| anyhow!("there is no archive {}; list_archives shows which exist", p.archive))?;
    let kind = p.kind.as_deref().map(|k| k.trim().to_lowercase());
    let changed_only = p.changed_only.unwrap_or(false);
    let path = archive.path.clone();

    let mut files = Vec::new();
    for index in 0..archive.len {
        let changed = world.fs.is_overridden_path(&path, index);
        if changed_only && !changed {
            continue;
        }
        let Some(data) = world.archive_member(p.archive, index) else { continue };
        let label = world.member_kind(p.archive, &data).label();
        if kind.as_ref().is_some_and(|k| !label.to_lowercase().contains(k)) {
            continue;
        }
        let mut v = json!({ "file": index, "kind": label, "bytes": data.len() });
        if changed {
            v["changed_by_project"] = json!(true);
        }
        files.push(v);
    }
    let total = files.len();
    let offset = p.offset.unwrap_or(0).min(total);
    let shown: Vec<Value> = files.into_iter().skip(offset).take(p.limit.unwrap_or(200).max(1)).collect();
    let mut out = json!({
        "archive": p.archive,
        "path": path,
        "holds": archive.known.map(|a| a.label()),
        "files_in_archive": archive.len,
        "matching": total,
        "offset": offset,
        "returned": shown.len(),
    });
    if offset + shown.len() < total {
        out["next_offset"] = json!(offset + shown.len());
    }
    out["files"] = json!(shown);
    Ok(Reply::json(&out))
}

// --------------------------------------------------------------- description

fn model_value(mdl: &Mdl0) -> Value {
    let models: Vec<Value> = mdl
        .models
        .iter()
        .map(|m| {
            let materials: Vec<Value> = m
                .materials
                .iter()
                .map(|mat| json!({ "name": mat.name, "texture": mat.texture, "palette": mat.palette }))
                .collect();
            json!({
                "name": m.name,
                "triangles": m.triangle_count,
                "quads": m.quad_count,
                "vertices": m.vertex_count,
                "joints": m.nodes.len(),
                "meshes": m.shapes.len(),
                "size": vec3(m.bbox_size),
                "materials": materials,
            })
        })
        .collect();
    let mut out = json!({ "models": models });
    if let Some(t) = &mdl.textures {
        out["embedded_textures"] = json!(t.textures.len());
    }
    out
}

fn textures_value(tex: &Tex0) -> Value {
    let textures: Vec<Value> = tex
        .textures
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let palette = tex.guess_palette(i).and_then(|p| tex.palettes.get(p)).map(|p| p.name.as_str());
            json!({ "name": t.name, "size": [t.width, t.height], "format": t.format.name(), "palette": palette })
        })
        .collect();
    let palettes: Vec<Value> = tex.palettes.iter().map(|p| json!({ "name": p.name, "colors": p.color_count })).collect();
    json!({ "textures": textures, "palettes": palettes })
}

fn hex_dump(data: &[u8], limit: usize) -> Vec<String> {
    data[..data.len().min(limit)]
        .chunks(16)
        .enumerate()
        .map(|(row, chunk)| {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
            let text: String =
                chunk.iter().map(|b| if b.is_ascii_graphic() || *b == b' ' { *b as char } else { '.' }).collect();
            format!("{:04X}  {:<47}  {text}", row * 16, hex.join(" "))
        })
        .collect()
}

/// What a file holds, as far as this tool can read it.
fn content_value(kind: &FileKind, data: &[u8]) -> Value {
    match kind {
        FileKind::Model => match Mdl0::from_bmd(data) {
            Ok(mdl) => model_value(&mdl),
            Err(e) => json!({ "problem": format!("the model could not be read: {e}") }),
        },
        FileKind::Textures => match Tex0::from_btx(data) {
            Ok(tex) => textures_value(&tex),
            Err(e) => json!({ "problem": format!("the textures could not be read: {e}") }),
        },
        FileKind::Text => match TextFile::parse(data) {
            Ok(text) => {
                let lines = text.lines();
                let first: Vec<&String> = lines.iter().take(12).collect();
                json!({ "lines": lines.len(), "first_lines": first, "hint": "read_text returns the whole file" })
            }
            Err(e) => json!({ "problem": format!("the text could not be read: {e}") }),
        },
        FileKind::JointAnimation | FileKind::TextureSrtAnimation | FileKind::TexturePatternAnimation => {
            json!({ "animation": describe_animation(kind, data) })
        }
        FileKind::Empty => json!({}),
        _ => {
            let mut out = json!({});
            if NitroFile::sniff(data).is_some() {
                if let Ok(file) = NitroFile::parse(data) {
                    let blocks: Vec<Value> = file
                        .blocks
                        .iter()
                        .map(|(magic, bytes)| json!({ "block": String::from_utf8_lossy(magic), "bytes": bytes.len() }))
                        .collect();
                    out["blocks"] = json!(blocks);
                }
            }
            out["first_bytes"] = json!(hex_dump(data, 128));
            out
        }
    }
}

pub fn describe_file(state: &mut State, p: FileParams) -> Result<Reply> {
    let world = state.world()?;
    let m = member(world, p.archive, p.file)?;
    let mut out = json!({
        "archive": p.archive,
        "file": p.file,
        "kind": m.kind.label(),
        "bytes": m.data.len(),
        "changed_by_project": m.modified,
    });
    if m.was_compressed {
        out["stored_compressed_as_bytes"] = json!(m.stored_size);
    }
    if m.sub_files.is_empty() {
        out["content"] = content_value(&m.kind, &m.data);
    } else {
        out["sub_files"] = json!(sub_files(&m));
        // What the first few files inside hold, so one call often suffices.
        let inside: Vec<Value> = m
            .sub_files
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s.kind, FileKind::Model | FileKind::Textures))
            .take(4)
            .map(|(i, s)| json!({ "sub_file": i, "content": content_value(&s.kind, &m.data[s.range.clone()]) }))
            .collect();
        if !inside.is_empty() {
            out["inside"] = json!(inside);
        }
    }
    let buildings = [
        vmap_formats::gen5::Arc::BuildingsExterior.id(world.game),
        vmap_formats::gen5::Arc::BuildingsInterior.id(world.game),
    ];
    let hint = match &m.kind {
        _ if buildings.contains(&p.archive) => Some(
            "this is a building bundle: list_buildings lists its buildings with the sub_file of each, \
             and render_model shows one",
        ),
        FileKind::Model => Some("render_model shows it"),
        FileKind::Textures => Some("get_textures shows them"),
        FileKind::Container(_) if m.sub_files.iter().any(|s| s.kind == FileKind::Model) => {
            Some("render_model shows a model inside (choose it with sub_file); get_textures shows textures inside")
        }
        _ => None,
    };
    if let Some(hint) = hint {
        out["hint"] = json!(hint);
    }
    Ok(Reply::json(&out))
}

// ------------------------------------------------------------------ pictures

/// Picks the part of a file a picture tool should work on: the file itself,
/// or one of the files inside a container. Returns its index inside the
/// container (if any) and its kind.
fn choose_part(m: &Member, asked: Option<usize>, wanted: &[FileKind], what: &str) -> Result<(Option<usize>, FileKind)> {
    if m.sub_files.is_empty() {
        if asked.is_some_and(|i| i != 0) {
            bail!("this file is no container, so it has no sub_file {}", asked.unwrap_or(0));
        }
        if !wanted.contains(&m.kind) {
            bail!("this file holds {}, not {what}; describe_file tells more about it", m.kind.label());
        }
        return Ok((None, m.kind.clone()));
    }
    let index = match asked {
        Some(i) => i,
        None => wanted
            .iter()
            .find_map(|k| m.sub_files.iter().position(|s| &s.kind == k))
            .ok_or_else(|| anyhow!("none of the {} files inside this container holds {what}", m.sub_files.len()))?,
    };
    let sub = m
        .sub_files
        .get(index)
        .ok_or_else(|| anyhow!("the container has sub-files 0 to {}; there is no sub_file {index}", m.sub_files.len() - 1))?;
    if !wanted.contains(&sub.kind) {
        bail!("sub_file {index} holds {}, not {what}; describe_file lists what is inside", sub.kind.label());
    }
    Ok((Some(index), sub.kind.clone()))
}

fn part<'a>(m: &'a Member, index: Option<usize>) -> &'a [u8] {
    match index {
        Some(i) => &m.data[m.sub_files[i].range.clone()],
        None => &m.data,
    }
}

pub fn render_model(state: &mut State, p: RenderModelParams) -> Result<Reply> {
    let (game, headless) = state.game_and_renderer()?;
    let protected = game.protected_folders();
    let OpenGame { world, chunk_areas, .. } = game;
    let m = member(world, p.archive, p.file)?;
    let (sub, _) = choose_part(&m, p.sub_file, &[FileKind::Model], "a 3D model")?;
    let data = part(&m, sub);

    // Textures the model does not carry itself come from where the game
    // takes them: the area of a map chunk, the texture set of a bundle.
    let areas = chunk_areas.get_or_insert_with(|| world.chunk_areas());
    let extra = world.textures_for(p.archive, p.file, areas);
    let extra: Vec<&Tex0> = extra.iter().collect();
    let animate = p.animations.unwrap_or(true);
    // A building inside a bundle brings its animations along.
    let as_building = sub.filter(|_| animate).and_then(|_| world.build_building_scene(&m.data, data, &extra).ok());
    let scene = match as_building {
        Some(scene) => scene,
        None => world.build_model_scene(data, &extra).context("reading the model")?,
    };

    let width = p.width.unwrap_or(768).clamp(*SIZE_RANGE.start(), *SIZE_RANGE.end());
    let height = p.height.unwrap_or(576).clamp(*SIZE_RANGE.start(), *SIZE_RANGE.end());
    let aspect = width as f32 / height as f32;
    let zoom = p.zoom.unwrap_or(1.0);
    if !(zoom.is_finite() && zoom > 0.0) {
        bail!("zoom must be greater than 0");
    }
    let mut camera = OrbitCamera {
        yaw: p.yaw.unwrap_or(35.0).to_radians(),
        pitch: p.pitch.unwrap_or(28.0).clamp(-89.0, 90.0).to_radians(),
        ..Default::default()
    };
    camera.frame(scene.min, scene.max, aspect);
    camera.distance *= 1.4 / zoom;
    let background = match &p.background {
        Some(text) => parse_color(text)?.map(|b| b as f32 / 255.0),
        None => [0.13, 0.15, 0.19],
    };
    let options = RenderOptions {
        lighting: p.lighting.unwrap_or(true),
        textures: p.textures.unwrap_or(true),
        vertex_colors: p.vertex_colors.unwrap_or(true),
        markers: false,
        background: Some(background),
        ..Default::default()
    };

    let mut gpu = headless.upload(&scene);
    let time = p.time.unwrap_or(0.0);
    if animate && time.is_finite() {
        gpu.animate(&headless.gpu.queue, time.max(0.0));
    }
    let rgba = headless.render(&gpu, &camera.camera(aspect), &options, width, height)?;
    let picture = Picture::new(width, height, rgba);
    let png = picture.png()?;

    let mut out = json!({ "archive": p.archive, "file": p.file });
    if let Some(sub) = sub {
        out["sub_file"] = json!(sub);
        let models = m.sub_files.iter().filter(|s| s.kind == FileKind::Model).count();
        if models > 1 {
            out["models_in_container"] = json!(models);
        }
    }
    if let Ok(mdl) = Mdl0::from_bmd(data) {
        out["model"] = model_value(&mdl);
    }
    let a = &scene.animations;
    out["animated_parts"] = json!({
        "sliding_textures": a.tex_srt.len(),
        "swapping_textures": a.tex_pattern.len(),
        "moving_joint_sets": a.joints.len(),
    });
    let untextured = scene.materials.iter().filter(|m| m.texture.is_none()).count();
    if untextured > 0 {
        out["materials_drawn_without_texture"] = json!(format!("{untextured} of {}", scene.materials.len()));
    }
    out["bounds"] = json!({ "min": vec3(scene.min), "max": vec3(scene.max) });
    out["camera"] = json!({
        "yaw": camera.yaw.to_degrees().round(),
        "pitch": camera.pitch.to_degrees().round(),
        "distance": camera.distance.round(),
    });
    out["picture"] = json!({ "width": width, "height": height });
    if let Some(path) = &p.save_to {
        out["picture"]["saved_to"] = json!(save_png(path, &png, &protected)?);
    }
    let image = picture.to_send(png)?;
    if image.is_lossy() {
        out["picture"]["sent_as"] = json!("jpeg");
    }
    Ok(Reply { text: to_text(&out), image: Some(image) })
}

/// Draws a decoded texture enlarged onto a checkerboard, so that
/// transparent parts can be told from dark ones.
fn draw_texture(sheet: &mut Picture, image: &vmap_formats::nitro::Image, left: u32, top: u32, zoom: u32) {
    for y in 0..image.height * zoom {
        for x in 0..image.width * zoom {
            let (px, py) = (left + x, top + y);
            let light = ((x / 8) + (y / 8)) % 2 == 0;
            sheet.put(px, py, if light { [92, 92, 96] } else { [66, 66, 70] });
            let i = (((y / zoom) * image.width + x / zoom) * 4) as usize;
            let texel = [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2], image.rgba[i + 3]];
            sheet.blend(px, py, texel);
        }
    }
}

pub fn get_textures(state: &mut State, p: TexturesParams) -> Result<Reply> {
    let game = state.game()?;
    let protected = game.protected_folders();
    let world = &game.world;
    let m = member(world, p.archive, p.file)?;
    let (sub, kind) = choose_part(&m, p.sub_file, &[FileKind::Textures, FileKind::Model], "textures")?;
    let data = part(&m, sub);
    let tex = match kind {
        FileKind::Model => Mdl0::from_bmd(data)
            .context("reading the model")?
            .textures
            .ok_or_else(|| anyhow!("this model carries no textures of its own; it uses those of its area or bundle"))?,
        _ => Tex0::from_btx(data).context("reading the textures")?,
    };
    if tex.textures.is_empty() {
        bail!("this file holds no textures");
    }
    if p.zoom.is_some_and(|z| !(1..=16).contains(&z)) {
        bail!("zoom must be between 1 and 16");
    }

    let mut out = json!({ "archive": p.archive, "file": p.file });
    if let Some(sub) = sub {
        out["sub_file"] = json!(sub);
    }
    let png;
    if let Some(name) = &p.texture {
        // One texture, enlarged.
        let index = tex.find_texture(name).ok_or_else(|| {
            let names: Vec<&str> = tex.textures.iter().take(40).map(|t| t.name.as_str()).collect();
            anyhow!("there is no texture \"{name}\" here; there are: {}", names.join(", "))
        })?;
        let palette = match &p.palette {
            Some(pal) => Some(tex.find_palette(pal).ok_or_else(|| {
                let names: Vec<&str> = tex.palettes.iter().take(40).map(|p| p.name.as_str()).collect();
                anyhow!("there is no palette \"{pal}\" here; there are: {}", names.join(", "))
            })?),
            None => tex.guess_palette(index),
        };
        let image = tex.decode(index, palette).ok_or_else(|| anyhow!("the texture could not be decoded"))?;
        let longest = image.width.max(image.height).max(1);
        let zoom = p.zoom.unwrap_or(4).min((SHEET_LIMIT / longest).max(1));
        let mut picture = Picture::filled(image.width * zoom, image.height * zoom, [0, 0, 0]);
        draw_texture(&mut picture, &image, 0, 0, zoom);
        let t = &tex.textures[index];
        out["texture"] = json!({
            "name": t.name,
            "size": [t.width, t.height],
            "format": t.format.name(),
            "palette": palette.and_then(|p| tex.palettes.get(p)).map(|p| p.name.as_str()),
            "first_color_transparent": t.color0_transparent,
        });
        out["picture"] = json!({ "width": picture.width, "height": picture.height, "enlarged": zoom });
        png = picture.png()?;
    } else {
        // All of them on one sheet, in rows.
        let decoded: Vec<(usize, vmap_formats::nitro::Image)> = (0..tex.textures.len())
            .filter_map(|i| tex.decode(i, tex.guess_palette(i)).map(|image| (i, image)))
            .collect();
        const GAP: u32 = 6;
        const SHEET_WIDTH: u32 = 1024;
        let layout = |zoom: u32| -> (Vec<(usize, u32, u32)>, u32, u32) {
            let (mut x, mut y, mut row, mut widest) = (GAP, GAP, 0, 0);
            let mut places = Vec::new();
            for (n, (_, image)) in decoded.iter().enumerate() {
                let (w, h) = (image.width * zoom, image.height * zoom);
                if x > GAP && x + w + GAP > SHEET_WIDTH {
                    (x, y, row) = (GAP, y + row + GAP, 0);
                }
                places.push((n, x, y));
                x += w + GAP;
                row = row.max(h);
                widest = widest.max(x);
            }
            (places, widest, y + row + GAP)
        };
        let mut zoom = p.zoom.unwrap_or(2);
        while zoom > 1 && { let (_, w, h) = layout(zoom); w.max(h) > SHEET_LIMIT } {
            zoom -= 1;
        }
        let (places, width, height) = layout(zoom);
        let (width, height) = (width.min(SHEET_LIMIT), height.min(SHEET_LIMIT));
        let mut sheet = Picture::filled(width.max(16), height.max(16), [28, 30, 36]);
        let mut listed = Vec::new();
        let mut left_out = 0;
        for (n, x, y) in places {
            let (index, image) = &decoded[n];
            if x + image.width * zoom > width || y + image.height * zoom > height {
                left_out += 1;
                continue;
            }
            draw_texture(&mut sheet, image, x, y, zoom);
            let t = &tex.textures[*index];
            listed.push(json!({
                "name": t.name,
                "size": [t.width, t.height],
                "format": t.format.name(),
                "palette": tex.guess_palette(*index).and_then(|p| tex.palettes.get(p)).map(|p| p.name.as_str()),
                // Top-left corner on the sheet, in pixels.
                "at": [x, y],
            }));
        }
        out["textures"] = json!(listed);
        out["palettes"] = json!(tex.palettes.iter().map(|p| p.name.as_str()).collect::<Vec<_>>());
        out["picture"] = json!({ "width": sheet.width, "height": sheet.height, "enlarged": zoom });
        if left_out > 0 {
            out["note"] = json!(format!(
                "{left_out} textures did not fit on the sheet; ask for them one at a time with `texture`: {}",
                tex.textures.iter().skip(listed.len()).take(60).map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        png = sheet.png()?;
    }
    if let Some(path) = &p.save_to {
        out["picture"]["saved_to"] = json!(save_png(path, &png, &protected)?);
    }
    // Texture sheets are sent exactly: they are small, and every texel counts.
    Ok(Reply { text: to_text(&out), image: Some(SentImage::png(png)) })
}
