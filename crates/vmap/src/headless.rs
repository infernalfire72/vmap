//! Rendering without a window: used by `vmap render`.

use anyhow::{anyhow, bail, Context, Result};
use glam::Vec3;
use vmap_render::{Headless, OrbitCamera, RenderOptions};
use vmap_world::{MapRequest, World};

use crate::RenderArgs;

pub fn render(args: &RenderArgs) -> Result<()> {
    let world = World::open(&args.game)?;
    let (matrix, only_zone) = match (args.matrix, args.zone) {
        (Some(m), z) => (m, z),
        (None, Some(z)) => {
            let header = world.zones.get(z as usize).ok_or_else(|| anyhow!("there is no zone {z}"))?;
            (header.matrix_id as usize, Some(z))
        }
        (None, None) => bail!("give --matrix or --zone"),
    };
    let request = MapRequest {
        matrix,
        only_zone,
        season: args.season,
        day_part: 1,
        terrain: !args.no_terrain,
        buildings: !args.no_buildings,
        markers: !args.no_markers,
        animations: true,
        hidden_zones: args.hide_zones.clone(),
        hide_unassigned: args.hide_unassigned,
    };
    let scene = world.build_map(&request)?;
    for w in scene.warnings.iter().take(10) {
        eprintln!("warning: {w}");
    }

    let mut headless = Headless::new()?;
    let mut gpu_scene = headless.upload(&scene);
    gpu_scene.animate(&headless.gpu.queue, args.time);
    let aspect = args.width as f32 / args.height.max(1) as f32;

    let mut cam = if args.top_down {
        OrbitCamera::top_down(scene.min, scene.max, aspect)
    } else {
        let mut c = OrbitCamera { yaw: args.yaw.to_radians(), pitch: args.pitch.to_radians(), ..Default::default() };
        c.frame(scene.min, scene.max, aspect);
        c
    };
    if let Some(t) = &args.target {
        if t.len() != 3 {
            bail!("--target needs three numbers: x,y,z");
        }
        cam.target = Vec3::new(t[0], t[1], t[2]);
    }
    if let Some(d) = args.distance {
        cam.distance = d;
    }

    let options = RenderOptions {
        lighting: !args.no_lighting,
        markers: !args.no_markers,
        marker_kinds: [!args.no_npcs, !args.no_warps, !args.no_triggers, !args.no_interactables],
        ..Default::default()
    };
    let pixels = headless.render(&gpu_scene, &cam.camera(aspect), &options, args.width, args.height)?;
    image::save_buffer(&args.out, &pixels, args.width, args.height, image::ColorType::Rgba8)
        .with_context(|| format!("writing {}", args.out.display()))?;
    println!(
        "wrote {} ({}x{}, matrix {matrix}, {} triangles, {} draw calls, {} animated parts, adapter: {})",
        args.out.display(),
        args.width,
        args.height,
        scene.triangle_count(),
        gpu_scene.draw_count(),
        scene.animations.len(),
        headless.gpu.adapter_name
    );
    Ok(())
}
