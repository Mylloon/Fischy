use std::ops::AddAssign;
use std::process::exit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use enigo::{
    Axis::Vertical,
    Button,
    Coordinate::Rel,
    Direction::{Click, Press, Release},
    Enigo, Mouse, Settings,
};
use fischy::utils::{
    args::rod_position_parser,
    checks::{chat_check, quest_check, scoreboard_check, server_alive_check, treasure_maps_check},
    clickers::{appraise_items, fetch_crab_cages, place_crab_cages, sell_items, summon_totem},
    colors::ColorTarget,
    fishing::Rod,
    fishing::{MiniGame, Move},
    geometry::{Dimensions, Point, Region},
    helpers::BadCast,
};
use fischy::{
    ScreenRecorder, Scroller, Stats, check_running, get_roblox_executable_name, sleep,
    sleep_with_jitter,
};
use image::{Rgb, RgbImage};
use log::{info, warn};
use rdev::{Event, EventType::KeyPress, Key, listen, simulate};
use window_raiser::raise;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

#[derive(Parser)]
#[command(
    version,
    about,
    long_about = r#"
To make this program work:
  - be sure to run Roblox maximised on your leftest screen
  - if you have your taskbar on the side of your screen, use fullscreen (F11)
  - don't be too close from the edge when fishing as it moves you a little"#
)]
#[allow(clippy::struct_excessive_bools)]
struct Args {
    /// Sleep between shakes to prevent capturing the cursor
    #[arg(long, default_value_t = 400)]
    lag: u32,

    /// Maximum shake count
    #[arg(long, default_value_t = 40)]
    max_shake_count: u8,

    /// Do only the shake part
    #[arg(short, long, default_value_t = false)]
    shake_only: bool,

    /// Don't print stats
    #[arg(long, default_value_t = false)]
    no_stats: bool,

    /// Disable camera setup looking down at the start
    #[arg(long)]
    no_camera_setup: bool,

    /// Placement of the fishing rod in the hotbar
    #[arg(long, default_value_t = 1, value_parser = rod_position_parser)]
    rod_position_hotbar: u16,

    /// Change reaction time, in milliseconds
    #[arg(long, default_value_t = 50)]
    sensitivity: u64,

    /// Debugging purposes
    #[arg(short, long, default_value_t = false)]
    verbose: bool,

    /// Click n times to put crab cages (you have to be already holding it being able to put them,
    /// only work where crab cages are stackable)
    #[arg(short, long, num_args(0..=1), default_missing_value = "65535")]
    place_crab_cages: Option<u16>,

    /// Retrieves crab cages (you have to be able to see the button to collect them,
    /// only work where crab cages are stackable)
    #[arg(short, long, num_args(0..=1), default_missing_value = "65535")]
    fetch_crab_cages: Option<u16>,

    /// Summon totem (you have to hold the one you want to use)
    #[arg(short('t'), long, num_args(0..=1), default_missing_value = "65535")]
    summon_totem: Option<u16>,

    /// Sell items to merchant (you have to see the "I'd like to sell this" with 2 dialogs options)
    #[arg(short('m'), long, num_args(0..=1), default_missing_value = "65535")]
    sell_items: Option<u16>,

    /// Appraise items (you have to hold the fish
    ///                 and see the "Can you appraise this fish?" with 3 dialogs options).
    /// By default, you have to press `Return` to do another appraisal
    /// if forced (set to true), then confirmation will be automatic (good for Shiny and Sparkling)
    #[arg(short('a'), long, num_args(0..=1), default_missing_value = "false")]
    appraise_items: Option<bool>,
}

/// Init logger based on verbose option
fn init_logger(verbose: bool) {
    let mut builder = env_logger::Builder::new();
    if verbose {
        builder.filter_level(log::LevelFilter::Info);
    } else {
        builder.filter_level(log::LevelFilter::Warn);
    }
    builder.init();
}

fn pre_init() -> Args {
    let args = Args::parse();
    init_logger(args.verbose);

    info!("Starting Roblox Fishing Macro");
    if args.verbose {
        info!("Debug mode enabled");
    }

    // Check that Roblox is running
    let roblox = get_roblox_executable_name();
    assert!(check_running(roblox), "Roblox not found.");
    info!("Roblox found.");

    match raise(roblox) {
        Ok(()) => info!("Raised Roblox window"),
        Err(err) => {
            warn!("Failed raising roblox window: {err}");
            let wait = 3;
            warn!("You have to focus it yourself (waiting {wait} seconds).");
            sleep(Duration::from_secs(wait), &SHUTDOWN);
        }
    }

    // Register keybinds to close the script
    register_keybinds();

    args
}

fn main() {
    let args = pre_init();

    let mut enigo = Enigo::new(&Settings::default()).expect("Failed to initialize I/O engine");
    let mut recorder = ScreenRecorder::new().expect("Failed to initialize screen monitoring");

    info!(
        "Detected screen dimensions: {}x{}",
        recorder.dimensions.width, recorder.dimensions.height
    );

    // Calculate regions based on screen dimensions
    let screen = recorder
        .take_screenshot()
        .expect("Couldn't take screenshot");

    // Click once, so we are sure we grabbed the window focus
    // TODO: Is this a Linux-only thing?
    enigo
        .button(Button::Left, Click)
        .expect("Failed while being sure to focus the window");

    let roblox_button_position = recorder.dimensions.find_roblox_button(&screen);

    scoreboard_check(&mut enigo, &screen);
    if let Some(p) = roblox_button_position.as_ref() {
        // Quest check after chat check, because chat window moves the arrow
        chat_check(&mut enigo, &screen, p, &SHUTDOWN);
        quest_check(&mut enigo, &screen, p, &SHUTDOWN);
    }

    let mut mini_game_region = recorder.dimensions.calculate_mini_game_region();
    let shake_region = recorder
        .dimensions
        .calculate_shake_region(roblox_button_position);
    let safe_point = recorder
        .dimensions
        .calculate_safe_point(&vec![&mini_game_region, &shake_region])
        .expect("Couldn't find any safe point, no region found.");

    #[cfg(feature = "imageproc")]
    {
        use fischy::utils::debug::Drawable;
        use std::sync::Arc;

        let screen = Arc::new(screen);

        mini_game_region
            .clone()
            .draw_async(screen.clone(), "mini_game.png", true);
        shake_region
            .clone()
            .draw_async(screen.clone(), "shake_region.png", true);
        safe_point
            .clone()
            .draw_async(screen, "safe_point.png", true);
    }

    if let Some(clicks) = args.place_crab_cages {
        place_crab_cages(&mut enigo, &safe_point, clicks, &SHUTDOWN);
        exit(0);
    }

    if let Some(cages) = args.fetch_crab_cages {
        fetch_crab_cages(&mut enigo, &safe_point, cages, &SHUTDOWN);
        exit(0);
    }

    if let Some(totems) = args.summon_totem {
        summon_totem(&mut enigo, &safe_point, totems, &SHUTDOWN);
        exit(0);
    }

    if let Some(items) = args.sell_items {
        sell_items(&mut enigo, &safe_point, items, &mut recorder, &SHUTDOWN);
        exit(0);
    }

    if let Some(deactivate_user_confirmation) = args.appraise_items {
        appraise_items(
            &mut enigo,
            &safe_point,
            &mut recorder,
            &SHUTDOWN,
            deactivate_user_confirmation,
        );
        exit(0);
    }

    let mut stats = Stats::new(!args.no_stats);

    if !args.no_camera_setup {
        initialize_viewpoint(&mut enigo, &recorder.dimensions, &SHUTDOWN);
    }

    macro_loop(
        &mut enigo,
        &mut recorder,
        &safe_point,
        &mut mini_game_region,
        &shake_region,
        &args,
        &mut stats,
    );

    stats.print();
}

/// Shake the rod and catch fishes
fn macro_loop(
    enigo: &mut Enigo,
    recorder: &mut ScreenRecorder,
    safe_point: &Point,
    mini_game: &mut MiniGame,
    shake_region: &Region,
    args: &Args,
    stats: &mut Stats,
) {
    let mut last_shake_time = Instant::now();
    let mut shake_count = 0;
    let mut tries_fishing = 0;

    // Initial reel
    reels(
        enigo,
        &mut last_shake_time,
        &mut shake_count,
        safe_point,
        stats,
    );

    while !SHUTDOWN.load(Ordering::Relaxed) {
        // Check for shake
        if let Some((Point { x, y }, image)) =
            check_shake(enigo, recorder, shake_region, safe_point, args)
            && server_alive_check(&image, &SHUTDOWN)
        {
            treasure_maps_check(enigo, &image, &SHUTDOWN);

            // Click at the shake position
            info!("Shake @ ({x}, {y})");
            enigo
                .move_mouse_ig_abs(x.cast_signed(), y.cast_signed())
                .expect("Failed moving mouse to shake bubble");
            sleep(Duration::from_millis(100), &SHUTDOWN); // we may move the mouse too fast
            enigo
                .button(Button::Left, Click)
                .expect("Failed clicking to shake bubble");

            stats.shakes.add_assign(1);

            // Too much tries
            if shake_count > args.max_shake_count {
                reels(
                    enigo,
                    &mut last_shake_time,
                    &mut shake_count,
                    safe_point,
                    stats,
                );
                continue;
            }

            shake_count += 1;
            last_shake_time = Instant::now();
            continue;
        }

        // Timeout
        if last_shake_time.elapsed() > Duration::from_secs(5) {
            reels(
                enigo,
                &mut last_shake_time,
                &mut shake_count,
                safe_point,
                stats,
            );
            continue;
        }

        // Take screenshot for processing
        let screen = recorder
            .take_screenshot()
            .expect("Failed taking screenshot");

        let is_hooked = mini_game.any_fish_hooked(&screen);

        // Called during the first fish
        if mini_game.rod.is_none() && is_hooked {
            // Wait slide-in animation of the minigame
            sleep(Duration::from_millis(150), &SHUTDOWN);
            let fresher_screen = recorder
                .take_screenshot()
                .expect("Failed taking screenshot");
            if let Ok(()) = mini_game.refine_area(&fresher_screen) {
                info!("Updating minigame structure");
            } else {
                info!("Failed updating the minigame structure");
                continue;
            }

            #[cfg(feature = "imageproc")]
            {
                use fischy::utils::debug::Drawable;
                use std::sync::Arc;

                mini_game.clone().draw_async(
                    Arc::new(fresher_screen.clone()),
                    "mini_game_refined.png",
                    true,
                );
            }

            mini_game.initialize_rod(Rod::new(&fresher_screen, mini_game));
        }

        // When one fish is on the hook
        if is_hooked {
            // Main fishing logic loop
            info!("Fishing...");
            fishing_loop(enigo, recorder, mini_game, args, stats);
            info!("Fishing ended!");

            // After fishing interaction, reel again
            sleep_with_jitter(2000, 100, &SHUTDOWN);
            reels(
                enigo,
                &mut last_shake_time,
                &mut shake_count,
                safe_point,
                stats,
            );
            tries_fishing = 0;
            continue;
        } else if tries_fishing >= 10 {
            tries_fishing = 0;
            reselect_rod(args).expect("Couldn't select the rod");
            continue;
        }

        tries_fishing += 1;
    }
}

/// Select the rod from the hotbar
/// FIXME: This doesn't work properly on all platforms (Wayland)
fn reselect_rod(args: &Args) -> Result<(), rdev::SimulateError> {
    simulate(&rdev::EventType::KeyPress(
        match args.rod_position_hotbar {
            1 => Ok(Key::Num1),
            2 => Ok(Key::Num2),
            3 => Ok(Key::Num3),
            4 => Ok(Key::Num4),
            5 => Ok(Key::Num5),
            6 => Ok(Key::Num6),
            7 => Ok(Key::Num7),
            8 => Ok(Key::Num8),
            9 => Ok(Key::Num9),
            _ => Err(()),
        }
        .expect("Unkown requested key"),
    ))
}

/// Catch a fish!
fn fishing_loop(
    enigo: &mut Enigo,
    recorder: &mut ScreenRecorder,
    mini_game: &mut MiniGame,
    args: &Args,
    stats: &mut Stats,
) {
    let fishing_time = Instant::now();
    let mut previous_hook_x = 0;
    while !SHUTDOWN.load(Ordering::Relaxed) {
        let screen = recorder
            .take_screenshot()
            .expect("Couldn't take screenshot");

        let hook = mini_game.find_hook(&screen);

        // Get current fish position
        let fish_x = if hook.fish_on
            && let Some(Point { x, .. }) = mini_game.get_fish(&screen)
        {
            if args.shake_only {
                continue;
            }
            x.cast_signed()
        } else {
            info!("The bite is over");
            enigo.button(Button::Left, Release).expect("Packup the rod");
            stats.add_fishing_time(fishing_time.elapsed().as_secs());
            break;
        };

        let fish_pos_as_minigame_bar_percentage = (((fish_x - mini_game.point1.x.cast_signed())
            .bad_cast()
            / mini_game.get_size().width.cast_signed().bad_cast())
            * 100.)
            .bad_cast();

        // % treshold defining extreme edge
        let mini_game_percentage_edge_treshold =
            // half of the hook bar
            (((hook.length / 2) * 100).cast_signed().bad_cast()
                / mini_game.get_size().width.cast_signed().bad_cast())
            .bad_cast();

        // Check if fish is very far left or very far right
        if fish_pos_as_minigame_bar_percentage < mini_game_percentage_edge_treshold {
            info!("Giving some slack...");
            enigo
                .button(Button::Left, Release)
                .expect("Failed giving slack");
            continue;
        } else if fish_pos_as_minigame_bar_percentage > 100 - mini_game_percentage_edge_treshold {
            info!("Tighting the line...");
            enigo
                .button(Button::Left, Press)
                .expect("Failed keeping the line tight");
            continue;
        }

        let hook_x = hook
            .position
            .expect("Can't find the hook")
            .absolute_mid_x
            .cast_signed();

        // INFO: As a side effect I did not explain yet, it tends to keep
        //       the fish on the 20% of the hook bar (pretty smart strategy IMO)
        let range = fish_x - hook_x;
        let speed = hook_x - previous_hook_x;
        match Move::decision(hook.length.cast_signed(), range, speed, 5) {
            Move::Left => {
                info!("<== To the left <==");
                enigo.button(Button::Left, Release).expect("Going left");
            }
            Move::Right => {
                info!("==> To the right ==>");
                enigo.button(Button::Left, Press).expect("Going right");
            }
            Move::Spam => {
                info!("=== Spamming, the fish is close ===");
                enigo.button(Button::Left, Click).expect("Clicking failed");
            }
        }

        info!("Found fish at x={fish_x} - distance fish<->hook is {range} - hook speed is {speed}");

        sleep_with_jitter(args.sensitivity, 3, &SHUTDOWN);
        previous_hook_x = hook_x; // update previous hook position
    }
}

/// Start the fishing process
fn reels(
    enigo: &mut Enigo,
    last_shake_time: &mut Instant,
    shake_count: &mut u8,
    safe_point: &Point,
    stats: &mut Stats,
) {
    // Move mouse
    enigo
        .move_mouse_ig_abs(safe_point.x.cast_signed(), safe_point.y.cast_signed())
        .expect("Can't move mouse");

    // Click to be sure we are not shaking, then sleep to prevent reel issue
    enigo
        .button(Button::Left, Click)
        .expect("Can't click before reel");
    sleep_with_jitter(500, 10, &SHUTDOWN);

    info!("Reeling...");
    // Casting motion
    enigo
        .button(Button::Left, Press)
        .expect("Can't backswing: failed to press mouse button");
    sleep_with_jitter(900, 300, &SHUTDOWN);
    enigo
        .button(Button::Left, Release)
        .expect("Can't release the line: failed to release mouse button");

    if stats.enabled {
        stats.reels.add_assign(1);
    }

    *shake_count = 0;
    *last_shake_time = Instant::now();
}

/// Returns the coordinates of the shake bubble
fn check_shake(
    #[allow(unused_variables)] enigo: &mut Enigo,
    recorder: &mut ScreenRecorder,
    region: &Region,
    #[allow(unused_variables)] safe_point: &Point,
    args: &Args,
) -> Option<(Point, RgbImage)> {
    let [x_min, y_min, x_max, y_max] = region.corners().map(u32::cast_signed);

    // Move cursor out of the region (Sober creates a custom cursor)
    #[cfg(target_os = "linux")]
    {
        // Sleep 1 / 3 to be sure we correctly move the cursor
        sleep(Duration::from_millis((args.lag / 3).into()), &SHUTDOWN);
        enigo
            .move_mouse_ig_abs(safe_point.x.cast_signed(), safe_point.y.cast_signed())
            .expect("Can't move mouse");
        // Sleep 2 / 3 to be sure the screenshot won't capture our cursor
        sleep(Duration::from_millis((args.lag * 2 / 3).into()), &SHUTDOWN);
    }

    #[cfg(not(target_os = "linux"))]
    {
        sleep(Duration::from_millis(args.lag.into()), &SHUTDOWN);
    }

    let pure_white = ColorTarget {
        color: Rgb([0xff, 0xff, 0xff]),
        variation: 1,
    };

    // Check image from bottom to top helps up leveraging broadcast messages that are
    // overlaping with the shaking area
    let screen = recorder.take_screenshot().expect("Can't take screenshot");

    let shake_point = (y_min..=y_max)
        .rev()
        .flat_map(|y| (x_min..=x_max).map(move |x| (x, y)))
        .find_map(|(x, y)| {
            let pixel = screen.get_pixel(x.cast_unsigned(), y.cast_unsigned());

            pure_white.matches(pixel).then(|| Point {
                x: x.cast_unsigned() + 20,
                y: y.cast_unsigned() - 10,
            })
        });

    #[cfg(feature = "imageproc")]
    {
        use fischy::utils::debug::Drawable;
        use std::sync::Arc;

        if let Some(p) = shake_point.clone() {
            p.draw_async(Arc::new(screen.clone()), "shakes/point.png", false);
        }
    }

    shake_point.map(|p| (p, screen))
}

/// Initialize where the player is looking
fn initialize_viewpoint(enigo: &mut Enigo, screen_dims: &Dimensions, cond: &AtomicBool) {
    let padding = screen_dims.width * 20 / 100;

    // "Safepoint"
    enigo
        .move_mouse_ig_abs(screen_dims.width.cast_signed() / 2, padding.cast_signed())
        .expect("Going to safepoint failed");

    // Looking at the floor
    let movement = (0, screen_dims.height.cast_signed() / 3);

    let steps = 2;
    (0..=steps).for_each(|_| {
        enigo.button(Button::Right, Press).expect("Pressing failed");
        sleep(Duration::from_millis(100), cond);

        enigo
            .move_mouse(movement.0, movement.1, Rel)
            .expect("Going down failed");
        sleep(Duration::from_millis(100), cond);

        // Release
        enigo
            .button(Button::Right, Release)
            .expect("Releasing failed");
        sleep(Duration::from_millis(100), cond);

        // Back to initial point
        enigo
            .move_mouse(-movement.0, -movement.1, Rel)
            .expect("Resetting position failed");
        sleep(Duration::from_millis(100), cond);
    });

    // Zoom
    enigo
        .scroll_ig(-Enigo::max_scroll(), Vertical)
        .expect("Can't zoom in");
    enigo.scroll_ig(1, Vertical).expect("Can't zoom out");
}

/// Register specific keypress that will stop the program
fn register_keybinds() {
    thread::spawn(|| {
        listen(|e| {
            if let Event {
                event_type: KeyPress(Key::Escape | Key::Space),
                ..
            } = e
            {
                info!("Closing due to key press...");
                SHUTDOWN.store(true, Ordering::Relaxed);
            }
        })
        .expect("Can't listen to keyboard");
    });
}
