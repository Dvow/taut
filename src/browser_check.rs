//! Tests that hold the library's text layout against a browser's: a page drawn headless by Edge
//! or Chrome, and where each colour of ink lies in it and in the library's drawing.
use crate::geom::Rect;

/// Where Edge or Chrome is, if installed.
pub(crate) fn browser() -> Option<std::path::PathBuf> {
    let edge_core = std::path::Path::new(r"C:\Program Files (x86)\Microsoft\EdgeCore");
    let versions = std::fs::read_dir(edge_core).into_iter().flatten().flatten().map(|entry| entry.path().join("msedge.exe"));
    let known = [r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe", r"C:\Program Files\Google\Chrome\Application\chrome.exe"].map(std::path::PathBuf::from);
    versions.chain(known).find(|path| path.exists())
}

/// One browser at a time: two at once draw each other's pages.
static BROWSING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A page as the browser draws it, `width` by `height` pixels at scale 1. (Kept in
/// `TAUT_SHOTS`, with ours, where that names a folder.)
pub(crate) fn browsed(browser: &std::path::Path, html: &str, [width, height]: [usize; 2], name: &str) -> Vec<u32> {
    let folder = std::env::temp_dir().join(format!("taut-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("page.html"), html).unwrap();
    let shot = folder.join("edge.png");
    let _alone = BROWSING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    std::process::Command::new(browser)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--disable-lcd-text",
            "--force-color-profile=srgb",
            "--hide-scrollbars",
            "--force-device-scale-factor=1",
            "--allow-file-access-from-files",
            "--virtual-time-budget=3000",
        ])
        .arg(format!("--user-data-dir={}", folder.join("profile").display()))
        .arg(format!("--window-size={width},{height}"))
        .arg(format!("--screenshot={}", shot.display()))
        .arg(format!("file:///{}", folder.join("page.html").to_string_lossy().replace('\\', "/")))
        .output()
        .expect("the browser runs");
    let pixels = crate::png::decode(&std::fs::read(&shot).expect("a screenshot")).expect("a picture").1;
    if let Ok(shots) = std::env::var("TAUT_SHOTS") {
        std::fs::copy(&shot, std::path::Path::new(&shots).join(format!("{name}-edge.png"))).unwrap();
    }
    let _ = std::fs::remove_dir_all(&folder);
    pixels
}

/// How much of ink `color` a pixel holds (0 to 1), where that colour over white explains it
/// best of all the `inks`; else 0.
pub(crate) fn ink_of(pixel: u32, color: u32, inks: &[u32]) -> f32 {
    let channels = |rgb: u32| [(rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255].map(|c| c as f32 - 255.0);
    let seen = channels(pixel);
    let fit = |ink: u32| {
        let c = channels(ink);
        let dot: f32 = c.iter().map(|c| c * c).sum();
        let alpha = (seen.iter().zip(&c).map(|(s, c)| s * c).sum::<f32>() / dot).clamp(0.0, 1.0);
        let error: f32 = seen.iter().zip(&c).map(|(s, c)| (s - c * alpha).powi(2)).sum();
        (alpha, error)
    };
    let best = inks.iter().copied().min_by(|one, other| fit(*one).1.total_cmp(&fit(*other).1)).unwrap_or(color);
    if best == color { fit(color).0 } else { 0.0 }
}

/// Where a colour's ink lies within a region of a picture: its middle (weighted by how much ink
/// each pixel holds), where there is some.
pub(crate) fn ink_middle(pixels: &[u32], width: usize, region: Rect, color: u32, inks: &[u32]) -> Option<[f32; 2]> {
    let (mut sums, height) = ([0.0f32; 3], pixels.len() / width);
    for y in (region.y.floor().max(0.0) as usize)..(region.bottom().ceil() as usize).min(height) {
        for x in (region.x.floor().max(0.0) as usize)..(region.right().ceil() as usize).min(width) {
            let ink = ink_of(pixels[y * width + x], color, inks);
            sums = [sums[0] + ink * (x as f32 + 0.5), sums[1] + ink * (y as f32 + 0.5), sums[2] + ink];
        }
    }
    (sums[2] > 2.0).then(|| [sums[0] / sums[2], sums[1] / sums[2]])
}
