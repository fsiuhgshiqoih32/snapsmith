//! Measures how fast the selection appears, as seen on screen, and checks that nothing else
//! (a black or blank frame) shows up before it.
//!
//!     cargo run --release --example startup_probe -- <path to snapsmith> [standby|cold]
//!
//! It watches a patch in the middle of the monitor under the pointer while starting a region
//! capture, either through a warm standby process or as a fresh process, then closes it again.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn stats(img: &image::RgbaImage) -> (f64, f64) {
    let n = (img.width() * img.height()) as f64;
    let lum: Vec<f64> =
        img.pixels().map(|p| 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64).collect();
    let mean = lum.iter().sum::<f64>() / n;
    let var = lum.iter().map(|l| (l - mean).powi(2)).sum::<f64>() / n;
    (mean, var.sqrt())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let exe = args.next().expect("usage: startup_probe <snapsmith exe> [standby|cold]");
    let mode = args.next().unwrap_or_else(|| "standby".into());
    let home = std::env::temp_dir().join("snapsmith-probe-home");
    let _ = std::fs::create_dir_all(&home);

    let monitor = cursor_monitor();
    let (w, h) = (monitor.width().unwrap(), monitor.height().unwrap());
    let (pw, ph) = (160, 120);
    let (px, py) = (w / 2 - pw / 2, h / 2 + 60);

    let mut child = if mode == "standby" {
        let mut c = Command::new(&exe)
            .arg("standby")
            .env("SNAPSMITH_HOME", &home)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_secs(3));
        let (base, _) = stats(&monitor.capture_region(px, py, pw, ph).unwrap());
        println!("baseline luminance {base:.1}");
        let start = Instant::now();
        let mut stdin = c.stdin.take().unwrap();
        stdin.write_all(b"capture region\n").unwrap();
        drop(stdin);
        probe(&monitor, (px, py, pw, ph), base, start);
        c
    } else {
        let (base, _) = stats(&monitor.capture_region(px, py, pw, ph).unwrap());
        println!("baseline luminance {base:.1}");
        let start = Instant::now();
        let c = Command::new(&exe)
            .args(["capture", "region"])
            .env("SNAPSMITH_HOME", &home)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        probe(&monitor, (px, py, pw, ph), base, start);
        c
    };
    let _ = child.kill();
    let _ = child.wait();
}

/// The monitor under the pointer (where the selection opens).
fn cursor_monitor() -> xcap::Monitor {
    #[cfg(windows)]
    {
        let mut p = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p) } != 0
            && let Ok(m) = xcap::Monitor::from_point(p.x, p.y)
        {
            return m;
        }
    }
    xcap::Monitor::all().unwrap().into_iter().find(|m| m.is_primary().unwrap_or(false)).unwrap()
}

fn probe(monitor: &xcap::Monitor, (x, y, w, h): (u32, u32, u32, u32), base: f64, start: Instant) {
    let mut first_change = None;
    let mut dark = 0;
    let mut samples = 0;
    while start.elapsed() < Duration::from_millis(2000) {
        let Ok(img) = monitor.capture_region(x, y, w, h) else { continue };
        samples += 1;
        let (mean, sd) = stats(&img);
        let t = start.elapsed().as_secs_f64() * 1000.0;
        if std::env::var_os("PROBE_VERBOSE").is_some() && t < 450.0 {
            println!("{t:7.1} ms  luminance {mean:5.1}  spread {sd:5.1}");
        }
        // The selection dims the screen by about a fifth; a flash would be near black and flat.
        if mean < base * 0.15 && sd < 4.0 {
            dark += 1;
            println!("{t:7.1} ms  BLACK frame (luminance {mean:.1})");
        }
        if mean > 240.0 && sd < 4.0 && base < 200.0 {
            dark += 1;
            println!("{t:7.1} ms  WHITE frame (luminance {mean:.1})");
        }
        if first_change.is_none() && (mean - base).abs() > base * 0.08 {
            first_change = Some(t);
            println!("{t:7.1} ms  selection visible (luminance {mean:.1} vs {base:.1})");
        }
    }
    println!(
        "{samples} samples; first visible change at {}; {dark} blank frames",
        first_change.map_or("never".into(), |t| format!("{t:.0} ms"))
    );
}
