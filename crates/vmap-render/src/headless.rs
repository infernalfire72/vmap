//! Rendering without a window, for the command line and the MCP server.

use anyhow::Result;
use vmap_world::Scene;

use crate::{Camera, Gpu, GpuScene, RenderOptions, RenderTarget, Renderer};

/// A graphics device and renderer that draw into memory.
pub struct Headless {
    pub gpu: Gpu,
    pub renderer: Renderer,
    target: Option<RenderTarget>,
}

impl Headless {
    pub fn new() -> Result<Self> {
        let gpu = Gpu::headless()?;
        let renderer = Renderer::new(&gpu.device);
        Ok(Self { gpu, renderer, target: None })
    }

    /// Prepares a scene for drawing.
    pub fn upload(&self, scene: &Scene) -> GpuScene {
        GpuScene::new(&self.gpu.device, &self.gpu.queue, &self.renderer, scene)
    }

    /// Draws a scene and returns the picture as RGBA bytes, row by row from
    /// the top.
    pub fn render(
        &mut self,
        scene: &GpuScene,
        camera: &Camera,
        options: &RenderOptions,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>> {
        if self.target.as_ref().map(|t| (t.width, t.height)) != Some((width, height)) {
            self.target = Some(RenderTarget::new(&self.gpu.device, &self.renderer, width, height));
        }
        let target = self.target.as_ref().expect("the target was just created");
        self.renderer.render(&self.gpu.device, &self.gpu.queue, scene, target, camera, options);
        target.read_pixels(&self.gpu.device, &self.gpu.queue)
    }
}
