//! Tests that hold the library's text layout against a browser's: a page drawn headless by Edge
//! or Chrome, and where each colour of ink lies in it and in the library's drawing.
#[cfg(any(feature = "right-to-left", feature = "ruby"))]
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
#[cfg(any(feature = "right-to-left", feature = "ruby"))]
fn browsed(browser: &std::path::Path, html: &str, [width, height]: [usize; 2], name: &str) -> Vec<u32> {
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

/// What lies elsewhere in ours than in the browser's drawing of a page, as `differ` finds it
/// (each difference: what, and how): only what does so in each of up to three shots. (Edge on
/// GitHub's runner sometimes sets one glyph of a page 1.3 px from where it sets it in other
/// runs.)
#[cfg(any(feature = "right-to-left", feature = "ruby"))]
pub(crate) fn lasting_differences(browser: &std::path::Path, html: &str, size: [usize; 2], name: &str, differ: impl Fn(&[u32]) -> Vec<(String, String)>) -> Vec<String> {
    let mut lasting = differences(browser, html, size, name, &differ);
    for _ in 1..3 {
        if lasting.is_empty() {
            break;
        }
        let again = differences(browser, html, size, name, &differ);
        lasting.retain(|what| again.contains(what));
    }
    lasting
}

/// The differences in one shot of the page: what each is, printed with how.
#[cfg(any(feature = "right-to-left", feature = "ruby"))]
fn differences(browser: &std::path::Path, html: &str, size: [usize; 2], name: &str, differ: &impl Fn(&[u32]) -> Vec<(String, String)>) -> Vec<String> {
    let found = differ(&browsed(browser, html, size, name));
    found.into_iter().inspect(|(what, how)| println!("  {what}: {how}")).map(|(what, _)| what).collect()
}

/// How much of ink `color` a pixel holds (0 to 1), where that colour over white explains it
/// best of all the `inks`; else 0.
#[cfg(any(feature = "right-to-left", feature = "ruby"))]
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
#[cfg(any(feature = "right-to-left", feature = "ruby"))]
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

/// Each character's box, as the browser lays out the text of each element of class `probe` in a
/// page: left, top, width and height, per element. (Read back from the page as the browser left
/// it, written there by a script.)
pub(crate) fn character_boxes(browser: &std::path::Path, html: &str) -> Vec<Vec<[f32; 4]>> {
    const READ: &str = "<pre id=boxes></pre><script>
for (const probe of document.querySelectorAll('.probe')) {
  const text = probe.firstChild, range = document.createRange(), boxes = [];
  for (let at = 0; at < text.length; at++) {
    range.setStart(text, at); range.setEnd(text, at + 1);
    const box = range.getBoundingClientRect();
    boxes.push([box.left, box.top, box.width, box.height].join(' '));
  }
  document.getElementById('boxes').textContent += boxes.join(',') + '\\n';
}
</script>";
    let folder = std::env::temp_dir().join(format!("taut-boxes-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("page.html"), format!("{html}{READ}")).unwrap();
    let _alone = BROWSING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let dom = std::process::Command::new(browser)
        .args(["--headless=new", "--disable-gpu", "--allow-file-access-from-files", "--virtual-time-budget=3000", "--dump-dom"])
        .arg(format!("--user-data-dir={}", folder.join("profile").display()))
        .arg(format!("file:///{}", folder.join("page.html").to_string_lossy().replace('\\', "/")))
        .output()
        .expect("the browser runs");
    let _ = std::fs::remove_dir_all(&folder);
    let dom = String::from_utf8_lossy(&dom.stdout);
    let boxes = dom.split("<pre id=\"boxes\">").nth(1).and_then(|rest| rest.split("</pre>").next()).unwrap_or("");
    let read = |one: &str| -> [f32; 4] {
        let numbers: Vec<f32> = one.split(' ').filter_map(|number| number.parse().ok()).collect();
        [0, 1, 2, 3].map(|n| numbers.get(n).copied().unwrap_or(0.0))
    };
    boxes.lines().map(|line| line.split(',').map(read).collect()).collect()
}
