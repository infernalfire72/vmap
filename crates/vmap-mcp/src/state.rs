//! What the server remembers between tool calls, and the thread it lives on.
//!
//! The opened game and the graphics device stay on one worker thread. Tool
//! calls are sent to it as jobs and answered in order, so a view is never
//! changed by two calls at once.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use anyhow::{anyhow, Context, Result};
use vmap_render::Headless;
use vmap_world::World;

use crate::reply::Reply;
use crate::view::{Built, MapView};

pub struct OpenGame {
    pub world: World,
    pub path: PathBuf,
    /// The map view `render_map` opened and `adjust_view` moves.
    pub view: Option<MapView>,
    /// Maps built lately, the latest first, so that going back to an earlier
    /// combination of options does not build it again.
    pub scenes: Vec<Built>,
    /// For every map chunk, an area it is drawn with. Filled when first needed.
    pub chunk_areas: Option<HashMap<u32, usize>>,
}

impl OpenGame {
    fn open(path: &Path) -> Result<Self> {
        let world = World::open(path).with_context(|| format!("opening {}", path.display()))?;
        Ok(Self { world, path: path.to_path_buf(), view: None, scenes: Vec::new(), chunk_areas: None })
    }

    /// Folders that belong to the game. Nothing is ever written into them.
    pub fn protected_folders(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let opened = self.world.fs.opened_path();
        out.push(if opened.is_dir() { opened.to_path_buf() } else { opened.parent().unwrap_or(opened).to_path_buf() });
        out.extend(self.world.fs.overlay_dir().map(Path::to_path_buf));
        out.extend(self.world.fs.base_dir().map(Path::to_path_buf));
        out
    }
}

#[derive(Default)]
pub struct State {
    pub game: Option<OpenGame>,
    pub headless: Option<Headless>,
    /// Why the game given at start could not be opened.
    pub startup_error: Option<String>,
}

impl State {
    pub fn open(&mut self, path: &Path) -> Result<&OpenGame> {
        let game = OpenGame::open(path)?;
        self.startup_error = None;
        Ok(self.game.insert(game))
    }

    fn no_game(&self) -> anyhow::Error {
        match &self.startup_error {
            Some(e) => anyhow!(
                "no game is open: the one given at start could not be opened ({e}). \
                 Call open_game with the path of a CTRMap project (.cmproj), an extracted ROM folder or a .nds file."
            ),
            None => anyhow!(
                "no game is open. Call open_game with the path of a CTRMap project (.cmproj), \
                 an extracted ROM folder or a .nds file."
            ),
        }
    }

    pub fn game(&mut self) -> Result<&mut OpenGame> {
        let missing = self.no_game();
        self.game.as_mut().ok_or(missing)
    }

    pub fn world(&mut self) -> Result<&World> {
        Ok(&self.game()?.world)
    }

    /// The open game together with the renderer, which is created on first
    /// use: tools that only read data need no graphics device.
    pub fn game_and_renderer(&mut self) -> Result<(&mut OpenGame, &mut Headless)> {
        let missing = self.no_game();
        if self.game.is_none() {
            return Err(missing);
        }
        if self.headless.is_none() {
            self.headless = Some(Headless::new().context("starting the renderer")?);
        }
        let game = self.game.as_mut().ok_or(missing)?;
        Ok((game, self.headless.as_mut().expect("created above")))
    }
}

type Job = Box<dyn FnOnce(&mut State) + Send>;

/// Handle to the worker thread that owns the [`State`].
#[derive(Clone)]
pub struct Worker {
    jobs: mpsc::Sender<Job>,
}

impl Worker {
    /// Starts the worker. A game given here is opened right away.
    pub fn start(initial: Option<PathBuf>) -> Result<Self> {
        let (jobs, queue) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("vmap-mcp-worker".into())
            .spawn(move || {
                let mut state = State::default();
                if let Some(path) = initial {
                    match state.open(&path) {
                        Ok(game) => eprintln!("vmap: opened {} ({})", path.display(), game.world.game.name()),
                        Err(e) => {
                            eprintln!("vmap: {e:#}");
                            state.startup_error = Some(format!("{e:#}"));
                        }
                    }
                }
                for job in queue {
                    job(&mut state);
                }
            })
            .context("starting the worker thread")?;
        Ok(Self { jobs })
    }

    /// Runs `work` on the worker thread and waits for its answer. A panic in
    /// it is reported as an error and leaves the server running.
    pub async fn run<F>(&self, work: F) -> Result<Reply>
    where
        F: FnOnce(&mut State) -> Result<Reply> + Send + 'static,
    {
        let (answer, waiting) = tokio::sync::oneshot::channel();
        let job: Job = Box::new(move |state| {
            let out = std::panic::catch_unwind(AssertUnwindSafe(|| work(state))).unwrap_or_else(|panic| {
                let what = panic
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| panic.downcast_ref::<&str>().copied())
                    .unwrap_or("no message");
                Err(anyhow!("vmap ran into a bug while doing this ({what}). The server is still running."))
            });
            let _ = answer.send(out);
        });
        self.jobs.send(job).map_err(|_| anyhow!("the worker thread has stopped"))?;
        waiting.await.map_err(|_| anyhow!("the worker thread has stopped"))?
    }
}
