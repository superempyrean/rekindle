//! Cursor Cat: a `pointer` + `overlay` plugin.
//!
//! A small cat lives on rekindle's overlay layer and chases the mouse. It reads
//! the cursor through `pointer-moved`, runs oneko's chase/idle state machine on
//! a repeating timer, and draws itself each tick by blitting a frame of its
//! sprite atlas to a position.
//!
//! The atlas is oneko.js's own sprite sheet (`src/oneko.gif`, adryd325/oneko.js,
//! MIT; see `src/oneko.gif.LICENSE`): a 256x128 GIF of 32x32 frames in an 8x4
//! grid. The host sniffs PNG or GIF, so any sheet of that layout drops in. The
//! behaviour is adryd325's oneko.js (MIT).

use rekindle_sdk::prelude::*;

const ATLAS: &[u8] = include_bytes!("oneko.gif");
const FRAME: u32 = 32; // oneko sheet cell size
const SPEED: f32 = 10.0; // nekoSpeed
const TICK_MS: u64 = 100; // oneko runs at ~10 fps

/// Per-direction frame cells `(col, row)` from oneko's `spriteSets`
/// (cell = (|x|, |y|)). Order: N, NE, E, SE, S, SW, W, NW.
const DIRS: [[(u32, u32); 2]; 8] = [
    [(1, 2), (1, 3)], // N
    [(0, 2), (0, 3)], // NE
    [(3, 0), (3, 1)], // E
    [(5, 1), (5, 2)], // SE
    [(6, 3), (7, 2)], // S
    [(5, 3), (6, 1)], // SW
    [(4, 2), (4, 3)], // W
    [(1, 0), (1, 1)], // NW
];

#[derive(Clone, Copy)]
enum Pose {
    Run(usize),
    Idle,
    Alert,
    Tired,
    Sleep,
}

struct Oneko {
    cat_x: f32,
    cat_y: f32,
    mouse_x: f32,
    mouse_y: f32,
    pose: Pose,
    frame: u32,
    idle_time: u32,
    sleep_frame: u32,
    vw: f32,
    vh: f32,
}

impl Plugin for Oneko {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        overlay::register_image("cat", ATLAS).map_err(|e| format!("cat atlas: {e:?}"))?;
        timer::repeat("step", TICK_MS).map_err(|e| format!("timer: {e:?}"))?;
        let (vw, vh) = overlay::viewport();
        Ok(Self {
            cat_x: vw / 2.0,
            cat_y: vh / 2.0,
            mouse_x: vw / 2.0,
            mouse_y: vh * 0.4,
            pose: Pose::Idle,
            frame: 0,
            idle_time: 0,
            sleep_frame: 0,
            vw,
            vh,
        })
    }

    fn on_event(&mut self, ev: Event) {
        if let Event::PointerMoved((x, y)) = ev {
            self.mouse_x = x;
            self.mouse_y = y;
        }
    }

    fn on_timer(&mut self, id: &str) {
        if id == "step" {
            // The client window can resize (and may not exist yet at activation).
            let (vw, vh) = overlay::viewport();
            if vw > 0.0 && vh > 0.0 {
                self.vw = vw;
                self.vh = vh;
            }
            self.step();
            self.render();
        }
    }
}

impl Oneko {
    /// One tick of oneko's chase/idle machine. `diff = cat - mouse`, so the cat
    /// steps *toward* the cursor.
    fn step(&mut self) {
        let dx = self.cat_x - self.mouse_x;
        let dy = self.cat_y - self.mouse_y;
        let dist = (dx * dx + dy * dy).sqrt();

        if dist < SPEED || dist < 48.0 {
            self.idle_step();
            return;
        }

        // Perk up for a few frames after idling before giving chase.
        if self.idle_time > 1 {
            self.pose = Pose::Alert;
            self.idle_time = self.idle_time.min(7).saturating_sub(1);
            return;
        }
        self.sleep_frame = 0;

        let nx = dx / dist;
        let ny = dy / dist;
        self.pose = Pose::Run(direction(nx, ny));
        self.frame = self.frame.wrapping_add(1);

        // `max` keeps the range valid when the viewport is tiny or unknown.
        self.cat_x = (self.cat_x - nx * SPEED).clamp(16.0, (self.vw - 16.0).max(16.0));
        self.cat_y = (self.cat_y - ny * SPEED).clamp(16.0, (self.vh - 16.0).max(16.0));
    }

    fn idle_step(&mut self) {
        self.idle_time = self.idle_time.saturating_add(1);
        if self.idle_time > 24 {
            let f = self.idle_time - 24;
            if f < 8 {
                self.pose = Pose::Tired;
            } else {
                self.pose = Pose::Sleep;
                self.sleep_frame = f - 8;
            }
        } else {
            self.pose = Pose::Idle;
        }
    }

    /// Atlas cell `(col, row)` for the current pose.
    fn cell(&self) -> (u32, u32) {
        match self.pose {
            Pose::Run(d) => DIRS[d][(self.frame % 2) as usize],
            Pose::Idle => (3, 3),
            Pose::Alert => (7, 3),
            Pose::Tired => (3, 2),
            Pose::Sleep => [(2, 0), (2, 1)][((self.sleep_frame / 4) % 2) as usize],
        }
    }

    fn render(&self) {
        let (col, row) = self.cell();
        let s = overlay::sprite(
            "cat",
            "cat",
            (col * FRAME, row * FRAME, FRAME, FRAME),
            (
                self.cat_x - 16.0,
                self.cat_y - 16.0,
                FRAME as f32,
                FRAME as f32,
            ),
        );
        // A frame that fails to draw is replaced on the next tick.
        let _ = overlay::draw(&s);
    }
}

/// oneko's direction pick: a unit vector `(nx, ny) = (cat - mouse)/dist`
/// chooses a sprite by which axes exceed 0.5. Returns an index into `DIRS`.
fn direction(nx: f32, ny: f32) -> usize {
    let n = ny > 0.5;
    let s = ny < -0.5;
    let w = nx > 0.5;
    let e = nx < -0.5;
    if n && w {
        7 // NW
    } else if n && e {
        1 // NE
    } else if s && w {
        5 // SW
    } else if s && e {
        3 // SE
    } else if n {
        0
    } else if s {
        4
    } else if w {
        6
    } else {
        2 // E (default)
    }
}

rekindle_sdk::plugin!(Oneko);
