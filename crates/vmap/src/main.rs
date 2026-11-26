mod headless;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use vmap_world::{MapRequest, World};

#[derive(Parser)]
#[command(
    name = "vmap",
    version,
    about = "Map viewer and asset viewer for Generation V Pokemon games",
    long_about = "Map viewer and asset viewer for Generation V Pokemon games.\n\n\
                  Run without a subcommand to open the viewer window. The subcommands \
                  work without a window and are meant for scripts and agents.",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// Game to open in the viewer window: a CTRMap project (.cmproj), an
    /// extracted ROM folder or a .nds file. Defaults to the last one opened.
    game: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the maps and assets of a game to an agent, as an MCP server on
    /// standard input and output.
    Mcp {
        /// Game to open at start. Without it, the agent opens one with the
        /// `open_game` tool.
        game: Option<PathBuf>,
    },
    /// Print a summary of a game: zones, areas and map matrices.
    Info {
        /// CTRMap project (.cmproj), extracted ROM folder or .nds file.
        game: PathBuf,
    },
    /// List every zone with its name, map matrix and area.
    Zones {
        game: PathBuf,
    },
    /// Print the lines of a text file from the system or script text archive.
    Text {
        game: PathBuf,
        /// File number inside the archive.
        file: usize,
        /// Read from the script text archive instead of the system text.
        #[arg(long)]
        script: bool,
    },
    /// Print what a map chunk contains: its model and placed buildings.
    Chunk {
        game: PathBuf,
        id: usize,
    },
    /// List the building models of a bundle with their IDs.
    Buildings {
        game: PathBuf,
        bundle: usize,
        /// Read the interior bundles instead of the exterior ones.
        #[arg(long)]
        interior: bool,
    },
    /// Build a stitched map and print what it contains.
    MapInfo {
        game: PathBuf,
        #[arg(long)]
        matrix: usize,
    },
    /// Render a stitched map to a PNG without opening a window.
    Render(RenderArgs),
    /// Open the window, select something, save a picture of it and exit.
    #[command(hide = true)]
    UiShot {
        game: PathBuf,
        #[arg(long, short)]
        out: PathBuf,
        #[arg(long)]
        assets: bool,
        #[arg(long)]
        map: Option<String>,
        #[arg(long)]
        top_down: bool,
        /// `archive:file` or `archive:file:subfile`.
        #[arg(long)]
        asset: Option<String>,
        /// Select this warp of the opened map (counting from 0).
        #[arg(long)]
        warp: Option<usize>,
        /// Go where the selected warp leads.
        #[arg(long)]
        follow: bool,
        /// Zones of the opened map to hide.
        #[arg(long = "hide-zone")]
        hide_zones: Vec<u32>,
        /// Hide the cells of the opened map that belong to no zone.
        #[arg(long)]
        hide_unassigned: bool,
        /// Click with the mouse at X,Y (pixels of the screenshot). May be given more than once.
        #[arg(long = "click", value_name = "X,Y")]
        clicks: Vec<String>,
    },
}

#[derive(Args)]
pub struct RenderArgs {
    /// CTRMap project (.cmproj), extracted ROM folder or .nds file.
    pub game: PathBuf,
    /// Map matrix to render.
    #[arg(long)]
    pub matrix: Option<usize>,
    /// Zone to render. Picks that zone's matrix and, on shared matrices,
    /// only the zone's own cells.
    #[arg(long)]
    pub zone: Option<u32>,
    /// Output PNG.
    #[arg(long, short, default_value = "vmap.png")]
    pub out: PathBuf,
    #[arg(long, default_value_t = 1600)]
    pub width: u32,
    #[arg(long, default_value_t = 1000)]
    pub height: u32,
    /// Flat top-down view with north at the top.
    #[arg(long)]
    pub top_down: bool,
    /// Camera heading in degrees (0 looks north).
    #[arg(long, default_value_t = 0.0)]
    pub yaw: f32,
    /// Camera angle below the horizon in degrees.
    #[arg(long, default_value_t = 50.0)]
    pub pitch: f32,
    /// Distance from the target; fits the whole map if omitted.
    #[arg(long)]
    pub distance: Option<f32>,
    /// Point to look at as `x,y,z` in world units; map centre if omitted.
    #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
    pub target: Option<Vec<f32>>,
    /// 0 spring, 1 summer, 2 autumn, 3 winter.
    #[arg(long, default_value_t = 0)]
    pub season: u8,
    /// Moment of the animations (water, flowers, windmills) in seconds.
    #[arg(long, default_value_t = 0.0)]
    pub time: f32,
    /// Leave out NPC, warp and trigger markers.
    #[arg(long)]
    pub no_markers: bool,
    /// Leave out NPC markers.
    #[arg(long)]
    pub no_npcs: bool,
    /// Leave out warp markers.
    #[arg(long)]
    pub no_warps: bool,
    /// Leave out trigger markers.
    #[arg(long)]
    pub no_triggers: bool,
    /// Leave out markers of interactable objects.
    #[arg(long)]
    pub no_interactables: bool,
    /// Leave out the ground.
    #[arg(long)]
    pub no_terrain: bool,
    /// Leave out buildings.
    #[arg(long)]
    pub no_buildings: bool,
    /// Leave out a zone of a map that holds several. May be given more than once.
    #[arg(long = "hide-zone", value_name = "ZONE")]
    pub hide_zones: Vec<u32>,
    /// Leave out the cells that belong to no zone (the sea and forest between zones).
    #[arg(long)]
    pub hide_unassigned: bool,
    /// Draw without lighting.
    #[arg(long)]
    pub no_lighting: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Mcp { game }) => vmap_mcp::serve(game),
        Some(Command::Info { game }) => info(&game),
        Some(Command::MapInfo { game, matrix }) => map_info(&game, matrix),
        Some(Command::Zones { game }) => {
            let w = World::open(&game)?;
            for (i, z) in w.zones.iter().enumerate() {
                println!(
                    "zone {i:3}  matrix {:3}  area {:3}  entities {:3}  {}",
                    z.matrix_id,
                    z.area_id,
                    z.entities_id,
                    w.location_name(i as u32).unwrap_or("")
                );
            }
            Ok(())
        }
        Some(Command::Text { game, file, script }) => {
            use vmap_formats::gen5::{Arc, TextFile};
            let w = World::open(&game)?;
            let arc = if script { Arc::ScriptText } else { Arc::SystemText };
            let data = w.fs.arc_file(arc, file).ok_or_else(|| anyhow::anyhow!("there is no text file {file}"))?;
            let text = TextFile::parse(&data)?;
            for (i, line) in text.lines().iter().enumerate() {
                println!("{i:4}  {}", line.replace('\n', "\\n"));
            }
            Ok(())
        }
        Some(Command::Chunk { game, id }) => {
            use vmap_formats::gen5::{Arc, MapChunk};
            let w = World::open(&game)?;
            let data = w.fs.arc_file(Arc::MapChunks, id).ok_or_else(|| anyhow::anyhow!("there is no chunk {id}"))?;
            let chunk = MapChunk::parse(&data)?;
            println!(
                "chunk {id}: kind {}, model {} bytes, {} buildings{}",
                chunk.kind,
                chunk.model.len(),
                chunk.buildings.len(),
                if w.fs.is_overridden(Arc::MapChunks, id) { ", changed by the project" } else { "" }
            );
            for (i, b) in chunk.buildings.iter().enumerate() {
                println!("  building {i}: model id {}, position {:?}, rotation {:.1} deg", b.model_uid, b.position, b.rotation_deg);
            }
            Ok(())
        }
        Some(Command::Buildings { game, bundle, interior }) => {
            use vmap_formats::gen5::{Arc, BuildingBundle};
            use vmap_formats::nitro::Mdl0;
            let w = World::open(&game)?;
            let arc = if interior { Arc::BuildingsInterior } else { Arc::BuildingsExterior };
            let data = w.fs.arc_file(arc, bundle).ok_or_else(|| anyhow::anyhow!("there is no bundle {bundle}"))?;
            let b = BuildingBundle::parse(&data)?;
            for r in &b.buildings {
                let name = Mdl0::from_bmd(r.model).ok().and_then(|m| m.models.first().map(|m| m.name.clone())).unwrap_or_default();
                let door = r.door_uid.map(|d| d.to_string()).unwrap_or("-".into());
                let anims: Vec<String> = r
                    .animations
                    .iter()
                    .map(|a| String::from_utf8_lossy(&a[..4.min(a.len())]).into_owned())
                    .collect();
                println!(
                    "  id {:4}  door {door:>5}  {:6} bytes  {name:16}  animation mode {} set size {} files {anims:?}",
                    r.uid,
                    r.model.len(),
                    r.anim_control,
                    r.anim_set_size
                );
            }
            Ok(())
        }
        Some(Command::Render(args)) => headless::render(&args),
        Some(Command::UiShot {
            game,
            out,
            assets,
            map,
            top_down,
            asset,
            warp,
            follow,
            hide_zones,
            hide_unassigned,
            clicks,
        }) => {
            let asset = asset.map(|a| {
                let n: Vec<usize> = a.split(':').filter_map(|p| p.parse().ok()).collect();
                (n.first().copied().unwrap_or(0) as u32, n.get(1).copied().unwrap_or(0), n.get(2).copied())
            });
            let assets = assets || asset.is_some();
            let script = vmap_ui::UiScript {
                screenshot: out,
                assets,
                map,
                top_down,
                asset,
                warp,
                follow,
                hide_zones,
                hide_unassigned,
                clicks: clicks
                    .iter()
                    .filter_map(|c| {
                        let (x, y) = c.split_once(',')?;
                        Some([x.trim().parse().ok()?, y.trim().parse().ok()?])
                    })
                    .collect(),
            };
            vmap_ui::run_with(Some(game), Some(script))
        }
        None => vmap_ui::run(cli.game),
    }
}

fn info(path: &PathBuf) -> Result<()> {
    let w = World::open(path)?;
    println!("{} ({}), source: {:?}", w.game.name(), w.fs.header().game_code, w.fs.kind());
    println!("{} zones, {} areas, {} map matrices", w.zones.len(), w.areas.len(), w.matrix_count);
    for m in w.matrices() {
        println!(
            "  matrix {:3}  {:2}x{:<2}  {:3} chunks  {:28} zones {:?}{}",
            m.id,
            m.width,
            m.height,
            m.cells,
            m.name,
            &m.zones[..m.zones.len().min(8)],
            if m.zones.len() > 8 { format!(" (+{})", m.zones.len() - 8) } else { String::new() }
        );
    }
    Ok(())
}

fn map_info(path: &PathBuf, matrix: usize) -> Result<()> {
    let w = World::open(path)?;
    let t = std::time::Instant::now();
    let s = w.build_map(&MapRequest::matrix(matrix))?;
    println!(
        "matrix {matrix}: {} nodes, {} models, {} materials, {} textures, {} triangles, {} markers, built in {:?}\n\
         animated: {} sliding textures, {} swapping textures, {} models with moving joints",
        s.nodes.len(),
        s.models.len(),
        s.materials.len(),
        s.textures.len(),
        s.triangle_count(),
        s.markers.len(),
        t.elapsed(),
        s.animations.tex_srt.len(),
        s.animations.tex_pattern.len(),
        s.animations.joints.len()
    );
    println!("bounds {:?} .. {:?}", s.min, s.max);
    for w in s.warnings.iter().take(20) {
        println!("warning: {w}");
    }
    Ok(())
}
