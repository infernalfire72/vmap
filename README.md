# vmap

A map viewer and asset viewer for Generation V Pokémon games (Black, White,
Black 2, White 2) and hacks of them. It reads a CTRMap project, an extracted
ROM folder or a `.nds` file, and never writes to any of them.

Unlike CTRMap, which shows one zone at a time, the map viewer stitches a whole
map matrix together: the complete overworld, with every zone's own textures
and buildings, in one view.

## Running

```
cargo run --release              # opens the last game you opened
cargo run --release -- path\to\project.cmproj
```

The window has two tabs.

**Map** lists every map. Large maps that hold several zones appear once;
searching also finds the individual zones on them and jumps there. Rooms that
share one layout appear once per zone. In the view:

- left drag moves, right drag rotates, the wheel zooms towards the cursor
- *3D* / *Top-down* / *Fit* change the view; the overview in the corner shows
  every chunk coloured by zone and can be clicked to jump
- markers show NPCs (blue), warps (pink), triggers (yellow) and interactables
  (green); click one for details, and *Follow warp* opens where it leads
- season and time of day change textures and lighting; *Animations* plays
  water, flowers, windmills and the like
- on a map with several zones, the list *Zones on this map* has an eye in
  front of every zone: click it to hide the zone (the eye is struck through)
  and again to show it. *Without a zone* does the same for the sea and
  forest that fill the gaps between zones, *Show all* brings everything back
- *Reload* reads the files again, for example after saving in CTRMap

**Assets** lists every archive of the game with its role where known, and
marks the files your project changed. Previews: 3D models with their textures
(buildings with their animations), texture sets, text files, what an
animation file contains, and a hex view for everything else.

## Without a window

The same code renders and reports without opening a window, for scripts and
agents:

```
vmap info <game>                       zones, areas and maps
vmap zones <game>                      every zone with its map and area
vmap map-info <game> --matrix 0        what a stitched map contains
vmap render <game> --matrix 0 --top-down -o overworld.png
vmap render <game> --zone 389 --pitch 35 --time 1.5 -o town.png
vmap chunk <game> 254                  a chunk's buildings
vmap buildings <game> 1                a building bundle's models
vmap text <game> 89                    lines of a text file
```

`vmap render --help` lists the camera options and the switches: `--no-markers`
(or one kind: `--no-npcs`, `--no-warps`, `--no-triggers`,
`--no-interactables`), `--no-buildings`, `--no-terrain`, `--no-lighting`,
`--hide-zone <ZONE>` (repeatable) and `--hide-unassigned`.

## For agents: the MCP server

`vmap mcp [game]` serves the maps and assets to an agent over the Model
Context Protocol (on standard input and output). The agent gets pictures and
data back, and can walk around a map step by step. To add it to Claude Code:

```
claude mcp add vmap -- C:\path\to\vmap.exe mcp C:\path\to\project.cmproj
```

Other clients take the same command in their configuration:

```json
{ "mcpServers": { "vmap": {
    "command": "C:\\path\\to\\vmap.exe",
    "args": ["mcp", "C:\\path\\to\\project.cmproj"]
} } }
```

Without a game on the command line, the agent opens one with `open_game`.

| Tool | What it does |
| --- | --- |
| `render_map` | Opens a view of a zone or a whole map matrix and returns a picture. |
| `adjust_view` | Changes that view and returns a new picture; what is not mentioned stays. |
| `get_view` | The current view's settings and camera, without drawing. |
| `pick` | What is drawn at a pixel: marker, building or terrain, with position, tile and zone. |
| `follow_warp` | Goes through a door: a view of where a warp leads, looking at the arrival point. |
| `list_maps`, `list_zones` | Find maps and zones, by name if wanted. |
| `get_map`, `get_zone` | A map's grid of chunks and zones; everything a zone's header says. |
| `list_entities` | NPCs, warps, triggers and interactables with positions and all fields. |
| `get_chunk`, `list_buildings` | A chunk and the buildings on it; the models of a building bundle. |
| `read_text` | System and script text files. |
| `list_archives`, `list_files`, `describe_file` | The asset catalogue, down to what a file holds. |
| `render_model`, `get_textures` | Pictures of a model from any angle, and of textures. |
| `open_game`, `reload_game`, `game_info` | Open a game, read it again after saving in CTRMap, summary. |

The map view takes every switch the window has, and a few more:

- parts: `terrain`, `buildings`, `markers` and per kind `npcs`, `warps`,
  `triggers`, `interactables`
- look: `lighting`, `textures`, `vertex_colors`, `animations` with `time`,
  `season`, `time_of_day`, `background`
- zones of a map that holds several: `hide_zones`, `show_zones`,
  `only_zones`, `show_all_zones`, `unassigned_cells`
- camera: `projection` (perspective or top-down), `yaw`, `pitch`, `distance`,
  `fov`, `target`, or `look_at_zone`, `look_at_tile`, `look_at_entity`, `fit`;
  in `adjust_view` also the relative `pan`, `move_by`, `rotate`, `tilt`, `zoom`
- output: `width`, `height`, `list_markers` and `list_zones` (what is in the
  picture, with pixel positions), `save_to` (a PNG file), `image: false` (data
  only)

Every answer says which settings and camera were actually used. A picture is
sent as PNG, or as JPEG when the PNG would be larger than about half a
megabyte; `save_to` always writes the PNG, and refuses paths inside the
game's own folders.

## Layout

| Crate | What it does |
| --- | --- |
| `vmap-formats` | Parsers only: ROM, archives, compression, models, textures, animations, map data, text. No file access, no graphics. |
| `vmap-fs` | Opens a game and layers a CTRMap project's changed files over the base ROM. Read-only. |
| `vmap-world` | Turns game data into scenes: stitched maps, single models, the asset catalogue. |
| `vmap-render` | Draws a scene with wgpu into an offscreen image. Used by the window and by `vmap render` alike. |
| `vmap-ui` | The window (egui). Only arranges panels and handles input. |
| `vmap-mcp` | The MCP server. Keeps the agent's map view and answers tool calls with text and pictures. |
| `vmap` | The executable: the window, the commands above and `vmap mcp`. |

`vmap-ui` and `vmap-mcp` are two front ends over the same `vmap-world` and
`vmap-render`; neither knows about the other.

## Not there yet

- Black 2 / White 2 are wired up (archive numbers, area rules) but untested.
- Map replacements (chunks swapped by game version or story progress) are
  not applied; maps show their default chunks.
- NPCs are shown as markers, not as their sprites.
- Animations that a script or the clock starts (doors on a map, lamps at
  night) do not play on maps; material colour and visibility animations are
  not read.
- 2D graphics, sound and scripts have no preview; nothing can be exported.

## Licence

GPL-3.0-or-later. The file formats were worked out with CTRMap
(<https://github.com/ds-pokemon-hacking/CTRMap-CE>,
<https://github.com/ds-pokemon-hacking/CTRMapV>, both GPL-3.0) as the
reference.
