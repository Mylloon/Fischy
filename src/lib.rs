use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use enigo::{Axis, Coordinate::Abs, Enigo, InputResult, Mouse};
use image::RgbImage;
use rand::RngExt;
use sysinfo::{ProcessRefreshKind, RefreshKind, System};
use xcap::{Frame, Monitor};

use crate::utils::{
    colors::ColorTarget,
    geometry::{Dimensions, Point, Region},
};

pub mod utils;

#[must_use]
#[cfg(target_os = "linux")]
pub fn get_roblox_executable_name<'a>() -> &'a str {
    "sober"
}

#[cfg(target_os = "windows")]
#[must_use]
pub fn get_roblox_executable_name<'a>() -> &'a str {
    "RobloxPlayerBeta.exe"
}

/// Check if a process is running
#[must_use]
pub fn check_running(name: &str) -> bool {
    let sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
    );

    sys.processes()
        .values()
        .any(|process| process.name() == name)
}

pub struct ScreenRecorder {
    old_frame: Arc<Mutex<Frame>>,

    pub dimensions: Dimensions,
}

impl ScreenRecorder {
    /// Initialize screen recording
    ///
    /// # Errors
    /// Can't capture screen
    pub fn new() -> Result<Self, String> {
        let monitor = Monitor::from_point(100, 100)
            .map_err(|e| format!("Couldn't find any monitor to capture: {e}"))?;
        let (video_recorder, sx) = monitor
            .video_recorder()
            .map_err(|e| format!("Couldn't instantiate the the video recorder: {e}"))?;

        video_recorder
            .start()
            .map_err(|e| format!("Couldn't start the video recording: {e}"))?;

        // We will always have a frame
        let first_frame = sx
            .recv()
            .map_err(|e| format!("Can't screenshot monitor: {e}"))?;

        let old_frame = Arc::new(Mutex::new(first_frame.clone()));

        // FIXME: Verify is next comment is true for the new xcap library,
        //        This workaround was needed for scap.
        //
        // TODO: Remove this after confirmation

        // We have to create a thread that consume all our frames to prevent a memory explosion
        let frame_clone = Arc::clone(&old_frame);
        thread::spawn(move || {
            while let Ok(frame) = sx.recv() {
                // Try to store the latest frame
                if let Ok(mut guard) = frame_clone.try_lock() {
                    *guard = frame;
                }
            }
        });

        Ok(Self {
            old_frame,
            dimensions: Dimensions {
                width: first_frame.width,
                height: first_frame.height,
            },
        })
    }

    fn take_frame(&mut self) -> Result<Frame, String> {
        match self.old_frame.lock() {
            Ok(f) => Ok(f.clone()),
            Err(e) => Err(format!("Can't read stored frame: {e}")),
        }
    }

    /// Take a screenshot
    ///
    /// # Errors
    /// Received unprocessable frame
    pub fn take_screenshot(&mut self) -> Result<RgbImage, String> {
        self.take_frame()
            .map(|f| {
                RgbImage::from_raw(
                    f.width,
                    f.height,
                    // Frames from XCap are RGBA
                    f.raw
                        .chunks(4)
                        .flat_map(|pixel| pixel.iter().take(3))
                        .copied()
                        .collect(),
                )
            })
            .and_then(|f| f.ok_or("Can't convert image from raw data".into()))
    }
}

impl Region {
    /// Search a color in the region
    fn search_color_impl<Xs, Ys>(
        screen: &RgbImage,
        targets: &[ColorTarget],
        xs: Xs,
        ys: &Ys,
    ) -> Option<Point>
    where
        Xs: IntoIterator<Item = u32>,
        Ys: IntoIterator<Item = u32> + Clone,
    {
        xs.into_iter()
            .flat_map(|x| ys.clone().into_iter().map(move |y| (x, y)))
            .find(|&(x, y)| targets.iter().any(|t| t.matches(screen.get_pixel(x, y))))
            .map(|(x, y)| Point { x, y })
    }

    /// Search a color in the middle row, left to right
    #[must_use]
    pub fn search_color_mid_ltr(
        &self,
        screen: &RgbImage,
        targets: &[ColorTarget],
    ) -> Option<Point> {
        let [x_min, y_min, x_max, y_max] = self.corners();
        let y = y_min.midpoint(y_max);
        Self::search_color_impl(screen, targets, x_min..=x_max, &[y])
    }

    /// Search a color in the left half
    #[must_use]
    pub fn search_color_left_half(
        &self,
        screen: &RgbImage,
        targets: &[ColorTarget],
    ) -> Option<Point> {
        let [x_min, y_min, _, y_max] = self.corners();
        let half_width = self.get_size().width / 2;
        Self::search_color_impl(
            screen,
            targets,
            x_min..=(x_min + half_width),
            &(y_min..=y_max),
        )
    }

    /// Search a color in the right half
    #[must_use]
    pub fn search_color_right_half(
        &self,
        screen: &RgbImage,
        targets: &[ColorTarget],
    ) -> Option<Point> {
        let [_, y_min, x_max, y_max] = self.corners();
        let half_width = self.get_size().width / 2;
        Self::search_color_impl(
            screen,
            targets,
            ((x_max - half_width)..=x_max).rev(),
            &(y_min..=y_max),
        )
    }
}

pub struct Stats {
    pub enabled: bool,
    /// Shake count
    pub shakes: Box<u64>,
    /// Reel count
    pub reels: Box<u64>,
    /// Fish count
    fishes: Box<u64>,
    /// Total fishing time in seconds
    total_fishing_time: Box<u64>,
    /// Maximum fishing time in seconds
    max_fishing_time: Box<u64>,
    /// Minimum fishing time in seconds
    min_fishing_time: Box<u64>,
}

impl Stats {
    #[must_use]
    pub fn new(enabled: bool) -> Self {
        Stats {
            enabled,
            reels: Box::new(0),
            shakes: Box::new(0),
            fishes: Box::new(0),
            total_fishing_time: Box::new(0),
            max_fishing_time: Box::new(u64::MIN),
            min_fishing_time: Box::new(u64::MAX),
        }
    }

    fn print_stats(self) {
        println!("Shake count: {}", self.shakes);
        println!("Reels tries count: {}", self.reels);
        println!("Missed reels count: {}", *self.reels - *self.fishes);
        println!("Fishes count: {}", self.fishes);
        if *self.max_fishing_time != u64::MIN {
            println!(
                "Average fishing time: {}s (maximum was {}s, minimum was {}s)",
                (*self.total_fishing_time / *self.reels),
                self.max_fishing_time,
                self.min_fishing_time
            );
        }
    }

    pub fn print(self) {
        if self.enabled {
            self.print_stats();
        }
    }

    pub fn add_fishing_time(&mut self, time: u64) {
        *self.fishes += 1;
        *self.total_fishing_time += time;
        *self.max_fishing_time = (*self.max_fishing_time).max(time);
        if time > 0 {
            *self.min_fishing_time = (*self.min_fishing_time).min(time);
        }
    }
}

/// Sleep `n` millis with random jitter in millis
pub fn sleep_with_jitter(ms: u64, jitter: i64, cond: &AtomicBool) {
    sleep(
        Duration::from_millis(
            (ms.cast_signed() + rand::rng().random_range(-jitter..=jitter))
                .max(0)
                .cast_unsigned(),
        ),
        cond,
    );
}

/// Sleep for `duration`
pub fn sleep(duration: Duration, cond: &AtomicBool) {
    let chunk = Duration::from_millis(1);
    let start = Instant::now();

    while start.elapsed() < duration && !cond.load(Ordering::Relaxed) {
        let remaining = duration.checked_sub(start.elapsed()).unwrap_or_default();
        thread::sleep(if remaining < chunk { remaining } else { chunk });
    }
}

pub trait Scroller {
    /// Scroll with fixes for Roblox
    ///
    /// # Errors
    /// If couldn't scroll
    fn scroll_ig(&mut self, length: i32, axis: Axis) -> InputResult<()>;

    /// Return maximum scroll needed for Fisch
    fn max_scroll() -> i32;

    /// Move mouse with fixes for Roblox by smoothing movement if needed
    ///
    /// # Errors
    /// If couldn't move the mouse
    fn move_mouse_ig_abs(&mut self, x: i32, y: i32) -> InputResult<()>;
}

impl Scroller for Enigo {
    fn scroll_ig(&mut self, length: i32, axis: Axis) -> InputResult<()> {
        #[cfg(not(target_os = "windows"))]
        {
            self.scroll(length, axis)
        }

        #[cfg(target_os = "windows")]
        {
            use std::thread::sleep;

            let step = length.signum();
            (0..length.abs()).try_for_each(|_| {
                sleep(Duration::from_millis(30));
                self.scroll(step, axis)
            })
        }
    }

    /// Return maximum scroll needed for Fisch
    fn max_scroll() -> i32 {
        #[cfg(not(target_os = "windows"))]
        {
            8
        }

        #[cfg(target_os = "windows")]
        {
            13
        }
    }

    fn move_mouse_ig_abs(&mut self, x: i32, y: i32) -> InputResult<()> {
        #[cfg(target_os = "windows")] // smooth movement on Windows
        {
            use crate::utils::helpers::BadCast;

            let (start_x, start_y) = self.location()?;
            let dx = (x - start_x).bad_cast();
            let dy = (y - start_y).bad_cast();

            let steps = 5;
            for i in 1..=steps {
                let progress = i.bad_cast() / steps.bad_cast();
                let eased_progress = 1.0 - (1.0 - progress).powi(2);

                let current_x = start_x + (dx * eased_progress).bad_cast();
                let current_y = start_y + (dy * eased_progress).bad_cast();

                self.move_mouse(current_x, current_y, Abs)?;

                thread::sleep(Duration::from_millis(10));
            }
        }

        self.move_mouse(x, y, Abs)
    }
}
