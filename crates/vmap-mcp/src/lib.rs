//! MCP server: lets an agent look at the maps and assets of a game through
//! tool calls.
//!
//! The server speaks MCP over standard input and output. It builds on the
//! same crates as the window (`vmap-world` for the data, `vmap-render` for
//! the pictures) and adds only what a conversation needs: a remembered map
//! view that can be moved step by step, and answers as text plus pictures.
//!
//! - [`params`]: what each tool accepts
//! - `view`: the map view (`render_map`, `adjust_view`, `pick`, ...)
//! - `info`: data tools (zones, maps, entities, chunks, text)
//! - `assets`: archives, files, model and texture pictures
//! - `state`: what is remembered, and the worker thread that owns it

mod assets;
mod info;
pub mod params;
mod reply;
mod state;
mod view;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler, ServiceExt};

use params::*;
use reply::Reply;
use state::{State, Worker};

const INSTRUCTIONS: &str = "\
vmap shows the maps and assets of a Generation V Pokemon game: a CTRMap project (.cmproj), an extracted ROM \
folder or a .nds file. It only reads the game and never changes it.

Finding things: list_zones and list_maps (both search by name), get_zone, get_map, list_entities.

Looking at a map: render_map opens a view of a zone or of a whole map matrix and returns a picture. adjust_view \
changes that same view (move, turn, tilt, zoom, switch parts on and off, hide zones) and returns a new picture; \
everything not mentioned stays as it is. pick tells what is drawn at a pixel. follow_warp goes through a door to \
where it leads. get_view reports the current settings.

Assets: list_archives, list_files, describe_file, then render_model for 3D models and get_textures for textures.

Coordinates: one tile is 16 world units, one map chunk is 32 by 32 tiles (512 units). X grows eastwards, Z \
southwards, Y upwards. A tile is [x, z], counted from the north-west corner of the map matrix. Camera yaw is a \
compass heading (0 looks north, 90 east); pitch 90 looks straight down.

Markers in pictures are coloured boxes: NPCs blue, warps pink, triggers yellow (flat), interactable objects green.";

type ToolResult = Result<CallToolResult, ErrorData>;

#[derive(Clone)]
pub struct VmapServer {
    worker: Worker,
    tool_router: ToolRouter<Self>,
}

impl VmapServer {
    fn new(worker: Worker) -> Self {
        Self { worker, tool_router: Self::tool_router() }
    }

    /// Runs a tool on the worker thread. What goes wrong inside a tool is
    /// told to the caller as the tool's answer, so that it can react.
    async fn call<F>(&self, work: F) -> ToolResult
    where
        F: FnOnce(&mut State) -> anyhow::Result<Reply> + Send + 'static,
    {
        Ok(match self.worker.run(work).await {
            Ok(reply) => reply.into_result(),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(format!("{e:#}"))]),
        })
    }
}

#[tool_router]
impl VmapServer {
    // ------------------------------------------------------------ the game

    #[tool(
        description = "Open a game: a CTRMap project (.cmproj), a folder with an extracted ROM, or a .nds file. \
                       Replaces the game that is open. Not needed if the server was started with a game.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn open_game(&self, Parameters(p): Parameters<OpenGameParams>) -> ToolResult {
        self.call(move |s| info::open_game(s, p)).await
    }

    #[tool(
        description = "Read the open game from disk again, to see what was saved since (for example in CTRMap). \
                       The map view is kept.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn reload_game(&self) -> ToolResult {
        self.call(info::reload_game).await
    }

    #[tool(
        description = "Which game is open, where from, how many zones and maps it has, and how coordinates work.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn game_info(&self) -> ToolResult {
        self.call(info::game_info).await
    }

    // ------------------------------------------------------- maps and zones

    #[tool(
        description = "List the map matrices: the grids of chunks that maps are made of. A matrix holds one zone \
                       (a room, a cave floor) or many (the overworld). `search` matches the names of the places on it.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_maps(&self, Parameters(p): Parameters<ListMapsParams>) -> ToolResult {
        self.call(move |s| info::list_maps(s, p)).await
    }

    #[tool(
        description = "List zones (the game's places: a town, a route, a room) with their name, map matrix and area. \
                       `search` matches the name, `matrix` limits the list to one map.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_zones(&self, Parameters(p): Parameters<ListZonesParams>) -> ToolResult {
        self.call(move |s| info::list_zones(s, p)).await
    }

    #[tool(
        description = "Everything about one zone: its name, where it lies on its map, its area (textures, buildings, \
                       lights), how many entities it has, and its header (scripts, text, music, flags).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_zone(&self, Parameters(p): Parameters<ZoneParams>) -> ToolResult {
        self.call(move |s| info::get_zone(s, p)).await
    }

    #[tool(
        description = "The layout of one map matrix: its size, the zones on it with the cells each owns, and the grid \
                       of chunk numbers (and of zone numbers, if the map holds several zones).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_map(&self, Parameters(p): Parameters<MapParams>) -> ToolResult {
        self.call(move |s| info::get_map(s, p)).await
    }

    #[tool(
        description = "The NPCs, warps (doors, stairs, exits), triggers and interactable objects of a zone, or of every \
                       zone on a map matrix, each with its position, tile and all fields of its record.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_entities(&self, Parameters(p): Parameters<ListEntitiesParams>) -> ToolResult {
        self.call(move |s| info::list_entities(s, p)).await
    }

    #[tool(
        description = "One map chunk (a 32 by 32 tile piece of a map): its model, the buildings standing on it, and \
                       where it is used.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_chunk(&self, Parameters(p): Parameters<ChunkParams>) -> ToolResult {
        self.call(move |s| info::get_chunk(s, p)).await
    }

    #[tool(
        description = "The building models of one building bundle (the set of buildings an area uses), with their \
                       model numbers, doors and animations.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_buildings(&self, Parameters(p): Parameters<BuildingsParams>) -> ToolResult {
        self.call(move |s| info::list_buildings(s, p)).await
    }

    #[tool(
        description = "Read a text file of the game: system texts (names, menus) or, with `script`, script texts \
                       (dialogue). A zone's dialogue is the script text file named by `text_file` in get_zone.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn read_text(&self, Parameters(p): Parameters<ReadTextParams>) -> ToolResult {
        self.call(move |s| info::read_text(s, p)).await
    }

    // ------------------------------------------------------------ map view

    #[tool(
        description = "Open a view of a zone or of a whole map matrix and return a picture of it. Every part can be \
                       switched: terrain, buildings, markers (as a whole and per kind: npcs, warps, triggers, \
                       interactables), lighting, textures, vertex colours, animations and their time, season, time of \
                       day, and on maps with several zones which zones are hidden. The camera can be placed freely \
                       (projection, yaw, pitch, distance, target, field of view) or pointed at a zone, a tile or an \
                       entity. Settings left out take their defaults; use adjust_view to change the view afterwards. \
                       The answer describes the view actually used.",
        annotations(read_only_hint = false, open_world_hint = false)
    )]
    async fn render_map(&self, Parameters(p): Parameters<RenderMapParams>) -> ToolResult {
        self.call(move |s| view::render_map(s, p)).await
    }

    #[tool(
        description = "Change the current map view and return a new picture. Takes every setting of render_map \
                       (anything left out stays as it is), plus moves relative to the current camera: pan, move_by, \
                       rotate, tilt, zoom. Use it to walk around a map step by step, to switch parts on and off, or \
                       to hide and show zones.",
        annotations(read_only_hint = false, open_world_hint = false)
    )]
    async fn adjust_view(&self, Parameters(p): Parameters<AdjustViewParams>) -> ToolResult {
        self.call(move |s| view::adjust_view(s, p)).await
    }

    #[tool(
        description = "Describe the current map view without drawing it: what it shows, every setting, the camera.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_view(&self) -> ToolResult {
        self.call(view::get_view).await
    }

    #[tool(
        description = "What is drawn at a pixel of the last picture of the map view: a marker (with the entity's \
                       record), a building or the terrain, with the world position, tile, map cell and zone.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn pick(&self, Parameters(p): Parameters<PickParams>) -> ToolResult {
        self.call(move |s| view::pick(s, p)).await
    }

    #[tool(
        description = "Go through a warp: open a view of the zone it leads to, looking at the warp one arrives at, \
                       and return a picture. The switches of the current view (markers, lighting, season, ...) are kept.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn follow_warp(&self, Parameters(p): Parameters<FollowWarpParams>) -> ToolResult {
        self.call(move |s| view::follow_warp(s, p)).await
    }

    // --------------------------------------------------------------- assets

    #[tool(
        description = "List the game's numbered archives (a/0/0/0 and so on) with what each holds, where known, and \
                       how many of its files the project changed.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_archives(&self, Parameters(p): Parameters<ListArchivesParams>) -> ToolResult {
        self.call(move |s| assets::list_archives(s, p)).await
    }

    #[tool(
        description = "List the files of one archive with their kind (3D model, textures, text, animation, container, \
                       ...) and size. Can be limited to one kind or to the files the project changed.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_files(&self, Parameters(p): Parameters<ListFilesParams>) -> ToolResult {
        self.call(move |s| assets::list_files(s, p)).await
    }

    #[tool(
        description = "What one file of an archive holds: for a model its meshes and materials, for textures their \
                       names, sizes and formats, for a container the files inside, for an animation what it moves, \
                       for text its first lines, otherwise its first bytes.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn describe_file(&self, Parameters(p): Parameters<FileParams>) -> ToolResult {
        self.call(move |s| assets::describe_file(s, p)).await
    }

    #[tool(
        description = "A picture of a 3D model file (or of a model inside a container, such as a building in a \
                       building bundle or the terrain of a map chunk), from any angle, with textures, lighting, \
                       vertex colours and animations switchable.",
        annotations(read_only_hint = false, open_world_hint = false)
    )]
    async fn render_model(&self, Parameters(p): Parameters<RenderModelParams>) -> ToolResult {
        self.call(move |s| assets::render_model(s, p)).await
    }

    #[tool(
        description = "The textures of a texture file (or the ones a model carries) as one picture, with the place of \
                       each on it; or a single texture enlarged, with a palette of choice.",
        annotations(read_only_hint = false, open_world_hint = false)
    )]
    async fn get_textures(&self, Parameters(p): Parameters<TexturesParams>) -> ToolResult {
        self.call(move |s| assets::get_textures(s, p)).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for VmapServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("vmap", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

/// Serves MCP on standard input and output until the client disconnects.
/// `game` is opened at start; without it the client calls `open_game`.
///
/// Standard output carries the protocol, so nothing else may be printed to
/// it; diagnostics go to standard error.
pub fn serve(game: Option<PathBuf>) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().context("starting the runtime")?;
    let result = runtime.block_on(async {
        let worker = Worker::start(game)?;
        let service = VmapServer::new(worker).serve(rmcp::transport::stdio()).await.context("starting the MCP service")?;
        service.waiting().await.context("serving MCP")?;
        anyhow::Ok(())
    });
    // A read of standard input cannot be cancelled; do not wait for it.
    runtime.shutdown_timeout(Duration::from_millis(200));
    result
}
